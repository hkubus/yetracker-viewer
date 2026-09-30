//! Background downloads: song media and era covers.
//!
//! Songs: `files` rows are picked while `pending` and due. pillows.su files
//! come from its download API, imgur.gg files from the direct link on their
//! file page, YouTube/Instagram/X media through yt-dlp. A failure backs off
//! exponentially (`next_attempt_at`) and becomes `failed` on 404/410, a
//! non-audio file, or after eight attempts; failed rows are tried again
//! after a 30-day cool-down. Network trouble (DNS, refused or reset
//! connections, timeouts, HTTP 5xx, 408 and 429, the same kinds of errors
//! reported by yt-dlp) backs off too but does not count as an attempt, and
//! a host is left alone for the rest of a sync after several such errors
//! in a row. A full disk (or quota) is not the link's fault either: it does
//! not count as an attempt and no further download starts in that sync.
//! Each sync downloads at most `MAX_DOWNLOADS_PER_CYCLE` files within
//! `DOWNLOAD_TIME_BUDGET_MINUTES`, so a large backlog never holds up the
//! next catalog import. Downloads are written to unique temporary files,
//! fsynced and renamed into place; a file that is already on disk under the
//! row's natural name is reused instead.
//!
//! Covers: artwork URLs only work for a few minutes after Google renders the
//! sheet, so the cover phase renders the sheet straight from Google when some
//! era needs a cover (or the daily refresh is due) and downloads every needed
//! image right away. Images are encoded to 512×512 AVIF plus a JPEG variant
//! (`covers/<era id>.avif` / `.jpg`); without ffmpeg the original image is
//! stored as is.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use futures_util::stream::{self, FuturesUnordered};
use regex::Regex;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use url::Url;

use crate::catalogs::{PRIMARY_CATALOG, google_sheet_url};
use crate::config::Config;
use crate::cover_version::cover_version_of;
use crate::db;
use crate::dominant_color;
use crate::error::ApiError;
use crate::importer;
use crate::media::{
    Quarantine, RunError, Tool, ToolSet, Uncertain, Verdict, file_input, quarantine,
    quarantined_at, run_grouped, stderr_tail, unique_suffix,
};
use crate::playable::is_safe_filename;
use crate::public_net::{PublicResolver, is_public_url};
use crate::state::AppState;

const FETCH_USER_AGENT: &str = "yetracker-viewer/1.0 (+https://yetracker.net)";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest pause between two reads of a response body.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_TIME_LIMIT: Duration = Duration::from_secs(30 * 60);
const PAGE_TIME_LIMIT: Duration = Duration::from_secs(60);
const PAGE_MAX_BYTES: usize = 2 * 1024 * 1024;
const YT_DLP_TIME_LIMIT: Duration = Duration::from_secs(30 * 60);
const STDERR_TAIL_LINES: usize = 20;
const MAX_ERROR_CHARS: usize = 2000;

/// Attempts after which a download is given up.
pub const MAX_ATTEMPTS: i64 = 8;
/// A download that gave up (`failed`) is tried again after this long: hosts
/// come back and files get re-uploaded under the same link.
pub const FAILED_RETRY_SECS: i64 = 30 * 24 * 60 * 60;
const BACKOFF_BASE_SECS: i64 = 30 * 60;
const BACKOFF_MAX_SECS: i64 = 7 * 24 * 60 * 60;
/// Longest wait after network trouble, which does not count as an attempt.
const TRANSIENT_BACKOFF_MAX_SECS: i64 = 6 * 60 * 60;
/// A host is left alone for the rest of a sync after this many network
/// errors in a row, so an outage costs a few requests per sync instead of
/// one per due row.
const HOST_PAUSE_AFTER: u32 = 5;
/// Downloads still running when the download time budget runs out get this
/// much longer before they are stopped (and retried by a later sync).
const BUDGET_GRACE: Duration = Duration::from_secs(5 * 60);
/// How `public_net::PublicResolver` words a host that only resolves to
/// non-public addresses: not a network hiccup, so it counts as an attempt.
const NON_PUBLIC_HOST: &str = "does not resolve to a public address";

const COVER_MAX_BYTES: usize = 20 * 1024 * 1024;
const COVER_SHEET_MAX_BYTES: usize = 50 * 1024 * 1024;
const COVER_SHEET_TIME_LIMIT: Duration = Duration::from_secs(90);
const COVER_FETCH_TIME_LIMIT: Duration = Duration::from_secs(60);
const COVER_FETCH_CONCURRENCY: usize = 4;
const COVER_ENCODE_TIMEOUT: Duration = Duration::from_secs(120);
const COVER_MAX_REDIRECTS: usize = 3;
const MEDIA_MAX_REDIRECTS: usize = 10;
/// How often every cover is re-fetched to pick up changed artwork.
const COVER_REFRESH_SECS: i64 = 24 * 60 * 60;
/// Largest image (in pixels) ffmpeg may decode for a cover.
const COVER_MAX_PIXELS: &str = "40000000";
/// Cover image extensions in lookup order: the encoded AVIF, then an original
/// stored while ffmpeg was unavailable (a `.jpg` next to an `.avif` is the
/// JPEG variant).
pub const COVER_EXTENSIONS: [&str; 5] = ["avif", "jpg", "png", "webp", "gif"];

const LATE_REGISTRATION_DOMINANT_COLOR: &str = "5a240a";
const LATE_REGISTRATION_ERA_NAME: &str = "Late Registration";

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
/// File extensions accepted as audio without an `audio/*` content type.
const AUDIO_EXTENSIONS: [&str; 20] = [
    "mp3", "m4a", "aac", "flac", "wav", "ogg", "oga", "opus", "aif", "aiff", "aifc", "wma", "weba",
    "alac", "mka", "ape", "wv", "ac3", "amr", "mp2",
];

static IMGUR_FILE_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/f/([A-Za-z0-9]+)/?$").expect("valid imgur path regex"));
static AUDIO_SRC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<audio\b[^>]*?\bsrc\s*=\s*"([^"]+)""#).expect("valid audio regex")
});
static ATTRIBUTE_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(?:src|href|content)\s*=\s*"(https://i\.imgur\.gg/[^"]+)""#)
        .expect("valid attribute regex")
});
static DISPOSITION_EXT_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|;)\s*filename\*\s*=\s*([^;]+)").expect("valid disposition regex")
});
static DISPOSITION_FILENAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:^|;)\s*filename\s*=\s*("(?:[^"\\]|\\.)*"|[^;]*)"#)
        .expect("valid disposition regex")
});
/// imgur.gg answers 200 with this heading for files that were deleted.
static IMGUR_FILE_MISSING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)>\s*File not found\s*<").expect("valid missing regex"));
static PILLOWS_HASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,128}$").expect("valid hash regex"));
/// yt-dlp failures caused by the network or an overloaded site: HTTP 5xx,
/// 429 and 408, timeouts, DNS failures (Python's and curl's wording),
/// refused/reset/aborted connections, unreachable networks, and yt-dlp's
/// own network error classes (`TransportError`, `IncompleteRead`).
static YT_DLP_TRANSIENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)HTTP Error (?:5\d\d|429|408)\b|timed out|temporary failure in name resolution|name or service not known|nodename nor servname|no address associated|getaddrinfo failed|could not resolve host|couldn'?t connect|failed to connect|connection ?(?:reset|refused|aborted)|network is unreachable|no route to host|remote end closed connection|incompleteread|transporterror|urlopen error",
    )
    .expect("valid transient regex")
});
/// yt-dlp (or its ffmpeg) failing because the disk or a quota is full.
static YT_DLP_DISK_FULL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)no space left on device|disk quota exceeded|\[Errno (?:28|122)\]")
        .expect("valid disk-full regex")
});

pub fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

// ---------------------------------------------------------------------------
// HTTP clients
// ---------------------------------------------------------------------------

/// The shared HTTP clients: one for media, which only reaches public
/// addresses (see [`crate::public_net`]; redirects included), and one for
/// covers that only talks HTTPS to the allowlisted artwork hosts. Both
/// ignore `HTTP(S)_PROXY`/`ALL_PROXY`: behind a proxy the proxy would
/// resolve the host names, bypassing the public-address check.
pub struct HttpClients {
    pub media: reqwest::Client,
    pub covers: reqwest::Client,
}

impl HttpClients {
    pub fn new() -> Result<Self, reqwest::Error> {
        let media = reqwest::Client::builder()
            .no_proxy()
            .user_agent(FETCH_USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .pool_idle_timeout(READ_TIMEOUT)
            .dns_resolver(Arc::new(PublicResolver))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= MEDIA_MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else if is_public_url(attempt.url()) {
                    attempt.follow()
                } else {
                    let host = attempt.url().host_str().unwrap_or_default().to_string();
                    attempt.error(format!("redirect to a non-public address ({host})"))
                }
            }))
            .build()?;
        let covers = reqwest::Client::builder()
            .no_proxy()
            .user_agent(FETCH_USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .pool_idle_timeout(READ_TIMEOUT)
            .https_only(true)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= COVER_MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else if is_allowed_cover_url(attempt.url()) {
                    attempt.follow()
                } else {
                    let host = attempt.url().host_str().unwrap_or_default().to_string();
                    attempt.error(format!("redirect to a disallowed host ({host})"))
                }
            }))
            .build()?;
        Ok(Self { media, covers })
    }
}

/// Artwork (and the sheet it comes from) is only fetched over HTTPS from
/// Google Docs and its image CDN.
pub fn is_allowed_cover_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && url.host_str().is_some_and(|host| {
            host == "docs.google.com" || host.ends_with(".googleusercontent.com")
        })
}

// ---------------------------------------------------------------------------
// Sources and file names
// ---------------------------------------------------------------------------

/// Where a song link is downloaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Pillows,
    ImgurGg,
    YouTube,
    Instagram,
    X,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Pillows => "pillows.su",
            Source::ImgurGg => "imgur.gg",
            Source::YouTube => "YouTube",
            Source::Instagram => "Instagram",
            Source::X => "X/Twitter",
        }
    }

    fn uses_yt_dlp(self) -> bool {
        matches!(self, Source::YouTube | Source::Instagram | Source::X)
    }
}

/// The downloader for `url`, or `None` when its host is not supported. Agrees
/// with `importer::is_downloadable_url`.
pub fn source_of(url: &Url) -> Option<Source> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    if host == PILLOWS_HOST {
        Some(Source::Pillows)
    } else if host == IMGUR_GG_HOST {
        IMGUR_FILE_PATH
            .is_match(url.path())
            .then_some(Source::ImgurGg)
    } else if YOUTUBE_HOSTS.contains(&host) {
        Some(Source::YouTube)
    } else if INSTAGRAM_HOSTS.contains(&host) {
        Some(Source::Instagram)
    } else if X_HOSTS.contains(&host) {
        Some(Source::X)
    } else {
        None
    }
}

fn pillows_hash(url: &Url) -> Option<&str> {
    url.path_segments()?
        .rev()
        .find(|segment| !segment.is_empty())
        .filter(|segment| PILLOWS_HASH.is_match(segment))
}

/// The stem a download of `url` is stored under: pillows.su files keep their
/// hash (as the original downloader named them), everything else uses the
/// SHA-256 of the link.
pub fn natural_stem(url: &str) -> String {
    if let Ok(parsed) = Url::parse(url)
        && parsed.host_str() == Some(PILLOWS_HOST)
        && let Some(hash) = pillows_hash(&parsed)
    {
        return hash.to_string();
    }
    sha256_hex(url)
}

/// `(stem, extension)` of a stored media name like `<stem>.<ext>`; temporary
/// and quarantined names don't qualify.
pub fn split_media_name(filename: &str) -> Option<(&str, &str)> {
    let (stem, extension) = filename.split_once('.')?;
    let plain = !stem.is_empty()
        && !extension.is_empty()
        && extension.len() <= 8
        && extension.bytes().all(|byte| byte.is_ascii_alphanumeric());
    plain.then_some((stem, extension))
}

/// A normalized extension (1–8 ASCII alphanumerics, lowercase).
fn normalized_extension(extension: &str) -> Option<String> {
    let valid = !extension.is_empty()
        && extension.len() <= 8
        && extension.bytes().all(|byte| byte.is_ascii_alphanumeric());
    valid.then(|| extension.to_ascii_lowercase())
}

fn extension_of_name(name: &str) -> Option<String> {
    let (_, extension) = name.rsplit_once('.')?;
    normalized_extension(extension)
}

fn percent_decode(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = (*bytes.get(index + 1)? as char).to_digit(16)?;
            let low = (*bytes.get(index + 2)? as char).to_digit(16)?;
            decoded.push((high * 16 + low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Some(decoded)
}

/// The file name a `Content-Disposition` header suggests: `filename*`
/// (RFC 5987, percent-decoded) wins over `filename` (RFC 6266).
pub fn disposition_filename(header: &str) -> Option<String> {
    if let Some(captures) = DISPOSITION_EXT_VALUE.captures(header) {
        let value = captures[1].trim().trim_matches('"');
        if let Some((charset, rest)) = value.split_once('\'')
            && let Some((_language, encoded)) = rest.split_once('\'')
            && let Some(bytes) = percent_decode(encoded)
        {
            let decoded = if charset.eq_ignore_ascii_case("utf-8") {
                String::from_utf8(bytes).ok()
            } else {
                // ISO-8859-1: every byte is the code point of the same value.
                Some(bytes.into_iter().map(char::from).collect())
            };
            if let Some(decoded) = decoded.filter(|name| !name.trim().is_empty()) {
                return Some(decoded);
            }
        }
    }
    let captures = DISPOSITION_FILENAME.captures(header)?;
    let raw = captures[1].trim();
    let value = match raw
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
    {
        Some(quoted) => {
            let mut unescaped = String::with_capacity(quoted.len());
            let mut characters = quoted.chars();
            while let Some(character) = characters.next() {
                if character == '\\' {
                    if let Some(escaped) = characters.next() {
                        unescaped.push(escaped);
                    }
                } else {
                    unescaped.push(character);
                }
            }
            unescaped
        }
        None => raw.to_string(),
    };
    (!value.trim().is_empty()).then_some(value)
}

fn extension_for_content_type(content_type: &str) -> Option<&'static str> {
    Some(match content_type {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave" => "wav",
        "audio/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => "m4a",
        "audio/aac" | "audio/x-aac" => "aac",
        "audio/aiff" | "audio/x-aiff" => "aiff",
        "audio/webm" => "weba",
        "audio/x-ms-wma" => "wma",
        _ => return None,
    })
}

fn content_type_of(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// A YouTube `t=` start offset in seconds: `123`, `33s`, `1m30s`, `1h2m3s`.
pub fn parse_start_offset(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse().ok();
    }
    let mut total: u64 = 0;
    let mut digits = String::new();
    // Units must appear in h, m, s order, each at most once.
    let mut previous_rank = u8::MAX;
    for character in value.chars() {
        if character.is_ascii_digit() {
            digits.push(character);
            continue;
        }
        let (rank, seconds) = match character.to_ascii_lowercase() {
            'h' => (2, 3600),
            'm' => (1, 60),
            's' => (0, 1),
            _ => return None,
        };
        if digits.is_empty() || rank >= previous_rank {
            return None;
        }
        previous_rank = rank;
        let amount: u64 = digits.parse().ok()?;
        total = total.checked_add(amount.checked_mul(seconds)?)?;
        digits.clear();
    }
    digits.is_empty().then_some(total)
}

/// The start offset of a link (`?t=` or `#t=`), when it has a valid one.
fn start_offset_of(url: &Url) -> Option<u64> {
    let from_query = url
        .query_pairs()
        .find(|(key, _)| key == "t")
        .map(|(_, value)| value.into_owned());
    let from_fragment = url
        .fragment()
        .and_then(|fragment| fragment.strip_prefix("t="))
        .map(str::to_string);
    let value = from_query.or(from_fragment)?;
    let offset = parse_start_offset(&value);
    if offset.is_none() {
        debug!(url = %url, value = %value, "ignoring an unrecognised start offset");
    }
    offset.filter(|offset| *offset > 0)
}

fn html_unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The direct media URL (`https://i.imgur.gg/<id>-<name>.<ext>`) on an
/// imgur.gg file page: the `<audio src>` if there is one, otherwise any
/// link to the file.
pub fn imgur_media_url(page: &str, id: &str) -> Option<Url> {
    let belongs_to_file = |url: &Url| {
        url.scheme() == "https"
            && url.host_str() == Some("i.imgur.gg")
            && url.path().strip_prefix('/').is_some_and(|name| {
                name.strip_prefix(id)
                    .is_some_and(|rest| rest.starts_with('-') || rest.starts_with('.'))
            })
    };
    let mut candidates = AUDIO_SRC
        .captures_iter(page)
        .chain(ATTRIBUTE_URL.captures_iter(page))
        .filter_map(|captures| Url::parse(html_unescape(captures[1].trim()).as_str()).ok());
    candidates.find(|candidate| belongs_to_file(candidate))
}

// ---------------------------------------------------------------------------
// Temporary files
// ---------------------------------------------------------------------------

/// A temporary file or directory that is removed when dropped unless kept.
struct Scratch {
    path: PathBuf,
    directory: bool,
    keep: bool,
}

impl Scratch {
    fn file(path: PathBuf) -> Self {
        Self {
            path,
            directory: false,
            keep: false,
        }
    }

    fn directory(path: PathBuf) -> Self {
        Self {
            path,
            directory: true,
            keep: false,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let _ = if self.directory {
            std::fs::remove_dir_all(&self.path)
        } else {
            std::fs::remove_file(&self.path)
        };
    }
}

/// Moves a finished temporary file to its final name.
async fn commit(mut scratch: Scratch, destination: &Path) -> io::Result<()> {
    tokio::fs::rename(&scratch.path, destination).await?;
    scratch.keep = true;
    Ok(())
}

async fn sync_file(path: &Path) -> io::Result<()> {
    tokio::fs::File::open(path).await?.sync_all().await
}

/// Free space on the filesystem holding `path`, in bytes.
fn available_bytes(path: &Path) -> io::Result<u64> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    // SAFETY: `statvfs` only writes into the zeroed struct we pass and reads
    // the NUL-terminated path, both valid for the duration of the call.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return Err(io::Error::last_os_error());
    }
    #[allow(clippy::unnecessary_cast)] // the field types differ between platforms
    Ok((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
}

async fn free_space(path: &Path) -> io::Result<u64> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || available_bytes(&path))
        .await
        .map_err(io::Error::other)?
}

// ---------------------------------------------------------------------------
// Download bookkeeping
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum DownloadError {
    /// The link or what it served failed; worth another attempt later, and
    /// counts towards [`MAX_ATTEMPTS`].
    Retryable(String),
    /// The network or the host failed (unreachable, timed out, HTTP 5xx,
    /// 408 or 429): backs off without counting towards [`MAX_ATTEMPTS`].
    Transient(String),
    /// The disk (or a quota) filled up while storing the file: backs off
    /// like [`DownloadError::Transient`], and no further download starts in
    /// this sync.
    DiskFull(String),
    /// Will not succeed: the file is gone or is not audio.
    Terminal(String),
    /// A needed tool is not installed; the row is left alone.
    ToolMissing(Tool),
}

/// Whether a filesystem error means the disk or a quota is full.
fn is_disk_full(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
    ) || matches!(error.raw_os_error(), Some(libc::ENOSPC | libc::EDQUOT))
}

impl DownloadError {
    fn network(what: &str, error: reqwest::Error) -> Self {
        let message = format!("{what}: {}", error_chain(&error));
        // Redirect and request-building errors come from the link itself.
        if error.is_redirect() || error.is_builder() || message.contains(NON_PUBLIC_HOST) {
            DownloadError::Retryable(message)
        } else {
            DownloadError::Transient(message)
        }
    }

    fn io(what: &str, error: io::Error) -> Self {
        let message = format!("{what}: {error}");
        if is_disk_full(&error) {
            DownloadError::DiskFull(message)
        } else {
            DownloadError::Retryable(message)
        }
    }

    fn status(what: &str, status: reqwest::StatusCode) -> Self {
        let message = format!("{what}: HTTP {}", status.as_u16());
        match status.as_u16() {
            404 | 410 => DownloadError::Terminal(message),
            408 | 429 | 500..=599 => DownloadError::Transient(message),
            _ => DownloadError::Retryable(message),
        }
    }

    /// A failed yt-dlp run: not counted when its output blames a full disk
    /// or the network.
    fn yt_dlp(status: std::process::ExitStatus, tail: &str) -> Self {
        let message = format!("yt-dlp exited with {status}: {tail}");
        if YT_DLP_DISK_FULL.is_match(tail) {
            DownloadError::DiskFull(message)
        } else if YT_DLP_TRANSIENT.is_match(tail) {
            DownloadError::Transient(message)
        } else {
            DownloadError::Retryable(message)
        }
    }
}

/// An error's message followed by its causes (`a: b: c`), skipping causes
/// the message already contains.
pub fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !message.contains(&cause_text) {
            message.push_str(": ");
            message.push_str(&cause_text);
        }
        source = cause.source();
    }
    message
}

/// Delay before retry number `attempts + 1`: 30 min × 2^(attempts − 1),
/// capped at 7 days.
pub fn backoff_secs(attempts: i64) -> i64 {
    doubling_backoff(attempts, BACKOFF_MAX_SECS)
}

/// Delay after the `failures`-th network error in a row: 30 min ×
/// 2^(failures − 1), capped at 6 hours.
pub fn transient_backoff_secs(failures: i64) -> i64 {
    doubling_backoff(failures, TRANSIENT_BACKOFF_MAX_SECS)
}

fn doubling_backoff(count: i64, max: i64) -> i64 {
    let exponent = (count.max(1) - 1).min(62) as u32;
    BACKOFF_BASE_SECS
        .checked_mul(1_i64.checked_shl(exponent).unwrap_or(i64::MAX))
        .unwrap_or(max)
        .min(max)
}

fn truncate_error(message: &str) -> String {
    if message.chars().count() <= MAX_ERROR_CHARS {
        return message.to_string();
    }
    let mut truncated: String = message.chars().take(MAX_ERROR_CHARS).collect();
    truncated.push('…');
    truncated
}

/// Marks a download as done.
pub async fn record_downloaded(
    state: &AppState,
    url: &str,
    filename: &str,
    duration: Option<f64>,
) -> Result<(), ApiError> {
    let (url, filename) = (url.to_string(), filename.to_string());
    db::call(&state.pool, move |conn| {
        conn.execute(
            "UPDATE files SET status = 'downloaded', downloaded = 1, filename = ?2, \
             duration = ?3, next_attempt_at = NULL, last_error = NULL, transient_failures = 0 \
             WHERE url = ?1",
            rusqlite::params![url, filename, duration],
        )?;
        Ok(())
    })
    .await
}

/// Records a failed attempt: backs off, or gives up (`failed`, tried again
/// after [`FAILED_RETRY_SECS`]) when `terminal` or out of attempts. Returns
/// whether the row is now `failed`.
pub async fn record_failure(
    state: &AppState,
    url: &str,
    error: &str,
    terminal: bool,
) -> Result<bool, ApiError> {
    let (url, error) = (url.to_string(), truncate_error(error));
    let now = db::unix_now();
    db::call(&state.pool, move |conn| {
        failure_update(conn, &url, &error, terminal, now)
    })
    .await
}

fn failure_update(
    conn: &rusqlite::Connection,
    url: &str,
    error: &str,
    terminal: bool,
    now: i64,
) -> Result<bool, ApiError> {
    // Read and write under the write lock: a deferred transaction could fail
    // to upgrade while another writer is busy.
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let attempts: i64 =
        transaction.query_row("SELECT attempts FROM files WHERE url = ?1", [url], |row| {
            row.get(0)
        })?;
    let attempts = attempts + 1;
    let give_up = terminal || attempts >= MAX_ATTEMPTS;
    let next_attempt_at = if give_up {
        now + FAILED_RETRY_SECS
    } else {
        now + backoff_secs(attempts)
    };
    transaction.execute(
        "UPDATE files SET attempts = ?2, last_error = ?3, next_attempt_at = ?4, \
         status = ?5, downloaded = 0, filename = NULL, duration = NULL WHERE url = ?1",
        rusqlite::params![
            url,
            attempts,
            error,
            next_attempt_at,
            if give_up { "failed" } else { "pending" }
        ],
    )?;
    transaction.commit()?;
    Ok(give_up)
}

/// Records network trouble: the row waits (see [`transient_backoff_secs`])
/// but keeps its attempts and its status.
async fn record_transient(state: &AppState, url: &str, error: &str) -> Result<i64, ApiError> {
    let (url, error) = (url.to_string(), truncate_error(error));
    let now = db::unix_now();
    db::call(&state.pool, move |conn| {
        transient_update(conn, &url, &error, now)
    })
    .await
}

fn transient_update(
    conn: &rusqlite::Connection,
    url: &str,
    error: &str,
    now: i64,
) -> Result<i64, ApiError> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let failures: i64 = transaction.query_row(
        "SELECT transient_failures FROM files WHERE url = ?1",
        [url],
        |row| row.get(0),
    )?;
    let failures = failures + 1;
    let next_attempt_at = now + transient_backoff_secs(failures);
    transaction.execute(
        "UPDATE files SET transient_failures = ?2, last_error = ?3, next_attempt_at = ?4 \
         WHERE url = ?1",
        rusqlite::params![url, failures, error, next_attempt_at],
    )?;
    transaction.commit()?;
    Ok(next_attempt_at)
}

/// Resets a row whose file vanished so it is downloaded again (not counted
/// as a failed attempt).
pub async fn record_missing(state: &AppState, url: &str) -> Result<(), ApiError> {
    let url = url.to_string();
    db::call(&state.pool, move |conn| {
        conn.execute(
            "UPDATE files SET status = 'pending', downloaded = 0, filename = NULL, \
             duration = NULL, next_attempt_at = NULL WHERE url = ?1",
            [url],
        )?;
        Ok(())
    })
    .await
}

/// Stored media files by stem (see [`natural_stem`]).
struct MediaIndex {
    by_stem: HashMap<String, Vec<String>>,
}

async fn index_media(dir: &Path) -> io::Result<MediaIndex> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut by_stem: HashMap<String, Vec<String>> = HashMap::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if let Some((stem, _)) = split_media_name(&name) {
                by_stem.entry(stem.to_string()).or_default().push(name);
            }
        }
        Ok(MediaIndex { by_stem })
    })
    .await
    .map_err(io::Error::other)?
}

/// Result of checking a stored media file with ffprobe.
pub(crate) enum FileCheck {
    /// Readable audio (`duration` when the container states one).
    Good(Option<f64>),
    /// ffprobe could not give an answer (missing, timed out); keep the file.
    Unverified,
    /// ffprobe rejected the file, which was moved aside.
    Rejected(String),
    /// Not there (or not a regular file).
    Missing,
}

/// Probes `filename` in the songs directory. Only a definitive rejection
/// moves the file aside (see `media::quarantine`).
pub(crate) async fn check_file(state: &AppState, filename: &str) -> FileCheck {
    if !is_safe_filename(filename) || filename == "." || filename == ".." {
        return FileCheck::Missing;
    }
    let path = state.config.songs_path.join(filename);
    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return FileCheck::Missing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return FileCheck::Missing,
        Err(error) => {
            debug!(filename, %error, "could not stat a media file");
            return FileCheck::Unverified;
        }
    };
    let mtime_ms = metadata
        .modified()
        .map(crate::serve::unix_millis)
        .unwrap_or(0);
    match state.probes.verdict(&path, metadata.len(), mtime_ms).await {
        Verdict::Valid { duration } => FileCheck::Good(duration),
        Verdict::Invalid(reason) => {
            state.probes.forget(&path);
            state.playable.set_playable(filename, false);
            match quarantine(
                &state.config.songs_path,
                filename,
                db::unix_now(),
                Quarantine::Rejected,
            )
            .await
            {
                Ok(moved) => {
                    warn!(filename, reason = reason.as_str(), quarantined = %moved, "rejected media file moved aside");
                    FileCheck::Rejected(format!(
                        "the stored file was rejected ({})",
                        reason.as_str()
                    ))
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => FileCheck::Missing,
                Err(error) => {
                    warn!(filename, %error, "could not move a rejected media file aside");
                    FileCheck::Unverified
                }
            }
        }
        Verdict::Unknown(uncertain) => {
            if uncertain == Uncertain::ToolMissing {
                state.tools.mark_missing(Tool::Ffprobe);
            } else {
                debug!(filename, error = %uncertain, "could not verify a media file");
            }
            FileCheck::Unverified
        }
    }
}

// ---------------------------------------------------------------------------
// Song downloads
// ---------------------------------------------------------------------------

/// What one sync's download phase did.
#[derive(Debug, Default, Clone, Copy)]
pub struct DownloadSummary {
    pub reused: usize,
    pub attempted: usize,
    pub downloaded: usize,
    /// Failures counted as attempts.
    pub failed: usize,
    pub gave_up: usize,
    /// Network trouble, not counted as attempts.
    pub transient: usize,
    /// Downloads that ran out of disk space (or quota), not counted as
    /// attempts either.
    pub disk_full: usize,
    /// Due rows skipped because their host is disabled by configuration.
    pub disabled_by_config: usize,
    /// Due rows skipped because a needed tool is missing.
    pub missing_tool: usize,
    /// Hosts left alone for the rest of the sync after repeated network
    /// errors.
    pub paused_hosts: usize,
    /// Due rows left for a later sync (per-sync cap, time budget, low or
    /// full disk, paused host, shutdown).
    pub deferred: usize,
}

struct Candidate {
    url: String,
    source: Option<Source>,
}

impl Candidate {
    /// What network errors are counted against (see [`HostBreaker`]).
    fn service(&self) -> &str {
        self.source.map_or("other", Source::label)
    }
}

/// Counts network errors per service in a row; after [`HOST_PAUSE_AFTER`]
/// the service is paused for the rest of the sync.
#[derive(Default)]
struct HostBreaker {
    failures: HashMap<String, u32>,
    paused: HashSet<String>,
}

impl HostBreaker {
    fn is_paused(&self, service: &str) -> bool {
        self.paused.contains(service)
    }

    /// Records an outcome for `service`; returns true when this failure
    /// paused it.
    fn record(&mut self, service: &str, transient: bool) -> bool {
        if !transient {
            self.failures.remove(service);
            return false;
        }
        let failures = self.failures.entry(service.to_string()).or_default();
        *failures += 1;
        *failures >= HOST_PAUSE_AFTER && self.paused.insert(service.to_string())
    }
}
/// Links that were not downloaded yet, with their stored file name.
async fn undownloaded_rows(state: &AppState) -> Result<Vec<(String, Option<String>)>, ApiError> {
    db::call(&state.pool, |conn| {
        let mut statement = conn.prepare(
            "SELECT url, filename FROM files WHERE url IS NOT NULL \
             AND (status != 'downloaded' OR filename IS NULL)",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await
}

/// Due downloads of links the catalog still uses: pending rows whose backoff
/// passed and failed rows whose cool-down passed; fresh ones first, then in
/// catalog order.
async fn due_rows(state: &AppState, now: i64) -> Result<Vec<String>, ApiError> {
    db::call(&state.pool, move |conn| due_urls(conn, now)).await
}

fn due_urls(conn: &rusqlite::Connection, now: i64) -> Result<Vec<String>, ApiError> {
    let mut statement = conn.prepare(
        "SELECT files.url, files.attempts, min(songs.position) AS first_position \
         FROM songs JOIN files ON files.url = songs.url \
         WHERE ((files.status = 'pending' \
                 AND (files.next_attempt_at IS NULL OR files.next_attempt_at <= ?1)) \
             OR (files.status = 'failed' AND files.next_attempt_at <= ?1)) \
           AND (songs.quality IS NULL OR songs.quality != 'Not Available') \
         GROUP BY files.url ORDER BY files.attempts, first_position",
    )?;
    let rows = statement.query_map([now], |row| row.get::<_, String>(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
}

/// Links rows that are not downloaded yet to media already on disk (under
/// the stored name or the natural one) instead of fetching them again.
async fn reuse_existing_files(state: &AppState, index: &MediaIndex) -> Result<usize, ApiError> {
    let mut reused = 0;
    for (url, stored) in undownloaded_rows(state).await? {
        let stem = natural_stem(&url);
        let mut names: Vec<&str> = Vec::new();
        if let Some(stored) = stored.as_deref() {
            names.push(stored);
        }
        if let Some(found) = index.by_stem.get(&stem) {
            names.extend(found.iter().map(String::as_str));
        }
        for name in names {
            let duration = match check_file(state, name).await {
                FileCheck::Good(duration) => duration,
                FileCheck::Unverified => None,
                FileCheck::Rejected(_) | FileCheck::Missing => continue,
            };
            record_downloaded(state, &url, name, duration).await?;
            state
                .playable
                .refresh_one(&state.config.songs_path, name)
                .await;
            info!(url = %url, filename = name, "reused a media file already on disk");
            reused += 1;
            break;
        }
    }
    Ok(reused)
}

/// Runs the song download phase: re-links media already on disk, then
/// downloads due rows (at most `MAX_DOWNLOADS_PER_CYCLE`, `DOWNLOAD_CONCURRENCY`
/// at a time, within `DOWNLOAD_TIME_BUDGET_MINUTES`) while the disk keeps
/// `MIN_FREE_DISK_MB` free.
pub async fn download_songs(
    state: &AppState,
    shutdown: &CancellationToken,
) -> Result<DownloadSummary, ApiError> {
    let mut summary = DownloadSummary::default();
    let songs_dir = state.config.songs_path.clone();
    if tokio::fs::metadata(&songs_dir).await.is_err() {
        if !state.config.songs_dir_is_managed() {
            warn!(
                path = %songs_dir.display(),
                "songs directory is missing and outside STORAGE_DIR; skipping downloads"
            );
            return Ok(summary);
        }
        tokio::fs::create_dir_all(&songs_dir)
            .await
            .map_err(|error| {
                ApiError::unexpected(format!("creating the songs directory: {error}"))
            })?;
    }

    let index = index_media(&songs_dir)
        .await
        .map_err(|error| ApiError::unexpected(format!("listing the songs directory: {error}")))?;
    summary.reused = reuse_existing_files(state, &index).await?;
    if !state.config.downloads_enabled {
        info!(
            reused = summary.reused,
            "song downloads are disabled (DOWNLOADS_ENABLED=false)"
        );
        return Ok(summary);
    }

    let started = Instant::now();
    let budget_end = state
        .config
        .download_time_budget
        .map(|budget| started + budget);
    let tools = state.tools.snapshot();
    let mut candidates = Vec::new();
    for url in due_rows(state, db::unix_now()).await? {
        let source = Url::parse(&url).ok().and_then(|parsed| source_of(&parsed));
        match source {
            Some(Source::YouTube) if !state.config.youtube_download => {
                summary.disabled_by_config += 1;
            }
            Some(source) if source.uses_yt_dlp() && !(tools.yt_dlp && tools.ffmpeg) => {
                summary.missing_tool += 1;
            }
            _ => candidates.push(Candidate { url, source }),
        }
    }

    let limit = state.config.max_downloads_per_cycle.unwrap_or(usize::MAX);
    let concurrency = state.config.download_concurrency.max(1);
    let mut queue = candidates.into_iter();
    let mut running = FuturesUnordered::new();
    let mut breaker = HostBreaker::default();
    let mut low_disk = false;
    let mut out_of_time = false;
    loop {
        out_of_time = out_of_time || budget_end.is_some_and(|end| Instant::now() >= end);
        while running.len() < concurrency
            && summary.attempted < limit
            && !low_disk
            && !out_of_time
            && !shutdown.is_cancelled()
        {
            let Some(candidate) = queue.next() else {
                break;
            };
            if breaker.is_paused(candidate.service()) {
                summary.deferred += 1;
                continue;
            }
            if state.config.min_free_disk_bytes > 0 {
                match free_space(&songs_dir).await {
                    Ok(free) if free < state.config.min_free_disk_bytes => {
                        warn!(
                            free_mb = free / (1024 * 1024),
                            required_mb = state.config.min_free_disk_bytes / (1024 * 1024),
                            "not enough free disk space; postponing further downloads"
                        );
                        low_disk = true;
                        summary.deferred += 1;
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => warn!(%error, "could not check free disk space"),
                }
            }
            summary.attempted += 1;
            running.push(async move {
                let outcome = download_candidate(state, &candidate).await;
                (candidate, outcome)
            });
        }
        if running.is_empty() {
            break;
        }
        // Past the budget, running downloads get a grace period, then they
        // are dropped (their temporary files and processes go with them).
        let stop_at = budget_end.map(|end| end + BUDGET_GRACE);
        let finished = tokio::select! {
            _ = shutdown.cancelled() => {
                summary.deferred += running.len();
                break;
            }
            () = sleep_until_or_forever(stop_at) => {
                warn!(
                    running = running.len(),
                    "the download time budget ran out; stopping the downloads still running"
                );
                summary.deferred += running.len();
                break;
            }
            // Wake up when the budget ends, to stop starting new downloads.
            () = sleep_until_or_forever(budget_end.filter(|_| !out_of_time)) => continue,
            finished = running.next() => finished,
        };
        let Some((candidate, outcome)) = finished else {
            break;
        };
        let service = candidate.service().to_string();
        // A row that cannot be updated is reported and retried next sync;
        // it must not stop the other downloads.
        match outcome {
            Ok((filename, duration)) => {
                breaker.record(&service, false);
                match record_downloaded(state, &candidate.url, &filename, duration).await {
                    Ok(()) => {
                        state.playable.refresh_one(&songs_dir, &filename).await;
                        summary.downloaded += 1;
                        info!(url = %candidate.url, filename = %filename, "downloaded");
                    }
                    Err(error) => {
                        warn!(url = %candidate.url, filename = %filename, %error, "could not record a download");
                    }
                }
            }
            Err(DownloadError::ToolMissing(tool)) => {
                state.tools.mark_missing(tool);
                summary.missing_tool += 1;
            }
            Err(DownloadError::Transient(error)) => {
                summary.transient += 1;
                match record_transient(state, &candidate.url, &error).await {
                    Ok(_) => {
                        warn!(url = %candidate.url, %error, "download failed (network); will retry")
                    }
                    Err(db_error) => {
                        warn!(url = %candidate.url, %error, %db_error, "download failed and could not be recorded");
                    }
                }
                if breaker.record(&service, true) {
                    summary.paused_hosts += 1;
                    warn!(
                        service = %service,
                        errors = HOST_PAUSE_AFTER,
                        "pausing downloads from this service for the rest of the sync after repeated network errors"
                    );
                }
            }
            // Neither the link's nor the host's fault: the row waits without
            // using an attempt, and nothing else starts in this sync.
            Err(DownloadError::DiskFull(error)) => {
                summary.disk_full += 1;
                match record_transient(state, &candidate.url, &error).await {
                    Ok(_) => {
                        warn!(url = %candidate.url, %error, "download failed (disk full); will retry")
                    }
                    Err(db_error) => {
                        warn!(url = %candidate.url, %error, %db_error, "download failed and could not be recorded");
                    }
                }
                if !low_disk {
                    low_disk = true;
                    warn!("the disk is full; postponing further downloads");
                }
            }
            Err(DownloadError::Retryable(error)) => {
                breaker.record(&service, false);
                match record_failure(state, &candidate.url, &error, false).await {
                    Ok(gave_up) => {
                        count_failure(&mut summary, gave_up);
                        warn!(url = %candidate.url, %error, gave_up, "download failed");
                    }
                    Err(db_error) => {
                        warn!(url = %candidate.url, %error, %db_error, "download failed and could not be recorded");
                    }
                }
            }
            Err(DownloadError::Terminal(error)) => {
                breaker.record(&service, false);
                match record_failure(state, &candidate.url, &error, true).await {
                    Ok(_) => {
                        count_failure(&mut summary, true);
                        warn!(url = %candidate.url, %error, "download failed permanently");
                    }
                    Err(db_error) => {
                        warn!(url = %candidate.url, %error, %db_error, "download failed and could not be recorded");
                    }
                }
            }
        }
    }
    summary.deferred += queue.len();
    info!(
        reused = summary.reused,
        attempted = summary.attempted,
        downloaded = summary.downloaded,
        failed = summary.failed,
        gave_up = summary.gave_up,
        transient = summary.transient,
        disk_full = summary.disk_full,
        paused_hosts = summary.paused_hosts,
        disabled_by_config = summary.disabled_by_config,
        missing_tool = summary.missing_tool,
        deferred = summary.deferred,
        out_of_time,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "song downloads finished"
    );
    Ok(summary)
}

/// Sleeps until `deadline`, or forever without one.
async fn sleep_until_or_forever(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}

fn count_failure(summary: &mut DownloadSummary, gave_up: bool) {
    summary.failed += 1;
    if gave_up {
        summary.gave_up += 1;
    }
}

/// Downloads one link and checks the result; returns the stored name and,
/// when ffprobe reported it, the duration.
async fn download_candidate(
    state: &AppState,
    candidate: &Candidate,
) -> Result<(String, Option<f64>), DownloadError> {
    let url = Url::parse(&candidate.url)
        .map_err(|error| DownloadError::Terminal(format!("invalid link: {error}")))?;
    let Some(source) = candidate.source else {
        return Err(DownloadError::Terminal(format!(
            "unsupported host {}",
            url.host_str().unwrap_or_default()
        )));
    };
    debug!(url = %candidate.url, source = source.label(), "downloading");
    let filename = match source {
        Source::Pillows => download_pillows(state, &url).await?,
        Source::ImgurGg => download_imgur(state, &url, &candidate.url).await?,
        Source::YouTube | Source::Instagram | Source::X => {
            download_with_yt_dlp(state, &url, &candidate.url).await?
        }
    };
    match check_file(state, &filename).await {
        FileCheck::Good(duration) => Ok((filename, duration)),
        // Accepted for now; the duration backfill verifies it later.
        FileCheck::Unverified => Ok((filename, None)),
        FileCheck::Rejected(reason) => Err(DownloadError::Retryable(reason)),
        FileCheck::Missing => Err(DownloadError::Retryable(
            "the downloaded file disappeared".to_string(),
        )),
    }
}

/// Streams a response body into a new temporary file next to `final_name`,
/// enforcing `MAX_DOWNLOAD_MB`, and fsyncs it.
async fn receive_body(
    state: &AppState,
    response: reqwest::Response,
    final_name: &str,
) -> Result<Scratch, DownloadError> {
    let limit = state.config.max_download_bytes;
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(DownloadError::Retryable(format!(
            "the file is larger than MAX_DOWNLOAD_MB ({} MB)",
            limit / (1024 * 1024)
        )));
    }
    let scratch = Scratch::file(
        state
            .config
            .songs_path
            .join(format!("{final_name}.{}.tmp", unique_suffix())),
    );
    let mut file = tokio::fs::File::create(&scratch.path)
        .await
        .map_err(|error| DownloadError::io("creating the temporary file", error))?;
    let mut body = response.bytes_stream();
    let mut received: u64 = 0;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|error| DownloadError::network("receiving the file", error))?;
        received += chunk.len() as u64;
        if received > limit {
            return Err(DownloadError::Retryable(format!(
                "the file is larger than MAX_DOWNLOAD_MB ({} MB)",
                limit / (1024 * 1024)
            )));
        }
        file.write_all(&chunk)
            .await
            .map_err(|error| DownloadError::io("writing the file", error))?;
    }
    file.flush()
        .await
        .map_err(|error| DownloadError::io("writing the file", error))?;
    file.sync_all()
        .await
        .map_err(|error| DownloadError::io("syncing the file", error))?;
    if received == 0 {
        return Err(DownloadError::Retryable(
            "the response was empty".to_string(),
        ));
    }
    Ok(scratch)
}

async fn store_download(
    state: &AppState,
    response: reqwest::Response,
    name: String,
) -> Result<String, DownloadError> {
    let scratch = receive_body(state, response, &name).await?;
    commit(scratch, &state.config.songs_path.join(&name))
        .await
        .map_err(|error| DownloadError::io("moving the file into place", error))?;
    Ok(name)
}

async fn download_pillows(state: &AppState, url: &Url) -> Result<String, DownloadError> {
    let hash = pillows_hash(url)
        .ok_or_else(|| DownloadError::Terminal("no file id in the pillows.su link".to_string()))?;
    let api = format!("https://api.pillows.su/api/download/{hash}");
    let response = state
        .http
        .media
        .get(&api)
        .timeout(DOWNLOAD_TIME_LIMIT)
        .send()
        .await
        .map_err(|error| DownloadError::network("requesting the pillows.su file", error))?;
    if !response.status().is_success() {
        return Err(DownloadError::status("pillows.su", response.status()));
    }
    let content_type = content_type_of(&response);
    if content_type.starts_with("text/") || content_type == "application/json" {
        return Err(DownloadError::Retryable(format!(
            "pillows.su answered with {content_type} instead of a file"
        )));
    }
    let extension = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .and_then(disposition_filename)
        .and_then(|name| extension_of_name(&name))
        .or_else(|| extension_for_content_type(&content_type).map(str::to_string))
        .unwrap_or_else(|| "bin".to_string());
    store_download(state, response, format!("{hash}.{extension}")).await
}

async fn fetch_page(
    client: &reqwest::Client,
    url: &str,
    what: &str,
) -> Result<String, DownloadError> {
    let response = client
        .get(url)
        .timeout(PAGE_TIME_LIMIT)
        .send()
        .await
        .map_err(|error| DownloadError::network(what, error))?;
    if !response.status().is_success() {
        return Err(DownloadError::status(what, response.status()));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| DownloadError::network(what, error))?;
        if body.len() + chunk.len() > PAGE_MAX_BYTES {
            return Err(DownloadError::Retryable(format!(
                "{what}: the page is too large"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

async fn download_imgur(
    state: &AppState,
    page_url: &Url,
    link: &str,
) -> Result<String, DownloadError> {
    let id = IMGUR_FILE_PATH
        .captures(page_url.path())
        .map(|captures| captures[1].to_string())
        .ok_or_else(|| DownloadError::Terminal("not an imgur.gg file link".to_string()))?;
    let page = fetch_page(&state.http.media, page_url.as_str(), "the imgur.gg page").await?;
    let Some(direct) = imgur_media_url(&page, &id) else {
        return Err(if IMGUR_FILE_MISSING.is_match(&page) {
            DownloadError::Terminal("imgur.gg: the file no longer exists".to_string())
        } else {
            DownloadError::Retryable("no media link on the imgur.gg page".to_string())
        });
    };
    let url_extension = direct
        .path_segments()
        .and_then(|mut segments| segments.next_back().map(str::to_string))
        .and_then(|name| {
            percent_decode(&name).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        })
        .and_then(|name| extension_of_name(&name));

    let response = state
        .http
        .media
        .get(direct.clone())
        .timeout(DOWNLOAD_TIME_LIMIT)
        .send()
        .await
        .map_err(|error| DownloadError::network("requesting the imgur.gg file", error))?;
    if !response.status().is_success() {
        return Err(DownloadError::status("imgur.gg file", response.status()));
    }
    let content_type = content_type_of(&response);
    let audio_extension = url_extension
        .as_deref()
        .filter(|extension| AUDIO_EXTENSIONS.contains(extension));
    let extension = match (audio_extension, content_type.starts_with("audio/")) {
        (Some(extension), _) => extension.to_string(),
        (None, true) => extension_for_content_type(&content_type)
            .map(str::to_string)
            .or(url_extension.clone())
            .unwrap_or_else(|| "bin".to_string()),
        (None, false) => {
            return Err(DownloadError::Terminal(format!(
                "not an audio file ({}{})",
                if content_type.is_empty() {
                    "no content type"
                } else {
                    &content_type
                },
                url_extension
                    .as_deref()
                    .map(|extension| format!(", .{extension}"))
                    .unwrap_or_default()
            )));
        }
    };
    store_download(state, response, format!("{}.{extension}", sha256_hex(link))).await
}

async fn download_with_yt_dlp(
    state: &AppState,
    url: &Url,
    link: &str,
) -> Result<String, DownloadError> {
    let songs_dir = &state.config.songs_path;
    let work = Scratch::directory(songs_dir.join(format!(".ytdl-{}", unique_suffix())));
    tokio::fs::create_dir(&work.path)
        .await
        .map_err(|error| DownloadError::io("creating the yt-dlp work directory", error))?;

    let mut command = Command::new(Tool::YtDlp.binary());
    command
        .args([
            "--no-playlist",
            "--ignore-config",
            // Only the site extractors: the generic one would fetch whatever
            // URL a post links to.
            "--use-extractors",
            "default,-generic",
            // A live stream never ends and dodges --max-filesize.
            "--match-filter",
            "!is_live",
            "--no-progress",
            "--no-mtime",
            "--socket-timeout",
            "30",
            "--max-filesize",
        ])
        .arg(state.config.max_download_bytes.to_string())
        .args(["-x", "--audio-format", "opus", "--audio-quality", "0", "-o"])
        .arg(work.path.join("audio.%(ext)s"));
    if let Some(offset) = start_offset_of(url) {
        command
            .arg("--download-sections")
            .arg(format!("*{offset}-inf"));
    }
    command.arg("--").arg(url.as_str());
    // yt-dlp runs helpers (its bundled interpreter, ffmpeg): its whole
    // process group is killed on timeout, cancellation and shutdown.
    let (status, tail) = match run_grouped(command, YT_DLP_TIME_LIMIT, STDERR_TAIL_LINES).await {
        Ok(finished) => finished,
        Err(RunError::NotFound) => return Err(DownloadError::ToolMissing(Tool::YtDlp)),
        Err(RunError::Io(error)) => return Err(DownloadError::io("running yt-dlp", error)),
        Err(RunError::TimedOut) => {
            return Err(DownloadError::Transient("yt-dlp timed out".to_string()));
        }
    };
    if !status.success() {
        return Err(DownloadError::yt_dlp(status, &tail));
    }

    let mut produced = Vec::new();
    let mut entries = tokio::fs::read_dir(&work.path)
        .await
        .map_err(|error| DownloadError::io("reading the yt-dlp output", error))?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_type().await.is_ok_and(|kind| kind.is_file()) {
            produced.push(entry.path());
        }
    }
    let output = produced
        .iter()
        .find(|path| {
            matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("opus" | "ogg")
            )
        })
        .or(produced.first())
        .cloned()
        .ok_or_else(|| DownloadError::Retryable(format!("yt-dlp produced no file: {tail}")))?;
    let size = tokio::fs::metadata(&output)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if size == 0 {
        return Err(DownloadError::Retryable(
            "yt-dlp produced an empty file".to_string(),
        ));
    }
    if size > state.config.max_download_bytes {
        return Err(DownloadError::Retryable(
            "the file is larger than MAX_DOWNLOAD_MB".to_string(),
        ));
    }
    sync_file(&output)
        .await
        .map_err(|error| DownloadError::io("syncing the yt-dlp output", error))?;
    let name = format!("{}.ogg", sha256_hex(link));
    tokio::fs::rename(&output, songs_dir.join(&name))
        .await
        .map_err(|error| DownloadError::io("moving the file into place", error))?;
    drop(work);
    Ok(name)
}

// ---------------------------------------------------------------------------
// Covers
// ---------------------------------------------------------------------------

pub fn covers_dir(config: &Config) -> PathBuf {
    config.storage_path.join("covers")
}

/// The era's current cover: the first existing, non-empty
/// `covers/<id>.<ext>` in [`COVER_EXTENSIONS`] order.
pub async fn primary_cover(covers: &Path, era_id: i64) -> Option<(PathBuf, &'static str)> {
    for extension in COVER_EXTENSIONS {
        let path = covers.join(format!("{era_id}.{extension}"));
        if let Ok(metadata) = tokio::fs::metadata(&path).await
            && metadata.is_file()
            && metadata.len() > 0
        {
            return Some((path, extension));
        }
    }
    None
}

/// A cover the cleanup set aside while its era is removed but may come back
/// (its tombstone lives): `<era id>.<ext>` quarantined as retired, i.e.
/// `<era id>.<ext>.<unix seconds>.removed`. Returns the era id, the
/// extension and when it was set aside.
pub fn set_aside_cover(name: &str) -> Option<(i64, &'static str, i64)> {
    let (at, Quarantine::Retired) = quarantined_at(name)? else {
        return None;
    };
    let mut parts = name.rsplitn(3, '.');
    let original = parts.nth(2)?;
    let (id, extension) = original.split_once('.')?;
    let extension = COVER_EXTENSIONS
        .into_iter()
        .find(|known| *known == extension)?;
    let id = id
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| id.parse::<i64>().ok())
        .flatten()
        .filter(|id| *id > 0)?;
    Some((id, extension, at))
}

/// Puts back the covers the cleanup set aside for eras that exist again
/// (restored from their tombstone), so they need no download. Only the most
/// recently set-aside cover of an era counts, and only while the era has no
/// cover of its own. Returns the ids of the eras whose cover came back.
async fn restore_set_aside_covers(covers: &Path, era_ids: &HashSet<i64>) -> Vec<i64> {
    let mut entries = match tokio::fs::read_dir(covers).await {
        Ok(entries) => entries,
        Err(error) => {
            warn!(path = %covers.display(), %error, "could not list the covers directory");
            return Vec::new();
        }
    };
    /// The files of an era's cover set aside at one time.
    struct SetAside {
        at: i64,
        files: Vec<(&'static str, String)>,
    }
    // Per era, the latest set-aside cover.
    let mut latest: HashMap<i64, SetAside> = HashMap::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some((id, extension, at)) = set_aside_cover(&name) else {
            continue;
        };
        if !era_ids.contains(&id) {
            continue;
        }
        let group = latest.entry(id).or_insert(SetAside {
            at,
            files: Vec::new(),
        });
        if at > group.at {
            *group = SetAside {
                at,
                files: Vec::new(),
            };
        }
        if at == group.at {
            group.files.push((extension, name));
        }
    }
    let mut restored = Vec::new();
    for (id, SetAside { mut files, .. }) in latest {
        if primary_cover(covers, id).await.is_some() {
            continue;
        }
        // The primary (AVIF) last: its bytes define the version, and its
        // variants must be in place by the time a client can learn it.
        files.sort_by_key(|(extension, _)| {
            std::cmp::Reverse(COVER_EXTENSIONS.iter().position(|known| known == extension))
        });
        let mut moved = 0;
        for (extension, name) in files {
            match tokio::fs::rename(covers.join(&name), covers.join(format!("{id}.{extension}")))
                .await
            {
                Ok(()) => moved += 1,
                Err(error) => {
                    warn!(era_id = id, file = %name, %error, "could not restore a set-aside cover");
                }
            }
        }
        if moved > 0 {
            info!(
                era_id = id,
                files = moved,
                "restored the cover of a returning era"
            );
            restored.push(id);
        }
    }
    restored
}

/// Image type of `bytes` by signature: `jpg`, `png`, `webp` or `gif`.
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else {
        None
    }
}

struct EraCover {
    id: i64,
    key: Option<String>,
    name: Option<String>,
    cover_version: Option<String>,
    cover_source: Option<String>,
    dominant_color: Option<String>,
    cover_attempts: i64,
    cover_next_attempt_at: Option<i64>,
}

/// What one sync's cover phase did.
#[derive(Debug, Default, Clone, Copy)]
pub struct CoverSummary {
    /// Covers put back for eras restored from their tombstone.
    pub restored: usize,
    pub needed: usize,
    pub fetched: usize,
    pub written: usize,
    pub unchanged: usize,
    pub failed: usize,
}

async fn load_eras(state: &AppState) -> Result<Vec<EraCover>, ApiError> {
    db::call(&state.pool, |conn| {
        let mut statement = conn.prepare(
            "SELECT id, key, name, cover_version, cover_source, cover_attempts, \
             cover_next_attempt_at, dominant_color FROM eras WHERE is_main = 1 \
             ORDER BY position, id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(EraCover {
                id: row.get(0)?,
                key: row.get(1)?,
                name: row.get(2)?,
                cover_version: row.get(3)?,
                cover_source: row.get(4)?,
                cover_attempts: row.get(5)?,
                cover_next_attempt_at: row.get(6)?,
                dominant_color: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await
}

/// Keeps the cover phase from failing on a DB write it can retry next time.
async fn update_era(
    state: &AppState,
    era_id: i64,
    sql: &'static str,
    values: Vec<rusqlite::types::Value>,
) {
    let result = db::call(&state.pool, move |conn| {
        conn.execute(sql, rusqlite::params_from_iter(values))?;
        Ok(())
    })
    .await;
    if let Err(error) = result {
        warn!(era_id, %error, "could not update the era's cover columns");
    }
}

async fn record_cover(
    state: &AppState,
    era: &EraCover,
    version: String,
    source: Option<String>,
    color: Option<String>,
) {
    use rusqlite::types::Value;
    let color = if era.name.as_deref() == Some(LATE_REGISTRATION_ERA_NAME) {
        Some(LATE_REGISTRATION_DOMINANT_COLOR.to_string())
    } else {
        color
    };
    let optional = |value: Option<String>| value.map_or(Value::Null, Value::Text);
    update_era(
        state,
        era.id,
        "UPDATE eras SET cover_version = ?1, cover_source = coalesce(?2, cover_source), \
         dominant_color = coalesce(?3, dominant_color), cover_attempts = 0, \
         cover_next_attempt_at = NULL, cover_last_error = NULL WHERE id = ?4",
        vec![
            Value::Text(version),
            optional(source),
            optional(color),
            Value::Integer(era.id),
        ],
    )
    .await;
}

async fn record_cover_failure(state: &AppState, era: &EraCover, error: &str) {
    use rusqlite::types::Value;
    let attempts = era.cover_attempts + 1;
    let next = db::unix_now() + backoff_secs(attempts);
    warn!(era_id = era.id, era_name = era.name.as_deref().unwrap_or_default(), %error, attempts, "cover download failed");
    update_era(
        state,
        era.id,
        "UPDATE eras SET cover_attempts = ?1, cover_next_attempt_at = ?2, cover_last_error = ?3 \
         WHERE id = ?4",
        vec![
            Value::Integer(attempts),
            Value::Integer(next),
            Value::Text(truncate_error(error)),
            Value::Integer(era.id),
        ],
    )
    .await;
}

async fn set_cover_version(state: &AppState, era_id: i64, version: Option<String>) {
    use rusqlite::types::Value;
    update_era(
        state,
        era_id,
        "UPDATE eras SET cover_version = ?1 WHERE id = ?2",
        vec![
            version.map_or(Value::Null, Value::Text),
            Value::Integer(era_id),
        ],
    )
    .await;
}

async fn read_version(path: &Path) -> Option<String> {
    let bytes = tokio::fs::read(path).await.ok()?;
    (!bytes.is_empty()).then(|| cover_version_of(&bytes))
}

/// Removes `covers/<id>.<ext>` for every cover extension not in `keep`.
async fn remove_other_covers(covers: &Path, era_id: i64, keep: &[&str]) {
    for extension in COVER_EXTENSIONS {
        if !keep.contains(&extension) {
            let _ = tokio::fs::remove_file(covers.join(format!("{era_id}.{extension}"))).await;
        }
    }
}

/// Encodes the image at `source` into `covers/<id>.avif` and
/// `covers/<id>.jpg` (512×512) and samples the dominant colour. The JPEG is
/// renamed into place before the AVIF, whose bytes define the cover version.
/// Returns (version, colour).
async fn encode_cover(
    covers: &Path,
    era_id: i64,
    source: &Path,
) -> Result<(String, Option<String>), String> {
    let suffix = unique_suffix();
    let avif = Scratch::file(covers.join(format!("{era_id}.{suffix}.avif.tmp")));
    let jpeg = Scratch::file(covers.join(format!("{era_id}.{suffix}.jpg.tmp")));

    let mut command = Command::new(Tool::Ffmpeg.binary());
    command
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-protocol_whitelist",
            "file",
            "-format_whitelist",
            "jpeg_pipe,png_pipe,webp_pipe,gif,gif_pipe,bmp_pipe,tiff_pipe,image2",
            "-max_pixels",
            COVER_MAX_PIXELS,
            "-i",
        ])
        .arg(file_input(source))
        .args([
            "-filter_complex",
            "[0:v:0]scale=512:512:force_original_aspect_ratio=increase,crop=512:512,setsar=1,split=2[avif][jpeg]",
            "-map",
            "[avif]",
            "-frames:v",
            "1",
            "-c:v",
            "libsvtav1",
            "-crf",
            "18",
            "-preset",
            "3",
            "-f",
            "avif",
        ])
        .arg(&avif.path)
        .args([
            "-map",
            "[jpeg]",
            "-frames:v",
            "1",
            "-c:v",
            "mjpeg",
            "-q:v",
            "3",
            "-pix_fmt",
            "yuvj420p",
            "-f",
            "mjpeg",
        ])
        .arg(&jpeg.path)
        // SVT-AV1 prints its banner unless told to log errors only.
        .env("SVT_LOG", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start ffmpeg: {error}"))?;
    let stderr = child.stderr.take().expect("ffmpeg stderr is piped");
    let stderr = tokio::spawn(stderr_tail(stderr, STDERR_TAIL_LINES));
    let status = match tokio::time::timeout(COVER_ENCODE_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => return Err(format!("waiting for ffmpeg failed: {error}")),
        Err(_) => {
            let _ = child.kill().await;
            return Err("encoding the cover timed out".to_string());
        }
    };
    let tail = stderr.await.unwrap_or_default();
    if !status.success() {
        return Err(format!("ffmpeg exited with {status}: {tail}"));
    }
    for output in [&avif.path, &jpeg.path] {
        sync_file(output)
            .await
            .map_err(|error| format!("syncing the encoded cover: {error}"))?;
    }
    let bytes = tokio::fs::read(&avif.path)
        .await
        .map_err(|error| format!("reading the encoded cover: {error}"))?;
    if bytes.is_empty() {
        return Err("ffmpeg produced an empty cover".to_string());
    }
    let color = match dominant_color::dominant_color(&jpeg.path).await {
        Ok(color) => Some(color),
        Err(error) => {
            warn!(era_id, %error, "could not sample the cover's dominant colour");
            None
        }
    };
    commit(jpeg, &covers.join(format!("{era_id}.jpg")))
        .await
        .map_err(|error| format!("storing the JPEG cover: {error}"))?;
    commit(avif, &covers.join(format!("{era_id}.avif")))
        .await
        .map_err(|error| format!("storing the AVIF cover: {error}"))?;
    remove_other_covers(covers, era_id, &["avif", "jpg"]).await;
    Ok((cover_version_of(&bytes), color))
}

/// The dominant colour of a stored cover, sampled from its JPEG variant
/// when there is one (any ffmpeg decodes it), else from `primary`.
async fn sample_cover_color(covers: &Path, era_id: i64, primary: &Path) -> Option<String> {
    let variant = covers.join(format!("{era_id}.jpg"));
    let source = if tokio::fs::metadata(&variant)
        .await
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
    {
        variant
    } else {
        primary.to_path_buf()
    };
    match dominant_color::dominant_color(&source).await {
        Ok(color) => Some(color),
        Err(error) => {
            warn!(era_id, %error, "could not sample the cover's dominant colour");
            None
        }
    }
}

/// Stores a downloaded image as is (ffmpeg unavailable). Returns the version.
async fn store_original_cover(
    covers: &Path,
    era_id: i64,
    image: DownloadedImage,
) -> Result<String, String> {
    let extension =
        sniff_image(&image.head).ok_or("the artwork is not a JPEG, PNG, WebP or GIF image")?;
    let version = image.version();
    commit(image.file, &covers.join(format!("{era_id}.{extension}")))
        .await
        .map_err(|error| format!("storing the cover: {error}"))?;
    // Anything else (an older AVIF, a JPEG variant) now describes another image.
    remove_other_covers(covers, era_id, &[extension]).await;
    Ok(version)
}

/// An artwork image downloaded into a temporary file in the covers
/// directory (removed unless it is stored as the cover).
struct DownloadedImage {
    file: Scratch,
    /// SHA-256 of the bytes, hex.
    sha256: String,
    len: usize,
    /// The first bytes, for sniffing the image type.
    head: Vec<u8>,
}

impl DownloadedImage {
    /// The cover version of these bytes ([`cover_version_of`]).
    fn version(&self) -> String {
        self.sha256[..crate::cover_version::COVER_VERSION_LEN].to_string()
    }
}

/// Downloads one artwork image (HTTPS, allowlisted hosts, 20 MB cap) into a
/// temporary file next to the era's cover, hashing it on the way.
async fn fetch_cover_image(
    client: &reqwest::Client,
    url: &str,
    covers: &Path,
    era_id: i64,
) -> Result<DownloadedImage, String> {
    let parsed = Url::parse(url).map_err(|error| format!("invalid artwork URL: {error}"))?;
    if !is_allowed_cover_url(&parsed) {
        return Err(format!(
            "artwork URL host {} is not allowed",
            parsed.host_str().unwrap_or_default()
        ));
    }
    let response = client
        .get(parsed)
        .timeout(COVER_FETCH_TIME_LIMIT)
        .send()
        .await
        .map_err(|error| format!("requesting the artwork: {}", error_chain(&error)))?;
    if !response.status().is_success() {
        return Err(format!("artwork: HTTP {}", response.status().as_u16()));
    }
    let content_type = content_type_of(&response);
    if !content_type.starts_with("image/") {
        return Err(format!("artwork is {content_type}, not an image"));
    }
    if response
        .content_length()
        .is_some_and(|length| length > COVER_MAX_BYTES as u64)
    {
        return Err("artwork is larger than 20 MB".to_string());
    }
    let file = Scratch::file(covers.join(format!("{era_id}.{}.src.tmp", unique_suffix())));
    let mut output = tokio::fs::File::create(&file.path)
        .await
        .map_err(|error| format!("creating a temporary artwork file: {error}"))?;
    let mut hasher = Sha256::new();
    let mut head = Vec::new();
    let mut len = 0;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|error| format!("receiving the artwork: {error}"))?;
        len += chunk.len();
        if len > COVER_MAX_BYTES {
            return Err("artwork is larger than 20 MB".to_string());
        }
        if head.len() < 16 {
            head.extend_from_slice(&chunk[..chunk.len().min(16 - head.len())]);
        }
        hasher.update(&chunk);
        output
            .write_all(&chunk)
            .await
            .map_err(|error| format!("writing the artwork: {error}"))?;
    }
    if len == 0 {
        return Err("artwork is empty".to_string());
    }
    output
        .sync_all()
        .await
        .map_err(|error| format!("syncing the artwork: {error}"))?;
    Ok(DownloadedImage {
        file,
        sha256: hex::encode(hasher.finalize()),
        len,
        head,
    })
}

/// Renders the sheet straight from Google (fresh artwork URLs) and returns
/// era key → artwork URL.
async fn fetch_artwork_urls(state: &AppState) -> Result<HashMap<String, String>, String> {
    let url = google_sheet_url(&PRIMARY_CATALOG);
    let response = state
        .http
        .covers
        .get(&url)
        .timeout(COVER_SHEET_TIME_LIMIT)
        .send()
        .await
        .map_err(|error| format!("rendering the sheet: {}", error_chain(&error)))?;
    if !response.status().is_success() {
        return Err(format!(
            "rendering the sheet: HTTP {}",
            response.status().as_u16()
        ));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("receiving the sheet: {error}"))?;
        if body.len() + chunk.len() > COVER_SHEET_MAX_BYTES {
            return Err("the sheet is larger than 50 MB".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    let html = String::from_utf8_lossy(&body).into_owned();
    let catalog = tokio::task::spawn_blocking(move || importer::parse_sheet(&html))
        .await
        .map_err(|error| format!("parsing the sheet: {error}"))??;
    Ok(catalog
        .eras
        .into_iter()
        .filter(|era| !era.image_url.is_empty())
        .map(|era| (era.key, era.image_url))
        .collect())
}

/// Runs the cover phase (see the module docs). Covers count as downloads:
/// nothing is fetched while `DOWNLOADS_ENABLED=false`.
pub async fn sync_covers(
    state: &AppState,
    shutdown: &CancellationToken,
    tools: ToolSet,
) -> Result<CoverSummary, ApiError> {
    let mut summary = CoverSummary::default();
    let covers = covers_dir(&state.config);
    tokio::fs::create_dir_all(&covers)
        .await
        .map_err(|error| ApiError::unexpected(format!("creating the covers directory: {error}")))?;
    let eras = load_eras(state).await?;
    let now = db::unix_now();
    let era_ids: HashSet<i64> = eras.iter().map(|era| era.id).collect();
    summary.restored = restore_set_aside_covers(&covers, &era_ids).await.len();

    // Keep `cover_version` (hasCover/coverVersion) in line with the files,
    // and encode originals stored while ffmpeg was missing.
    let mut has_cover = HashSet::new();
    for era in &eras {
        let current = primary_cover(&covers, era.id).await;
        if let Some((path, extension)) = &current
            && tools.ffmpeg
            && *extension != "avif"
        {
            match encode_cover(&covers, era.id, path).await {
                Ok((version, color)) => {
                    record_cover(state, era, version, None, color).await;
                    has_cover.insert(era.id);
                    summary.written += 1;
                    continue;
                }
                Err(error) => warn!(era_id = era.id, %error, "could not encode a stored cover"),
            }
        }
        let version = match &current {
            Some((path, _)) => read_version(path).await,
            None => None,
        };
        if version.is_some() {
            has_cover.insert(era.id);
        }
        match (version, &current) {
            // A cover the row doesn't know yet (an era restored with its
            // old id, or covers kept across a database reset): adopt it, and
            // its colour when the row has none.
            (Some(version), Some((path, _)))
                if era.cover_version.is_none()
                    && tools.ffmpeg
                    && era
                        .dominant_color
                        .as_deref()
                        .is_none_or(|color| color == "666666") =>
            {
                let color = sample_cover_color(&covers, era.id, path).await;
                record_cover(state, era, version, None, color).await;
            }
            (version, _) if version != era.cover_version => {
                set_cover_version(state, era.id, version).await;
            }
            _ => {}
        }
    }

    if !state.config.downloads_enabled {
        return Ok(summary);
    }
    let last_refresh = db::call(&state.pool, |conn| {
        db::meta_get_i64(conn, db::meta_keys::LAST_COVER_REFRESH_AT)
    })
    .await?
    .unwrap_or(0);
    let refresh_due = now - last_refresh >= COVER_REFRESH_SECS;
    let due = |era: &&EraCover| era.cover_next_attempt_at.is_none_or(|at| at <= now);
    let needed: Vec<&EraCover> = eras
        .iter()
        .filter(|era| !has_cover.contains(&era.id))
        .filter(due)
        .collect();
    summary.needed = needed.len();
    if needed.is_empty() && !refresh_due {
        return Ok(summary);
    }
    let targets: Vec<&EraCover> = if refresh_due {
        eras.iter()
            .filter(|era| has_cover.contains(&era.id) || due(era))
            .collect()
    } else {
        needed
    };

    let artwork = match fetch_artwork_urls(state).await {
        Ok(artwork) => artwork,
        Err(error) => {
            for era in targets.iter().filter(|era| !has_cover.contains(&era.id)) {
                record_cover_failure(state, era, &error).await;
            }
            return Err(ApiError::unexpected(error));
        }
    };
    // Artwork URLs expire minutes after the render: download every image
    // now, into temporary files, then process them one at a time.
    let requests: Vec<(usize, Option<String>)> = targets
        .iter()
        .enumerate()
        .map(|(index, era)| {
            let url = era.key.as_deref().and_then(|key| artwork.get(key)).cloned();
            (index, url)
        })
        .collect();
    let mut fetched: Vec<(usize, Result<DownloadedImage, String>)> = stream::iter(requests)
        .map(|(index, url)| {
            let client = state.http.covers.clone();
            let covers = covers.clone();
            let era_id = targets[index].id;
            async move {
                let result = match url {
                    Some(url) => fetch_cover_image(&client, &url, &covers, era_id).await,
                    None => Err("the sheet has no artwork for this era".to_string()),
                };
                (index, result)
            }
        })
        .buffer_unordered(COVER_FETCH_CONCURRENCY)
        .collect()
        .await;
    fetched.sort_by_key(|(index, _)| *index);

    for (index, result) in fetched {
        let era = targets[index];
        if shutdown.is_cancelled() {
            break;
        }
        let had_cover = has_cover.contains(&era.id);
        let image = match result {
            Ok(image) => image,
            Err(error) if had_cover => {
                debug!(era_id = era.id, %error, "cover refresh failed; keeping the stored cover");
                continue;
            }
            Err(error) => {
                record_cover_failure(state, era, &error).await;
                summary.failed += 1;
                continue;
            }
        };
        summary.fetched += 1;
        let source_tag = format!("sha256:{}", image.sha256);
        let current = primary_cover(&covers, era.id).await;
        let up_to_date = era.cover_source.as_deref() == Some(source_tag.as_str())
            && current.is_some_and(|(_, extension)| extension == "avif" || !tools.ffmpeg);
        if up_to_date {
            summary.unchanged += 1;
            continue;
        }
        let bytes = image.len;
        let stored = if tools.ffmpeg {
            encode_cover(&covers, era.id, &image.file.path).await
        } else {
            store_original_cover(&covers, era.id, image)
                .await
                .map(|version| (version, None))
        };
        match stored {
            Ok((version, color)) => {
                record_cover(state, era, version, Some(source_tag), color).await;
                summary.written += 1;
                info!(
                    era_id = era.id,
                    era_name = era.name.as_deref().unwrap_or_default(),
                    bytes,
                    "cover stored"
                );
            }
            Err(error) if had_cover => {
                warn!(era_id = era.id, %error, "could not store a refreshed cover; keeping the old one");
            }
            Err(error) => {
                record_cover_failure(state, era, &error).await;
                summary.failed += 1;
            }
        }
    }
    if refresh_due {
        let now = now.to_string();
        db::call(&state.pool, move |conn| {
            db::meta_set(conn, db::meta_keys::LAST_COVER_REFRESH_AT, Some(&now))
        })
        .await?;
    }
    info!(
        restored = summary.restored,
        needed = summary.needed,
        fetched = summary.fetched,
        written = summary.written,
        unchanged = summary.unchanged,
        failed = summary.failed,
        "covers synced"
    );
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_matches_the_reference_digest() {
        // Stored media filenames are derived from this digest, so it must stay stable.
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn files_database() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let nowhere = std::env::temp_dir().join("yt-downloader-test-nowhere");
        db::migrate(&conn, &nowhere, &nowhere).unwrap();
        conn.execute_batch(
            "INSERT INTO songs (id, era, name, url, position) VALUES \
               (1, 1, 'a', 'https://imgur.gg/f/a', 1), (2, 1, 'b', 'https://imgur.gg/f/b', 2);
             INSERT INTO files (url, status, last_seen_at) VALUES \
               ('https://imgur.gg/f/a', 'pending', 1), ('https://imgur.gg/f/b', 'pending', 1);",
        )
        .unwrap();
        conn
    }

    fn row(conn: &rusqlite::Connection, url: &str) -> (String, i64, i64, Option<i64>) {
        conn.query_row(
            "SELECT status, attempts, transient_failures, next_attempt_at FROM files WHERE url = ?1",
            [url],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
    }

    #[test]
    fn network_trouble_backs_off_without_using_up_attempts() {
        let conn = files_database();
        let url = "https://imgur.gg/f/a";
        let now = 1_790_000_000;
        for failures in 1..=12 {
            let next = transient_update(&conn, url, "HTTP 503", now).unwrap();
            assert_eq!(next, now + transient_backoff_secs(failures));
        }
        let (status, attempts, transient, next) = row(&conn, url);
        assert_eq!((status.as_str(), attempts, transient), ("pending", 0, 12));
        assert_eq!(next, Some(now + TRANSIENT_BACKOFF_MAX_SECS));
        // Not due while it waits; due afterwards.
        assert_eq!(due_urls(&conn, now).unwrap(), ["https://imgur.gg/f/b"]);
        assert!(
            due_urls(&conn, now + TRANSIENT_BACKOFF_MAX_SECS)
                .unwrap()
                .contains(&url.to_string())
        );
    }

    #[test]
    fn failed_downloads_are_retried_after_a_cool_down() {
        let conn = files_database();
        let url = "https://imgur.gg/f/a";
        let now = 1_790_000_000;
        for attempt in 1..MAX_ATTEMPTS {
            assert!(!failure_update(&conn, url, "HTTP 403", false, now).unwrap());
            assert_eq!(row(&conn, url).1, attempt);
        }
        assert!(failure_update(&conn, url, "HTTP 403", false, now).unwrap());
        let (status, attempts, _, next) = row(&conn, url);
        assert_eq!((status.as_str(), attempts), ("failed", MAX_ATTEMPTS));
        assert_eq!(next, Some(now + FAILED_RETRY_SECS));
        assert!(
            !due_urls(&conn, now + FAILED_RETRY_SECS - 1)
                .unwrap()
                .contains(&url.to_string())
        );
        assert!(
            due_urls(&conn, now + FAILED_RETRY_SECS)
                .unwrap()
                .contains(&url.to_string())
        );
        // A terminal failure gives up at once, with the same cool-down.
        assert!(failure_update(&conn, "https://imgur.gg/f/b", "HTTP 404", true, now).unwrap());
        assert_eq!(
            row(&conn, "https://imgur.gg/f/b").3,
            Some(now + FAILED_RETRY_SECS)
        );
    }

    #[test]
    fn failures_are_classified() {
        use reqwest::StatusCode;
        let kind = |error: DownloadError| match error {
            DownloadError::Retryable(_) => "counted",
            DownloadError::Transient(_) => "transient",
            DownloadError::DiskFull(_) => "disk full",
            DownloadError::Terminal(_) => "terminal",
            DownloadError::ToolMissing(_) => "tool",
        };
        for (status, expected) in [
            (404, "terminal"),
            (410, "terminal"),
            (429, "transient"),
            (408, "transient"),
            (500, "transient"),
            (502, "transient"),
            (503, "transient"),
            (403, "counted"),
            (401, "counted"),
            (400, "counted"),
        ] {
            assert_eq!(
                kind(DownloadError::status(
                    "x",
                    StatusCode::from_u16(status).unwrap()
                )),
                expected,
                "{status}"
            );
        }
        for (error, expected) in [
            (io::Error::from_raw_os_error(libc::ENOSPC), "disk full"),
            (io::Error::from_raw_os_error(libc::EDQUOT), "disk full"),
            (io::Error::from(io::ErrorKind::StorageFull), "disk full"),
            (io::Error::from_raw_os_error(libc::EACCES), "counted"),
            (io::Error::from_raw_os_error(libc::EIO), "counted"),
        ] {
            let message = error.to_string();
            assert_eq!(
                kind(DownloadError::io("writing the file", error)),
                expected,
                "{message}"
            );
        }
    }

    /// yt-dlp's last stderr lines (as `stderr_tail` joins them) decide
    /// whether a failed run uses up an attempt.
    #[test]
    fn yt_dlp_failures_are_classified_by_their_output() {
        use std::os::unix::process::ExitStatusExt;
        let failed = std::process::ExitStatus::from_raw(1 << 8);
        let kind = |tail: &str| match DownloadError::yt_dlp(failed, tail) {
            DownloadError::Retryable(_) => "counted",
            DownloadError::Transient(_) => "transient",
            DownloadError::DiskFull(_) => "disk full",
            DownloadError::Terminal(_) => "terminal",
            DownloadError::ToolMissing(_) => "tool",
        };
        for tail in [
            "ERROR: [youtube] abc: Unable to download webpage: HTTP Error 503: Service Unavailable",
            "ERROR: Unable to download webpage: <urlopen error [Errno -3] Temporary failure in name resolution>",
            "ERROR: unable to download video data: HTTP Error 429: Too Many Requests",
            "ERROR: The read operation timed out",
            // curl (yt-dlp's impersonation backend) while DNS or the network is down.
            "[Instagram] Extracting URL: https://www.instagram.com/p/C8xYz12AbCd/ | [Instagram] C8xYz12AbCd: \
             Setting up session | ERROR: [Instagram] C8xYz12AbCd: Unable to download webpage: Failed to perform, \
             curl: (6) Could not resolve host: www.instagram.com. See https://curl.se/libcurl/c/libcurl-errors.html \
             first for more details. (caused by TransportError('Failed to perform, curl: (6) Could not resolve \
             host: www.instagram.com.'))",
            "ERROR: [twitter] 1790000000000000000: Unable to download JSON metadata: Failed to perform, curl: (7) \
             Failed to connect to api.x.com port 443 after 2 ms: Couldn't connect to server. See \
             https://curl.se/libcurl/c/libcurl-errors.html first for more details.",
            "ERROR: [twitter] 1790000000000000000: Unable to download JSON metadata: Failed to perform, curl: (28) \
             Operation timed out after 30001 milliseconds with 0 bytes received.",
            "ERROR: [youtube] dQw4w9WgXcQ: Unable to download API page: <urlopen error [Errno 101] Network is \
             unreachable> (caused by TransportError('<urlopen error [Errno 101] Network is unreachable>'))",
            "ERROR: [youtube] dQw4w9WgXcQ: Unable to download webpage: [Errno 104] Connection reset by peer \
             (caused by ConnectionResetError(104, 'Connection reset by peer'))",
            "ERROR: [download] Got error: HTTPSConnectionPool(host='rr1---sn-4g5e6nzz.googlevideo.com', \
             port=443): Read timed out. (read timeout=30.0)",
            "ERROR: unable to download video data: [Errno 111] Connection refused",
            "ERROR: [youtube] abc: Unable to download webpage: [Errno 113] No route to host",
            "ERROR: [youtube] abc: Unable to download webpage: <urlopen error [Errno 8] nodename nor servname \
             provided, or not known>",
            "ERROR: [Instagram] abc: Unable to download webpage: <urlopen error [Errno -2] Name or service not \
             known> (caused by TransportError('<urlopen error [Errno -2] Name or service not known>'))",
            "ERROR: [youtube] abc: Unable to download webpage: ('Connection aborted.', \
             RemoteDisconnected('Remote end closed connection without response')) (caused by TransportError(...))",
        ] {
            assert_eq!(kind(tail), "transient", "{tail}");
        }
        for tail in [
            "ERROR: unable to write data: [Errno 28] No space left on device",
            "ERROR: Postprocessing: audio conversion failed: Error writing trailer of \
             /srv/songs/.ytdl-0123456789abcdef/audio.opus: No space left on device",
            "ERROR: unable to open for writing: [Errno 122] Disk quota exceeded: \
             '/srv/songs/.ytdl-0123456789abcdef/audio.webm.part'",
        ] {
            assert_eq!(kind(tail), "disk full", "{tail}");
        }
        for tail in [
            "ERROR: [youtube] abc: Video unavailable. This video has been removed by the uploader",
            "ERROR: [instagram] abc: Requested content is not available, rate-limit reached or login required",
            "ERROR: unable to download video data: HTTP Error 403: Forbidden",
            "ERROR: [youtube] abc: Sign in to confirm you're not a bot. Use --cookies-from-browser or --cookies \
             for the authentication.",
            "ERROR: [twitter] 1790000000000000000: No video could be found in this tweet",
            "ERROR: [youtube] abc: Private video. Sign in if you've been granted access to this video",
            "ERROR: [youtube] abc: Unable to download webpage: HTTP Error 404: Not Found",
        ] {
            assert_eq!(kind(tail), "counted", "{tail}");
        }
    }

    #[tokio::test]
    async fn network_errors_are_transient_unless_the_host_is_not_public() {
        let clients = HttpClients::new().unwrap();
        let refused = clients
            .media
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .unwrap_err();
        assert!(matches!(
            DownloadError::network("x", refused),
            DownloadError::Transient(_)
        ));
        let private = clients
            .media
            .get("http://localhost:1/")
            .send()
            .await
            .unwrap_err();
        assert!(matches!(
            DownloadError::network("x", private),
            DownloadError::Retryable(message) if message.contains(NON_PUBLIC_HOST)
        ));
    }

    #[test]
    fn repeated_network_errors_pause_a_service_for_the_sync() {
        let mut breaker = HostBreaker::default();
        for _ in 1..HOST_PAUSE_AFTER {
            assert!(!breaker.record("imgur.gg", true));
        }
        // Any other outcome starts the count again.
        breaker.record("imgur.gg", false);
        for _ in 1..HOST_PAUSE_AFTER {
            assert!(!breaker.record("imgur.gg", true));
        }
        assert!(!breaker.is_paused("imgur.gg"));
        assert!(breaker.record("imgur.gg", true));
        assert!(breaker.is_paused("imgur.gg"));
        assert!(!breaker.record("imgur.gg", true), "reported once");
        assert!(!breaker.is_paused("pillows.su"));
    }

    #[test]
    fn backoff_doubles_from_thirty_minutes_up_to_a_week() {
        assert_eq!(backoff_secs(1), 30 * 60);
        assert_eq!(backoff_secs(2), 60 * 60);
        assert_eq!(backoff_secs(3), 2 * 60 * 60);
        assert_eq!(backoff_secs(8), 64 * 60 * 60);
        assert_eq!(backoff_secs(9), 128 * 60 * 60);
        assert_eq!(backoff_secs(10), BACKOFF_MAX_SECS);
        assert_eq!(backoff_secs(200), BACKOFF_MAX_SECS);
        assert_eq!(backoff_secs(0), 30 * 60);
        assert_eq!(transient_backoff_secs(1), 30 * 60);
        assert_eq!(transient_backoff_secs(4), 4 * 60 * 60);
        assert_eq!(transient_backoff_secs(5), 6 * 60 * 60);
        assert_eq!(transient_backoff_secs(500), 6 * 60 * 60);
    }

    #[test]
    fn parses_youtube_start_offsets() {
        assert_eq!(parse_start_offset("123"), Some(123));
        assert_eq!(parse_start_offset("33s"), Some(33));
        assert_eq!(parse_start_offset("1m30s"), Some(90));
        assert_eq!(parse_start_offset("1h2m3s"), Some(3723));
        assert_eq!(parse_start_offset("2h"), Some(7200));
        assert_eq!(parse_start_offset("1H2M"), Some(3720));
        for invalid in [
            "",
            "abc",
            "1m30",
            "30s1m",
            "1s1s",
            "m",
            "1.5s",
            "-5",
            "1d",
            "99999999999999999999h",
        ] {
            assert_eq!(parse_start_offset(invalid), None, "{invalid}");
        }
        let url = Url::parse("https://youtu.be/abc?t=1m30s").unwrap();
        assert_eq!(start_offset_of(&url), Some(90));
        let url = Url::parse("https://www.youtube.com/watch?v=abc&t=0").unwrap();
        assert_eq!(start_offset_of(&url), None);
        let url = Url::parse("https://www.youtube.com/watch?v=abc#t=45").unwrap();
        assert_eq!(start_offset_of(&url), Some(45));
        let url = Url::parse("https://www.youtube.com/watch?v=abc&t=later").unwrap();
        assert_eq!(start_offset_of(&url), None);
    }

    #[test]
    fn classifies_download_sources_like_the_importer() {
        for (link, expected) in [
            (
                "https://pillows.su/f/0598cc0e31be5d31d4a1c2501c659989",
                Some(Source::Pillows),
            ),
            ("https://imgur.gg/f/K1hj8pX", Some(Source::ImgurGg)),
            ("https://imgur.gg/f/K1hj8pX/", Some(Source::ImgurGg)),
            ("https://imgur.gg/a/K1hj8pX", None),
            ("https://i.imgur.gg/K1hj8pX-x.mp3", None),
            ("https://youtu.be/abc?t=33", Some(Source::YouTube)),
            ("http://www.youtube.com/watch?v=abc", Some(Source::YouTube)),
            (
                "https://music.youtube.com/watch?v=abc",
                Some(Source::YouTube),
            ),
            ("https://m.youtube.com/watch?v=abc", Some(Source::YouTube)),
            ("https://www.instagram.com/p/abc/", Some(Source::Instagram)),
            ("https://x.com/user/status/1", Some(Source::X)),
            ("https://mobile.twitter.com/user/status/1", Some(Source::X)),
            ("https://soundcloud.com/a/b", None),
            ("ftp://pillows.su/f/abc", None),
        ] {
            let url = Url::parse(link).unwrap();
            assert_eq!(source_of(&url), expected, "{link}");
            assert_eq!(
                importer::is_downloadable_url(link),
                expected.is_some(),
                "importer disagrees on {link}"
            );
        }
    }

    #[test]
    fn natural_names() {
        assert_eq!(
            natural_stem("https://pillows.su/f/0598cc0e31be5d31d4a1c2501c659989"),
            "0598cc0e31be5d31d4a1c2501c659989"
        );
        assert_eq!(
            natural_stem("https://imgur.gg/f/abc"),
            sha256_hex("https://imgur.gg/f/abc")
        );
        assert_eq!(split_media_name("abc.mp3"), Some(("abc", "mp3")));
        assert_eq!(split_media_name("abc.mp3.1790000000.invalid"), None);
        assert_eq!(split_media_name("abc.mp3.0123456789abcdef.tmp"), None);
        assert_eq!(split_media_name("noext"), None);
        assert_eq!(split_media_name(".ytdl-1234"), None);
    }

    #[test]
    fn reads_content_disposition_names() {
        assert_eq!(
            disposition_filename("attachment; filename=\"x.mp3\"").as_deref(),
            Some("x.mp3")
        );
        assert_eq!(
            disposition_filename("attachment; filename*=UTF-8''a%20b.flac").as_deref(),
            Some("a b.flac")
        );
        assert_eq!(
            disposition_filename(
                "attachment; filename=\"fallback.wav\"; filename*=utf-8''real%C3%A9.flac"
            )
            .as_deref(),
            Some("realé.flac")
        );
        assert_eq!(
            disposition_filename("attachment; FILENAME=plain.ogg").as_deref(),
            Some("plain.ogg")
        );
        assert_eq!(
            disposition_filename("attachment; filename=\"quo\\\"ted.m4a\"").as_deref(),
            Some("quo\"ted.m4a")
        );
        assert_eq!(disposition_filename("attachment"), None);
        assert_eq!(
            extension_of_name("KW - Interlude (4.11.13).MP3").as_deref(),
            Some("mp3")
        );
        assert_eq!(extension_of_name("noext"), None);
        assert_eq!(extension_of_name("x.tar.gz!"), None);
    }

    #[test]
    fn finds_the_media_link_on_imgur_pages() {
        let audio_page = r#"<link rel="preload" as="image" href="https://i.imgur.gg/other-image.png"/>
            <h2>KW - Interlude Ref (4.11.13).mp3</h2>
            <audio src="https://i.imgur.gg/K1hj8pX-KW_-_Interlude_Ref_(4.11.13).mp3"></audio>"#;
        assert_eq!(
            imgur_media_url(audio_page, "K1hj8pX").unwrap().as_str(),
            "https://i.imgur.gg/K1hj8pX-KW_-_Interlude_Ref_(4.11.13).mp3"
        );
        let image_page = r#"<link rel="preload" as="image" href="https://i.imgur.gg/HWl40jA-image.png"/>
            <meta property="og:image" content="https://i.imgur.gg/HWl40jA-image.png"/>"#;
        assert_eq!(
            imgur_media_url(image_page, "HWl40jA").unwrap().as_str(),
            "https://i.imgur.gg/HWl40jA-image.png"
        );
        let escaped = r#"<audio controls src="https://i.imgur.gg/abc-Tom &amp; Jerry.mp3">"#;
        assert_eq!(
            imgur_media_url(escaped, "abc").unwrap().path(),
            "/abc-Tom%20&%20Jerry.mp3"
        );
        // Links to other files or hosts are ignored.
        assert!(imgur_media_url(image_page, "K1hj8pX").is_none());
        assert!(
            imgur_media_url(r#"<audio src="https://evil.example/abc-x.mp3">"#, "abc").is_none()
        );
        assert!(
            imgur_media_url(r#"<audio src="https://i.imgur.gg/abcdef-x.mp3">"#, "abc").is_none()
        );
        let missing = r#"<div class="border"><h1 class="text-2xl">File not found</h1></div>"#;
        assert!(imgur_media_url(missing, "abc").is_none());
        assert!(IMGUR_FILE_MISSING.is_match(missing));
        assert!(!IMGUR_FILE_MISSING.is_match(audio_page));
    }

    #[test]
    fn cover_hosts_are_allowlisted() {
        for allowed in [
            "https://docs.google.com/sheets-images-rt/abc=s512",
            "https://lh7-rt.googleusercontent.com/rd-sheets-images-rt/abc=s512",
        ] {
            assert!(
                is_allowed_cover_url(&Url::parse(allowed).unwrap()),
                "{allowed}"
            );
        }
        for denied in [
            "http://docs.google.com/sheets-images-rt/abc",
            "https://docs.google.com:8443/x",
            "https://googleusercontent.com.evil.example/x",
            "https://evilgoogleusercontent.com/x",
            "https://example.com/x.png",
        ] {
            assert!(
                !is_allowed_cover_url(&Url::parse(denied).unwrap()),
                "{denied}"
            );
        }
    }

    #[test]
    fn sniffs_image_types() {
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n...."), Some("png"));
        assert_eq!(sniff_image(b"RIFF\x00\x00\x00\x00WEBPVP8 "), Some("webp"));
        assert_eq!(sniff_image(b"GIF89a..."), Some("gif"));
        assert_eq!(sniff_image(b"<html>"), None);
    }

    #[test]
    fn free_space_is_reported() {
        assert!(available_bytes(&std::env::temp_dir()).unwrap() > 0);
        assert!(available_bytes(Path::new("/no/such/dir")).is_err());
    }
}
