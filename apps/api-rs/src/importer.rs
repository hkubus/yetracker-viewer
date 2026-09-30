//! Catalog importer.
//!
//! Loads the tracker sheet (yetracker.net's mirror of the Google Sheets
//! `htmlview`, or `CATALOG_SHEET_FILE`), parses it into eras and songs
//! ([`parse_sheet`], pure) and writes the catalog in one transaction
//! ([`apply_import`]) while keeping ids stable: eras are identified by their
//! normalized name, songs by a content key with fallbacks (see
//! [`assign_song_ids`]). Imports that look broken are refused without touching
//! the catalog, an unchanged catalog is not rewritten, and every outcome is
//! recorded in `meta`.
//!
//! Sheet layout (one `<tr>` per sheet row):
//! - a header row naming the columns (Era, Name, Notes, Track Length, File
//!   Date, Leak Date, Available Length, Quality, Link(s));
//! - era rows: fewer cells than the header, spanning the same columns, with
//!   the era name in the cell that starts at the Name column (or, in any
//!   case, artwork in the penultimate cell); the last four cells are name
//!   (first line) + aliases (further lines), notes, artwork (may be missing)
//!   and description;
//! - song rows: one cell per header column;
//! - sub-era header rows: song-shaped, but with no song data and an era cell
//!   that names no era (normally empty); the name cell holds the section title,
//!   which applies to the following songs of the era.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use futures_util::StreamExt;
use regex::Regex;
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use scraper::{ElementRef, Html, Selector};
use sha2::{Digest, Sha256};
use tracing::{error, info, warn};
use url::Url;

use crate::catalogs::{PRIMARY_CATALOG, PRIMARY_CATALOG_ID, catalog_sheet_url, catalog_source_url};
use crate::db::{self, meta_keys};
use crate::downloader::error_chain;
use crate::error::ApiError;
use crate::search_text::{self, SearchFields};
use crate::state::SharedState;
use crate::text;

/// Part of the catalog fingerprint: bump it whenever parsing or a derived
/// column (`search_text`, `sort_title`, `category_rank`, `song_key`) changes,
/// so that the next import rewrites the catalog even if the sheet did not.
const PARSER_VERSION: u32 = 2;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const FETCH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FETCH_ATTEMPTS: u32 = 3;
const MAX_SHEET_BYTES: usize = 50 * 1024 * 1024;
const FETCH_USER_AGENT: &str = "yetracker-viewer/1.0 (+https://yetracker.net)";
/// An import that would shrink a catalog of more than this many songs below
/// `SHRINK_GUARD_PERCENT` of its size is refused unless `IMPORT_FORCE` is set.
const SHRINK_GUARD_MIN_SONGS: i64 = 100;
const SHRINK_GUARD_PERCENT: i64 = 80;
const MAX_ERROR_LEN: usize = 500;

const QUALITY_NOT_AVAILABLE: &str = "Not Available";
const QUALITIES: [&str; 6] = [
    "Low Quality",
    "High Quality",
    "CD Quality",
    "Lossless",
    QUALITY_NOT_AVAILABLE,
    "Recording",
];
const AVAILABLE_LENGTHS: [&str; 10] = [
    "Full",
    "Snippet",
    "Confirmed",
    "Beat Only",
    "Partial",
    "Tagged",
    "OG File",
    "Stem Bounce",
    "Rumored",
    "Conflicting Sources",
];

const PILLOWS_HOST: &str = "pillows.su";
const IMGUR_GG_HOST: &str = "imgur.gg";
const YOUTUBE_HOSTS: [&str; 5] = [
    "youtu.be",
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "music.youtube.com",
];
const INSTAGRAM_HOSTS: [&str; 2] = ["instagram.com", "www.instagram.com"];
const X_HOSTS: [&str; 5] = [
    "twitter.com",
    "www.twitter.com",
    "mobile.twitter.com",
    "x.com",
    "www.x.com",
];
const TRACKING_PARAMS: [&str; 6] = [
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
];

static ROW_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<tr[\s>][\s\S]*?</tr\s*>").expect("valid row regex"));
static LINE_BREAK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").expect("valid br regex"));
static CELL_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("td, th").expect("valid selector"));
static IMAGE_RENDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"=[whs]\d+(?:-[a-z0-9]+)*$").expect("valid image render regex"));
static TEXT_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)https?://[^\s<>"]+"#).expect("valid url regex"));
static IMGUR_FILE_PATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/f/[A-Za-z0-9]+/?$").expect("valid imgur path regex"));
static DATE_NUMERIC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9]{1,2})/([0-9]{1,2})/([0-9]{4})$").expect("valid date regex")
});
static DATE_ISO_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9]{4})-([0-9]{2})-([0-9]{2})$").expect("valid date regex"));
static DATE_MONTH_DAY_YEAR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z]+)\.? ([0-9]{1,2}),? ([0-9]{4})$").expect("valid date regex")
});
static DATE_MONTH_YEAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z]+)\.?,? ([0-9]{4})$").expect("valid date regex"));
static DATE_YEAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9]{4})$").expect("valid date regex"));

/// Serializes imports: the boot import and the periodic sync must not
/// interleave their catalog rewrites.
static IMPORT_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

// ---------------------------------------------------------------------------
// Parsed catalog
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatePrecision {
    Day,
    Month,
    Year,
}

impl DatePrecision {
    pub fn as_str(self) -> &'static str {
        match self {
            DatePrecision::Day => "day",
            DatePrecision::Month => "month",
            DatePrecision::Year => "year",
        }
    }
}

/// A sheet date: Unix seconds at UTC midnight of the first day of the period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogDate {
    pub timestamp: i64,
    pub precision: DatePrecision,
}

/// An anchor inside a song's notes cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotesLink {
    pub text: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEra {
    /// Identity across imports ([`era_key`] of the name).
    pub key: String,
    /// First line of the era-name cell.
    pub name: String,
    /// The other lines of the era-name cell (aliases) joined with a space.
    pub subtitle: Option<String>,
    pub notes: String,
    pub description: String,
    /// Artwork URL (`=s512` rendition). Google Sheets issues these per page
    /// render and they stop working after a while.
    pub image_url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSong {
    /// Index into [`ParsedCatalog::eras`].
    pub era: usize,
    /// 1-based position inside the era, in sheet order.
    pub era_position: i64,
    /// Full name cell; lines are title, credits, alternate titles.
    pub name: String,
    /// First line of `name`.
    pub title: String,
    pub sub_era: Option<String>,
    pub notes: Option<String>,
    pub notes_links: Vec<NotesLink>,
    /// Every http(s) link of the Link(s) cell, primary first.
    pub links: Vec<String>,
    pub file_date: Option<CatalogDate>,
    pub leak_date: Option<CatalogDate>,
    pub track_length: Option<i64>,
    pub track_length_approx: bool,
    pub available_length: Option<String>,
    pub quality: Option<String>,
    /// Content key ([`song_key`]); unique within a catalog.
    pub key: String,
}

impl ParsedSong {
    /// The primary link.
    pub fn url(&self) -> Option<&str> {
        self.links.first().map(String::as_str)
    }

    /// The primary link when the downloader should fetch it: a supported host
    /// and a quality other than "Not Available".
    pub fn download_url(&self) -> Option<&str> {
        self.url()
            .filter(|_| self.quality.as_deref() != Some(QUALITY_NOT_AVAILABLE))
            .filter(|url| is_downloadable_url(url))
    }
}

/// What the parser skipped or had to guess, for the import log.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ParseStats {
    /// Sub-era header rows.
    pub sub_eras: usize,
    /// Song rows whose era cell names no era; filed under their section's era.
    pub unknown_era_rows: usize,
    /// The era names of those rows, by era key: (name as written, rows).
    pub unknown_eras: BTreeMap<String, (String, usize)>,
    /// Era rows without artwork.
    pub eras_without_artwork: usize,
    /// Song rows above the first era row (dropped).
    pub orphan_rows: usize,
    /// Rows identical to an earlier row of the same era (dropped).
    pub duplicate_rows: usize,
    /// Unrecognized cell values (stored as NULL), e.g. `quality "Foo"`.
    pub unknown_values: BTreeMap<String, usize>,
}

#[derive(Debug, Clone)]
pub struct ParsedCatalog {
    pub eras: Vec<ParsedEra>,
    pub songs: Vec<ParsedSong>,
    pub stats: ParseStats,
}

impl ParsedCatalog {
    /// Distinct downloadable primary links ([`ParsedSong::download_url`]).
    pub fn download_links(&self) -> BTreeSet<&str> {
        self.songs
            .iter()
            .filter_map(ParsedSong::download_url)
            .collect()
    }

    /// Hash of everything the import writes except the artwork URLs. The raw
    /// page differs on every render (redirect-link signatures, script nonces,
    /// artwork tokens), so comparing page bytes would never find the catalog
    /// unchanged.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        {
            let mut field = |value: &str| {
                hasher.update((value.len() as u64).to_le_bytes());
                hasher.update(value.as_bytes());
            };
            field(&format!("parser v{PARSER_VERSION}"));
            for era in &self.eras {
                field("era");
                field(&era.key);
                field(&era.name);
                field(era.subtitle.as_deref().unwrap_or("\u{0}"));
                field(&era.notes);
                field(&era.description);
            }
            let date = |date: Option<CatalogDate>| match date {
                Some(date) => format!("{}/{}", date.timestamp, date.precision.as_str()),
                None => String::new(),
            };
            for song in &self.songs {
                field("song");
                field(&song.key);
                field(&self.eras[song.era].key);
                field(&song.name);
                field(song.sub_era.as_deref().unwrap_or("\u{0}"));
                field(song.notes.as_deref().unwrap_or("\u{0}"));
                for link in &song.notes_links {
                    field(&link.text);
                    field(&link.url);
                }
                field("links");
                for link in &song.links {
                    field(link);
                }
                field(&date(song.file_date));
                field(&date(song.leak_date));
                field(&format!(
                    "{:?}/{}",
                    song.track_length, song.track_length_approx
                ));
                field(song.available_length.as_deref().unwrap_or("\u{0}"));
                field(song.quality.as_deref().unwrap_or("\u{0}"));
            }
        }
        hex::encode(hasher.finalize())
    }
}

/// Era identity: the name with zero-width characters removed, whitespace
/// collapsed and lowercased. Song rows reference eras by this key too.
pub fn era_key(name: &str) -> String {
    let key = text::clean_line(name).to_lowercase();
    // v1 stored this sheet name renamed; both spellings are the same era, so
    // an upgraded database keeps its ids for it and the songs in it.
    if key == "travis scott collaboration" {
        return "collaboration with travis scott".to_string();
    }
    key
}

/// Content key of a song, stored in `songs.song_key`: a hash of the era key,
/// the folded name and notes, the primary link and the track length. Rows with
/// the same key are the same recording listed twice; distinct recordings that
/// share a name (e.g. several "???" snippets) differ in link or length.
pub fn song_key(
    era_key: &str,
    name: &str,
    notes: Option<&str>,
    url: Option<&str>,
    track_length: Option<i64>,
) -> String {
    let mut hasher = Sha256::new();
    for part in [
        era_key,
        &search_text::fold(name),
        &search_text::fold(notes.unwrap_or_default()),
        url.unwrap_or_default(),
        &track_length
            .map(|value| value.to_string())
            .unwrap_or_default(),
    ] {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(&hasher.finalize()[..16])
}

// ---------------------------------------------------------------------------
// Cell values
// ---------------------------------------------------------------------------

fn month_number(name: &str) -> Option<u32> {
    let month = match name.to_ascii_lowercase().as_str() {
        "jan" | "january" => 1,
        "feb" | "february" => 2,
        "mar" | "march" => 3,
        "apr" | "april" => 4,
        "may" => 5,
        "jun" | "june" => 6,
        "jul" | "july" => 7,
        "aug" | "august" => 8,
        "sep" | "sept" | "september" => 9,
        "oct" | "october" => 10,
        "nov" | "november" => 11,
        "dec" | "december" => 12,
        _ => return None,
    };
    Some(month)
}

/// Parses a sheet date. The whole cell must match one of: `M/D/YYYY`,
/// `YYYY-MM-DD`, `Mon D, YYYY` / `Month D YYYY` (day precision), `Mon YYYY` /
/// `Month YYYY` (month precision) or `YYYY` (year precision). Anything else,
/// impossible dates and years outside 1900–2100 are `None`.
pub fn parse_catalog_date(value: &str) -> Option<CatalogDate> {
    let value = text::clean_line(value);
    let number = |text: &str| text.parse::<u32>().ok();
    let (year, month, day, precision) = if let Some(parts) = DATE_NUMERIC_RE.captures(&value) {
        (
            number(&parts[3])?,
            number(&parts[1])?,
            number(&parts[2])?,
            DatePrecision::Day,
        )
    } else if let Some(parts) = DATE_ISO_RE.captures(&value) {
        (
            number(&parts[1])?,
            number(&parts[2])?,
            number(&parts[3])?,
            DatePrecision::Day,
        )
    } else if let Some(parts) = DATE_MONTH_DAY_YEAR_RE.captures(&value) {
        (
            number(&parts[3])?,
            month_number(&parts[1])?,
            number(&parts[2])?,
            DatePrecision::Day,
        )
    } else if let Some(parts) = DATE_MONTH_YEAR_RE.captures(&value) {
        (
            number(&parts[2])?,
            month_number(&parts[1])?,
            1,
            DatePrecision::Month,
        )
    } else {
        let parts = DATE_YEAR_RE.captures(&value)?;
        (number(&parts[1])?, 1, 1, DatePrecision::Year)
    };
    if !(1900..=2100).contains(&year) {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(year as i32, month, day)?;
    Some(CatalogDate {
        timestamp: date.and_hms_opt(0, 0, 0)?.and_utc().timestamp(),
        precision,
    })
}

/// Parses a track length (`m:ss` or `h:mm:ss`, a leading `~` marks it as
/// approximate). Returns the seconds and the approximate flag; unknown digits
/// (`3:??`) and malformed values are `(None, false)`.
pub fn parse_track_length(value: &str) -> (Option<i64>, bool) {
    let value = text::clean_line(value);
    let (approx, rest) = match value.strip_prefix('~') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, value.as_str()),
    };
    let mut numbers = Vec::with_capacity(3);
    for part in rest.split(':') {
        if part.is_empty() || part.len() > 6 || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return (None, false);
        }
        numbers.push(part.parse::<i64>().unwrap_or(i64::MAX));
    }
    let seconds = match numbers[..] {
        [minutes, seconds] if seconds < 60 => minutes
            .checked_mul(60)
            .and_then(|total| total.checked_add(seconds)),
        [hours, minutes, seconds] if minutes < 60 && seconds < 60 => hours
            .checked_mul(3600)
            .and_then(|total| total.checked_add(minutes * 60 + seconds)),
        _ => None,
    };
    match seconds {
        Some(seconds) => (Some(seconds), approx),
        None => (None, false),
    }
}

/// Returns `value` when it is one of `allowed`; otherwise counts it and
/// returns `None`.
fn known_value(
    value: &str,
    allowed: &[&str],
    label: &str,
    stats: &mut ParseStats,
) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    if allowed.contains(&value) {
        return Some(value.to_string());
    }
    *stats
        .unknown_values
        .entry(format!("{label} {value:?}"))
        .or_default() += 1;
    None
}

/// `https://www.google.com/url?q=<target>&…` (how Google Sheets wraps every
/// hyperlink) → `<target>`.
fn unwrap_google_redirect(url: Url) -> Url {
    let is_redirect =
        matches!(url.host_str(), Some("www.google.com" | "google.com")) && url.path() == "/url";
    if is_redirect
        && let Some(target) = url
            .query_pairs()
            .find(|(key, _)| key == "q")
            .map(|(_, value)| value.into_owned())
        && let Ok(target) = Url::parse(target.trim())
    {
        return target;
    }
    url
}

/// Normalizes a link target: redirect unwrapped, http(s) only.
fn normalize_link(raw: &str) -> Option<String> {
    let raw = text::strip_zero_width(raw.trim());
    let url = unwrap_google_redirect(Url::parse(&raw).ok()?);
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then(|| url.to_string())
}

/// Download priority of a link's host (lower first); `None` for hosts the
/// downloader does not support.
fn link_priority(url: &Url) -> Option<u8> {
    let host = url.host_str()?;
    if host == PILLOWS_HOST {
        Some(0)
    } else if host == IMGUR_GG_HOST {
        IMGUR_FILE_PATH_RE.is_match(url.path()).then_some(1)
    } else if YOUTUBE_HOSTS.contains(&host) {
        Some(2)
    } else if INSTAGRAM_HOSTS.contains(&host) {
        Some(3)
    } else if X_HOSTS.contains(&host) {
        Some(4)
    } else {
        None
    }
}

/// Whether the downloader can fetch `url`: `pillows.su`, `imgur.gg/f/<id>`,
/// YouTube, Instagram and X/Twitter links.
pub fn is_downloadable_url(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .and_then(|url| link_priority(&url))
        .is_some()
}

/// Moves the preferred link (pillows.su, then imgur.gg files, YouTube,
/// Instagram, X/Twitter; the first link otherwise) to the front, keeping the
/// others in cell order.
fn order_links(mut links: Vec<String>) -> Vec<String> {
    let primary = links
        .iter()
        .enumerate()
        .filter_map(|(index, link)| {
            let priority = Url::parse(link).ok().and_then(|url| link_priority(&url))?;
            Some((priority, index))
        })
        .min()
        .map(|(_, index)| index);
    if let Some(index) = primary
        && index > 0
    {
        let link = links.remove(index);
        links.insert(0, link);
    }
    links
}

fn sanitize_image_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let Ok(mut parsed) = Url::parse(trimmed) else {
        return String::new();
    };
    if parsed.scheme() != "https" {
        return String::new();
    }
    // Drop only known tracking params; keep legitimate query strings intact.
    let surviving: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| !TRACKING_PARAMS.contains(&key.as_ref()))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    parsed.set_query(None);
    if !surviving.is_empty() {
        let mut pairs = parsed.query_pairs_mut();
        for (key, value) in &surviving {
            pairs.append_pair(key, value);
        }
    }
    let mut sanitized = parsed.to_string();
    // Google Sheets serves artwork at the displayed cell size (e.g.
    // `=w102-h104`); request a full 512px rendition so covers aren't upscaled
    // from a thumbnail.
    if sanitized.contains("docs.google.com/sheets-images-rt") {
        sanitized = IMAGE_RENDER_RE.replace(&sanitized, "=s512").into_owned();
    }
    sanitized
}

// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------

/// A run of cell text, or an anchor (link target + text).
enum Segment {
    Text(String),
    Anchor { href: String, text: String },
}

/// The content of one table cell. Line breaks (`<br>`) are `\n`.
struct Cell {
    segments: Vec<Segment>,
    image: Option<String>,
    /// Columns the cell covers (`colspan`, at least 1).
    span: usize,
}

impl Cell {
    fn read(cell: ElementRef<'_>) -> Cell {
        let span = cell
            .value()
            .attr("colspan")
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|span| (1..=1000).contains(span))
            .unwrap_or(1);
        let mut content = Cell {
            segments: Vec::new(),
            image: None,
            span,
        };
        content.walk(cell, None);
        content
    }

    fn walk(&mut self, element: ElementRef<'_>, anchor: Option<usize>) {
        for child in element.children() {
            if let Some(text) = child.value().as_text() {
                match anchor.and_then(|index| self.segments.get_mut(index)) {
                    Some(Segment::Anchor {
                        text: anchor_text, ..
                    }) => anchor_text.push_str(text),
                    _ => match self.segments.last_mut() {
                        Some(Segment::Text(run)) => run.push_str(text),
                        _ => self.segments.push(Segment::Text(text.to_string())),
                    },
                }
            } else if let Some(child) = ElementRef::wrap(child) {
                match child.value().name() {
                    "img" => {
                        if self.image.is_none() {
                            self.image = child.value().attr("src").map(str::to_string);
                        }
                    }
                    "a" if anchor.is_none() => {
                        self.segments.push(Segment::Anchor {
                            href: child.value().attr("href").unwrap_or_default().to_string(),
                            text: String::new(),
                        });
                        let index = self.segments.len() - 1;
                        self.walk(child, Some(index));
                    }
                    _ => self.walk(child, anchor),
                }
            }
        }
    }

    fn text(&self) -> String {
        let mut text = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Text(run) => text.push_str(run),
                Segment::Anchor { text: anchor, .. } => text.push_str(anchor),
            }
        }
        text
    }

    /// Link targets in cell order: anchor hrefs, plus URLs typed as plain text
    /// outside any anchor. Deduplicated.
    fn links(&self) -> Vec<String> {
        let mut links: Vec<String> = Vec::new();
        let mut push = |candidate: Option<String>| {
            if let Some(link) = candidate
                && !links.contains(&link)
            {
                links.push(link);
            }
        };
        for segment in &self.segments {
            match segment {
                Segment::Anchor { href, .. } => push(normalize_link(href)),
                Segment::Text(run) => {
                    for found in TEXT_URL_RE.find_iter(run) {
                        push(normalize_link(found.as_str()));
                    }
                }
            }
        }
        links
    }

    /// Anchors as notes links (the anchor text, or the URL when it has none).
    fn notes_links(&self) -> Vec<NotesLink> {
        let mut links: Vec<NotesLink> = Vec::new();
        for segment in &self.segments {
            if let Segment::Anchor { href, text } = segment
                && let Some(url) = normalize_link(href)
            {
                let text = text::clean_line(text);
                let link = NotesLink {
                    text: if text.is_empty() { url.clone() } else { text },
                    url,
                };
                if !links.contains(&link) {
                    links.push(link);
                }
            }
        }
        links
    }
}

/// Column indexes of the song rows.
struct Columns {
    /// Cells in the header row (song rows have one per column).
    width: usize,
    /// Sheet columns the header row covers (its cells' spans added up).
    span: usize,
    /// Sheet column at which the Name cell starts.
    name_column: usize,
    era: usize,
    name: usize,
    notes: usize,
    track_length: usize,
    file_date: usize,
    leak_date: usize,
    available_length: usize,
    quality: usize,
    link: usize,
}

fn header_names(cells: &[Cell]) -> Option<Vec<String>> {
    let names: Vec<String> = cells
        .iter()
        .map(|cell| text::clean_line(&cell.text()).to_lowercase())
        .collect();
    let has_era = names.iter().any(|name| name == "era");
    let has_name = names
        .iter()
        .any(|name| name == "name" || name.starts_with("name "));
    (has_era && has_name).then_some(names)
}

impl Columns {
    fn from_headers(headers: &[String], spans: &[usize]) -> Result<Columns, String> {
        let mut missing = Vec::new();
        let mut find = |label: &str, matches: &dyn Fn(&str) -> bool| {
            let index = headers.iter().position(|header| matches(header));
            if index.is_none() {
                missing.push(label.to_string());
            }
            index.unwrap_or(0)
        };
        let mut columns = Columns {
            width: headers.len(),
            span: spans.iter().sum(),
            name_column: 0,
            era: find("Era", &|header| header == "era" || header == "main era"),
            name: find("Name", &|header| {
                header == "name" || header.starts_with("name ")
            }),
            notes: find("Notes", &|header| {
                header == "notes" || header.starts_with("notes ")
            }),
            track_length: find("Track Length", &|header| {
                matches!(header, "track length" | "length" | "full length")
            }),
            file_date: find("File Date", &|header| {
                matches!(header, "file date" | "date made")
            }),
            leak_date: find("Leak Date", &|header| header == "leak date"),
            available_length: find("Available Length", &|header| header == "available length"),
            quality: find("Quality", &|header| header == "quality"),
            link: find("Link(s)", &|header| header.starts_with("link")),
        };
        columns.name_column = spans[..columns.name].iter().sum();
        if missing.is_empty() {
            Ok(columns)
        } else {
            Err(format!(
                "the sheet's header row lacks the column(s): {}",
                missing.join(", ")
            ))
        }
    }
}

/// A song-shaped row, cleaned but not interpreted yet.
struct LineRow {
    era: String,
    name: String,
    notes: String,
    notes_links: Vec<NotesLink>,
    track_length: String,
    file_date: String,
    leak_date: String,
    available_length: String,
    quality: String,
    link_text: String,
    links: Vec<String>,
}

impl LineRow {
    fn read(cells: &[Cell], columns: &Columns) -> LineRow {
        let line = |index: usize| text::clean_line(&cells[index].text());
        LineRow {
            era: line(columns.era),
            name: text::clean_multiline(&cells[columns.name].text()),
            notes: text::clean_multiline(&cells[columns.notes].text()),
            notes_links: cells[columns.notes].notes_links(),
            track_length: line(columns.track_length),
            file_date: line(columns.file_date),
            leak_date: line(columns.leak_date),
            available_length: line(columns.available_length),
            quality: line(columns.quality),
            link_text: line(columns.link),
            links: order_links(cells[columns.link].links()),
        }
    }

    /// Any of the song columns filled in. Sub-era header rows have none.
    fn has_song_data(&self) -> bool {
        [
            &self.track_length,
            &self.file_date,
            &self.leak_date,
            &self.available_length,
            &self.quality,
            &self.link_text,
        ]
        .iter()
        .any(|value| !value.is_empty())
            || !self.links.is_empty()
    }
}

enum SheetRow {
    Era(ParsedEra),
    Line(LineRow),
}

/// An era row: at least five cells but fewer than a song row, laid out over
/// the same columns with the name cell starting at the Name column; or, the
/// shape the sheet used before, artwork in the penultimate cell. The artwork
/// is optional: an era added before its artwork (or with artwork Google
/// Sheets does not render) is still an era.
fn read_era_row(cells: &[Cell], columns: &Columns) -> Option<ParsedEra> {
    let count = cells.len();
    if count < 5 || count >= columns.width {
        return None;
    }
    let span: usize = cells.iter().map(|cell| cell.span).sum();
    let name_column: usize = cells[..count - 4].iter().map(|cell| cell.span).sum();
    let image = cells[count - 2].image.as_deref();
    let by_layout = span == columns.span && name_column == columns.name_column;
    if !by_layout && image.is_none() {
        return None;
    }
    let name_cell = text::clean_multiline(&cells[count - 4].text());
    let mut lines = name_cell.split('\n').filter(|line| !line.is_empty());
    let name = lines.next()?.to_string();
    let subtitle = lines.collect::<Vec<_>>().join(" ");
    Some(ParsedEra {
        key: era_key(&name),
        name,
        subtitle: (!subtitle.is_empty()).then_some(subtitle),
        notes: text::clean_multiline(&cells[cells.len() - 3].text()),
        description: text::clean_multiline(&cells[cells.len() - 1].text()),
        image_url: image.map(sanitize_image_url).unwrap_or_default(),
    })
}

/// Parses the sheet HTML into eras and songs (in sheet order).
pub fn parse_sheet(html: &str) -> Result<ParsedCatalog, String> {
    let mut columns: Option<Columns> = None;
    let mut rows: Vec<SheetRow> = Vec::new();
    for row in ROW_RE.find_iter(html) {
        // html5ever does not turn `<br>` into a line break in text content;
        // do it up front so multi-line cells keep their lines.
        let row_html = LINE_BREAK_RE.replace_all(row.as_str(), "\n");
        // A bare `<tr>` outside a table would be dropped by html5ever.
        let document = Html::parse_fragment(&format!("<table>{row_html}</table>"));
        let cells: Vec<Cell> = document.select(&CELL_SELECTOR).map(Cell::read).collect();
        let Some(columns) = &columns else {
            if let Some(headers) = header_names(&cells) {
                let spans: Vec<usize> = cells.iter().map(|cell| cell.span).collect();
                columns = Some(Columns::from_headers(&headers, &spans)?);
            }
            continue;
        };
        if let Some(era) = read_era_row(&cells, columns) {
            rows.push(SheetRow::Era(era));
        } else if cells.len() == columns.width {
            rows.push(SheetRow::Line(LineRow::read(&cells, columns)));
        }
    }
    if columns.is_none() {
        return Err("the sheet has no header row with Era and Name columns".to_string());
    }
    Ok(build_catalog(rows))
}

fn build_catalog(rows: Vec<SheetRow>) -> ParsedCatalog {
    let mut stats = ParseStats::default();
    let mut eras: Vec<ParsedEra> = Vec::new();
    let mut era_index: HashMap<String, usize> = HashMap::new();
    for row in &rows {
        let SheetRow::Era(era) = row else { continue };
        match era_index.get(&era.key) {
            // A repeated era row only fills in what the first one lacked.
            Some(&index) => {
                let existing = &mut eras[index];
                if existing.subtitle.is_none() {
                    existing.subtitle = era.subtitle.clone();
                }
                for (target, source) in [
                    (&mut existing.notes, &era.notes),
                    (&mut existing.description, &era.description),
                    (&mut existing.image_url, &era.image_url),
                ] {
                    if target.is_empty() {
                        target.clone_from(source);
                    }
                }
            }
            None => {
                era_index.insert(era.key.clone(), eras.len());
                eras.push(era.clone());
            }
        }
    }
    stats.eras_without_artwork = eras.iter().filter(|era| era.image_url.is_empty()).count();

    let mut songs: Vec<ParsedSong> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut era_counts = vec![0i64; eras.len()];
    let mut section: Option<usize> = None;
    let mut sub_era: Option<String> = None;
    for row in rows {
        let line = match row {
            SheetRow::Era(era) => {
                section = era_index.get(&era.key).copied();
                sub_era = None;
                continue;
            }
            SheetRow::Line(line) => line,
        };
        if line.name.is_empty() || line.era.eq_ignore_ascii_case("era") {
            continue;
        }
        let resolved = Some(era_key(&line.era))
            .filter(|key| !key.is_empty())
            .and_then(|key| era_index.get(&key).copied());
        let era = match resolved {
            Some(era) => era,
            None if !line.has_song_data() => {
                if section.is_some() {
                    sub_era = Some(text::clean_line(&line.name));
                    stats.sub_eras += 1;
                }
                continue;
            }
            None => match section {
                Some(era) => {
                    stats.unknown_era_rows += 1;
                    if !line.era.is_empty() {
                        stats
                            .unknown_eras
                            .entry(era_key(&line.era))
                            .or_insert_with(|| (line.era.clone(), 0))
                            .1 += 1;
                    }
                    era
                }
                None => {
                    stats.orphan_rows += 1;
                    continue;
                }
            },
        };

        let (track_length, track_length_approx) = parse_track_length(&line.track_length);
        let notes = (!line.notes.is_empty()).then_some(line.notes);
        let key = song_key(
            &eras[era].key,
            &line.name,
            notes.as_deref(),
            line.links.first().map(String::as_str),
            track_length,
        );
        if !seen.insert(key.clone()) {
            stats.duplicate_rows += 1;
            continue;
        }
        let mut date = |value: &str, label: &str| {
            let parsed = parse_catalog_date(value);
            if parsed.is_none() && !value.is_empty() {
                *stats
                    .unknown_values
                    .entry(format!("{label} {value:?}"))
                    .or_default() += 1;
            }
            parsed
        };
        let file_date = date(&line.file_date, "file date");
        let leak_date = date(&line.leak_date, "leak date");
        let available_length = known_value(
            &line.available_length,
            &AVAILABLE_LENGTHS,
            "available length",
            &mut stats,
        );
        let quality = known_value(&line.quality, &QUALITIES, "quality", &mut stats);
        era_counts[era] += 1;
        songs.push(ParsedSong {
            era,
            era_position: era_counts[era],
            title: text::first_line(&line.name).to_string(),
            name: line.name,
            sub_era: if section == Some(era) {
                sub_era.clone()
            } else {
                None
            },
            notes,
            notes_links: line.notes_links,
            links: line.links,
            file_date,
            leak_date,
            track_length,
            track_length_approx,
            available_length,
            quality,
            key,
        });
    }

    ParsedCatalog { eras, songs, stats }
}

// ---------------------------------------------------------------------------
// Writing the catalog
// ---------------------------------------------------------------------------

/// An import that keeps fewer than this share (in percent) of the stored
/// catalog's distinct downloadable links is refused (see [`check_links`]).
const LINK_GUARD_PERCENT: usize = 80;
/// The link guard only applies to catalogs with more links than this.
const LINK_GUARD_MIN_LINKS: usize = 100;
/// Deleted songs and eras keep a tombstone this long, so that rows restored
/// upstream within that time get their old ids back (and eras their covers,
/// which the cleanup sets aside meanwhile).
pub(crate) const TOMBSTONE_RETENTION_SECS: i64 = 90 * 24 * 60 * 60;

#[derive(Debug, Clone, Copy)]
pub struct ImportOptions {
    /// Skip the sanity guards and rewrite even when unchanged.
    pub force: bool,
    /// Unix seconds recorded as the import time and `files.last_seen_at`.
    pub now: i64,
}

/// How the imported songs got their ids.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MatchCounts {
    /// Same content key as a stored song.
    pub content: usize,
    /// Same era, folded name and primary link.
    pub link: usize,
    /// Same folded title and primary link in any era (songs that moved to
    /// another era, or whose credits changed).
    pub moved: usize,
    /// Same era and folded name.
    pub name: usize,
    /// Deleted by an earlier import and back now, with their old ids.
    pub restored: usize,
}

impl MatchCounts {
    /// Songs that kept the id of a song in the stored catalog.
    pub fn kept(&self) -> usize {
        self.content + self.link + self.moved + self.name
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ImportSummary {
    /// The catalog matched the last import; eras and songs were not rewritten.
    pub unchanged: bool,
    pub eras: usize,
    pub eras_added: usize,
    pub eras_removed: usize,
    /// Eras whose name changed; they kept their id.
    pub eras_renamed: usize,
    /// Eras deleted by an earlier import and back now, with their old ids.
    pub eras_restored: usize,
    pub songs: usize,
    /// Songs with a new id.
    pub songs_added: usize,
    pub songs_removed: usize,
    pub songs_matched: MatchCounts,
    /// Distinct downloadable primary links of the catalog.
    pub download_links: usize,
    pub files_added: usize,
    pub files_seen: usize,
}

/// A song whose id an imported song can take over: a stored song, or the
/// tombstone of a deleted one.
#[derive(Debug, Clone)]
pub struct KnownSong {
    pub id: i64,
    pub era_key: String,
    pub folded_name: String,
    /// Folded first line of the name.
    pub folded_title: String,
    pub url: Option<String>,
    /// Content key ([`song_key`]).
    pub key: String,
}

impl KnownSong {
    pub fn new(id: i64, era_key: String, name: &str, url: Option<String>, key: String) -> Self {
        KnownSong {
            id,
            era_key,
            folded_name: search_text::fold(name),
            folded_title: search_text::fold(text::first_line(name)),
            url,
            key,
        }
    }
}

/// Known songs plus which of them already passed their id on.
struct Pool<'a> {
    known: &'a [KnownSong],
    taken: Vec<bool>,
}

impl<'a> Pool<'a> {
    fn new(known: &'a [KnownSong]) -> Self {
        Pool {
            known,
            taken: vec![false; known.len()],
        }
    }

    /// Matches equal keys in order (several known songs with the same key
    /// are consumed one by one). Returns the number of matches.
    fn match_in_order<K: Eq + std::hash::Hash>(
        &mut self,
        incoming: &[KnownSong],
        assigned: &mut [Option<i64>],
        key: impl Fn(&KnownSong) -> Option<K>,
    ) -> usize {
        let mut queues: HashMap<K, VecDeque<usize>> = HashMap::new();
        for (index, known) in self.known.iter().enumerate() {
            if !self.taken[index]
                && let Some(key) = key(known)
            {
                queues.entry(key).or_default().push_back(index);
            }
        }
        let mut matched = 0;
        for (index, song) in incoming.iter().enumerate() {
            if assigned[index].is_none()
                && let Some(key) = key(song)
                && let Some(known_index) = queues.get_mut(&key).and_then(VecDeque::pop_front)
            {
                assigned[index] = Some(self.known[known_index].id);
                self.taken[known_index] = true;
                matched += 1;
            }
        }
        matched
    }

    /// Matches keys that are unique among the unmatched songs on both
    /// sides. Returns the number of matches.
    fn match_unique<K: Eq + std::hash::Hash>(
        &mut self,
        incoming: &[KnownSong],
        assigned: &mut [Option<i64>],
        key: impl Fn(&KnownSong) -> Option<K>,
    ) -> usize {
        let mut old: HashMap<K, Vec<usize>> = HashMap::new();
        for (index, known) in self.known.iter().enumerate() {
            if !self.taken[index]
                && let Some(key) = key(known)
            {
                old.entry(key).or_default().push(index);
            }
        }
        let mut new: HashMap<K, Vec<usize>> = HashMap::new();
        for (index, song) in incoming.iter().enumerate() {
            if assigned[index].is_none()
                && let Some(key) = key(song)
            {
                new.entry(key).or_default().push(index);
            }
        }
        let mut matched = 0;
        for (key, new_indexes) in new {
            if let ([new_index], Some([old_index])) =
                (new_indexes.as_slice(), old.get(&key).map(Vec::as_slice))
            {
                assigned[*new_index] = Some(self.known[*old_index].id);
                self.taken[*old_index] = true;
                matched += 1;
            }
        }
        matched
    }
}

type EraNameLink = (String, String, Option<String>);

/// Assigns ids to `songs`, best match first. Stored songs: (1) the same
/// content key; (2) the same era, folded name and primary link; (3) the same
/// folded name and primary link in any era, then the same folded title and
/// primary link in any era (a song moved to another era, or with edited
/// credits); (4) the same era and folded name. Then tombstones of deleted
/// songs: the same content key; the same era, folded name and link; the same
/// folded title and link. Except for the content key, a tier only matches
/// pairs that are unique among the songs still unmatched on both sides.
/// Everything else gets a new id from `next_id`, so an id never passes to a
/// different song.
pub fn assign_song_ids(
    stored: &[KnownSong],
    tombstones: &[KnownSong],
    songs: &[ParsedSong],
    eras: &[ParsedEra],
    next_id: &mut i64,
    matched: &mut MatchCounts,
) -> Vec<i64> {
    let incoming: Vec<KnownSong> = songs
        .iter()
        .map(|song| {
            KnownSong::new(
                0,
                eras[song.era].key.clone(),
                &song.name,
                song.url().map(str::to_string),
                song.key.clone(),
            )
        })
        .collect();
    let mut assigned: Vec<Option<i64>> = vec![None; songs.len()];
    let content = |song: &KnownSong| Some(song.key.clone());
    let era_name_link = |song: &KnownSong| -> Option<EraNameLink> {
        Some((
            song.era_key.clone(),
            song.folded_name.clone(),
            song.url.clone(),
        ))
    };
    let name_link = |song: &KnownSong| song.url.clone().map(|url| (song.folded_name.clone(), url));
    let title_link =
        |song: &KnownSong| song.url.clone().map(|url| (song.folded_title.clone(), url));
    let era_name = |song: &KnownSong| Some((song.era_key.clone(), song.folded_name.clone()));

    let mut live = Pool::new(stored);
    matched.content = live.match_in_order(&incoming, &mut assigned, content);
    matched.link = live.match_unique(&incoming, &mut assigned, era_name_link);
    // Rows written before names kept their line breaks have no title line
    // of their own, so the whole name is tried first.
    matched.moved = live.match_unique(&incoming, &mut assigned, name_link)
        + live.match_unique(&incoming, &mut assigned, title_link);
    matched.name = live.match_unique(&incoming, &mut assigned, era_name);

    let mut deleted = Pool::new(tombstones);
    matched.restored = deleted.match_in_order(&incoming, &mut assigned, content)
        + deleted.match_unique(&incoming, &mut assigned, era_name_link)
        + deleted.match_unique(&incoming, &mut assigned, title_link);

    assigned
        .into_iter()
        .map(|id| {
            id.unwrap_or_else(|| {
                let id = *next_id;
                *next_id += 1;
                id
            })
        })
        .collect()
}

/// Maps eras that disappeared to new eras that took over most of their
/// songs (matched by folded title and primary link): the era was renamed,
/// so it keeps its id. `stored_songs` holds `(era id, folded title, url)` of
/// the stored songs; the result maps stored era ids to indexes into
/// `catalog.eras`.
fn detect_era_renames(
    vanished: &[i64],
    appeared: &[usize],
    stored_songs: &[(Option<i64>, String, Option<String>)],
    catalog: &ParsedCatalog,
) -> HashMap<i64, usize> {
    let mut renames = HashMap::new();
    if vanished.is_empty() || appeared.is_empty() {
        return renames;
    }
    let mut new_songs: HashMap<usize, HashSet<(String, Option<&str>)>> = HashMap::new();
    for song in &catalog.songs {
        if appeared.contains(&song.era) {
            new_songs
                .entry(song.era)
                .or_default()
                .insert((search_text::fold(&song.title), song.url()));
        }
    }
    let mut candidates: Vec<(usize, i64, usize)> = Vec::new();
    for &old_id in vanished {
        let songs: Vec<(&str, Option<&str>)> = stored_songs
            .iter()
            .filter(|(era, _, _)| *era == Some(old_id))
            .map(|(_, title, url)| (title.as_str(), url.as_deref()))
            .collect();
        if songs.is_empty() {
            continue;
        }
        for (&new_index, pairs) in &new_songs {
            let hits = songs
                .iter()
                .filter(|(title, url)| pairs.contains(&(title.to_string(), *url)))
                .count();
            if hits * 2 > songs.len() {
                candidates.push((hits, old_id, new_index));
            }
        }
    }
    // Most shared songs first; ties go to the older era, then sheet order.
    candidates.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.cmp(&right.2))
    });
    let mut used: HashSet<usize> = HashSet::new();
    for (_, old_id, new_index) in candidates {
        if !renames.contains_key(&old_id) && used.insert(new_index) {
            renames.insert(old_id, new_index);
        }
    }
    renames
}

fn count(conn: &Connection, sql: &str) -> Result<i64, ApiError> {
    Ok(conn.query_row(sql, [], |row| row.get(0))?)
}

/// Refuses a catalog that shrank below [`SHRINK_GUARD_PERCENT`] of the
/// current one.
fn check_shrink(current: i64, new: i64) -> Result<(), ApiError> {
    if current > SHRINK_GUARD_MIN_SONGS && new * 100 < current * SHRINK_GUARD_PERCENT {
        return Err(ApiError::unexpected(format!(
            "refusing to replace {current} songs with {new} (below {SHRINK_GUARD_PERCENT}%); \
             set IMPORT_FORCE=true to accept"
        )));
    }
    Ok(())
}

/// Refuses a catalog whose distinct downloadable links fell below
/// [`LINK_GUARD_PERCENT`] of the stored catalog's. The song count alone
/// misses a change in how the sheet writes its links (e.g. a different
/// redirect wrapper): every song survives, but its media would be unlinked
/// and, after a while, cleaned up.
fn check_links(current: usize, new: usize) -> Result<(), ApiError> {
    if current > LINK_GUARD_MIN_LINKS && new * 100 < current * LINK_GUARD_PERCENT {
        return Err(ApiError::unexpected(format!(
            "refusing an import whose downloadable links dropped from {current} to {new} \
             (below {LINK_GUARD_PERCENT}%); set IMPORT_FORCE=true to accept"
        )));
    }
    Ok(())
}

/// Distinct downloadable primary links of the stored catalog (the last
/// successful import), counted like [`ParsedSong::download_url`].
fn stored_download_links(conn: &Connection) -> Result<usize, ApiError> {
    let mut statement = conn.prepare(
        "SELECT DISTINCT url FROM songs WHERE catalog_id = 'unreleased' AND url IS NOT NULL \
         AND (quality IS NULL OR quality != ?1)",
    )?;
    let rows = statement.query_map([QUALITY_NOT_AVAILABLE], |row| row.get::<_, String>(0))?;
    let mut links = 0;
    for url in rows {
        if is_downloadable_url(&url?) {
            links += 1;
        }
    }
    Ok(links)
}

/// Refuses a catalog in which song rows name a stored era that has no era
/// row any more: its songs would be filed under whatever era precedes them
/// and lose their ids, which usually means the era row changed shape.
fn check_missing_eras(conn: &Connection, stats: &ParseStats) -> Result<(), ApiError> {
    if stats.unknown_eras.is_empty() {
        return Ok(());
    }
    let stored: HashSet<String> = {
        let mut statement =
            conn.prepare("SELECT key FROM eras WHERE is_main = 1 AND key IS NOT NULL")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let missing: Vec<String> = stats
        .unknown_eras
        .iter()
        .filter(|(key, _)| stored.contains(*key))
        .map(|(_, (name, rows))| format!("{name:?} ({rows} songs)"))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(ApiError::unexpected(format!(
        "refusing an import in which song rows name eras the sheet has no era row for: {}; \
         set IMPORT_FORCE=true to accept",
        missing.join(", ")
    )))
}

/// The sanity guards (see [`check_shrink`], [`check_links`] and
/// [`check_missing_eras`]); `IMPORT_FORCE` skips them.
fn check_guards(conn: &Connection, catalog: &ParsedCatalog) -> Result<(), ApiError> {
    let current = count(
        conn,
        "SELECT count(*) FROM songs WHERE catalog_id = 'unreleased'",
    )?;
    check_shrink(current, catalog.songs.len() as i64)?;
    check_links(stored_download_links(conn)?, catalog.download_links().len())?;
    check_missing_eras(conn, &catalog.stats)
}

/// Whether the stored catalog is exactly the last import of this catalog.
fn catalog_unchanged(
    conn: &Connection,
    catalog: &ParsedCatalog,
    fingerprint: &str,
) -> Result<bool, ApiError> {
    if db::meta_get(conn, meta_keys::LAST_SHEET_SHA256)?.as_deref() != Some(fingerprint) {
        return Ok(false);
    }
    // Guard against rows written by something else since (an older binary, a
    // manual edit): only skip when the table still looks like our import.
    let songs = count(
        conn,
        "SELECT count(*) FROM songs WHERE catalog_id = 'unreleased'",
    )?;
    let incomplete = count(
        conn,
        "SELECT count(*) FROM songs WHERE position IS NULL OR song_key IS NULL",
    )?;
    let eras = count(conn, "SELECT count(*) FROM eras")?;
    Ok(songs == catalog.songs.len() as i64 && incomplete == 0 && eras == catalog.eras.len() as i64)
}

/// Writes a parsed catalog in one transaction and records the import in
/// `meta`. Nothing is written when the import is refused.
pub fn apply_import(
    conn: &Connection,
    catalog: &ParsedCatalog,
    options: &ImportOptions,
) -> Result<ImportSummary, ApiError> {
    if catalog.eras.is_empty() || catalog.songs.is_empty() {
        return Err(ApiError::unexpected(
            "the sheet contained no eras or no songs",
        ));
    }
    let fingerprint = catalog.fingerprint();
    // Take the write lock up front: a deferred transaction that reads first
    // could fail to upgrade if the downloader writes in between.
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let mut summary = ImportSummary {
        eras: catalog.eras.len(),
        songs: catalog.songs.len(),
        download_links: catalog.download_links().len(),
        ..ImportSummary::default()
    };
    summary.unchanged = !options.force && catalog_unchanged(&transaction, catalog, &fingerprint)?;
    if summary.unchanged {
        // Artwork URLs are only valid for a while after each page render, so
        // keep the freshest ones even when nothing else changed.
        let mut update = transaction.prepare("UPDATE eras SET image_url = ?1 WHERE key = ?2")?;
        for era in &catalog.eras {
            update.execute(params![era.image_url, era.key])?;
        }
    } else {
        if !options.force {
            check_guards(&transaction, catalog)?;
        }
        write_catalog(&transaction, catalog, options, &mut summary)?;
    }
    upsert_files(&transaction, catalog, options.now, &mut summary)?;
    db::meta_set(
        &transaction,
        meta_keys::LAST_SHEET_SHA256,
        Some(&fingerprint),
    )?;
    db::meta_set(
        &transaction,
        meta_keys::LAST_IMPORT_AT,
        Some(&options.now.to_string()),
    )?;
    db::meta_set(&transaction, meta_keys::LAST_IMPORT_OK, Some("true"))?;
    db::meta_set(&transaction, meta_keys::LAST_IMPORT_ERROR, None)?;
    transaction.commit()?;
    if !summary.unchanged {
        conn.execute_batch("PRAGMA optimize")?;
    }
    Ok(summary)
}

/// A song row as stored before the import.
struct StoredSong {
    id: i64,
    era: Option<i64>,
    era_key: String,
    name: String,
    notes: Option<String>,
    url: Option<String>,
    track_length: Option<i64>,
    key: Option<String>,
}

fn load_stored_songs(conn: &Connection) -> Result<Vec<StoredSong>, ApiError> {
    let mut statement = conn.prepare(
        "SELECT songs.id, songs.era, eras.key, songs.name, songs.notes, songs.url, \
         songs.track_length, songs.song_key FROM songs LEFT JOIN eras ON eras.id = songs.era \
         WHERE songs.catalog_id = 'unreleased' \
         ORDER BY coalesce(songs.position, songs.id), songs.id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(StoredSong {
            id: row.get(0)?,
            era: row.get(1)?,
            era_key: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            name: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            notes: row.get(4)?,
            url: row.get(5)?,
            track_length: row.get(6)?,
            key: row.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Tombstones of deleted songs whose ids are not live.
fn load_song_tombstones(conn: &Connection) -> Result<Vec<KnownSong>, ApiError> {
    let mut statement = conn.prepare(
        "SELECT id, song_key, era_key, name, url FROM song_tombstones \
         WHERE id NOT IN (SELECT id FROM songs) ORDER BY deleted_at DESC, id",
    )?;
    let rows = statement.query_map([], |row| {
        let name: Option<String> = row.get(3)?;
        Ok(KnownSong::new(
            row.get(0)?,
            row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            name.as_deref().unwrap_or_default(),
            row.get(4)?,
            row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn write_catalog(
    conn: &Connection,
    catalog: &ParsedCatalog,
    options: &ImportOptions,
    summary: &mut ImportSummary,
) -> Result<(), ApiError> {
    let now = options.now;
    let expired = now - TOMBSTONE_RETENTION_SECS;
    conn.execute(
        "DELETE FROM song_tombstones WHERE deleted_at < ?1",
        [expired],
    )?;
    conn.execute(
        "DELETE FROM era_tombstones WHERE deleted_at < ?1",
        [expired],
    )?;

    let max_era_id = count(conn, "SELECT coalesce(max(id), 0) FROM eras")?;
    let max_song_id = count(conn, "SELECT coalesce(max(id), 0) FROM songs")?;
    let mut next_era_id = db::meta_get_i64(conn, meta_keys::NEXT_ERA_ID)?
        .unwrap_or(1)
        .max(max_era_id + 1);
    let mut next_song_id = db::meta_get_i64(conn, meta_keys::NEXT_SONG_ID)?
        .unwrap_or(1)
        .max(max_song_id + 1);

    // Eras keep their id when their key is unchanged, when they were renamed
    // (see `detect_era_renames`), or when a deleted era comes back.
    let stored_eras: Vec<(i64, Option<String>)> = {
        let mut statement = conn.prepare("SELECT id, key FROM eras")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let era_ids_by_key: HashMap<&str, i64> = stored_eras
        .iter()
        .filter_map(|(id, key)| Some((key.as_deref()?, *id)))
        .collect();
    let parsed_keys: HashSet<&str> = catalog.eras.iter().map(|era| era.key.as_str()).collect();
    let stored_songs = load_stored_songs(conn)?;
    let vanished: Vec<i64> = stored_eras
        .iter()
        .filter(|(_, key)| key.as_deref().is_none_or(|key| !parsed_keys.contains(key)))
        .map(|(id, _)| *id)
        .collect();
    let appeared: Vec<usize> = (0..catalog.eras.len())
        .filter(|&index| !era_ids_by_key.contains_key(catalog.eras[index].key.as_str()))
        .collect();
    let renames = {
        let titles: Vec<(Option<i64>, String, Option<String>)> = stored_songs
            .iter()
            .map(|song| {
                (
                    song.era,
                    search_text::fold(text::first_line(&song.name)),
                    song.url.clone(),
                )
            })
            .collect();
        detect_era_renames(&vanished, &appeared, &titles, catalog)
    };
    let renamed_to: HashMap<usize, i64> = renames
        .iter()
        .map(|(old_id, new_index)| (*new_index, *old_id))
        .collect();
    let era_tombstones: HashMap<String, i64> = {
        let mut statement = conn.prepare(
            "SELECT key, id FROM era_tombstones WHERE id NOT IN (SELECT id FROM eras) \
             ORDER BY deleted_at, id",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    // (id, whether a stored row keeps it)
    let mut era_ids: Vec<(i64, bool)> = Vec::with_capacity(catalog.eras.len());
    let mut restored_eras: Vec<i64> = Vec::new();
    for (index, era) in catalog.eras.iter().enumerate() {
        if let Some(&id) = era_ids_by_key.get(era.key.as_str()) {
            era_ids.push((id, true));
        } else if let Some(&id) = renamed_to.get(&index) {
            summary.eras_renamed += 1;
            era_ids.push((id, true));
        } else if let Some(&id) = era_tombstones.get(&era.key) {
            summary.eras_restored += 1;
            restored_eras.push(id);
            era_ids.push((id, false));
        } else {
            summary.eras_added += 1;
            era_ids.push((next_era_id, false));
            next_era_id += 1;
        }
    }

    // Stored songs of renamed eras are matched as if they had been filed
    // under the new name all along.
    let new_era_key = |era: Option<i64>| -> Option<&str> {
        let index = *renames.get(&era?)?;
        Some(catalog.eras[index].key.as_str())
    };
    let stored: Vec<KnownSong> = stored_songs
        .iter()
        .map(|song| {
            let (era_key, key) = match new_era_key(song.era) {
                Some(era_key) => (
                    era_key.to_string(),
                    song_key(
                        era_key,
                        &song.name,
                        song.notes.as_deref(),
                        song.url.as_deref(),
                        song.track_length,
                    ),
                ),
                None => (
                    song.era_key.clone(),
                    song.key.clone().unwrap_or_else(|| {
                        song_key(
                            &song.era_key,
                            &song.name,
                            song.notes.as_deref(),
                            song.url.as_deref(),
                            song.track_length,
                        )
                    }),
                ),
            };
            KnownSong::new(song.id, era_key, &song.name, song.url.clone(), key)
        })
        .collect();
    let tombstones = load_song_tombstones(conn)?;
    let song_ids = assign_song_ids(
        &stored,
        &tombstones,
        &catalog.songs,
        &catalog.eras,
        &mut next_song_id,
        &mut summary.songs_matched,
    );
    let kept_songs = summary.songs_matched.kept();
    summary.songs_added = catalog.songs.len() - kept_songs - summary.songs_matched.restored;
    summary.songs_removed = stored.len() - kept_songs;

    let kept_eras: HashSet<i64> = era_ids.iter().map(|(id, _)| *id).collect();
    {
        let mut delete = conn.prepare("DELETE FROM eras WHERE id = ?1")?;
        let mut bury = conn.prepare(
            "INSERT OR REPLACE INTO era_tombstones (id, key, deleted_at) VALUES (?1, ?2, ?3)",
        )?;
        for (id, key) in stored_eras.iter().filter(|(id, _)| !kept_eras.contains(id)) {
            delete.execute([id])?;
            if let Some(key) = key {
                bury.execute(params![id, key, now])?;
            }
            summary.eras_removed += 1;
        }
        let mut unbury = conn.prepare("DELETE FROM era_tombstones WHERE id = ?1")?;
        for id in &restored_eras {
            unbury.execute([id])?;
        }
        let mut update = conn.prepare(
            "UPDATE eras SET key = ?1, name = ?2, subtitle = ?3, notes = ?4, image_url = ?5, \
             description = ?6, position = ?7, is_main = 1 WHERE id = ?8",
        )?;
        let mut insert = conn.prepare(
            "INSERT INTO eras (id, key, name, subtitle, notes, image_url, description, position, \
             is_main, dominant_color) VALUES (?8, ?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, '666666')",
        )?;
        for (index, (era, (id, stored))) in catalog.eras.iter().zip(&era_ids).enumerate() {
            let values = params![
                era.key,
                era.name,
                era.subtitle,
                era.notes,
                era.image_url,
                era.description,
                index as i64 + 1,
                id,
            ];
            if *stored {
                update.execute(values)?;
            } else {
                insert.execute(values)?;
            }
        }
    }

    conn.execute("DELETE FROM songs", [])?;
    {
        let live: HashSet<i64> = song_ids.iter().copied().collect();
        let mut bury = conn.prepare(
            "INSERT OR REPLACE INTO song_tombstones (id, song_key, era_key, name, url, deleted_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for (song, known) in stored_songs.iter().zip(&stored) {
            if !live.contains(&song.id) {
                bury.execute(params![
                    song.id,
                    known.key,
                    known.era_key,
                    song.name,
                    song.url,
                    now
                ])?;
            }
        }
        let mut unbury = conn.prepare("DELETE FROM song_tombstones WHERE id = ?1")?;
        for tombstone in tombstones.iter().filter(|known| live.contains(&known.id)) {
            unbury.execute([tombstone.id])?;
        }
    }
    let mut insert = conn.prepare(
        "INSERT INTO songs (id, era, catalog_id, name, notes, file_date, leak_date, \
         available_length, track_length, quality, url, position, era_position, title, sub_era, \
         links, notes_links, file_date_precision, leak_date_precision, track_length_approx, \
         search_text, sort_title, category_rank, song_key, song_search_text) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
         ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
    )?;
    for (index, (song, id)) in catalog.songs.iter().zip(&song_ids).enumerate() {
        let era = &catalog.eras[song.era];
        let links = serde_json::to_string(&song.links).expect("links serialise");
        let notes_links = serde_json::Value::Array(
            song.notes_links
                .iter()
                .map(|link| serde_json::json!({ "text": link.text, "url": link.url }))
                .collect(),
        )
        .to_string();
        let fields = SearchFields {
            name: &song.name,
            notes: song.notes.as_deref(),
            era_name: Some(&era.name),
            era_subtitle: era.subtitle.as_deref(),
            sub_era: song.sub_era.as_deref(),
            quality: song.quality.as_deref(),
            available_length: song.available_length.as_deref(),
        };
        insert.execute(params![
            id,
            era_ids[song.era].0,
            PRIMARY_CATALOG_ID,
            song.name,
            song.notes,
            song.file_date.map(|date| date.timestamp),
            song.leak_date.map(|date| date.timestamp),
            song.available_length,
            song.track_length,
            song.quality,
            song.url(),
            index as i64 + 1,
            song.era_position,
            song.title,
            song.sub_era,
            links,
            notes_links,
            song.file_date.map(|date| date.precision.as_str()),
            song.leak_date.map(|date| date.precision.as_str()),
            song.track_length_approx,
            search_text::search_text_for(&fields),
            search_text::sort_title(&song.name),
            search_text::category_rank(&song.name),
            song.key,
            search_text::song_search_text_for(&fields),
        ])?;
    }

    db::meta_set(conn, meta_keys::NEXT_ERA_ID, Some(&next_era_id.to_string()))?;
    db::meta_set(
        conn,
        meta_keys::NEXT_SONG_ID,
        Some(&next_song_id.to_string()),
    )?;
    Ok(())
}

/// Keeps `files` in step with the catalog: a new row (pending, no filename
/// yet) for every new downloadable primary link, and `last_seen_at` for
/// every stored row whose link the sheet still lists, primary or not: a
/// link that merely stopped being a song's first choice still belongs to
/// the catalog, so its row and media stay. Rows are never deleted here; the
/// cleanup job removes rows no import has seen for a while.
fn upsert_files(
    conn: &Connection,
    catalog: &ParsedCatalog,
    now: i64,
    summary: &mut ImportSummary,
) -> Result<(), ApiError> {
    let before = count(conn, "SELECT count(*) FROM files")?;
    let mut upsert = conn.prepare(
        "INSERT INTO files (url, status, attempts, last_seen_at) VALUES (?1, 'pending', 0, ?2) \
         ON CONFLICT(url) DO UPDATE SET last_seen_at = excluded.last_seen_at",
    )?;
    let downloads = catalog.download_links();
    for url in &downloads {
        upsert.execute(params![url, now])?;
    }
    let mut touch = conn.prepare("UPDATE files SET last_seen_at = ?2 WHERE url = ?1")?;
    let mut listed: HashSet<&str> = HashSet::new();
    for link in catalog.songs.iter().flat_map(|song| &song.links) {
        if !downloads.contains(link.as_str()) && listed.insert(link) {
            touch.execute(params![link, now])?;
        }
    }
    summary.files_seen = downloads.len();
    summary.files_added = (count(conn, "SELECT count(*) FROM files")? - before).max(0) as usize;
    Ok(())
}

// ---------------------------------------------------------------------------
// Loading the sheet
// ---------------------------------------------------------------------------

enum FetchError {
    /// Network trouble or a server error: worth another attempt.
    Retryable(String),
    Fatal(String),
}

async fn fetch_once(client: &reqwest::Client, url: &str) -> Result<String, FetchError> {
    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, FETCH_USER_AGENT)
        .send()
        .await
        .map_err(|error| {
            // Keep the cause ("connection refused", "dns error: …"): reqwest's
            // own message only names the URL.
            let message = error_chain(&error);
            if error.is_builder() || error.is_redirect() {
                FetchError::Fatal(message)
            } else {
                FetchError::Retryable(message)
            }
        })?;
    let status = response.status();
    if status.is_server_error() {
        return Err(FetchError::Retryable(format!("HTTP {status}")));
    }
    if !status.is_success() {
        return Err(FetchError::Fatal(format!("HTTP {status}")));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SHEET_BYTES as u64)
    {
        return Err(FetchError::Fatal("the sheet is larger than 50 MiB".into()));
    }
    let mut body: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| FetchError::Retryable(error_chain(&error)))?;
        if body.len() + chunk.len() > MAX_SHEET_BYTES {
            return Err(FetchError::Fatal("the sheet is larger than 50 MiB".into()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// GETs the sheet: 30 s per attempt, three attempts with backoff, retrying
/// only network errors and 5xx responses.
async fn fetch_sheet(url: &str) -> Result<String, ApiError> {
    let client = reqwest::Client::builder()
        .connect_timeout(FETCH_CONNECT_TIMEOUT)
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(ApiError::unexpected)?;
    let mut delay = Duration::from_millis(500);
    let mut attempt = 1;
    loop {
        match fetch_once(&client, url).await {
            Ok(body) => return Ok(body),
            Err(FetchError::Retryable(message)) if attempt < FETCH_ATTEMPTS => {
                warn!(url, attempt, error = %message, "catalog fetch failed; retrying");
                tokio::time::sleep(delay + Duration::from_millis(pseudo_random(250))).await;
                delay *= 2;
                attempt += 1;
            }
            Err(FetchError::Retryable(message) | FetchError::Fatal(message)) => {
                return Err(ApiError::unexpected(format!(
                    "fetching {url} failed: {message}"
                )));
            }
        }
    }
}

async fn read_sheet_file(path: &Path) -> Result<String, ApiError> {
    let unreadable = |error: std::io::Error| {
        ApiError::unexpected(format!("reading {} failed: {error}", path.display()))
    };
    let metadata = tokio::fs::metadata(path).await.map_err(unreadable)?;
    if metadata.len() > MAX_SHEET_BYTES as u64 {
        return Err(ApiError::unexpected(format!(
            "{} is larger than 50 MiB",
            path.display()
        )));
    }
    let bytes = tokio::fs::read(path).await.map_err(unreadable)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn pseudo_random(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as u64)
        .unwrap_or(0);
    nanos % max
}

/// Logs what the parser skipped or guessed. Only called for imports that
/// changed the catalog, so an unchanged sheet does not repeat the same
/// warnings every sync.
fn log_parse_stats(stats: &ParseStats) {
    if stats.eras_without_artwork > 0 {
        warn!(
            eras = stats.eras_without_artwork,
            "era rows without artwork; their covers are kept or stay missing"
        );
    }
    if stats.unknown_era_rows > 0 {
        warn!(
            rows = stats.unknown_era_rows,
            "song rows whose era cell names no era were filed under the era of their section"
        );
    }
    if stats.orphan_rows > 0 {
        warn!(
            rows = stats.orphan_rows,
            "dropped song rows above the first era row"
        );
    }
    if !stats.unknown_values.is_empty() {
        let values: Vec<String> = stats
            .unknown_values
            .iter()
            .take(20)
            .map(|(value, count)| format!("{value} ×{count}"))
            .collect();
        warn!(
            distinct = stats.unknown_values.len(),
            values = %values.join(", "),
            "unrecognized cell values were stored as empty"
        );
    }
}

async fn run_import(state: &SharedState) -> Result<ImportSummary, ApiError> {
    let started = Instant::now();
    let (html, source) = match &state.config.catalog_sheet_file {
        Some(path) => (read_sheet_file(path).await?, path.display().to_string()),
        None => {
            let url = catalog_sheet_url(&PRIMARY_CATALOG);
            (fetch_sheet(&url).await?, url)
        }
    };
    let catalog = tokio::task::spawn_blocking(move || parse_sheet(&html))
        .await
        .map_err(ApiError::unexpected)?
        .map_err(ApiError::unexpected)?;
    let stats = catalog.stats.clone();

    let options = ImportOptions {
        force: state.config.import_force,
        now: db::unix_now(),
    };
    let summary = db::call(&state.pool, move |conn| {
        apply_import(conn, &catalog, &options)
    })
    .await?;

    let elapsed_ms = started.elapsed().as_millis() as u64;
    if !summary.unchanged {
        log_parse_stats(&stats);
    }
    if summary.unchanged {
        info!(
            source = %source,
            eras = summary.eras,
            songs = summary.songs,
            elapsed_ms,
            "catalog unchanged since the last import; kept as is"
        );
    } else {
        info!(
            source = %source,
            catalog = %catalog_source_url(PRIMARY_CATALOG.gid),
            eras = summary.eras,
            eras_added = summary.eras_added,
            eras_removed = summary.eras_removed,
            eras_renamed = summary.eras_renamed,
            eras_restored = summary.eras_restored,
            songs = summary.songs,
            songs_added = summary.songs_added,
            songs_removed = summary.songs_removed,
            kept_by_content = summary.songs_matched.content,
            kept_by_link = summary.songs_matched.link,
            kept_by_move = summary.songs_matched.moved,
            kept_by_name = summary.songs_matched.name,
            restored = summary.songs_matched.restored,
            sub_eras = stats.sub_eras,
            duplicates = stats.duplicate_rows,
            download_links = summary.download_links,
            files_added = summary.files_added,
            files = summary.files_seen,
            elapsed_ms,
            "catalog imported"
        );
    }
    Ok(summary)
}

/// Imports the catalog (see the module docs). On failure the catalog is left
/// untouched and the error is recorded in `meta` before it is returned.
pub async fn import_data(state: SharedState) -> Result<(), ApiError> {
    let _running = IMPORT_LOCK.lock().await;
    match run_import(&state).await {
        Ok(_) => Ok(()),
        Err(import_error) => {
            let mut message = import_error.to_string();
            if message.len() > MAX_ERROR_LEN {
                let mut end = MAX_ERROR_LEN;
                while !message.is_char_boundary(end) {
                    end -= 1;
                }
                message.truncate(end);
            }
            let recorded = db::call(&state.pool, move |conn| {
                db::meta_set(conn, meta_keys::LAST_IMPORT_OK, Some("false"))?;
                db::meta_set(conn, meta_keys::LAST_IMPORT_ERROR, Some(&message))
            })
            .await;
            if let Err(error) = recorded {
                error!(%error, "failed to record the import failure");
            }
            Err(import_error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/sheet.html");
    const NOW: i64 = 1_790_000_000;

    fn fixture() -> ParsedCatalog {
        parse_sheet(FIXTURE).expect("fixture parses")
    }

    fn titled<'a>(catalog: &'a ParsedCatalog, title: &str) -> Vec<&'a ParsedSong> {
        catalog
            .songs
            .iter()
            .filter(|song| song.title == title)
            .collect()
    }

    fn one<'a>(catalog: &'a ParsedCatalog, title: &str) -> &'a ParsedSong {
        let songs = titled(catalog, title);
        assert_eq!(songs.len(), 1, "exactly one song titled {title:?}");
        songs[0]
    }

    /// Byte range of the `<tr>` of a sheet row (1-based sheet row number).
    fn row_range(html: &str, number: usize) -> std::ops::Range<usize> {
        let marker = format!("id=\"34972268R{}\"", number - 1);
        let at = html.find(&marker).expect("row present");
        let start = html[..at].rfind("<tr").expect("row start");
        let end = at + html[at..].find("</tr>").expect("row end") + "</tr>".len();
        start..end
    }

    fn without_row(html: &str, number: usize) -> String {
        let range = row_range(html, number);
        format!("{}{}", &html[..range.start], &html[range.end..])
    }

    /// Inserts a copy of a row right after it, with `from` replaced by `to`.
    fn with_copied_row(html: &str, number: usize, from: &str, to: &str) -> String {
        let range = row_range(html, number);
        let copy = html[range.clone()].replacen(from, to, 1);
        assert_ne!(copy, html[range.clone()], "replacement applied");
        format!("{}{copy}{}", &html[..range.end], &html[range.end..])
    }

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let nowhere = std::env::temp_dir().join("yt-importer-test-nowhere");
        db::migrate(&conn, &nowhere, &nowhere).unwrap();
        conn
    }

    fn import(conn: &Connection, catalog: &ParsedCatalog) -> ImportSummary {
        apply_import(
            conn,
            catalog,
            &ImportOptions {
                force: false,
                now: NOW,
            },
        )
        .expect("import succeeds")
    }

    /// `(song_key, id)` of every stored song.
    fn stored_ids(conn: &Connection) -> HashMap<String, i64> {
        let mut statement = conn.prepare("SELECT song_key, id FROM songs").unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn scalar<T: rusqlite::types::FromSql>(conn: &Connection, sql: &str) -> T {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn era_keys_match_the_name_v1_stored() {
        assert_eq!(
            era_key("Travis  Scott Collaboration"),
            era_key("Collaboration with Travis Scott")
        );
        assert_eq!(era_key(" Ye\u{200B}ezus "), "yeezus");
    }

    #[test]
    fn eras_keep_the_lines_of_their_name_cell() {
        let catalog = fixture();
        let names: Vec<&str> = catalog.eras.iter().map(|era| era.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Before The College Dropout",
                "SWISH",
                "Yandhi [V1]",
                "God's Country"
            ]
        );
        assert_eq!(
            catalog.eras[0].subtitle.as_deref(),
            Some("(World Record Holders)")
        );
        assert_eq!(catalog.eras[2].subtitle, None);
        assert!(
            catalog.eras[3]
                .subtitle
                .as_deref()
                .unwrap()
                .starts_with("(JESUS IS LORD, Our Beautiful Amazing Reality")
        );
        assert_eq!(catalog.eras[0].key, "before the college dropout");
        assert_eq!(
            catalog.eras[0].notes,
            "(06/08/1977) (Ye is born in Atlanta)\n(08/18/2002) (Kanye announces he signed to Roc-A-Fella)"
        );
        assert!(
            catalog.eras[0]
                .description
                .starts_with("Before Kanye released")
        );
        for era in &catalog.eras {
            assert!(
                era.image_url
                    .starts_with("https://docs.google.com/sheets-images-rt/")
                    && era.image_url.ends_with("=s512"),
                "{}",
                era.image_url
            );
        }
    }

    #[test]
    fn songs_follow_sheet_order_and_sub_era_rows() {
        let catalog = fixture();
        assert_eq!(catalog.songs.len(), 30);
        let per_era: Vec<usize> = (0..catalog.eras.len())
            .map(|era| catalog.songs.iter().filter(|song| song.era == era).count())
            .collect();
        assert_eq!(per_era, [9, 7, 10, 4]);
        for era in 0..catalog.eras.len() {
            let positions: Vec<i64> = catalog
                .songs
                .iter()
                .filter(|song| song.era == era)
                .map(|song| song.era_position)
                .collect();
            let expected: Vec<i64> = (1..=positions.len() as i64).collect();
            assert_eq!(positions, expected);
        }
        assert_eq!(catalog.stats.sub_eras, 6);

        let sub_eras: Vec<(&str, Option<&str>)> = catalog
            .songs
            .iter()
            .filter(|song| song.era == 2)
            .map(|song| (song.title.as_str(), song.sub_era.as_deref()))
            .collect();
        assert_eq!(
            sub_eras,
            [
                ("Brothers [V10]", Some("Pre-Yandhi")),
                ("✨ Brothers [V11]", Some("Pre-Yandhi")),
                ("Brothers [V16]", Some("SNL")),
                ("???", Some("Black Friday")),
                ("???", Some("Black Friday")),
                ("???", Some("Black Friday")),
                ("???", Some("Black Friday")),
                ("???", Some("Black Friday")),
                (
                    "Ty Dolla $ign - Thank You (Vol. 2) [V1]",
                    Some("Collaboration with Ty Dolla $ign")
                ),
                (
                    "Thank You (Vol. 2) [V2]",
                    Some("Collaboration with Ty Dolla $ign")
                ),
            ]
        );
        // Songs above the first sub-era row of their era have none.
        assert!(
            catalog
                .songs
                .iter()
                .filter(|song| song.era == 0)
                .all(|song| song.sub_era.is_none())
        );
        assert_eq!(
            one(&catalog, "12,000 Acres [V10]").sub_era.as_deref(),
            Some("Palm Springs")
        );
    }

    #[test]
    fn a_stray_era_cell_on_a_sub_era_row_does_not_make_it_a_song() {
        // The Hollywood Bowl header row has "x" in its era cell.
        let catalog = fixture();
        assert!(titled(&catalog, "808s & Heartbreak at the Hollywood Bowl").is_empty());
        let swish: Vec<(&str, Option<&str>)> = catalog
            .songs
            .iter()
            .filter(|song| song.era == 1)
            .map(|song| (song.title.as_str(), song.sub_era.as_deref()))
            .collect();
        assert_eq!(swish[4], ("Travis Scott - 3500 [V14]", None));
        assert_eq!(
            swish[5],
            ("Amazing", Some("808s & Heartbreak at the Hollywood Bowl"))
        );
        assert_eq!(
            swish[6],
            (
                "Bad News [V1]",
                Some("808s & Heartbreak at the Hollywood Bowl")
            )
        );
    }

    #[test]
    fn names_and_notes_keep_line_breaks_without_zero_width_characters() {
        let catalog = fixture();
        let song = one(&catalog, "Gangster Lovers");
        assert_eq!(
            song.name,
            "Gangster Lovers\n(prod. Post Malone)\n(Me Or Us)"
        );
        let acres = one(&catalog, "12,000 Acres [V10]");
        assert_eq!(
            acres.name,
            "12,000 Acres [V10]\n(ref. Consequence) (prod. BoogzDaBeast & FNZ)\n(12 Thousand Acres)"
        );
        let thank_you = one(&catalog, "Ty Dolla $ign - Thank You (Vol. 2) [V1]");
        assert_eq!(
            thank_you.name,
            "Ty Dolla $ign - Thank You (Vol. 2) [V1]\n\
             (prod. BoogzDaBeast, Ty Dolla $ign, Audio Anthem & damn james!)\n(Track 5, Track 6)"
        );
        assert!(catalog.songs.iter().all(|song| {
            !song.name.contains('\u{200B}')
                && !song
                    .notes
                    .as_deref()
                    .unwrap_or_default()
                    .contains('\u{200B}')
        }));
        assert_eq!(
            one(&catalog, "✨ Don't Jump [V3]").title,
            "✨ Don't Jump [V3]"
        );
    }

    #[test]
    fn dates_and_lengths_keep_their_precision() {
        let catalog = fixture();
        let ya_feel = one(&catalog, "Ya Feel");
        assert_eq!(
            ya_feel.leak_date,
            Some(CatalogDate {
                timestamp: 1_509_494_400,
                precision: DatePrecision::Month
            })
        );
        let rhymefest = one(&catalog, "Rhymefest - ???");
        assert_eq!(
            rhymefest.file_date,
            Some(CatalogDate {
                timestamp: 1_012_521_600,
                precision: DatePrecision::Month
            })
        );
        assert_eq!(rhymefest.leak_date, None);
        assert_eq!(
            one(&catalog, "187th").leak_date,
            Some(CatalogDate {
                timestamp: 1_240_358_400,
                precision: DatePrecision::Day
            })
        );
        let gangster = one(&catalog, "Gangster Lovers");
        assert_eq!(
            (gangster.track_length, gangster.track_length_approx),
            (Some(3600), true)
        );
        let brothers = one(&catalog, "Brothers [V16]");
        assert_eq!(
            (brothers.track_length, brothers.track_length_approx),
            (Some(300), true)
        );
        let exact = one(&catalog, "187th");
        assert_eq!(
            (exact.track_length, exact.track_length_approx),
            (Some(176), false)
        );
        assert!(
            catalog.stats.unknown_values.is_empty(),
            "{:?}",
            catalog.stats.unknown_values
        );
    }

    #[test]
    fn links_are_read_from_hrefs_with_the_downloadable_one_first() {
        let catalog = fixture();
        // An http link survives, but the YouTube link is the primary one.
        let consequence = one(&catalog, "Consequence - The Good, The Bad, The Ugly [V1]");
        assert_eq!(
            consequence.links,
            [
                "https://youtu.be/sA3TpJzsFHc",
                "http://players.brightcove.net/4863540648001/HklmClHkx_default/index.html?videoId=5236088640001"
            ]
        );
        assert_eq!(consequence.url(), Some("https://youtu.be/sA3TpJzsFHc"));
        // imgur.gg files beat other hosts regardless of cell order.
        let rhymefest = one(&catalog, "Rhymefest - ???");
        assert_eq!(rhymefest.url(), Some("https://imgur.gg/f/darVaMt"));
        assert_eq!(rhymefest.links.len(), 2);
        // Cell order is kept among equals; the Google redirect is unwrapped.
        assert_eq!(
            one(&catalog, "30 Hours [V8]").links,
            ["https://imgur.gg/f/8jCuohn", "https://imgur.gg/f/LeeWJuS"]
        );
        // The href wins over a different URL shown as the anchor text.
        let unknown = titled(&catalog, "???");
        let reference = unknown
            .iter()
            .find(|song| song.era == 3)
            .expect("God's Country ???");
        assert_eq!(
            reference.links,
            ["https://www.highsnobiety.com/p/asap-ferg-value/"]
        );
        // "N/A" as plain text is no link.
        assert!(one(&catalog, "Brothers [V16]").links.is_empty());
        let multi = catalog
            .songs
            .iter()
            .filter(|song| song.links.len() > 1)
            .count();
        assert_eq!(multi, 6);
    }

    #[test]
    fn notes_anchors_become_notes_links() {
        let catalog = fixture();
        assert_eq!(
            one(&catalog, "Beat 1").notes_links,
            [
                NotesLink {
                    text: "\"The World is a Ghetto\" by George Benson".into(),
                    url: "https://youtu.be/I9HZe5vP5-M".into(),
                },
                NotesLink {
                    text: "the Common vs. Kanye freestyle battle".into(),
                    url: "https://imgur.gg/f/nhOhAwL".into(),
                },
            ]
        );
        assert!(one(&catalog, "187th").notes_links.is_empty());
    }

    #[test]
    fn identical_rows_collapse_but_distinct_recordings_survive() {
        let catalog = fixture();
        assert_eq!(catalog.stats.duplicate_rows, 1);
        // Five "???" snippets in Yandhi [V1] share name and notes but differ in
        // link and length; the God's Country row is listed twice verbatim.
        assert_eq!(
            titled(&catalog, "???")
                .iter()
                .filter(|song| song.era == 2)
                .count(),
            5
        );
        assert_eq!(
            titled(&catalog, "???")
                .iter()
                .filter(|song| song.era == 3)
                .count(),
            1
        );
        let keys: HashSet<&str> = catalog.songs.iter().map(|song| song.key.as_str()).collect();
        assert_eq!(keys.len(), catalog.songs.len());
    }

    #[test]
    fn rows_naming_an_unknown_era_join_the_era_of_their_section() {
        let catalog = fixture();
        assert_eq!(catalog.stats.unknown_era_rows, 1);
        let partial = catalog
            .songs
            .iter()
            .find(|song| song.url() == Some("https://imgur.gg/f/TIatYzy"))
            .expect("edited row imported");
        assert_eq!(catalog.eras[partial.era].name, "Yandhi [V1]");
        assert_eq!(partial.sub_era.as_deref(), Some("Black Friday"));
    }

    #[test]
    fn a_header_without_required_columns_aborts() {
        let html = FIXTURE.replacen(">Quality</td>", ">Grade</td>", 1);
        let error = parse_sheet(&html).unwrap_err();
        assert!(error.contains("Quality"), "{error}");
        let error = parse_sheet("<table><tr><td>nothing</td></tr></table>").unwrap_err();
        assert!(error.contains("header"), "{error}");
    }

    #[test]
    fn dates_parse_strictly() {
        let day = |timestamp| {
            Some(CatalogDate {
                timestamp,
                precision: DatePrecision::Day,
            })
        };
        assert_eq!(parse_catalog_date("Apr 22, 2009"), day(1_240_358_400));
        assert_eq!(parse_catalog_date("April 22 2009"), day(1_240_358_400));
        assert_eq!(parse_catalog_date("4/22/2009"), day(1_240_358_400));
        assert_eq!(parse_catalog_date("2009-04-22"), day(1_240_358_400));
        assert_eq!(parse_catalog_date("Feb 29, 2000"), day(951_782_400));
        assert_eq!(
            parse_catalog_date("Sept 2015"),
            Some(CatalogDate {
                timestamp: 1_441_065_600,
                precision: DatePrecision::Month
            })
        );
        assert_eq!(
            parse_catalog_date("2015"),
            Some(CatalogDate {
                timestamp: 1_420_070_400,
                precision: DatePrecision::Year
            })
        );
        for invalid in [
            "",
            "z",
            "Ooc",
            "Nov ??, 2025",
            "Feb 30, 2020",
            "13/01/2020",
            "Jan 1, 1800",
            "3000",
            "Late 2019",
            "2019?",
            "Nov 2017 (rumored)",
        ] {
            assert_eq!(parse_catalog_date(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn track_lengths_parse_with_approximation_and_overflow_checks() {
        assert_eq!(parse_track_length("3:07"), (Some(187), false));
        assert_eq!(parse_track_length("1:02:03"), (Some(3723), false));
        assert_eq!(parse_track_length("~2:00"), (Some(120), true));
        assert_eq!(parse_track_length("~ 1:02:03"), (Some(3723), true));
        assert_eq!(parse_track_length("60:00"), (Some(3600), false));
        for invalid in [
            "",
            "3:75",
            "1:60:00",
            "3:??",
            "?:??",
            "~",
            "nope",
            "3",
            "1:2:3:4",
            ":30",
            "999999999:00",
        ] {
            assert_eq!(parse_track_length(invalid), (None, false), "{invalid}");
        }
    }

    #[test]
    fn redirects_hosts_and_download_support() {
        assert_eq!(
            normalize_link(
                "https://www.google.com/url?q=https://youtu.be/gPhO7Pzhsws?t%3D96&sa=D&source=editors&ust=1&usg=x"
            )
            .as_deref(),
            Some("https://youtu.be/gPhO7Pzhsws?t=96")
        );
        assert_eq!(normalize_link("mailto:someone@example.com"), None);
        assert_eq!(normalize_link("not a url"), None);
        for downloadable in [
            "https://pillows.su/f/abc123",
            "https://imgur.gg/f/kYj3fdI",
            "https://youtu.be/xyz",
            "https://music.youtube.com/watch?v=1",
            "https://m.youtube.com/watch?v=1",
            "https://www.instagram.com/p/abc/",
            "https://instagram.com/p/abc/",
            "https://x.com/user/status/1",
            "https://www.x.com/user/status/1",
            "https://mobile.twitter.com/user/status/1",
            "http://twitter.com/user/status/1",
        ] {
            assert!(is_downloadable_url(downloadable), "{downloadable}");
        }
        for other in [
            "https://imgur.gg/abc",
            "https://i.imgur.gg/abc-name.mp3",
            "https://imgur.com/abc",
            "https://www.facebook.com/x",
            "ftp://pillows.su/f/abc",
        ] {
            assert!(!is_downloadable_url(other), "{other}");
        }
        assert_eq!(
            order_links(vec![
                "https://x.com/a/status/1".into(),
                "https://www.youtube.com/watch?v=1".into(),
                "https://pillows.su/f/abc".into(),
            ]),
            [
                "https://pillows.su/f/abc",
                "https://x.com/a/status/1",
                "https://www.youtube.com/watch?v=1"
            ]
        );
    }

    #[test]
    fn plain_text_urls_outside_anchors_are_kept_and_na_anchors_count() {
        let html = format!(
            "{}{}",
            &FIXTURE[..FIXTURE.find("</tbody>").unwrap()],
            "<tr><th>9999</th><td>God's Country</td><td>Test Song</td><td></td><td></td><td></td>\
             <td></td><td>Snippet</td><td>Recording</td><td><a href=\"https://www.google.com/url?q=https://imgur.gg/f/AAAA&amp;sa=D\">N/A</a>\
             <br>https://ibb.co/xyz https://x.com/a/status/2</td></tr></tbody></table></body></html>"
        );
        let catalog = parse_sheet(&html).unwrap();
        let song = one(&catalog, "Test Song");
        assert_eq!(
            song.links,
            [
                "https://imgur.gg/f/AAAA",
                "https://ibb.co/xyz",
                "https://x.com/a/status/2"
            ]
        );
        assert_eq!(song.download_url(), Some("https://imgur.gg/f/AAAA"));
    }

    #[test]
    fn sanitize_image_url_upgrades_google_render_spec() {
        assert_eq!(
            sanitize_image_url("https://docs.google.com/sheets-images-rt/abc=w102-h104"),
            "https://docs.google.com/sheets-images-rt/abc=s512"
        );
        assert_eq!(
            sanitize_image_url("https://img.example/a.jpg?utm_source=x&keep=1"),
            "https://img.example/a.jpg?keep=1"
        );
        assert_eq!(sanitize_image_url("http://img.example/a.jpg"), "");
    }

    #[test]
    fn the_fingerprint_ignores_artwork_urls_only() {
        let catalog = fixture();
        let mut rerendered = catalog.clone();
        rerendered.eras[0].image_url = "https://docs.google.com/sheets-images-rt/other=s512".into();
        assert_eq!(catalog.fingerprint(), rerendered.fingerprint());
        let mut edited = catalog.clone();
        edited.songs[3].notes = Some("edited".into());
        assert_ne!(catalog.fingerprint(), edited.fingerprint());
        // Signatures in the redirect links change on every render too.
        let rendered_again = FIXTURE.replace("ust=17907", "ust=18907");
        assert_ne!(rendered_again, FIXTURE);
        assert_eq!(
            parse_sheet(&rendered_again).unwrap().fingerprint(),
            catalog.fingerprint()
        );
    }

    #[test]
    fn a_repeated_import_of_the_same_catalog_is_a_no_op() {
        let conn = database();
        let catalog = fixture();
        let first = import(&conn, &catalog);
        assert!(!first.unchanged);
        assert_eq!((first.eras_added, first.songs_added), (4, 30));
        assert_eq!(first.files_added, 23);
        let ids = stored_ids(&conn);
        assert_eq!(ids.len(), 30);
        let mut sorted: Vec<i64> = ids.values().copied().collect();
        sorted.sort();
        assert_eq!(sorted, (1..=30).collect::<Vec<_>>());

        let second = apply_import(
            &conn,
            &catalog,
            &ImportOptions {
                force: false,
                now: NOW + 60,
            },
        )
        .unwrap();
        assert!(second.unchanged);
        assert_eq!(stored_ids(&conn), ids);
        assert_eq!(
            scalar::<i64>(
                &conn,
                "SELECT count(*) FROM files WHERE last_seen_at = 1790000060"
            ),
            23
        );
        assert_eq!(
            db::meta_get(&conn, meta_keys::LAST_IMPORT_AT)
                .unwrap()
                .as_deref(),
            Some("1790000060")
        );
        assert_eq!(
            db::meta_get(&conn, meta_keys::LAST_IMPORT_OK)
                .unwrap()
                .as_deref(),
            Some("true")
        );
    }

    #[test]
    fn stored_rows_carry_the_derived_columns() {
        let conn = database();
        import(&conn, &fixture());
        let field = |column: &str| -> rusqlite::types::Value {
            scalar(
                &conn,
                &format!("SELECT {column} FROM songs WHERE title = '✨ Brothers [V11]'"),
            )
        };
        let text = |value: &str| rusqlite::types::Value::Text(value.to_string());
        let integer = rusqlite::types::Value::Integer;
        assert_eq!(
            field("name"),
            text("✨ Brothers [V11]\n(prod. Kanye West, Andrew Dawson, 7 Aurelius & Irv Gotti)")
        );
        assert_eq!(field("title"), text("✨ Brothers [V11]"));
        assert_eq!(field("sub_era"), text("Pre-Yandhi"));
        assert_eq!(field("links"), text(r#"["https://imgur.gg/f/8VNcQVw"]"#));
        assert_eq!(field("notes_links"), text("[]"));
        assert_eq!(field("category_rank"), integer(1));
        assert_eq!(field("sort_title"), text("brothers v0000000011"));
        assert_eq!(field("leak_date_precision"), text("day"));
        assert_eq!(field("position"), integer(18));
        assert_eq!(field("era_position"), integer(2));
        let search_text: String = scalar(
            &conn,
            "SELECT search_text FROM songs WHERE title = 'Beat 1'",
        );
        assert!(search_text.starts_with("beat 1 prod kanye west track 1 from the september 1997"));
        assert!(
            search_text.ends_with(
                "before the college dropout world record holders high quality beat only"
            )
        );
        let song_search_text: String = scalar(
            &conn,
            "SELECT song_search_text FROM songs WHERE title = 'Beat 1'",
        );
        assert!(song_search_text.starts_with("beat 1 prod kanye west track 1 from the september"));
        assert!(song_search_text.ends_with(" high quality beat only"));
        assert!(!song_search_text.contains("college dropout"));
        let notes_links: String = scalar(
            &conn,
            "SELECT notes_links FROM songs WHERE title = 'Beat 1'",
        );
        assert_eq!(
            notes_links,
            r#"[{"text":"\"The World is a Ghetto\" by George Benson","url":"https://youtu.be/I9HZe5vP5-M"},{"text":"the Common vs. Kanye freestyle battle","url":"https://imgur.gg/f/nhOhAwL"}]"#
        );
        let missing_dates: (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT file_date, file_date_precision FROM songs WHERE title = '187th'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(missing_dates, (None, None));
        let eras: Vec<(i64, String, Option<String>, i64)> = conn
            .prepare("SELECT id, name, subtitle, position FROM eras ORDER BY position")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            eras[1],
            (
                2,
                "SWISH".into(),
                Some("(SWISH: The Overground Hell Road)".into()),
                2
            )
        );
        assert_eq!(
            scalar::<String>(&conn, "SELECT dominant_color FROM eras WHERE id = 1"),
            "666666"
        );
    }

    #[test]
    fn inserting_or_removing_rows_upstream_keeps_every_other_id() {
        let conn = database();
        import(&conn, &fixture());
        let before = stored_ids(&conn);

        let inserted = with_copied_row(FIXTURE, 3032, "Brothers [V10]", "Brothers [V10.5]");
        let catalog = parse_sheet(&inserted).unwrap();
        let summary = import(&conn, &catalog);
        assert_eq!((summary.songs_added, summary.songs_removed), (1, 0));
        let after = stored_ids(&conn);
        for (key, id) in &before {
            assert_eq!(after.get(key), Some(id), "id of {key} changed");
        }
        let new_id: i64 = scalar(
            &conn,
            "SELECT id FROM songs WHERE title = 'Brothers [V10.5]'",
        );
        assert_eq!(new_id, 31);
        let positions: Vec<(String, i64, i64)> = conn
            .prepare("SELECT title, position, era_position FROM songs WHERE era = 3 ORDER BY position LIMIT 3")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            positions,
            [
                ("Brothers [V10]".into(), 17, 1),
                ("Brothers [V10.5]".into(), 18, 2),
                ("✨ Brothers [V11]".into(), 19, 3)
            ]
        );

        // Edited notes keep the id (matched by era, name and link); a removed
        // row's id is never handed out to another song, and the song gets it
        // back when it returns.
        let brothers: i64 = scalar(&conn, "SELECT id FROM songs WHERE title = 'Brothers [V10]'");
        let edited = inserted.replacen(
            "Track 1 from the September 1997 demo beat tape.",
            "Track 1 of the 1997 tape.",
            1,
        );
        let removed = without_row(&edited, 3032);
        let summary = import(&conn, &parse_sheet(&removed).unwrap());
        assert_eq!(summary.songs_matched.link, 1);
        assert_eq!(summary.songs_removed, 1);
        assert_eq!(
            scalar::<i64>(&conn, "SELECT id FROM songs WHERE title = 'Beat 1'"),
            6
        );
        assert_eq!(
            scalar::<i64>(&conn, "SELECT count(*) FROM song_tombstones"),
            1
        );
        let restored = import(&conn, &parse_sheet(&inserted).unwrap());
        assert_eq!(
            (restored.songs_added, restored.songs_matched.restored),
            (0, 1)
        );
        assert_eq!(
            scalar::<i64>(&conn, "SELECT id FROM songs WHERE title = 'Brothers [V10]'"),
            brothers
        );
        assert_eq!(
            scalar::<i64>(&conn, "SELECT count(*) FROM song_tombstones"),
            0
        );
        assert_eq!(
            db::meta_get_i64(&conn, meta_keys::NEXT_SONG_ID).unwrap(),
            Some(32)
        );
    }

    #[test]
    fn a_catalog_that_shrank_is_refused_unless_forced() {
        let conn = database();
        let catalog = fixture();
        import(&conn, &catalog);
        conn.execute_batch(
            "WITH RECURSIVE n(i) AS (SELECT 1000 UNION ALL SELECT i + 1 FROM n WHERE i < 1199) \
             INSERT INTO songs (id, era, name, position, song_key) SELECT i, 1, 'filler', i, 'k' || i FROM n;",
        )
        .unwrap();
        let error = apply_import(
            &conn,
            &catalog,
            &ImportOptions {
                force: false,
                now: NOW,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("IMPORT_FORCE"), "{error}");
        assert_eq!(scalar::<i64>(&conn, "SELECT count(*) FROM songs"), 230);
        let forced = apply_import(
            &conn,
            &catalog,
            &ImportOptions {
                force: true,
                now: NOW,
            },
        )
        .unwrap();
        assert_eq!(forced.songs_removed, 200);
        assert_eq!(scalar::<i64>(&conn, "SELECT count(*) FROM songs"), 30);
        assert_eq!(
            db::meta_get_i64(&conn, meta_keys::NEXT_SONG_ID).unwrap(),
            Some(1200)
        );
    }

    #[test]
    fn file_rows_start_pending_and_are_never_deleted() {
        let conn = database();
        import(&conn, &fixture());
        assert_eq!(
            scalar::<i64>(
                &conn,
                "SELECT count(*) FROM files WHERE status = 'pending' AND filename IS NULL \
                 AND downloaded IS NULL AND last_seen_at = 1790000000"
            ),
            23
        );
        conn.execute_batch(
            "UPDATE files SET status = 'downloaded', downloaded = 1, filename = 'a.mp3' \
             WHERE url = 'https://imgur.gg/f/kAuV1tA'",
        )
        .unwrap();
        let without = without_row(FIXTURE, 3032);
        let summary = apply_import(
            &conn,
            &parse_sheet(&without).unwrap(),
            &ImportOptions {
                force: false,
                now: NOW + 5,
            },
        )
        .unwrap();
        assert_eq!((summary.files_seen, summary.files_added), (22, 0));
        let kept: (String, String, i64) = conn
            .query_row(
                "SELECT status, filename, last_seen_at FROM files WHERE url = 'https://imgur.gg/f/kAuV1tA'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(kept, ("downloaded".into(), "a.mp3".into(), NOW));
    }

    /// A catalog of one era with `count` songs whose only link is `link(i)`.
    fn synthetic(count: usize, link: impl Fn(usize) -> String) -> ParsedCatalog {
        let eras = vec![ParsedEra {
            key: "era".into(),
            name: "Era".into(),
            subtitle: None,
            notes: String::new(),
            description: String::new(),
            image_url: String::new(),
        }];
        let songs = (0..count)
            .map(|index| {
                let name = format!("Song {index}");
                let links = vec![link(index)];
                ParsedSong {
                    era: 0,
                    era_position: index as i64 + 1,
                    title: name.clone(),
                    key: song_key("era", &name, None, Some(&links[0]), None),
                    name,
                    sub_era: None,
                    notes: None,
                    notes_links: Vec::new(),
                    links,
                    file_date: None,
                    leak_date: None,
                    track_length: None,
                    track_length_approx: false,
                    available_length: Some("Full".into()),
                    quality: Some("CD Quality".into()),
                }
            })
            .collect();
        ParsedCatalog {
            eras,
            songs,
            stats: ParseStats::default(),
        }
    }

    fn import_with(
        conn: &Connection,
        catalog: &ParsedCatalog,
        force: bool,
        now: i64,
    ) -> Result<ImportSummary, ApiError> {
        apply_import(conn, catalog, &ImportOptions { force, now })
    }

    #[test]
    fn an_import_that_loses_most_download_links_is_refused_unless_forced() {
        let conn = database();
        let imgur = |index: usize| format!("https://imgur.gg/f/file{index}");
        import(&conn, &synthetic(150, imgur));
        conn.execute_batch(
            "UPDATE files SET status = 'downloaded', downloaded = 1, filename = 'a.mp3' \
             WHERE url = 'https://imgur.gg/f/file0'",
        )
        .unwrap();
        // Same songs, but most links now come wrapped in a way the importer
        // does not unwrap: nothing to download any more.
        let wrapped = synthetic(150, |index| {
            if index < 100 {
                format!("https://www.google.com/url?target=https://imgur.gg/f/file{index}")
            } else {
                imgur(index)
            }
        });
        let error = import_with(&conn, &wrapped, false, NOW + 60).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("downloadable links dropped from 150 to 50"),
            "{message}"
        );
        assert!(message.contains("IMPORT_FORCE"), "{message}");
        // Nothing was written: links, files and the import time are unchanged.
        assert_eq!(
            scalar::<i64>(
                &conn,
                "SELECT count(*) FROM songs WHERE url LIKE 'https://imgur.gg/%'"
            ),
            150
        );
        assert_eq!(
            scalar::<i64>(
                &conn,
                "SELECT count(*) FROM files WHERE last_seen_at = 1790000000"
            ),
            150
        );
        assert_eq!(
            db::meta_get_i64(&conn, meta_keys::LAST_IMPORT_AT).unwrap(),
            Some(NOW)
        );
        // A smaller loss passes.
        let fewer = synthetic(150, |index| {
            if index < 20 {
                format!("https://example.com/{index}")
            } else {
                imgur(index)
            }
        });
        assert_eq!(
            import_with(&conn, &fewer, false, NOW + 90)
                .unwrap()
                .download_links,
            130
        );
        // Forcing accepts the loss.
        let forced = import_with(&conn, &wrapped, true, NOW + 120).unwrap();
        assert_eq!(forced.download_links, 50);
    }

    #[test]
    fn links_that_stay_in_the_sheet_keep_their_rows_seen() {
        let conn = database();
        import(&conn, &fixture());
        // A pillows.su link joins "30 Hours [V8]" and becomes its primary
        // link; the imgur.gg one it had is still listed.
        let html = FIXTURE.replacen(
            ">https://imgur.gg/f/LeeWJuS</a></span>",
            ">https://imgur.gg/f/LeeWJuS</a></span><a href=\"https://pillows.su/f/abc123\">pillows</a>",
            1,
        );
        assert_ne!(html, FIXTURE);
        let catalog = parse_sheet(&html).unwrap();
        assert_eq!(
            one(&catalog, "30 Hours [V8]").url(),
            Some("https://pillows.su/f/abc123")
        );
        import_with(&conn, &catalog, false, NOW + 600).unwrap();
        let seen = |url: &str| -> i64 {
            conn.query_row(
                "SELECT last_seen_at FROM files WHERE url = ?1",
                [url],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(seen("https://pillows.su/f/abc123"), NOW + 600);
        assert_eq!(seen("https://imgur.gg/f/8jCuohn"), NOW + 600);
    }

    #[test]
    fn an_era_row_without_artwork_is_still_an_era() {
        let conn = database();
        import(&conn, &fixture());
        let before = stored_ids(&conn);
        let range = row_range(FIXTURE, 3030);
        let row = &FIXTURE[range.clone()];
        let image_start = row.find("<img").expect("era row has artwork");
        let image_end = image_start + row[image_start..].find("/>").unwrap() + 2;
        let html = format!(
            "{}{}{}{}",
            &FIXTURE[..range.start],
            &row[..image_start],
            &row[image_end..],
            &FIXTURE[range.end..]
        );
        let catalog = parse_sheet(&html).unwrap();
        assert_eq!(catalog.eras.len(), 4);
        assert_eq!(catalog.eras[2].name, "Yandhi [V1]");
        assert_eq!(catalog.eras[2].image_url, "");
        assert_eq!(catalog.stats.eras_without_artwork, 1);
        assert_eq!(catalog.stats.unknown_era_rows, 1, "only the edited row");
        let summary = import(&conn, &catalog);
        assert_eq!((summary.eras_added, summary.eras_removed), (0, 0));
        assert_eq!(stored_ids(&conn), before);
    }

    #[test]
    fn songs_naming_an_era_whose_row_vanished_are_not_moved_silently() {
        let conn = database();
        import(&conn, &fixture());
        let before = stored_ids(&conn);
        // The God's Country era row is gone (e.g. it changed shape); its song
        // rows still name it.
        let html = without_row(FIXTURE, 4243);
        let catalog = parse_sheet(&html).unwrap();
        assert_eq!(catalog.eras.len(), 3);
        let error = import_with(&conn, &catalog, false, NOW + 60).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("God's Country"), "{message}");
        assert!(message.contains("(5 songs)"), "{message}");
        assert_eq!(stored_ids(&conn), before);
        assert_eq!(scalar::<i64>(&conn, "SELECT count(*) FROM eras"), 4);
        // Forced, the songs go to the era of their section.
        let forced = import_with(&conn, &catalog, true, NOW + 120).unwrap();
        assert_eq!(forced.eras_removed, 1);
    }

    #[test]
    fn a_renamed_era_keeps_its_id_and_its_songs_keep_theirs() {
        let conn = database();
        import(&conn, &fixture());
        let before = stored_ids(&conn);
        let swish: i64 = scalar(&conn, "SELECT id FROM eras WHERE name = 'SWISH'");
        let renamed = FIXTURE.replace(">SWISH<", ">SWISH (2016)<");
        let catalog = parse_sheet(&renamed).unwrap();
        assert_eq!(catalog.eras[1].name, "SWISH (2016)");
        let summary = import(&conn, &catalog);
        assert_eq!(
            (
                summary.eras_renamed,
                summary.eras_added,
                summary.eras_removed
            ),
            (1, 0, 0)
        );
        assert_eq!((summary.songs_added, summary.songs_removed), (0, 0));
        assert_eq!(
            scalar::<i64>(&conn, "SELECT id FROM eras WHERE name = 'SWISH (2016)'"),
            swish
        );
        let after: Vec<i64> = {
            let mut ids: Vec<i64> = stored_ids(&conn).into_values().collect();
            ids.sort();
            ids
        };
        let mut expected: Vec<i64> = before.values().copied().collect();
        expected.sort();
        assert_eq!(after, expected);
        assert_eq!(
            scalar::<i64>(
                &conn,
                "SELECT count(*) FROM songs WHERE era = ?1"
                    .replace("?1", &swish.to_string())
                    .as_str()
            ),
            7
        );
    }

    #[test]
    fn a_song_moved_to_another_era_keeps_its_id() {
        let conn = database();
        import(&conn, &fixture());
        let id: i64 = scalar(&conn, "SELECT id FROM songs WHERE title = '30 Hours [V8]'");
        // Cut the row out of SWISH and paste it into Yandhi [V1].
        let range = row_range(FIXTURE, 2063);
        let row = FIXTURE[range.clone()].replacen(">SWISH<", ">Yandhi [V1]<", 1);
        let without = format!("{}{}", &FIXTURE[..range.start], &FIXTURE[range.end..]);
        let target = row_range(&without, 3032);
        let moved = format!("{}{row}{}", &without[..target.end], &without[target.end..]);
        let catalog = parse_sheet(&moved).unwrap();
        let song = one(&catalog, "30 Hours [V8]");
        assert_eq!(catalog.eras[song.era].name, "Yandhi [V1]");
        let summary = import(&conn, &catalog);
        assert_eq!(summary.songs_matched.moved, 1);
        assert_eq!((summary.songs_added, summary.songs_removed), (0, 0));
        let (moved_id, era): (i64, String) = conn
            .query_row(
                "SELECT songs.id, eras.name FROM songs JOIN eras ON eras.id = songs.era \
                 WHERE songs.title = '30 Hours [V8]'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((moved_id, era.as_str()), (id, "Yandhi [V1]"));
    }

    #[test]
    fn a_deleted_era_and_its_songs_get_their_ids_back_when_restored() {
        let conn = database();
        import(&conn, &fixture());
        let before = stored_ids(&conn);
        let era_id: i64 = scalar(&conn, "SELECT id FROM eras WHERE name = 'God''s Country'");
        // The whole era goes: its era row and every song row under it.
        let mut html = FIXTURE.to_string();
        for number in [4243, 4244, 4254, 4391, 4395, 4396, 4476] {
            html = without_row(&html, number);
        }
        let catalog = parse_sheet(&html).unwrap();
        assert_eq!(catalog.eras.len(), 3);
        let removed = import_with(&conn, &catalog, false, NOW + 60).unwrap();
        assert_eq!((removed.eras_removed, removed.songs_removed), (1, 4));
        assert_eq!(
            scalar::<i64>(&conn, "SELECT count(*) FROM song_tombstones"),
            4
        );
        // A new era meanwhile gets a fresh id.
        let restored = import_with(&conn, &fixture(), false, NOW + 120).unwrap();
        assert_eq!(
            (
                restored.eras_restored,
                restored.songs_matched.restored,
                restored.songs_added
            ),
            (1, 4, 0)
        );
        assert_eq!(stored_ids(&conn), before);
        assert_eq!(
            scalar::<i64>(&conn, "SELECT id FROM eras WHERE name = 'God''s Country'"),
            era_id
        );
        assert_eq!(
            scalar::<i64>(&conn, "SELECT count(*) FROM song_tombstones")
                + scalar::<i64>(&conn, "SELECT count(*) FROM era_tombstones"),
            0
        );
    }

    #[test]
    fn tombstones_expire() {
        let conn = database();
        import(&conn, &fixture());
        let without = without_row(FIXTURE, 3033);
        import_with(&conn, &parse_sheet(&without).unwrap(), false, NOW + 60).unwrap();
        let id: i64 = scalar(&conn, "SELECT id FROM song_tombstones");
        // 91 days later an unrelated change prunes the tombstone, so the
        // song comes back with a new id.
        let later = NOW + 91 * 24 * 60 * 60;
        let edited = without.replacen(
            "Track 1 from the September 1997 demo beat tape.",
            "Track 1 of the 1997 tape.",
            1,
        );
        assert_ne!(edited, without);
        import_with(&conn, &parse_sheet(&edited).unwrap(), false, later).unwrap();
        assert_eq!(
            scalar::<i64>(&conn, "SELECT count(*) FROM song_tombstones"),
            0
        );
        let summary =
            import_with(&conn, &parse_sheet(FIXTURE).unwrap(), false, later + 60).unwrap();
        assert_eq!(summary.songs_matched.restored, 0);
        assert_ne!(
            scalar::<i64>(
                &conn,
                "SELECT id FROM songs WHERE title = '✨ Brothers [V11]'"
            ),
            id
        );
    }

    #[tokio::test]
    async fn fetch_errors_keep_their_cause() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let Err(FetchError::Retryable(message)) =
            fetch_once(&client, "http://127.0.0.1:1/sheet").await
        else {
            panic!("a refused connection is worth retrying");
        };
        assert!(
            message.to_lowercase().contains("connection refused"),
            "{message}"
        );
    }

    /// The v1 schema and a v1-style import of the fixture: collapsed names,
    /// 0 for missing dates, no approximate lengths, a placeholder era for the
    /// Hollywood Bowl row, and the rows v1 deduplicated left out.
    fn v1_database(songs_dir: &Path) -> (Connection, HashMap<String, i64>) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE eras (id INTEGER PRIMARY KEY, name TEXT, notes TEXT, image_url TEXT, \
               description TEXT, dominant_color TEXT, cover_source TEXT, is_main INTEGER NOT NULL DEFAULT 1);
             CREATE TABLE songs (id INTEGER PRIMARY KEY, era INTEGER, catalog_id TEXT NOT NULL DEFAULT 'unreleased', \
               name TEXT, notes TEXT, file_date INTEGER, leak_date INTEGER, available_length TEXT, \
               track_length INTEGER, quality TEXT, url TEXT);
             CREATE TABLE files (url TEXT PRIMARY KEY, downloaded INTEGER, filename TEXT, duration REAL);
             CREATE INDEX songs_quality_index ON songs (quality);
             INSERT INTO eras (id, name, notes, image_url, description, dominant_color, is_main) VALUES
               (1, 'Before The College Dropout', '', '', '', '112233', 1),
               (2, 'SWISH', '', '', '', NULL, 1),
               (3, 'x', '', '', '', NULL, 0),
               (4, 'Yandhi [V1]', '', '', '', NULL, 1),
               (5, 'God''s Country', '', '', '', NULL, 1);",
        )
        .unwrap();
        let catalog = fixture();
        let v1_era = [1, 2, 4, 5];
        let mut ids = HashMap::new();
        let mut next_id = 1;
        for song in &catalog.songs {
            // v1 kept the first of rows sharing era, name and notes.
            if song.era == 2
                && song.title == "???"
                && matches!(song.track_length, Some(35) | Some(177))
            {
                continue;
            }
            if song.title == "Amazing" {
                conn.execute(
                    "INSERT INTO songs (id, era, name, notes, file_date, leak_date) \
                     VALUES (?1, 3, '808s & Heartbreak at the Hollywood Bowl', '(09/25/2015) …', 0, 0)",
                    [next_id],
                )
                .unwrap();
                next_id += 1;
            }
            let url = if song.title == "Consequence - The Good, The Bad, The Ugly [V1]" {
                // v1 ignored http links and took the first https one.
                Some("https://example.com/other-order".to_string())
            } else {
                song.url().map(str::to_string)
            };
            conn.execute(
                "INSERT INTO songs (id, era, name, notes, file_date, leak_date, available_length, \
                 track_length, quality, url) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    next_id,
                    v1_era[song.era],
                    text::clean_line(&song.name),
                    song.notes.as_deref().map(text::clean_line),
                    song.file_date.map_or(0, |date| date.timestamp),
                    song.leak_date.map_or(0, |date| date.timestamp),
                    song.available_length,
                    song.track_length.filter(|_| !song.track_length_approx),
                    song.quality,
                    url,
                ],
            )
            .unwrap();
            ids.insert(song.key.clone(), next_id);
            next_id += 1;
        }
        std::fs::create_dir_all(songs_dir).unwrap();
        std::fs::write(songs_dir.join("on-disk.wav"), b"RIFF").unwrap();
        // A TEXT primary key may be NULL in SQLite; old versions wrote such a
        // row now and then.
        conn.execute_batch(
            "INSERT INTO files (url, downloaded, filename) VALUES
               ('https://imgur.gg/f/kYj3fdI', 1, 'on-disk.wav'),
               ('https://imgur.gg/f/A3zZucA', 1, 'gone.wav'),
               ('https://imgur.gg/f/ajJNgTI', 0, 'deadbeef'),
               ('https://pillows.su/f/stale', 2, 'deadbeef2'),
               (NULL, 1, 'on-disk.wav');",
        )
        .unwrap();
        (conn, ids)
    }

    #[test]
    fn a_v1_database_is_upgraded_in_place_and_keeps_its_ids() {
        let dir = std::env::temp_dir().join(format!("yt-v1-upgrade-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let covers = dir.join("covers");
        std::fs::create_dir_all(&covers).unwrap();
        std::fs::write(covers.join("2.avif"), b"abc").unwrap();
        let songs_dir = dir.join("songs");
        let (conn, v1_ids) = v1_database(&songs_dir);
        assert_eq!(v1_ids.len(), 28);

        db::migrate(&conn, &covers, &songs_dir).unwrap();
        assert_eq!(
            db::meta_get_i64(&conn, meta_keys::SCHEMA_VERSION).unwrap(),
            Some(2)
        );
        assert_eq!(
            db::meta_get_i64(&conn, meta_keys::NEXT_SONG_ID).unwrap(),
            Some(30)
        );
        assert_eq!(
            db::meta_get_i64(&conn, meta_keys::NEXT_ERA_ID).unwrap(),
            Some(6)
        );
        // The API keeps working before the first v2 import.
        assert_eq!(
            scalar::<i64>(
                &conn,
                "SELECT count(*) FROM songs WHERE position IS NULL OR title IS NULL OR search_text IS NULL OR sort_title IS NULL OR category_rank IS NULL OR song_key IS NULL OR era_position IS NULL"
            ),
            0
        );
        assert_eq!(
            scalar::<i64>(
                &conn,
                "SELECT count(*) FROM songs WHERE leak_date = 0 OR file_date = 0"
            ),
            0
        );
        assert_eq!(
            scalar::<String>(&conn, "SELECT cover_version FROM eras WHERE id = 2"),
            "ba7816bf8f01"
        );
        assert_eq!(
            scalar::<Option<String>>(&conn, "SELECT cover_version FROM eras WHERE id = 1"),
            None
        );
        assert_eq!(
            scalar::<i64>(&conn, "SELECT position FROM eras WHERE id = 4"),
            3
        );
        assert_eq!(
            scalar::<i64>(&conn, "SELECT position FROM eras WHERE id = 3"),
            5
        );
        let files: Vec<(String, String, Option<String>, Option<i64>)> = conn
            .prepare("SELECT url, status, filename, downloaded FROM files ORDER BY url")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            files,
            [
                (
                    "https://imgur.gg/f/A3zZucA".into(),
                    "pending".into(),
                    None,
                    Some(0)
                ),
                (
                    "https://imgur.gg/f/ajJNgTI".into(),
                    "pending".into(),
                    None,
                    Some(0)
                ),
                (
                    "https://imgur.gg/f/kYj3fdI".into(),
                    "downloaded".into(),
                    Some("on-disk.wav".into()),
                    Some(1)
                ),
                (
                    "https://pillows.su/f/stale".into(),
                    "pending".into(),
                    None,
                    Some(2)
                ),
            ]
        );
        let index_names: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(!index_names.contains(&"songs_quality_index".to_string()));
        assert!(index_names.contains(&"eras_key_index".to_string()));
        // Running the migrations again changes nothing.
        db::migrate(&conn, &covers, &songs_dir).unwrap();
        assert_eq!(
            db::meta_get_i64(&conn, meta_keys::NEXT_SONG_ID).unwrap(),
            Some(30)
        );

        let summary = import(&conn, &fixture());
        assert_eq!(summary.songs_removed, 1, "the Hollywood Bowl header row");
        assert_eq!(summary.songs_added, 2, "the two snippets v1 deduplicated");
        assert_eq!(summary.eras_removed, 1, "the placeholder era");
        assert_eq!(summary.eras_added, 0);
        let after = stored_ids(&conn);
        for (key, id) in &v1_ids {
            assert_eq!(after.get(key), Some(id), "v1 id {id} changed");
        }
        let mut new_ids: Vec<i64> = after.values().filter(|id| **id >= 30).copied().collect();
        new_ids.sort();
        assert_eq!(new_ids, [30, 31]);
        let eras: Vec<i64> = conn
            .prepare("SELECT id FROM eras ORDER BY position")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(eras, [1, 2, 4, 5]);
        assert_eq!(
            scalar::<String>(&conn, "SELECT dominant_color FROM eras WHERE id = 1"),
            "112233"
        );
        assert_eq!(
            scalar::<String>(&conn, "SELECT cover_version FROM eras WHERE id = 2"),
            "ba7816bf8f01"
        );
        assert_eq!(
            scalar::<i64>(&conn, "SELECT count(*) FROM files"),
            23 + 1,
            "the unseen pillows.su row stays"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Manual check against a saved `htmlview/sheet` response:
    /// `IMPORT_SHEET=/path/sheet.html cargo test --lib live_sheet_snapshot -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_sheet_snapshot() {
        let path = std::env::var("IMPORT_SHEET").expect("set IMPORT_SHEET to the saved HTML path");
        let html = std::fs::read_to_string(&path).expect("read sheet html");
        let started = Instant::now();
        let catalog = parse_sheet(&html).unwrap();
        println!(
            "eras={} songs={} download_links={} parse_ms={} stats={:?}",
            catalog.eras.len(),
            catalog.songs.len(),
            catalog.download_links().len(),
            started.elapsed().as_millis(),
            catalog.stats
        );
        assert!(catalog.eras.iter().all(|era| !era.description.is_empty()));
    }
}
