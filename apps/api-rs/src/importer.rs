//! Catalog importer, ported from `scraper/importer.ts`.
//!
//! Fetches each catalog's HTML sheet view, parses the table rows into fresh
//! `eras`/`songs` tables, and replaces the contents transactionally while
//! preserving existing cover colours and `files` rows for still-present URLs.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{NaiveDate, NaiveDateTime};
use futures_util::stream::{self, StreamExt};
use once_cell::sync::Lazy;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use sha2::{Digest, Sha256};
use url::Url;

use crate::catalogs::{catalog_source_url, CatalogDefinition, CATALOGS, PRIMARY_CATALOG_ID};
use crate::db;
use crate::error::ApiError;
use crate::state::SharedState;
use crate::text;

const CATALOG_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOADABLE_HOSTS: [&str; 5] = [
    "pillows.su",
    "youtu.be",
    "www.youtube.com",
    "www.instagram.com",
    "twitter.com",
];
const TRACKING_PARAMS: [&str; 6] = [
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
];
const FETCH_USER_AGENT: &str = "yetracker-viewer/1.0 (+https://yetracker.net)";

static ROW_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<tr[\s>][\s\S]*?</tr\s*>").expect("valid row regex"));
static LINE_BREAK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)<br\s*/?>").expect("valid br regex"));
static CELL_SELECTOR: Lazy<Selector> = Lazy::new(|| Selector::parse("td, th").expect("valid selector"));
static IMG_SELECTOR: Lazy<Selector> = Lazy::new(|| Selector::parse("img").expect("valid selector"));
static IMAGE_RENDER_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"=[whs]\d+(?:-[a-z0-9]+)*$").expect("valid image render regex"));

#[derive(Clone)]
struct EraRecord {
    id: i64,
    name: String,
    notes: String,
    image_url: String,
    description: String,
    dominant_color: Option<String>,
    cover_source: Option<String>,
    is_main: i64,
}

struct ImportedSong {
    id: i64,
    catalog_id: String,
    era_id: i64,
    name: String,
    notes: Option<String>,
    track_length: Option<i64>,
    file_date: i64,
    leak_date: i64,
    available_length: Option<String>,
    quality: Option<String>,
    url: Option<String>,
}

struct ImportState {
    eras: Vec<EraRecord>,
    songs: Vec<ImportedSong>,
    urls: Vec<(String, String)>,
    era_by_name: HashMap<String, usize>,
    seen_main_songs: HashSet<String>,
    next_era_id: i64,
    next_song_id: i64,
    main_era_count: i64,
    main_song_count: i64,
}

impl ImportState {
    fn new() -> Self {
        Self {
            eras: Vec::new(),
            songs: Vec::new(),
            urls: Vec::new(),
            era_by_name: HashMap::new(),
            seen_main_songs: HashSet::new(),
            next_era_id: 1,
            next_song_id: 1,
            main_era_count: 0,
            main_song_count: 0,
        }
    }
}

fn normalize_text(value: &str) -> String {
    text::collapse_whitespace(value)
}

fn normalize_header(value: &str) -> String {
    normalize_text(value).to_lowercase()
}

fn normalize_era_name(value: &str) -> String {
    let normalized = normalize_text(value);
    if normalized == "Travis Scott Collaboration" {
        "Collaboration with Travis Scott".to_string()
    } else {
        normalized
    }
}

fn normalize_era_key(value: &str) -> String {
    normalize_era_name(value).to_lowercase()
}

fn find_column(headers: &[String], predicate: impl Fn(&str) -> bool) -> i32 {
    headers.iter().position(|header| predicate(header)).map(|index| index as i32).unwrap_or(-1)
}

fn cell_text(cells: &[ElementRef<'_>], index: i32) -> String {
    if index < 0 {
        return String::new();
    }
    cells
        .get(index as usize)
        .map(|cell| normalize_text(&cell.text().collect::<String>()))
        .unwrap_or_default()
}

fn parse_duration(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let raw_parts: Vec<&str> = trimmed.split(':').collect();
    if raw_parts.len() < 2 || raw_parts.len() > 3 {
        return None;
    }
    let mut parts = Vec::with_capacity(raw_parts.len());
    for part in raw_parts {
        let parsed: f64 = if part.is_empty() { 0.0 } else { part.parse().ok()? };
        if !parsed.is_finite() || parsed.fract() != 0.0 || parsed < 0.0 {
            return None;
        }
        parts.push(parsed as i64);
    }

    if parts.len() == 2 {
        let (minutes, seconds) = (parts[0], parts[1]);
        if seconds >= 60 {
            return None;
        }
        return Some(minutes * 60 + seconds);
    }
    let (hours, minutes, seconds) = (parts[0], parts[1], parts[2]);
    if minutes >= 60 || seconds >= 60 {
        return None;
    }
    Some(hours * 3600 + minutes * 60 + seconds)
}

fn parse_date(value: &str) -> i64 {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return 0;
    }
    if let Ok(datetime) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return datetime.timestamp();
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(datetime) = NaiveDateTime::parse_from_str(trimmed, format) {
            return datetime.and_utc().timestamp();
        }
    }
    for format in [
        "%Y-%m-%d",
        "%Y/%m/%d",
        "%m/%d/%Y",
        "%B %d, %Y",
        "%B %d %Y",
        "%b %d, %Y",
        "%b %d %Y",
        "%d %B %Y",
        "%d %b %Y",
        "%B %Y",
        "%b %Y",
    ] {
        if let Ok(date) = NaiveDate::parse_from_str(trimmed, format) {
            if let Some(datetime) = date.and_hms_opt(0, 0, 0) {
                return datetime.and_utc().timestamp();
            }
        }
    }
    if trimmed.len() == 4 {
        if let Ok(year) = trimmed.parse::<i32>() {
            if let Some(date) = NaiveDate::from_ymd_opt(year, 1, 1) {
                if let Some(datetime) = date.and_hms_opt(0, 0, 0) {
                    return datetime.and_utc().timestamp();
                }
            }
        }
    }
    0
}

fn parse_urls(value: &str) -> Vec<Url> {
    let mut urls = Vec::new();
    for part in value.split(text::is_js_whitespace).filter(|part| !part.is_empty()) {
        if !part.starts_with("https://") {
            continue;
        }
        if let Ok(url) = Url::parse(part) {
            if url.scheme() == "https" {
                urls.push(url);
            }
        }
    }
    urls
}

fn get_source_url(value: &str) -> Option<String> {
    let urls = parse_urls(value);
    urls.iter()
        .find(|url| url.host_str() == Some("pillows.su"))
        .or_else(|| urls.first())
        .map(|url| url.to_string())
}

fn can_download(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(|host| DOWNLOADABLE_HOSTS.contains(&host)))
        .unwrap_or(false)
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

fn parse_available_length(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    match value {
        "Full" | "Snippet" | "Confirmed" | "Beat Only" | "Partial" | "Tagged" | "OG File" | "Stem Bounce"
        | "Rumored" | "Conflicting Sources" => Some(value.to_string()),
        _ => {
            eprintln!("Unknown AvailableLength \"{value}\", defaulting to null");
            None
        }
    }
}

fn parse_quality(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    match value {
        "Low Quality" | "High Quality" | "CD Quality" | "Lossless" | "Not Available" | "Recording" => {
            Some(value.to_string())
        }
        _ => {
            eprintln!("Unknown Quality \"{value}\", defaulting to null");
            None
        }
    }
}

fn ensure_era(state: &mut ImportState, name: &str, is_main: bool, metadata: Option<&EraRecord>) -> i64 {
    let normalized_name = normalize_era_name(name);
    let key = normalized_name.to_lowercase();
    if let Some(index) = state.era_by_name.get(&key).copied() {
        if is_main {
            state.eras[index].is_main = 1;
        }
        if let Some(metadata) = metadata {
            // An era first seen on a song row has no metadata yet; backfill it
            // when the era's metadata row is parsed later.
            if state.eras[index].image_url.is_empty() {
                state.eras[index].image_url = metadata.image_url.clone();
            }
            if state.eras[index].notes.is_empty() {
                state.eras[index].notes = metadata.notes.clone();
            }
            if state.eras[index].description.is_empty() {
                state.eras[index].description = metadata.description.clone();
            }
            if state.eras[index].dominant_color.is_none() {
                state.eras[index].dominant_color = metadata.dominant_color.clone();
            }
        }
        return state.eras[index].id;
    }

    let era = EraRecord {
        id: state.next_era_id,
        name: normalized_name,
        notes: metadata.map(|metadata| metadata.notes.clone()).unwrap_or_default(),
        image_url: metadata.map(|metadata| metadata.image_url.clone()).unwrap_or_default(),
        description: metadata.map(|metadata| metadata.description.clone()).unwrap_or_default(),
        dominant_color: metadata.and_then(|metadata| metadata.dominant_color.clone()),
        cover_source: None,
        is_main: if is_main { 1 } else { 0 },
    };
    state.next_era_id += 1;
    state.era_by_name.insert(key, state.eras.len());
    state.eras.push(era);
    if is_main {
        state.main_era_count += 1;
    }
    state.eras.last().expect("era just pushed").id
}

fn is_header_row(cells: &[ElementRef<'_>]) -> bool {
    let headers: Vec<String> = cells.iter().map(|cell| normalize_header(&cell.text().collect::<String>())).collect();
    headers.iter().any(|header| header == "era")
        && headers
            .iter()
            .any(|header| header == "name" || header.starts_with("name "))
}

#[allow(clippy::too_many_lines)]
fn import_catalog(text: &str, catalog: &CatalogDefinition, state: &mut ImportState) -> Result<usize, ApiError> {
    let mut headers: Option<Vec<String>> = None;
    let mut era_column = -1i32;
    let mut name_column = -1i32;
    let mut notes_column = -1i32;
    let mut track_length_column = -1i32;
    let mut file_date_column = -1i32;
    let mut leak_date_column = -1i32;
    let mut available_length_column = -1i32;
    let mut quality_column = -1i32;
    let mut link_column = -1i32;
    let mut type_column = -1i32;
    let mut streaming_column = -1i32;
    let mut imported_songs = 0usize;

    for row_match in ROW_RE.captures_iter(text) {
        let row_html = row_match.get(0).map(|matched| matched.as_str()).unwrap_or_default();
        // node-html-parser turns `<br>` into a line break in `textContent`;
        // html5ever does not, so normalise before parsing. This keeps both the
        // "Name (Sheet Link)" header match and the era-name newline split.
        let row_html = LINE_BREAK_RE.replace_all(row_html, "\n");
        // Wrap in a table so html5ever keeps the row/cells (a bare `<tr>` in a
        // body fragment would be dropped, unlike node-html-parser's leniency).
        let document = Html::parse_fragment(&format!("<table>{row_html}</table>"));
        let cells: Vec<ElementRef<'_>> = document.select(&CELL_SELECTOR).collect();

        if headers.is_none() {
            if !is_header_row(&cells) {
                continue;
            }
            let new_headers: Vec<String> =
                cells.iter().map(|cell| normalize_header(&cell.text().collect::<String>())).collect();
            era_column = find_column(&new_headers, |header| header == "era" || header == "main era");
            name_column = find_column(&new_headers, |header| header == "name" || header.starts_with("name "));
            notes_column = find_column(&new_headers, |header| header == "notes" || header.starts_with("notes "));
            track_length_column = find_column(&new_headers, |header| {
                header == "track length" || header == "length" || header == "full length" || header == "copy length"
            });
            file_date_column = find_column(&new_headers, |header| {
                header == "file date" || header == "date made" || header == "release date"
            });
            leak_date_column = find_column(&new_headers, |header| header == "leak date");
            available_length_column = find_column(&new_headers, |header| header == "available length");
            quality_column = find_column(&new_headers, |header| header == "quality");
            link_column = find_column(&new_headers, |header| header.starts_with("link"));
            type_column = find_column(&new_headers, |header| header == "type");
            streaming_column = find_column(&new_headers, |header| header == "streaming");
            headers = Some(new_headers);
            continue;
        }
        let current_headers = headers.as_ref().expect("headers set above");

        if catalog.id == PRIMARY_CATALOG_ID && cells.len() >= 5 {
            // Era metadata rows carry the artwork <img> in the penultimate cell.
            // The sheet gained a leading stats column, but name/notes/image/
            // description are always the last four cells in every layout.
            let image_element = cells
                .get(cells.len() - 2)
                .and_then(|cell| cell.select(&IMG_SELECTOR).next());
            if let Some(image_element) = image_element {
                let raw_name = cells
                    .get(cells.len() - 4)
                    .map(|cell| cell.text().collect::<String>())
                    .unwrap_or_default();
                // Some name cells start with a leading <br>, so take the first
                // non-empty line (the italic alias/subtitle follows on later lines).
                let first_line = raw_name
                    .split('\n')
                    .map(|line| line.trim_end_matches('\r'))
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or_default();
                let name = normalize_era_name(first_line);
                if name.is_empty() {
                    continue;
                }

                let raw_image_url = image_element.value().attr("src").unwrap_or_default();
                let image_url = sanitize_image_url(raw_image_url);

                let metadata = EraRecord {
                    id: 0,
                    name: name.clone(),
                    notes: normalize_text(
                        &cells
                            .get(cells.len() - 3)
                            .map(|cell| cell.text().collect::<String>())
                            .unwrap_or_default(),
                    ),
                    image_url,
                    description: normalize_text(
                        &cells
                            .get(cells.len() - 1)
                            .map(|cell| cell.text().collect::<String>())
                            .unwrap_or_default(),
                    ),
                    dominant_color: None,
                    cover_source: None,
                    is_main: 1,
                };
                ensure_era(state, &name, true, Some(&metadata));
                continue;
            }
        }

        if cells.len() != current_headers.len() || era_column < 0 || name_column < 0 {
            continue;
        }

        let era_name = normalize_era_name(&cell_text(&cells, era_column));
        let song_name = cell_text(&cells, name_column);
        if era_name.is_empty() || song_name.is_empty() || era_name.to_lowercase() == "era" {
            continue;
        }

        let notes = cell_text(&cells, notes_column);
        let catalog_id = catalog.id;
        if catalog_id == PRIMARY_CATALOG_ID {
            let duplicate_key = format!(
                "{}\u{0}{}\u{0}{}",
                song_name.to_lowercase(),
                notes.to_lowercase(),
                era_name.to_lowercase()
            );
            if !state.seen_main_songs.insert(duplicate_key) {
                continue;
            }
        }

        // A song row only *references* an era; it does not define one. The sheet's
        // era metadata rows (which carry artwork and a description) are what make an
        // era browsable, so a placeholder or typo era name on a song row — the sheet
        // uses "x" for one-off performances — must not become a real era.
        let era_id = ensure_era(state, &era_name, false, None);
        let kind = cell_text(&cells, type_column);
        let streaming = cell_text(&cells, streaming_column);
        let mut extra_notes = Vec::new();
        if !kind.is_empty() {
            extra_notes.push(format!("Type: {kind}"));
        }
        if !streaming.is_empty() {
            extra_notes.push(format!("Streaming: {streaming}"));
        }
        let extra_notes = extra_notes.join(" · ");
        let song_notes = if extra_notes.is_empty() {
            notes.clone()
        } else {
            [notes.as_str(), extra_notes.as_str()]
                .iter()
                .filter(|value| !value.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        };

        let track_length = parse_duration(&cell_text(&cells, track_length_column));
        let file_date = parse_date(&cell_text(&cells, file_date_column));
        let leak_date = parse_date(&cell_text(&cells, leak_date_column));
        let available_length = parse_available_length(&cell_text(&cells, available_length_column));
        let quality = parse_quality(&cell_text(&cells, quality_column));
        let url = get_source_url(&cell_text(&cells, link_column));

        if let Some(song_url) = &url {
            if song_url != "N/A"
                && song_url != "Link Needed"
                && quality.as_deref() != Some("Not Available")
                && can_download(song_url)
            {
                let hash = hex::encode(Sha256::digest(song_url.as_bytes()));
                state.urls.push((song_url.clone(), hash));
            }
        }

        state.songs.push(ImportedSong {
            id: state.next_song_id,
            catalog_id: catalog_id.to_string(),
            era_id,
            name: song_name,
            notes: if song_notes.is_empty() { None } else { Some(song_notes) },
            track_length,
            file_date,
            leak_date,
            available_length,
            quality,
            url,
        });
        state.next_song_id += 1;
        imported_songs += 1;
        if catalog_id == PRIMARY_CATALOG_ID {
            state.main_song_count += 1;
        }
    }

    if headers.is_none() {
        return Err(ApiError::unexpected(format!(
            "Catalog {} did not contain a recognizable header row",
            catalog.name
        )));
    }
    Ok(imported_songs)
}

async fn fetch_catalog_text(client: &reqwest::Client, catalog: &CatalogDefinition) -> Result<String, ApiError> {
    let url = format!("https://yetracker.net/htmlview/sheet?headers=true&gid={}", catalog.gid);
    let mut delay_ms: u64 = 500;
    for attempt in 0..3 {
        let response = client
            .get(&url)
            .header(reqwest::header::USER_AGENT, FETCH_USER_AGENT)
            .timeout(CATALOG_FETCH_TIMEOUT)
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => match response.text().await {
                Ok(text) => return Ok(text),
                Err(error) => {
                    if attempt == 2 {
                        return Err(ApiError::unexpected(error));
                    }
                    eprintln!("Retrying catalog {} after fetch failure (attempt {})", catalog.name, attempt + 1);
                }
            },
            Ok(response) => {
                if attempt == 2 {
                    return Err(ApiError::unexpected(format!(
                        "Failed to fetch {} catalog: {} {}",
                        catalog.name,
                        response.status().as_u16(),
                        response.status().canonical_reason().unwrap_or("")
                    )));
                }
                eprintln!("Retrying catalog {} after fetch failure (attempt {})", catalog.name, attempt + 1);
            }
            Err(error) => {
                if attempt == 2 {
                    return Err(ApiError::unexpected(error));
                }
                eprintln!(
                    "Retrying catalog {} after fetch failure (attempt {}) {error}",
                    catalog.name,
                    attempt + 1
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(delay_ms + pseudo_random(250))).await;
        delay_ms *= 2;
    }
    Err(ApiError::unexpected(format!(
        "Failed to fetch {} catalog after retry",
        catalog.name
    )))
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

pub async fn import_data(state: SharedState) -> Result<(), ApiError> {
    let client = reqwest::Client::builder()
        .timeout(CATALOG_FETCH_TIMEOUT)
        .build()
        .map_err(ApiError::unexpected)?;

    let mut import_state = ImportState::new();
    // Futures are pushed directly rather than produced by a `map` closure:
    // a closure here makes rustc demand higher-ranked lifetimes that its own
    // region inference cannot satisfy once `import_data` runs inside
    // `tokio::spawn` ("implementation of FnOnce is not general enough").
    let mut fetches = Vec::with_capacity(CATALOGS.len());
    for catalog in CATALOGS.iter() {
        let client = &client;
        fetches.push(async move {
            match fetch_catalog_text(client, catalog).await {
                Ok(text) => (catalog, Some(text)),
                Err(error) => {
                    eprintln!("skipping catalog {} after fetch/import failure {error:?}", catalog.name);
                    (catalog, None)
                }
            }
        });
    }
    let results: Vec<(&'static CatalogDefinition, Option<String>)> =
        stream::iter(fetches).buffered(4).collect().await;

    for (catalog, text) in results {
        let Some(text) = text else { continue };
        match import_catalog(&text, catalog, &mut import_state) {
            Ok(imported) => println!(
                "imported {}: {} songs ({})",
                catalog.name,
                imported,
                catalog_source_url(catalog.gid)
            ),
            Err(error) => eprintln!("skipping catalog {} after import failure {error:?}", catalog.name),
        }
    }

    if import_state.main_era_count == 0 || import_state.main_song_count == 0 {
        return Err(ApiError::unexpected(
            "Fetched song catalog did not contain any main eras or songs",
        ));
    }

    // Preserve existing cover colours and the artwork URL the current cover was
    // generated from, keyed by normalized era name. The colour survives even if
    // a later cover refresh fails; `cover_source` lets the downloader know when
    // the artwork URL changed and the cover must be re-fetched.
    let existing_eras = db::call(&state.pool, |conn| {
        let mut statement = conn.prepare("SELECT name, dominant_color, cover_source FROM eras")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await?;
    let preserved_by_era_name: HashMap<String, (Option<String>, Option<String>)> = existing_eras
        .into_iter()
        .filter_map(|(name, color, cover_source)| name.map(|name| (normalize_era_key(&name), (color, cover_source))))
        .collect();
    let eras_final: Vec<EraRecord> = import_state
        .eras
        .iter()
        .map(|era| {
            let mut era = era.clone();
            if let Some((color, cover_source)) = preserved_by_era_name.get(&normalize_era_key(&era.name)) {
                era.dominant_color = color.clone();
                era.cover_source = cover_source.clone();
            }
            era
        })
        .collect();

    // Deduplicate URLs, keeping first-seen order but the last filename.
    let mut url_order: Vec<String> = Vec::new();
    let mut url_filenames: HashMap<String, String> = HashMap::new();
    for (url, filename) in &import_state.urls {
        if !url_filenames.contains_key(url) {
            url_order.push(url.clone());
        }
        url_filenames.insert(url.clone(), filename.clone());
    }
    let live_urls: HashSet<String> = url_order.iter().cloned().collect();

    let existing_files = db::call(&state.pool, |conn| {
        let mut statement = conn.prepare("SELECT url FROM files")?;
        let rows = statement.query_map([], |row| row.get::<_, Option<String>>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await?;
    let stale_urls: Vec<String> = existing_files
        .into_iter()
        .flatten()
        .filter(|url| !live_urls.contains(url))
        .collect();

    let songs_final = std::mem::take(&mut import_state.songs);

    db::call(&state.pool, move |conn| {
        let transaction = conn.unchecked_transaction()?;
        transaction.execute("DELETE FROM songs", [])?;
        transaction.execute("DELETE FROM eras", [])?;

        for chunk in stale_urls.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let query = format!("DELETE FROM files WHERE url IN ({placeholders})");
            transaction.execute(&query, rusqlite::params_from_iter(chunk.iter()))?;
        }

        {
            let mut statement = transaction.prepare(
                "INSERT OR IGNORE INTO eras (id, name, notes, image_url, description, dominant_color, cover_source, is_main) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for era in &eras_final {
                statement.execute(rusqlite::params![
                    era.id,
                    era.name,
                    era.notes,
                    era.image_url,
                    era.description,
                    era.dominant_color,
                    era.cover_source,
                    era.is_main,
                ])?;
            }

            let mut file_statement =
                transaction.prepare("INSERT OR IGNORE INTO files (url, filename) VALUES (?1, ?2)")?;
            for url in &url_order {
                let filename = url_filenames.get(url).cloned().unwrap_or_default();
                file_statement.execute(rusqlite::params![url, filename])?;
            }

            let mut song_statement = transaction.prepare(
                "INSERT OR IGNORE INTO songs \
                 (id, era, catalog_id, name, notes, file_date, leak_date, available_length, track_length, quality, url) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for song in &songs_final {
                song_statement.execute(rusqlite::params![
                    song.id,
                    song.era_id,
                    song.catalog_id,
                    song.name,
                    song.notes,
                    song.file_date,
                    song.leak_date,
                    song.available_length,
                    song.track_length,
                    song.quality,
                    song.url,
                ])?;
            }
        }

        transaction.commit()?;
        Ok(())
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import_primary(html: &str) -> ImportState {
        let catalog = CATALOGS.iter().find(|catalog| catalog.id == PRIMARY_CATALOG_ID).unwrap();
        let mut state = ImportState::new();
        import_catalog(html, catalog, &mut state).unwrap();
        state
    }

    #[test]
    fn parses_primary_eras_and_songs() {
let html = r#"
            <table>
              <tr><th>Era</th><th>Name<br>(Sheet Link)</th><th>Notes (Official Discord Server)</th><th>Link</th><th>Type</th><th>Quality</th></tr>
              <tr><td>1</td><td>Era One<br>meta</td><td>Era notes</td><td><img src="https://img.example/a.jpg?utm_source=x"></td><td>Description</td></tr>
              <tr><td>Era One</td><td>My Song</td><td>Song notes</td><td>https://pillows.su/f/abc123</td><td>Leak</td><td>CD Quality</td></tr>
            </table>
        "#;
        let state = import_primary(html);
        assert_eq!(state.eras.len(), 1);
        assert_eq!(state.eras[0].name, "Era One");
        assert_eq!(state.eras[0].image_url, "https://img.example/a.jpg");
        assert_eq!(state.songs.len(), 1);
        assert_eq!(state.songs[0].name, "My Song");
        assert_eq!(state.songs[0].notes.as_deref(), Some("Song notes\nType: Leak"));
        assert_eq!(state.songs[0].quality.as_deref(), Some("CD Quality"));
        assert_eq!(state.songs[0].url.as_deref(), Some("https://pillows.su/f/abc123"));
        assert_eq!(state.urls.len(), 1);
        assert_eq!(state.urls[0].1.len(), 64);
    }

    #[test]
    fn parses_six_cell_primary_era_rows() {
        let html = r#"
            <table>
              <tr><th>Era</th><th>Name<br>(Sheet Link)</th><th>Notes</th><th>Link</th><th>Type</th><th>Quality</th></tr>
              <tr><th>2</th><td>1 OG File(s)<br>38 Full</td><td>Era One<br><span>(Sub)</span></td><td>Era notes</td><td><img src="https://docs.google.com/sheets-images-rt/abc=w102-h104"></td><td>Description text</td></tr>
            </table>
        "#;
        let state = import_primary(html);
        assert_eq!(state.eras.len(), 1);
        assert_eq!(state.eras[0].name, "Era One");
        assert_eq!(state.eras[0].notes, "Era notes");
        assert_eq!(state.eras[0].description, "Description text");
        assert_eq!(state.eras[0].image_url, "https://docs.google.com/sheets-images-rt/abc=s512");
    }

    #[test]
    fn ignores_footer_stat_rows_without_images() {
        let html = r#"
            <table>
              <tr><th>Era</th><th>Name<br>(Sheet Link)</th><th>Notes</th><th>Link</th><th>Type</th><th>Quality</th></tr>
              <tr><td>1</td><td>Era One<br>meta</td><td>Era notes</td><td><img src="https://img.example/a.jpg?utm_source=x"></td><td>Description</td></tr>
              <tr><th>1</th><td>Links</td><td>Quality</td><td>Availability</td><td>Highlighted</td></tr>
              <tr><td>Era One</td><td>My Song</td><td>Song notes</td><td>https://pillows.su/f/abc123</td><td>Leak</td><td>CD Quality</td></tr>
            </table>
        "#;
        let state = import_primary(html);
        assert!(state.eras.iter().all(|era| era.name != "Links"));
        assert_eq!(state.eras.len(), 1);
        assert_eq!(state.eras[0].name, "Era One");
        assert_eq!(state.songs.len(), 1);
    }

    #[test]
    fn song_rows_alone_do_not_create_browsable_eras() {
        let html = r#"
            <table>
              <tr><th>Era</th><th>Name<br>(Sheet Link)</th><th>Notes</th><th>Link</th><th>Type</th><th>Quality</th></tr>
              <tr><td>1</td><td>Era One<br>meta</td><td>Era notes</td><td><img src="https://img.example/a.jpg"></td><td>Description</td></tr>
              <tr><td>x</td><td>Hollywood Bowl</td><td>Live show, no era</td><td></td><td></td><td></td></tr>
              <tr><td>Era One</td><td>My Song</td><td>Song notes</td><td></td><td></td><td>CD Quality</td></tr>
            </table>
        "#;
        let state = import_primary(html);
        let main_eras: Vec<&str> = state
            .eras
            .iter()
            .filter(|era| era.is_main == 1)
            .map(|era| era.name.as_str())
            .collect();
        assert_eq!(main_eras, ["Era One"]);
        // The song keeps its row so it stays searchable, but its placeholder era is
        // not listed by /eras.
        let placeholder = state.eras.iter().find(|era| era.name == "x").expect("placeholder era row");
        assert_eq!(placeholder.is_main, 0);
        assert_eq!(state.songs.len(), 2);
    }

    #[test]
    fn sanitize_image_url_upgrades_google_render_spec() {
        assert_eq!(
            sanitize_image_url("https://docs.google.com/sheets-images-rt/abc=w102-h104"),
            "https://docs.google.com/sheets-images-rt/abc=s512"
        );
        assert_eq!(sanitize_image_url("https://img.example/a.jpg"), "https://img.example/a.jpg");
    }

    #[test]
    fn duration_and_date_parsing() {
        assert_eq!(parse_duration("3:07"), Some(187));
        assert_eq!(parse_duration("1:02:03"), Some(3723));
        assert_eq!(parse_duration("3:75"), None);
        assert_eq!(parse_duration("nope"), None);
        assert_eq!(parse_date("2020-01-01"), 1_577_836_800);
        assert!(parse_date("not a date") == 0);
    }

    /// Manual parity check against a saved `htmlview/sheet` response:
    /// `IMPORT_SHEET=/tmp/sheet.html cargo test --lib live_sheet_snapshot -- --ignored --nocapture`
    /// The printed song count must match the same file through `importer.ts`.
    #[test]
    #[ignore]
    fn live_sheet_snapshot() {
        let path = std::env::var("IMPORT_SHEET").expect("set IMPORT_SHEET to the saved HTML path");
        let text = std::fs::read_to_string(&path).expect("read sheet html");
        let catalog = CATALOGS.iter().find(|catalog| catalog.id == PRIMARY_CATALOG_ID).unwrap();
        let mut state = ImportState::new();
        let count = import_catalog(&text, catalog, &mut state).unwrap();
        println!("count={count} eras={} urls={}", state.eras.len(), state.urls.len());
        assert!(state.main_era_count > 0, "expected main eras");
        assert!(
            state.eras.iter().filter(|era| era.is_main == 1).all(|era| !era.image_url.is_empty()),
            "every main era should have artwork"
        );
        assert!(
            state.eras.iter().filter(|era| era.is_main == 1).all(|era| !era.description.is_empty()),
            "every main era should have a description"
        );
        if let Ok(dump) = std::env::var("IMPORT_SHEET_DUMP") {
            let body: String = state.urls.iter().map(|(url, _)| format!("{url}\n")).collect();
            std::fs::write(dump, body).unwrap();
        }
        for song in state.songs.iter().take(5) {
            println!(
                "{}|{}|{}",
                song.name,
                song.quality.as_deref().unwrap_or(""),
                song.available_length.as_deref().unwrap_or("")
            );
        }
    }
}
