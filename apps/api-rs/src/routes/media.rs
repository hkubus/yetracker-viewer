//! Media endpoints: `/songs/{id}/stream` (the stored file, or an Opus or AAC
//! transcode with `?quality=<kbps>`), `/songs/{id}/download`,
//! `/songs/{id}/duration` and `/eras/{id}/cover`.
//!
//! Handlers never delete media or change download state: a stored file that
//! turns out to be missing, empty or unreadable is answered with 404 and
//! queued for the background sync to re-verify.
//!
//! Transcodes are cached as `<STORAGE_DIR>/transcodes/<key>.ogg` (or `.aac`)
//! and then served like stored files. An uncached transcode runs ffmpeg at
//! full speed into a temporary file (in its own task, holding one of
//! `MAX_CONCURRENT_TRANSCODES` slots until ffmpeg exits) while the request
//! streams the growing file; other requests for the same transcode join it.
//! A transcode nobody reads any more is stopped after [`ABANDONED_GRACE`].
//! Transcodes that start at an offset (`?start=`) are never cached.

use std::collections::HashMap;
use std::io;
use std::path::{Path as FsPath, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use regex::Regex;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{OwnedSemaphorePermit, watch};
use tracing::{error, info, warn};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use super::{EraId, SongId, js_float};
use crate::config::Config;
use crate::cover_version::cover_version_of;
use crate::db;
use crate::downloader::{covers_dir, primary_cover};
use crate::error::ApiError;
use crate::media::{
    Tool, Uncertain, Verdict, file_input, lower_priority, stderr_tail, unique_suffix,
};
use crate::playable::stored_song_path;
use crate::request::Query;
use crate::serve::{self, ByteRange, FileHeaders, Growth, set_header};
use crate::state::{AppState, SharedState};

/// Stored files and cached transcodes: cacheable, but revalidated (the
/// ETag changes when a song's file is replaced).
const MEDIA_CACHE: &str = "public, no-cache";
const COVER_IMMUTABLE: &str = "public, max-age=31536000, immutable";
const COVER_REVALIDATE: &str = "public, no-cache";
/// A song's file (and so its duration) can change under the same URL, so the
/// duration is revalidated too (JSON responses get an ETag).
const DURATION_CACHE: &str = "public, no-cache";
const OPUS_CONTENT_TYPE: &str = "audio/ogg; codecs=opus";
const AAC_CONTENT_TYPE: &str = "audio/aac";
const JSON_CONTENT_TYPE: &str = "application/json";
/// How long a transcode request waits for a free slot before answering 503.
const SLOT_WAIT: Duration = Duration::from_secs(15);
/// How long a request waits for a new transcode's first bytes.
const FIRST_OUTPUT_WAIT: Duration = Duration::from_secs(30);
const TRANSCODE_TIME_LIMIT: Duration = Duration::from_secs(30 * 60);
/// How long a transcode keeps running once its last reader is gone (a player
/// that reconnects or seeks within it joins the running job instead). A
/// client polling it with HEAD (the web player warming up a quality) counts
/// as a reader.
const ABANDONED_GRACE: Duration = Duration::from_secs(10);
/// The same for a transcode starting at an offset (`?start=`): nothing else
/// ever joins it and a seek replaces it with another one, so it goes sooner.
const SEEK_ABANDONED_GRACE: Duration = Duration::from_secs(3);
/// Latest `?start=` a transcode may begin at, in milliseconds (a day).
const MAX_START_MS: u64 = 86_400_000;
const BUSY_RETRY_SECS: u32 = 5;
const PROBE_RETRY_SECS: u32 = 30;
const STDERR_TAIL_LINES: usize = 20;
const DOWNLOAD_NAME_MAX_CHARS: usize = 120;

static UNSAFE_FILENAME_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[\x00-\x1f\x7f<>:"/\\|?*]+"#).expect("valid filename regex"));
static WHITESPACE_RUN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("valid whitespace regex"));

/// Bitrates the web player offers. Every other value is refused: each bitrate
/// is its own cached transcode, so accepting any number would let a client
/// keep every transcode slot busy (and churn the cache) with fresh variants.
pub const TRANSCODE_QUALITIES: [u16; 5] = [64, 128, 192, 256, 320];

/// `?quality=<kbps>`: absent or blank means the original file; otherwise one
/// of [`TRANSCODE_QUALITIES`], written like an id (digits, no leading zero).
/// Parameters resolve like on the JSON routes (see [`Query`]).
fn transcode_quality(query: Option<&str>) -> Result<Option<u16>, ApiError> {
    let invalid = || ApiError::bad_request("Invalid quality for file");
    let query = Query::parse(query);
    let Some(value) = query.value("quality").map_err(|_| invalid())? else {
        return Ok(None);
    };
    if value.len() > 3 || value.starts_with('0') || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid());
    }
    match value.parse::<u16>() {
        Ok(kbps) if TRANSCODE_QUALITIES.contains(&kbps) => Ok(Some(kbps)),
        _ => Err(invalid()),
    }
}

/// Output format of a transcode (`?format=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscodeFormat {
    /// Ogg/Opus, the default (what the web player uses).
    Opus,
    /// ADTS AAC-LC, for AVPlayer (iOS), which cannot play Ogg.
    Aac,
}

impl TranscodeFormat {
    fn content_type(self) -> &'static str {
        match self {
            Self::Opus => OPUS_CONTENT_TYPE,
            Self::Aac => AAC_CONTENT_TYPE,
        }
    }

    /// Extension of a cached transcode (also what the cache trimming counts).
    fn extension(self) -> &'static str {
        match self {
            Self::Opus => "ogg",
            Self::Aac => "aac",
        }
    }

    /// Encoder and container arguments for ffmpeg.
    fn ffmpeg_args(self) -> [&'static str; 4] {
        match self {
            Self::Opus => ["-c:a", "libopus", "-f", "ogg"],
            Self::Aac => ["-c:a", "aac", "-f", "adts"],
        }
    }
}

/// A transcode as asked for by `?quality=`, `?format=` and `?start=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TranscodeRequest {
    kbps: u16,
    format: TranscodeFormat,
    /// Where the output begins, in milliseconds (`None` = the beginning).
    start_ms: Option<u64>,
}

impl TranscodeRequest {
    /// Only whole-song transcodes are cached: every seek of a client that
    /// restarts the stream at an offset would otherwise be its own entry.
    fn cacheable(&self) -> bool {
        self.start_ms.is_none()
    }

    /// How long the job outlives its last reader.
    fn abandoned_grace(&self) -> Duration {
        if self.cacheable() {
            ABANDONED_GRACE
        } else {
            SEEK_ABANDONED_GRACE
        }
    }
}

/// `?quality=` (see [`transcode_quality`]) plus, only with a quality,
/// `?format=opus|aac` (absent or blank = `opus`) and `?start=<seconds>`
/// (plain decimal seconds, at most 3 decimals, up to 86400; absent, blank or
/// 0 = the beginning), so clients whose player cannot seek a live stream can
/// restart it at an offset.
fn transcode_request(query: Option<&str>) -> Result<Option<TranscodeRequest>, ApiError> {
    let Some(kbps) = transcode_quality(query)? else {
        return Ok(None);
    };
    let parsed = Query::parse(query);
    let format = match parsed.value("format") {
        Ok(None | Some("opus")) => TranscodeFormat::Opus,
        Ok(Some("aac")) => TranscodeFormat::Aac,
        _ => return Err(ApiError::bad_request("Invalid format")),
    };
    let start_ms = match parsed.value("start") {
        Ok(None) => None,
        Ok(Some(value)) => {
            start_millis(value).ok_or_else(|| ApiError::bad_request("Invalid start"))?
        }
        Err(_) => return Err(ApiError::bad_request("Invalid start")),
    };
    Ok(Some(TranscodeRequest {
        kbps,
        format,
        start_ms,
    }))
}

/// `93.5` → `Some(Some(93500))`, `0` → `Some(None)`, anything that is not
/// plain decimal seconds within [`MAX_START_MS`] → `None`.
fn start_millis(value: &str) -> Option<Option<u64>> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if whole.is_empty()
        || whole.len() > 5
        || !digits(whole)
        || fraction.len() > 3
        || !digits(fraction)
        || (value.contains('.') && fraction.is_empty())
    {
        return None;
    }
    let millis =
        whole.parse::<u64>().ok()? * 1000 + format!("{fraction:0<3}").parse::<u64>().ok()?;
    (millis <= MAX_START_MS).then_some((millis > 0).then_some(millis))
}

fn file_not_found() -> ApiError {
    ApiError::not_found("Song file not found")
}

struct SongFile {
    title: Option<String>,
    name: Option<String>,
    url: Option<String>,
    filename: Option<String>,
    duration: Option<f64>,
}

async fn lookup_song_file(state: &AppState, song_id: i64) -> Result<SongFile, ApiError> {
    let song = db::call(&state.pool, move |conn| {
        let mut statement = conn.prepare_cached(
            "SELECT songs.title, songs.name, files.url, files.filename, files.duration \
             FROM songs LEFT JOIN files ON files.url = songs.url WHERE songs.id = ?1",
        )?;
        let mut rows = statement.query_map([song_id], |row| {
            Ok(SongFile {
                title: row.get(0)?,
                name: row.get(1)?,
                url: row.get(2)?,
                filename: row.get(3)?,
                duration: row.get(4)?,
            })
        })?;
        rows.next().transpose().map_err(ApiError::from)
    })
    .await?;
    song.ok_or_else(|| ApiError::not_found("Song not found"))
}

/// A song's stored file, opened (so the validators describe exactly the
/// bytes that get served).
struct StoredFile {
    file: tokio::fs::File,
    path: PathBuf,
    filename: String,
    size: u64,
    modified: Option<SystemTime>,
}

impl StoredFile {
    fn mtime_ms(&self) -> i64 {
        self.modified.map(serve::unix_millis).unwrap_or(0)
    }
}

/// Opens the song's stored file. A missing or empty file is a 404, stops
/// counting as playable at once, and is queued for re-verification.
async fn open_stored_file(state: &AppState, song: &SongFile) -> Result<StoredFile, ApiError> {
    let Some(filename) = song.filename.clone() else {
        return Err(file_not_found());
    };
    let path = stored_song_path(&state.config.songs_path, &filename)?;
    let reverify = || {
        state.playable.mark_missing(&filename);
        if let Some(url) = &song.url {
            state.reverify.enqueue(url);
        }
    };
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            reverify();
            return Err(file_not_found());
        }
        Err(error) => {
            error!(path = %path.display(), %error, "could not open song file");
            return Err(ApiError::internal("Could not read song file"));
        }
    };
    let metadata = file.metadata().await.map_err(|error| {
        error!(path = %path.display(), %error, "could not stat song file");
        ApiError::internal("Could not read song file")
    })?;
    if !metadata.is_file() || metadata.len() == 0 {
        reverify();
        return Err(file_not_found());
    }
    Ok(StoredFile {
        file,
        path,
        filename,
        size: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

/// `Content-Type` of a stored song by extension.
pub fn content_type_for(filename: &str) -> &'static str {
    let extension = FsPath::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        "mp3" => "audio/mpeg",
        "opus" => "audio/ogg; codecs=opus",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "aif" | "aiff" | "aifc" => "audio/aiff",
        "m4a" | "mp4" | "alac" => "audio/mp4",
        "aac" => "audio/aac",
        "webm" | "weba" => "audio/webm",
        "wma" => "audio/x-ms-wma",
        _ => "application/octet-stream",
    }
}

async fn serve_stored(
    headers: &HeaderMap,
    head_only: bool,
    stored: StoredFile,
    file_headers: &FileHeaders<'_>,
) -> Result<Response, ApiError> {
    let path = stored.path.clone();
    serve::file_response(
        headers,
        head_only,
        stored.file,
        stored.size,
        stored.modified,
        file_headers,
    )
    .await
    .map_err(|error| {
        error!(path = %path.display(), %error, "could not stream song file");
        ApiError::internal("Could not stream song")
    })
}

pub async fn stream_song(
    State(state): State<SharedState>,
    method: Method,
    SongId(song_id): SongId,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let wanted = transcode_request(query.as_deref())?;
    let song = lookup_song_file(&state, song_id).await?;
    let stored = open_stored_file(&state, &song).await?;
    let head_only = method == Method::HEAD;
    match wanted {
        Some(wanted) => transcode(&state, &headers, head_only, stored, song.url, wanted).await,
        None => {
            let content_type = content_type_for(&stored.filename);
            let file_headers = FileHeaders {
                content_type,
                cache_control: MEDIA_CACHE,
                content_disposition: None,
            };
            serve_stored(&headers, head_only, stored, &file_headers).await
        }
    }
}

pub async fn download_song(
    State(state): State<SharedState>,
    method: Method,
    SongId(song_id): SongId,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let song = lookup_song_file(&state, song_id).await?;
    let stored = open_stored_file(&state, &song).await?;
    let extension = FsPath::new(&stored.filename)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_default();
    let title = song.title.as_deref().or(song.name.as_deref());
    let name = download_filename(title, song_id, &extension);
    let disposition = content_disposition(&name);
    let file_headers = FileHeaders {
        content_type: "application/octet-stream",
        cache_control: MEDIA_CACHE,
        content_disposition: Some(&disposition),
    };
    serve_stored(&headers, method == Method::HEAD, stored, &file_headers).await
}

pub async fn get_song_duration(
    State(state): State<SharedState>,
    SongId(song_id): SongId,
) -> Result<Response, ApiError> {
    let song = lookup_song_file(&state, song_id).await?;
    // The stored duration only counts while the file is there.
    let stored = open_stored_file(&state, &song).await?;
    if let Some(duration) = song
        .duration
        .filter(|duration| duration.is_finite() && *duration > 0.0)
    {
        return Ok(duration_response(duration));
    }
    if !state.tools.available(Tool::Ffprobe) {
        return Err(ApiError::unavailable("Duration probing unavailable"));
    }
    let verdict = state
        .probes
        .verdict(&stored.path, stored.size, stored.mtime_ms())
        .await;
    match verdict {
        Verdict::Valid {
            duration: Some(duration),
        } => {
            if let Some(url) = song.url.clone() {
                let filename = stored.filename.clone();
                // Idempotent cache of the probe; the row is only touched while
                // it still points at the file that was probed.
                let persisted = db::call(&state.pool, move |conn| {
                    conn.execute(
                        "UPDATE files SET duration = ?1 WHERE url = ?2 AND filename = ?3",
                        rusqlite::params![duration, url, filename],
                    )?;
                    Ok(())
                })
                .await;
                if let Err(error) = persisted {
                    warn!(song_id, %error, "could not store the probed duration");
                }
            }
            Ok(duration_response(duration))
        }
        Verdict::Valid { duration: None } => {
            Err(ApiError::unprocessable("Could not determine file duration"))
        }
        Verdict::Invalid(reason) => {
            if let Some(url) = &song.url {
                state.reverify.enqueue(url);
            }
            info!(
                song_id,
                filename = %stored.filename,
                reason = reason.as_str(),
                "ffprobe rejected a stored file; queued for re-verification"
            );
            Err(file_not_found())
        }
        Verdict::Unknown(Uncertain::ToolMissing) => {
            state.tools.mark_missing(Tool::Ffprobe);
            Err(ApiError::unavailable("Duration probing unavailable"))
        }
        Verdict::Unknown(uncertain) => {
            warn!(song_id, filename = %stored.filename, error = %uncertain, "duration probe failed");
            Err(ApiError::busy(
                "Could not determine file duration",
                PROBE_RETRY_SECS,
            ))
        }
    }
}

fn duration_response(duration: f64) -> Response {
    let body = serde_json::to_vec(&serde_json::json!({ "duration": js_float(duration) }))
        .expect("a JSON object serialises");
    let mut response = Response::new(Body::from(body));
    set_header(&mut response, header::CONTENT_TYPE, JSON_CONTENT_TYPE);
    set_header(&mut response, header::CACHE_CONTROL, DURATION_CACHE);
    response
}

// ---------------------------------------------------------------------------
// Download names
// ---------------------------------------------------------------------------

/// Invisible formatting characters (bidi overrides, zero-width marks) that
/// could disguise a file name.
fn is_format_char(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

fn is_reserved_windows_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit())
}

/// Cuts `text` to at most `max_chars` characters, preferring the last word
/// boundary in the second half of the allowance.
fn truncate_at_word_boundary(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    let boundary = cut
        .char_indices()
        .rev()
        .find(|(index, character)| {
            *character == ' ' && cut[..*index].chars().count() >= max_chars / 2
        })
        .map(|(index, _)| index);
    let kept = match boundary {
        Some(index) => &cut[..index],
        None => cut.as_str(),
    };
    kept.trim_end_matches([' ', '.']).to_string()
}

/// The file name offered for `/download`: the song's title (first line),
/// without characters that are unsafe in file names, at most 120 characters
/// cut at a word boundary, plus the stored file's extension.
pub fn download_filename(title: Option<&str>, song_id: i64, extension: &str) -> String {
    let title = title
        .and_then(|title| title.lines().next())
        .unwrap_or_default();
    let visible: String = UNSAFE_FILENAME_CHARS
        .replace_all(title, " ")
        .chars()
        .map(|character| {
            if character.is_control() || is_format_char(character) {
                ' '
            } else {
                character
            }
        })
        .collect();
    let collapsed = WHITESPACE_RUN.replace_all(visible.trim(), " ");
    let trimmed = collapsed.trim_matches([' ', '.']);
    let mut base = truncate_at_word_boundary(trimmed, DOWNLOAD_NAME_MAX_CHARS);
    if base.is_empty() {
        base = format!("song-{song_id}");
    } else if is_reserved_windows_name(&base) {
        base.push('_');
    }
    format!("{base}{extension}")
}

/// RFC 5987 `attr-char`.
fn is_attr_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
        )
}

fn encode_ext_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        if is_attr_char(byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `attachment` with an ASCII `filename` (accents stripped, other non-ASCII
/// characters replaced) and the exact name in `filename*`.
pub fn content_disposition(name: &str) -> String {
    let ascii: String = name
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
        .map(|character| {
            if character.is_ascii()
                && !character.is_ascii_control()
                && !matches!(character, '"' | '\\' | '%')
            {
                character
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{}",
        encode_ext_value(name)
    )
}

// ---------------------------------------------------------------------------
// Transcodes
// ---------------------------------------------------------------------------

pub fn transcodes_dir(config: &Config) -> PathBuf {
    config.storage_path.join("transcodes")
}

/// Cache key of a transcode: the stored file's identity (a hash of its name,
/// its size and mtime) plus the bitrate, and the format and start offset
/// when they are not the defaults (so existing Opus entries keep their keys).
fn transcode_key(filename: &str, size: u64, mtime_ms: i64, wanted: &TranscodeRequest) -> String {
    let digest = Sha256::digest(filename.as_bytes());
    let mut key = format!(
        "{}-{size:x}-{:x}-{}",
        hex::encode(&digest[..8]),
        mtime_ms.max(0),
        wanted.kbps
    );
    if wanted.format != TranscodeFormat::Opus {
        key.push('-');
        key.push_str(wanted.format.extension());
    }
    if let Some(start_ms) = wanted.start_ms {
        key.push_str(&format!("-s{start_ms}"));
    }
    key
}

#[derive(Debug, Clone, Copy)]
struct JobProgress {
    growth: Growth,
    /// The finished output was moved into the cache.
    cached: bool,
    /// ffmpeg could not be started because it is not installed.
    tool_missing: bool,
}

struct TranscodeJob {
    temp_path: PathBuf,
    cache_path: PathBuf,
    content_type: &'static str,
    /// The job's own receiver: [`watch::Sender::receiver_count`] minus this
    /// one is the number of requests reading (or waiting for) the output.
    progress: watch::Receiver<JobProgress>,
    /// When a HEAD request last asked about the job.
    polled_at: Mutex<Option<Instant>>,
}

impl TranscodeJob {
    fn note_poll(&self) {
        *self.polled_at.lock().expect("transcode poll time poisoned") = Some(Instant::now());
    }

    fn polled_within(&self, period: Duration) -> bool {
        self.polled_at
            .lock()
            .expect("transcode poll time poisoned")
            .is_some_and(|polled| polled.elapsed() < period)
    }
}

impl Drop for TranscodeJob {
    /// An uncached output is deleted only once nothing can reach the job any
    /// more: a request that joined it (and still has to open the file) keeps
    /// the name alive, so it never finds a finished job without its output.
    /// A cached output was renamed away, so this is a no-op then.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.temp_path);
    }
}

/// Transcodes in progress, by cache key.
#[derive(Default)]
pub struct Transcodes {
    jobs: Mutex<HashMap<String, Arc<TranscodeJob>>>,
}

impl Transcodes {
    fn get(&self, key: &str) -> Option<Arc<TranscodeJob>> {
        self.jobs
            .lock()
            .expect("transcode registry poisoned")
            .get(key)
            .cloned()
    }

    /// Registers `job` unless another one already runs for `key`, which is
    /// returned instead.
    fn register(&self, key: &str, job: &Arc<TranscodeJob>) -> Option<Arc<TranscodeJob>> {
        let mut jobs = self.jobs.lock().expect("transcode registry poisoned");
        if let Some(existing) = jobs.get(key) {
            return Some(existing.clone());
        }
        jobs.insert(key.to_string(), job.clone());
        None
    }

    fn remove(&self, key: &str, job: &Arc<TranscodeJob>) {
        let mut jobs = self.jobs.lock().expect("transcode registry poisoned");
        if jobs
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, job))
        {
            jobs.remove(key);
        }
    }
}

fn busy(retry_after_secs: u32) -> ApiError {
    ApiError::busy(
        "Transcoding capacity reached; try again shortly",
        retry_after_secs,
    )
}

fn unavailable() -> ApiError {
    ApiError::unavailable("Transcoding unavailable")
}

/// Headers of a live (uncached) transcode. The body has no known length, so
/// no `Content-Length` is sent (also for HEAD).
fn live_response(content_type: &'static str, body: Body) -> Response {
    let mut response = Response::new(body);
    set_header(&mut response, header::CONTENT_TYPE, content_type);
    set_header(&mut response, header::ACCEPT_RANGES, "none");
    set_header(&mut response, header::CACHE_CONTROL, "no-store");
    response
}

fn unknown_length_empty_body() -> Body {
    Body::from_stream(futures_util::stream::empty::<Result<Bytes, io::Error>>())
}

/// Serves a cached transcode like a stored file, if there is one.
async fn serve_cached_transcode(
    headers: &HeaderMap,
    head_only: bool,
    path: &FsPath,
    content_type: &'static str,
) -> Result<Option<Response>, ApiError> {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            warn!(path = %path.display(), %error, "could not open a cached transcode");
            return Ok(None);
        }
    };
    let Ok(metadata) = file.metadata().await else {
        return Ok(None);
    };
    if !metadata.is_file() || metadata.len() == 0 {
        return Ok(None);
    }
    touch_access_time(path);
    let file_headers = FileHeaders {
        content_type,
        cache_control: MEDIA_CACHE,
        content_disposition: None,
    };
    let response = serve::file_response(
        headers,
        head_only,
        file,
        metadata.len(),
        metadata.modified().ok(),
        &file_headers,
    )
    .await
    .map_err(|error| {
        error!(path = %path.display(), %error, "could not stream a cached transcode");
        ApiError::internal("Could not stream song")
    })?;
    Ok(Some(response))
}

/// Sample rates of the ADTS `sampling_frequency_index`.
const ADTS_SAMPLE_RATES: [u64; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

/// Byte offset of the frame holding the sample at `start_ms` in a complete
/// ADTS stream of `len` bytes, found by walking its frame headers. `None`
/// when the stream ends before that, or is not plain ADTS from its first
/// byte (a leading tag, a damaged header, a changing sample rate).
fn adts_frame_at<R: io::Read + io::Seek>(
    reader: &mut io::BufReader<R>,
    len: u64,
    start_ms: u64,
) -> io::Result<Option<u64>> {
    let mut header = [0u8; 7];
    let mut offset = 0u64;
    let mut samples = 0u64;
    // The first frame's sampling-frequency index, and the target sample.
    let mut rate: Option<(u8, u64)> = None;
    while offset + header.len() as u64 <= len {
        io::Read::read_exact(reader, &mut header)?;
        // Syncword 0xFFF, layer 0.
        if header[0] != 0xFF || header[1] & 0xF6 != 0xF0 {
            return Ok(None);
        }
        let header_len = if header[1] & 0x01 == 1 { 7 } else { 9 };
        let index = (header[2] >> 2) & 0x0F;
        let frame_len = (u64::from(header[3] & 0x03) << 11)
            | (u64::from(header[4]) << 3)
            | u64::from(header[5] >> 5);
        let frame_samples = (u64::from(header[6] & 0x03) + 1) * 1024;
        if frame_len < header_len || offset + frame_len > len {
            return Ok(None);
        }
        let target = match rate {
            None => {
                let Some(hz) = ADTS_SAMPLE_RATES.get(usize::from(index)) else {
                    return Ok(None);
                };
                let target = start_ms * hz / 1000;
                rate = Some((index, target));
                target
            }
            Some((first, target)) if first == index => target,
            Some(_) => return Ok(None),
        };
        if samples + frame_samples > target {
            return Ok(Some(offset));
        }
        samples += frame_samples;
        offset += frame_len;
        reader.seek_relative(frame_len as i64 - header.len() as i64)?;
    }
    Ok(None)
}

/// Answers `?format=aac&start=` from the cached whole-song AAC transcode
/// when there is one: its bytes from the frame holding the start (see
/// [`adts_frame_at`]), with the headers of a live transcode, without ffmpeg
/// or a transcode slot. `None` sends the request down the ffmpeg path.
async fn serve_cached_seek(
    dir: &FsPath,
    source: &StoredFile,
    wanted: &TranscodeRequest,
    head_only: bool,
) -> Result<Option<Response>, ApiError> {
    let (TranscodeFormat::Aac, Some(start_ms)) = (wanted.format, wanted.start_ms) else {
        return Ok(None);
    };
    let whole = TranscodeRequest {
        start_ms: None,
        ..*wanted
    };
    let key = transcode_key(&source.filename, source.size, source.mtime_ms(), &whole);
    let path = dir.join(format!("{key}.{}", whole.format.extension()));
    let scanned = path.clone();
    let found =
        tokio::task::spawn_blocking(move || -> io::Result<Option<(std::fs::File, u64, u64)>> {
            let file = match std::fs::File::open(&scanned) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                return Ok(None);
            }
            let len = metadata.len();
            let mut reader = io::BufReader::with_capacity(64 * 1024, &file);
            let offset = adts_frame_at(&mut reader, len, start_ms)?;
            drop(reader);
            Ok(offset.map(|offset| (file, offset, len)))
        })
        .await
        .map_err(ApiError::unexpected)?;
    let (file, offset, len) = match found {
        Ok(Some(found)) => found,
        Ok(None) => return Ok(None),
        Err(error) => {
            warn!(path = %path.display(), %error, "could not read a cached transcode");
            return Ok(None);
        }
    };
    touch_access_time(&path);
    if head_only {
        return Ok(Some(live_response(
            AAC_CONTENT_TYPE,
            unknown_length_empty_body(),
        )));
    }
    let range = ByteRange {
        start: offset,
        end: len - 1,
    };
    let body = serve::file_body(tokio::fs::File::from_std(file), Some(range))
        .await
        .map_err(|error| {
            error!(path = %path.display(), %error, "could not stream a cached transcode");
            ApiError::internal("Could not stream song")
        })?;
    Ok(Some(live_response(AAC_CONTENT_TYPE, body)))
}

/// Records a cache hit for the LRU eviction (the access time; the mtime is
/// part of the ETag and stays untouched).
fn touch_access_time(path: &FsPath) {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        if let Ok(file) = std::fs::File::open(&path) {
            let _ = file.set_times(std::fs::FileTimes::new().set_accessed(SystemTime::now()));
        }
    });
}

async fn transcode(
    state: &SharedState,
    headers: &HeaderMap,
    head_only: bool,
    source: StoredFile,
    url: Option<String>,
    wanted: TranscodeRequest,
) -> Result<Response, ApiError> {
    let dir = transcodes_dir(&state.config);
    let key = transcode_key(&source.filename, source.size, source.mtime_ms(), &wanted);
    let cache_path = dir.join(format!("{key}.{}", wanted.format.extension()));
    let content_type = wanted.format.content_type();

    if wanted.cacheable()
        && let Some(response) =
            serve_cached_transcode(headers, head_only, &cache_path, content_type).await?
    {
        return Ok(response);
    }
    if let Some(response) = serve_cached_seek(&dir, &source, &wanted, head_only).await? {
        return Ok(response);
    }
    if !state.tools.available(Tool::Ffmpeg) {
        return Err(unavailable());
    }
    if let Some(job) = state.transcodes.get(&key) {
        return attach(headers, head_only, job).await;
    }
    if head_only {
        // Never start a transcode for HEAD; report what a GET would get.
        return if state.transcode_slots.available_permits() == 0 {
            Err(busy(BUSY_RETRY_SECS))
        } else {
            Ok(live_response(content_type, unknown_length_empty_body()))
        };
    }

    let permit = match tokio::time::timeout(
        SLOT_WAIT,
        state.transcode_slots.clone().acquire_owned(),
    )
    .await
    {
        Ok(Ok(permit)) => permit,
        _ => return Err(busy(BUSY_RETRY_SECS)),
    };
    // The transcode may have been finished or started while this request
    // waited for a slot.
    if wanted.cacheable()
        && let Some(response) =
            serve_cached_transcode(headers, false, &cache_path, content_type).await?
    {
        return Ok(response);
    }
    if let Some(job) = state.transcodes.get(&key) {
        drop(permit);
        return attach(headers, false, job).await;
    }

    if let Err(error) = tokio::fs::create_dir_all(&dir).await {
        error!(path = %dir.display(), %error, "could not create the transcode cache");
        return Err(ApiError::internal("Could not transcode song"));
    }
    let temp_path = dir.join(format!("{key}.{}.tmp", unique_suffix()));
    let output = match tokio::fs::File::create(&temp_path).await {
        Ok(output) => output,
        Err(error) => {
            error!(path = %temp_path.display(), %error, "could not create a transcode file");
            return Err(ApiError::internal("Could not transcode song"));
        }
    };
    let (sender, receiver) = watch::channel(JobProgress {
        growth: Growth::Writing { written: 0 },
        cached: false,
        tool_missing: false,
    });
    let job = Arc::new(TranscodeJob {
        temp_path,
        cache_path,
        content_type,
        progress: receiver,
        polled_at: Mutex::new(None),
    });
    if let Some(existing) = state.transcodes.register(&key, &job) {
        drop(output);
        drop(permit);
        let _ = tokio::fs::remove_file(&job.temp_path).await;
        return attach(headers, false, existing).await;
    }
    tokio::spawn(run_transcode(
        state.clone(),
        key,
        job.clone(),
        sender,
        TranscodeInput {
            source: source.path,
            wanted,
            output,
            url,
        },
        permit,
    ));
    attach(headers, false, job).await
}

/// Answers a request from a running transcode: waits for its first bytes (a
/// failure before that is a clean 500/503), then streams the growing file.
async fn attach(
    headers: &HeaderMap,
    head_only: bool,
    job: Arc<TranscodeJob>,
) -> Result<Response, ApiError> {
    if head_only {
        job.note_poll();
        return Ok(live_response(job.content_type, unknown_length_empty_body()));
    }
    let mut progress = job.progress.clone();
    let started = tokio::time::timeout(FIRST_OUTPUT_WAIT, async {
        progress
            .wait_for(|progress| progress.growth != Growth::Writing { written: 0 })
            .await
            .map(|snapshot| *snapshot)
    })
    .await;
    let snapshot = match started {
        Ok(Ok(snapshot)) => snapshot,
        // The job ended without another update: its last state is final.
        Ok(Err(_)) => *job.progress.borrow(),
        Err(_) => {
            error!(path = %job.temp_path.display(), "the transcode produced no output in time");
            return Err(ApiError::internal("Could not transcode song"));
        }
    };
    match snapshot.growth {
        Growth::Failed if snapshot.tool_missing => return Err(unavailable()),
        Growth::Failed | Growth::Complete { size: 0 } | Growth::Writing { written: 0 } => {
            return Err(ApiError::internal("Could not transcode song"));
        }
        _ => {}
    }
    match tokio::fs::File::open(&job.temp_path).await {
        Ok(file) => Ok(live_response(
            job.content_type,
            serve::growing_file_body(file, job.progress.clone(), |progress: &JobProgress| {
                progress.growth
            }),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // It finished (and moved into the cache) in the meantime.
            let cached = job.progress.borrow().cached;
            if cached
                && let Some(response) =
                    serve_cached_transcode(headers, false, &job.cache_path, job.content_type)
                        .await?
            {
                return Ok(response);
            }
            Err(busy(1))
        }
        Err(error) => {
            error!(path = %job.temp_path.display(), %error, "could not open a running transcode");
            Err(ApiError::internal("Could not transcode song"))
        }
    }
}

struct TranscodeInput {
    source: PathBuf,
    wanted: TranscodeRequest,
    output: tokio::fs::File,
    url: Option<String>,
}

enum TranscodeError {
    ToolMissing,
    /// Every reader left and none came back in time (see
    /// [`TranscodeRequest::abandoned_grace`]).
    Abandoned,
    Failed(String),
}

/// Runs one transcode to completion, independent of the requests reading it.
async fn run_transcode(
    state: SharedState,
    key: String,
    job: Arc<TranscodeJob>,
    progress: watch::Sender<JobProgress>,
    input: TranscodeInput,
    permit: OwnedSemaphorePermit,
) {
    let TranscodeInput {
        source,
        wanted,
        output,
        url,
    } = input;
    let kbps = wanted.kbps;
    let outcome = run_ffmpeg(&source, &wanted, output, &progress, &job).await;
    // The slot belongs to ffmpeg, not to the clients reading its output.
    drop(permit);
    match outcome {
        Ok(size) if size > 0 => {
            // An output larger than the whole budget would evict itself at
            // once, and every request would find the cache empty anyway.
            let keep = wanted.cacheable() && size <= state.config.transcode_cache_max_bytes;
            let cached = keep
                && match tokio::fs::rename(&job.temp_path, &job.cache_path).await {
                    Ok(()) => true,
                    Err(error) => {
                        warn!(path = %job.cache_path.display(), %error, "could not cache a transcode");
                        false
                    }
                };
            progress.send_modify(|progress| {
                progress.growth = Growth::Complete { size };
                progress.cached = cached;
            });
            // An uncached output stays until the job is dropped (see `Drop`).
            if cached {
                let dir = transcodes_dir(&state.config);
                let budget = state.config.transcode_cache_max_bytes;
                if let Err(error) = enforce_transcode_budget(dir, budget).await {
                    warn!(%error, "could not trim the transcode cache");
                }
            }
        }
        Ok(_) => {
            warn!(source = %source.display(), kbps, "ffmpeg produced no output");
            fail_transcode(&state, &job, &progress, false, url.as_deref()).await;
        }
        Err(TranscodeError::Abandoned) => {
            info!(source = %source.display(), kbps, "stopped a transcode nobody reads");
            // Not the source's fault: no re-verification.
            fail_transcode(&state, &job, &progress, false, None).await;
        }
        Err(TranscodeError::ToolMissing) => {
            state.tools.mark_missing(Tool::Ffmpeg);
            fail_transcode(&state, &job, &progress, true, None).await;
        }
        Err(TranscodeError::Failed(detail)) => {
            warn!(source = %source.display(), kbps, error = %detail, "transcode failed");
            fail_transcode(&state, &job, &progress, false, url.as_deref()).await;
        }
    }
    state.transcodes.remove(&key, &job);
}

async fn fail_transcode(
    state: &AppState,
    job: &TranscodeJob,
    progress: &watch::Sender<JobProgress>,
    tool_missing: bool,
    url: Option<&str>,
) {
    progress.send_modify(|progress| {
        progress.growth = Growth::Failed;
        progress.tool_missing = tool_missing;
    });
    let _ = tokio::fs::remove_file(&job.temp_path).await;
    // A source ffmpeg cannot read may be broken; let the sync check it.
    if let Some(url) = url {
        state.reverify.enqueue(url);
    }
}

/// ffmpeg arguments for `wanted` from `input` (a `file:` URL) to stdout.
fn ffmpeg_args(input: &std::ffi::OsStr, wanted: &TranscodeRequest) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = [
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-protocol_whitelist",
        "file",
    ]
    .map(Into::into)
    .into();
    if let Some(start_ms) = wanted.start_ms {
        // Before `-i`: fast input seeking.
        args.push("-ss".into());
        args.push(format!("{}.{:03}", start_ms / 1000, start_ms % 1000).into());
    }
    args.push("-i".into());
    args.push(input.to_owned());
    args.extend(["-map", "0:a:0", "-vn", "-sn", "-dn", "-map_metadata", "0"].map(Into::into));
    args.extend(wanted.format.ffmpeg_args().map(Into::into));
    args.push("-b:a".into());
    args.push(format!("{}k", wanted.kbps).into());
    args.push("pipe:1".into());
    args
}

async fn run_ffmpeg(
    source: &FsPath,
    wanted: &TranscodeRequest,
    mut output: tokio::fs::File,
    progress: &watch::Sender<JobProgress>,
    job: &TranscodeJob,
) -> Result<u64, TranscodeError> {
    let grace = wanted.abandoned_grace();
    let mut command = Command::new(Tool::Ffmpeg.binary());
    lower_priority(&mut command);
    command
        .args(ffmpeg_args(file_input(source).as_ref(), wanted))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            TranscodeError::ToolMissing
        } else {
            TranscodeError::Failed(format!("could not start ffmpeg: {error}"))
        }
    })?;
    let mut stdout = child.stdout.take().expect("ffmpeg stdout is piped");
    let stderr = child.stderr.take().expect("ffmpeg stderr is piped");
    let stderr = tokio::spawn(stderr_tail(stderr, STDERR_TAIL_LINES));

    let copy = async {
        let mut buffer = vec![0u8; 64 * 1024];
        let mut written = 0u64;
        let mut check = tokio::time::interval(Duration::from_secs(1));
        let mut unread_since: Option<Instant> = None;
        loop {
            let read = tokio::select! {
                read = stdout.read(&mut buffer) => read?,
                _ = check.tick() => {
                    // The job's own receiver does not count as a reader.
                    if progress.receiver_count() > 1 || job.polled_within(grace) {
                        unread_since = None;
                    } else if unread_since.get_or_insert_with(Instant::now).elapsed() >= grace {
                        return Ok(None);
                    }
                    continue;
                }
            };
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read]).await?;
            // Readers follow `written`, so the bytes must be in the file first.
            output.flush().await?;
            written += read as u64;
            progress.send_modify(|progress| progress.growth = Growth::Writing { written });
        }
        output.sync_all().await?;
        Ok::<Option<u64>, io::Error>(Some(written))
    };
    let written = match tokio::time::timeout(TRANSCODE_TIME_LIMIT, copy).await {
        Ok(Ok(Some(written))) => written,
        Ok(Ok(None)) => {
            let _ = child.kill().await;
            return Err(TranscodeError::Abandoned);
        }
        Ok(Err(error)) => {
            let _ = child.kill().await;
            return Err(TranscodeError::Failed(format!(
                "writing the transcode failed: {error}"
            )));
        }
        Err(_) => {
            let _ = child.kill().await;
            return Err(TranscodeError::Failed(
                "the transcode timed out".to_string(),
            ));
        }
    };
    let status = child
        .wait()
        .await
        .map_err(|error| TranscodeError::Failed(format!("waiting for ffmpeg failed: {error}")))?;
    if !status.success() {
        let tail = stderr.await.unwrap_or_default();
        return Err(TranscodeError::Failed(format!(
            "ffmpeg exited with {status}: {tail}"
        )));
    }
    Ok(written)
}

/// Deletes the least recently used cached transcodes until the cache fits in
/// `max_bytes` (0 keeps none). Returns how many files and bytes were removed.
pub async fn enforce_transcode_budget(dir: PathBuf, max_bytes: u64) -> io::Result<(usize, u64)> {
    tokio::task::spawn_blocking(move || {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((0, 0)),
            Err(error) => return Err(error),
        };
        let mut files = Vec::new();
        let mut total = 0u64;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let extension = path.extension().and_then(|value| value.to_str());
            let formats = [TranscodeFormat::Opus, TranscodeFormat::Aac];
            if !formats
                .iter()
                .any(|format| extension == Some(format.extension()))
            {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let used = metadata.accessed().unwrap_or(modified).max(modified);
            total += metadata.len();
            files.push((used, metadata.len(), path));
        }
        if total <= max_bytes {
            return Ok((0, 0));
        }
        files.sort_by_key(|(used, _, _)| *used);
        let (mut removed, mut freed) = (0usize, 0u64);
        for (_, size, path) in files {
            if total <= max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= size;
                removed += 1;
                freed += size;
            }
        }
        Ok((removed, freed))
    })
    .await
    .map_err(io::Error::other)?
}

// ---------------------------------------------------------------------------
// Covers
// ---------------------------------------------------------------------------

fn cover_content_type(extension: &str) -> &'static str {
    match extension {
        "avif" => "image/avif",
        "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}

async fn read_cover(path: &FsPath) -> Result<Option<(Vec<u8>, Option<SystemTime>)>, ApiError> {
    let unreadable = |error: io::Error| {
        error!(path = %path.display(), %error, "could not read a cover");
        ApiError::internal("Could not load cover")
    };
    let modified = match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_file() && metadata.len() > 0 => metadata.modified().ok(),
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(error)),
    };
    match tokio::fs::read(path).await {
        Ok(bytes) if !bytes.is_empty() => Ok(Some((bytes, modified))),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(unreadable(error)),
    }
}

/// `GET /eras/{id}/cover[?v=<coverVersion>][&format=jpeg]`: the era's cover
/// (AVIF, or the original image when it was stored without ffmpeg), or its
/// JPEG variant. Immutable caching only for the current version.
pub async fn get_era_cover(
    State(state): State<SharedState>,
    EraId(era_id): EraId,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let query = Query::parse(query.as_deref());
    // A version that doesn't decode is simply not the current one.
    let requested_version = query.value("v").ok().flatten();
    let jpeg = match query.value("format") {
        Ok(None) => false,
        Ok(Some("jpeg" | "jpg")) => true,
        _ => return Err(ApiError::bad_request("Invalid cover format")),
    };
    let not_found = || ApiError::not_found("Cover not found");

    let covers = covers_dir(&state.config);
    let Some((primary_path, primary_extension)) = primary_cover(&covers, era_id).await else {
        return Err(not_found());
    };
    let Some((primary_bytes, primary_modified)) = read_cover(&primary_path).await? else {
        return Err(not_found());
    };
    // The version is the hash of the primary cover's bytes. The JPEG variant
    // is renamed into place before the primary, so a URL carrying a new
    // version never gets the previous JPEG.
    let version = cover_version_of(&primary_bytes);
    let (bytes, modified, extension) = if jpeg && primary_extension != "jpg" {
        let Some((bytes, modified)) = read_cover(&covers.join(format!("{era_id}.jpg"))).await?
        else {
            return Err(not_found());
        };
        (bytes, modified, "jpg")
    } else {
        (primary_bytes, primary_modified, primary_extension)
    };
    if jpeg && extension != "jpg" {
        return Err(not_found());
    }

    let etag = format!("\"{}\"", cover_version_of(&bytes));
    let cache_control = if requested_version == Some(version.as_str()) {
        COVER_IMMUTABLE
    } else {
        COVER_REVALIDATE
    };
    let mut response = if serve::is_not_modified(&headers, &etag, modified) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        response
    } else {
        let length = bytes.len();
        let mut response = Response::new(Body::from(bytes));
        set_header(
            &mut response,
            header::CONTENT_TYPE,
            cover_content_type(extension),
        );
        set_header(&mut response, header::CONTENT_LENGTH, &length.to_string());
        response
    };
    set_header(&mut response, header::ETAG, &etag);
    set_header(&mut response, header::CACHE_CONTROL, cache_control);
    if let Some(modified) = modified {
        set_header(
            &mut response,
            header::LAST_MODIFIED,
            &httpdate::fmt_http_date(modified),
        );
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One ADTS frame: `len` bytes in all (7-byte header, no CRC), sample-rate
    /// index `rate`, `blocks` raw data blocks of 1024 samples each.
    fn adts_frame(len: usize, rate: u8, blocks: u8) -> Vec<u8> {
        let mut frame = vec![0u8; len];
        frame[0] = 0xFF;
        frame[1] = 0xF1;
        frame[2] = (1 << 6) | (rate << 2);
        frame[3] = ((len >> 11) & 0x03) as u8;
        frame[4] = ((len >> 3) & 0xFF) as u8;
        frame[5] = (((len & 0x07) << 5) as u8) | 0x1F;
        frame[6] = 0xFC | (blocks - 1);
        frame
    }

    fn frame_at(stream: &[u8], start_ms: u64) -> Option<u64> {
        let mut reader = io::BufReader::with_capacity(16, io::Cursor::new(stream.to_vec()));
        adts_frame_at(&mut reader, stream.len() as u64, start_ms).unwrap()
    }

    #[test]
    fn adts_seeks_land_on_the_frame_holding_the_start() {
        // 48 kHz (index 3): 1024 samples = 21.33 ms per frame; frames of
        // alternating sizes, so offsets are not a multiple of one size.
        let stream: Vec<u8> = (0..100)
            .flat_map(|frame| adts_frame(if frame % 2 == 0 { 300 } else { 340 }, 3, 1))
            .collect();
        assert_eq!(frame_at(&stream, 1), Some(0));
        assert_eq!(frame_at(&stream, 21), Some(0));
        // Sample 1024 is the first of frame 1.
        assert_eq!(frame_at(&stream, 22), Some(300));
        // 1 s = sample 48000, in frame 46 (46 × 1024 = 47104).
        assert_eq!(frame_at(&stream, 1000), Some(23 * 300 + 23 * 340));
        // Past the end: nothing to serve from.
        assert_eq!(frame_at(&stream, 2200), None);

        // Two raw data blocks per frame: 2048 samples each.
        let doubled: Vec<u8> = (0..10).flat_map(|_| adts_frame(500, 3, 2)).collect();
        assert_eq!(frame_at(&doubled, 50), Some(500));
    }

    #[test]
    fn adts_seeks_refuse_streams_they_cannot_walk() {
        let mut tagged = b"ID3".to_vec();
        tagged.extend((0..10).flat_map(|_| adts_frame(300, 3, 1)));
        assert_eq!(frame_at(&tagged, 30), None);

        let mut changing: Vec<u8> = (0..5).flat_map(|_| adts_frame(300, 3, 1)).collect();
        changing.extend(adts_frame(300, 4, 1));
        assert_eq!(frame_at(&changing, 10), Some(0));
        assert_eq!(frame_at(&changing, 200), None);

        // A frame running past the end of the file.
        let mut cut: Vec<u8> = (0..3).flat_map(|_| adts_frame(300, 3, 1)).collect();
        cut.truncate(800);
        assert_eq!(frame_at(&cut, 50), None);
        // A length shorter than the header.
        let mut short = adts_frame(300, 3, 1);
        short[3..6].copy_from_slice(&[0, 0, (5 << 5) | 0x1F]);
        assert_eq!(frame_at(&short, 0), None);
        assert_eq!(frame_at(&[], 10), None);
    }

    #[test]
    fn seek_jobs_are_abandoned_sooner() {
        let whole = TranscodeRequest {
            kbps: 128,
            format: TranscodeFormat::Aac,
            start_ms: None,
        };
        let seek = TranscodeRequest {
            start_ms: Some(1000),
            ..whole
        };
        assert_eq!(whole.abandoned_grace(), ABANDONED_GRACE);
        assert_eq!(seek.abandoned_grace(), SEEK_ABANDONED_GRACE);
    }

    #[test]
    fn validates_transcode_quality() {
        assert_eq!(transcode_quality(None).unwrap(), None);
        assert_eq!(transcode_quality(Some("quality=")).unwrap(), None);
        assert_eq!(transcode_quality(Some("quality=%20")).unwrap(), None);
        assert_eq!(transcode_quality(Some("x=1")).unwrap(), None);
        assert_eq!(transcode_quality(Some("quality=64")).unwrap(), Some(64));
        assert_eq!(transcode_quality(Some("quality=+64+")).unwrap(), Some(64));
        assert_eq!(transcode_quality(Some("quality=320")).unwrap(), Some(320));
        assert_eq!(
            transcode_quality(Some("a=b&quality=128")).unwrap(),
            Some(128)
        );
        // The last occurrence wins, as on the JSON routes.
        assert_eq!(
            transcode_quality(Some("quality=bogus&quality=192")).unwrap(),
            Some(192)
        );
        assert!(transcode_quality(Some("quality=192&quality=bogus")).is_err());
        assert!(transcode_quality(Some("quality=%FF")).is_err());
        for invalid in [
            "8",
            "137",
            "7",
            "321",
            "0",
            "abc",
            "12.5",
            "-8",
            "0128",
            // Leading zeros are refused like in ids, also where the rest
            // would be a valid bitrate.
            "064",
            "00",
            "099999999999",
            "99999999999",
        ] {
            assert!(
                transcode_quality(Some(&format!("quality={invalid}"))).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn download_names_are_safe_and_short() {
        assert_eq!(
            download_filename(Some("NEBRASKA [V4]\n(feat. Pusha T)"), 1, ".mp3"),
            "NEBRASKA [V4].mp3"
        );
        assert_eq!(
            download_filename(Some("a/b\\c:d*e?f\"g<h>i|j"), 1, ".wav"),
            "a b c d e f g h i j.wav"
        );
        assert_eq!(
            download_filename(Some("  ...  "), 42, ".mp3"),
            "song-42.mp3"
        );
        assert_eq!(download_filename(None, 42, ""), "song-42");
        assert_eq!(download_filename(Some("CON"), 1, ".mp3"), "CON_.mp3");
        assert_eq!(
            download_filename(Some("evil\u{202E}3pm.exe"), 1, ".mp3"),
            "evil 3pm.exe.mp3"
        );
        assert_eq!(download_filename(Some("tab\there"), 1, ""), "tab here");

        let long = "word ".repeat(40);
        let name = download_filename(Some(&long), 1, ".mp3");
        let base = name.strip_suffix(".mp3").unwrap();
        assert!(base.chars().count() <= DOWNLOAD_NAME_MAX_CHARS, "{name}");
        assert!(base.ends_with("word"), "cut at a word boundary: {name}");
        let unbroken = "x".repeat(300);
        let name = download_filename(Some(&unbroken), 1, "");
        assert_eq!(name.chars().count(), DOWNLOAD_NAME_MAX_CHARS);
        let accented = "é".repeat(200);
        assert_eq!(
            download_filename(Some(&accented), 1, "").chars().count(),
            DOWNLOAD_NAME_MAX_CHARS
        );
    }

    #[test]
    fn content_disposition_has_ascii_and_utf8_names() {
        assert_eq!(
            content_disposition("Beyoncé & JAŸ-Z (50%).mp3"),
            "attachment; filename=\"Beyonce & JAY-Z (50_).mp3\"; \
             filename*=UTF-8''Beyonc%C3%A9%20&%20JA%C5%B8-Z%20%2850%25%29.mp3"
        );
        assert_eq!(
            content_disposition("中文.wav"),
            "attachment; filename=\"__.wav\"; filename*=UTF-8''%E4%B8%AD%E6%96%87.wav"
        );
    }

    fn opus(kbps: u16) -> TranscodeRequest {
        TranscodeRequest {
            kbps,
            format: TranscodeFormat::Opus,
            start_ms: None,
        }
    }

    #[test]
    fn transcode_keys_identify_file_bitrate_format_and_start() {
        let key = transcode_key("abc.wav", 255, 4096, &opus(128));
        assert!(key.ends_with("-ff-1000-128"), "{key}");
        assert_ne!(key, transcode_key("abd.wav", 255, 4096, &opus(128)));
        assert_ne!(key, transcode_key("abc.wav", 255, 4097, &opus(128)));
        assert_ne!(key, transcode_key("abc.wav", 255, 4096, &opus(96)));
        let aac = TranscodeRequest {
            format: TranscodeFormat::Aac,
            ..opus(128)
        };
        assert_eq!(
            transcode_key("abc.wav", 255, 4096, &aac),
            format!("{key}-aac")
        );
        let seeked = TranscodeRequest {
            start_ms: Some(93_500),
            ..aac
        };
        assert_eq!(
            transcode_key("abc.wav", 255, 4096, &seeked),
            format!("{key}-aac-s93500")
        );
        assert!(!seeked.cacheable());
    }

    #[test]
    fn transcode_requests_add_format_and_start_to_a_quality() {
        let parse = |query: &str| transcode_request(Some(query));
        assert_eq!(
            parse("format=aac&start=5").unwrap(),
            None,
            "no quality: the original"
        );
        assert_eq!(parse("quality=128").unwrap(), Some(opus(128)));
        assert_eq!(
            parse("quality=128&format=opus&start=0").unwrap(),
            Some(opus(128))
        );
        assert_eq!(
            parse("quality=128&format=%20&start=").unwrap(),
            Some(opus(128))
        );
        assert_eq!(
            parse("quality=64&format=aac&start=93.5").unwrap(),
            Some(TranscodeRequest {
                kbps: 64,
                format: TranscodeFormat::Aac,
                start_ms: Some(93_500),
            })
        );
        assert_eq!(
            parse("quality=64&start=86400").unwrap().unwrap().start_ms,
            Some(86_400_000)
        );
        for bogus in ["AAC", "mp3", "ogg"] {
            assert!(
                parse(&format!("quality=64&format={bogus}")).is_err(),
                "{bogus}"
            );
        }
        for bogus in [
            "-1", "1e3", ".5", "5.", "1.2.3", "abc", "86400.5", "NaN", "inf", "1.2345", "123456",
        ] {
            assert!(
                parse(&format!("quality=64&start={bogus}")).is_err(),
                "{bogus}"
            );
        }
    }

    #[test]
    fn ffmpeg_args_keep_the_opus_default_and_add_aac_and_seeking() {
        let render = |wanted: &TranscodeRequest| {
            ffmpeg_args("file:/songs/a.flac".as_ref(), wanted)
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let prefix = "-nostdin -hide_banner -loglevel error -protocol_whitelist file";
        let map = "-map 0:a:0 -vn -sn -dn -map_metadata 0";
        assert_eq!(
            render(&opus(128)),
            format!("{prefix} -i file:/songs/a.flac {map} -c:a libopus -f ogg -b:a 128k pipe:1")
        );
        let seeked = TranscodeRequest {
            kbps: 64,
            format: TranscodeFormat::Aac,
            start_ms: Some(93_050),
        };
        assert_eq!(
            render(&seeked),
            format!(
                "{prefix} -ss 93.050 -i file:/songs/a.flac {map} -c:a aac -f adts -b:a 64k pipe:1"
            )
        );
    }

    /// A request that finds a song's file gone answers 404 and, from then on,
    /// the song is no longer advertised as playable: not in its payload, not
    /// by `playable=true`, not in `/status` — without waiting for a sync and
    /// without touching the database.
    #[tokio::test]
    async fn a_missing_file_stops_being_advertised_at_once() {
        use tower::ServiceExt;

        let root = std::env::temp_dir().join(format!("yt-media-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let songs = root.join("songs");
        std::fs::create_dir_all(&songs).unwrap();
        let config = Config::for_tests(&root, &songs);
        let pool = crate::db::create_pool(&root.join("db.sqlite3")).unwrap();
        crate::db::run_migrations(&pool, &config).unwrap();
        let url = "https://pillows.su/f/0123456789abcdef";
        {
            let conn = pool.get().unwrap();
            conn.execute_batch(&format!(
                "INSERT INTO eras (id, key, name, is_main, position) VALUES (1, 'era', 'Era', 1, 1);
                 INSERT INTO songs (id, era, catalog_id, name, title, url, position, era_position,
                   search_text, category_rank)
                   VALUES (1, 1, 'unreleased', 'Song', 'Song', '{url}', 1, 1, 'song era', 4);
                 INSERT INTO files (url, filename, status, downloaded)
                   VALUES ('{url}', 'song.mp3', 'downloaded', 1);"
            ))
            .unwrap();
        }
        std::fs::write(songs.join("song.mp3"), b"ID3 audio").unwrap();
        let state = crate::state::AppState::new(config, pool);
        state.playable.refresh(&songs).await.unwrap();
        let app = crate::http::app(state.clone());
        let get = |uri: &str| {
            let request = axum::http::Request::builder()
                .uri(uri)
                .body(Body::empty())
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let body = axum::body::to_bytes(response.into_body(), 1 << 20)
                    .await
                    .unwrap();
                (status, serde_json::from_slice(&body).unwrap_or_default())
            }
        };
        let (status, song): (_, serde_json::Value) = get("/songs/1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(song["playable"], true);
        assert_eq!(get("/status").await.1["playableSongs"], 1);

        std::fs::remove_file(songs.join("song.mp3")).unwrap();
        // The set built by the last scan still lists the file...
        assert_eq!(get("/songs/1").await.1["playable"], true);
        // ...until a request finds it gone.
        assert_eq!(get("/songs/1/stream").await.0, StatusCode::NOT_FOUND);
        let song = get("/songs/1").await.1;
        assert_eq!(song["playable"], false);
        assert_eq!(song["downloadState"], "pending");
        assert_eq!(get("/songs?playable=true").await.1["total"], 0);
        assert_eq!(get("/status").await.1["playableSongs"], 0);
        assert_eq!(state.reverify.len(), 1, "queued for the sync to re-check");
        let conn = state.pool.get().unwrap();
        let (filename, file_status): (Option<String>, String) = conn
            .query_row("SELECT filename, status FROM files", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(
            filename.as_deref(),
            Some("song.mp3"),
            "the row is untouched"
        );
        assert_eq!(file_status, "downloaded");
        drop(conn);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn content_types_by_extension() {
        assert_eq!(content_type_for("a.MP3"), "audio/mpeg");
        assert_eq!(content_type_for("a.ogg"), "audio/ogg");
        assert_eq!(content_type_for("a.opus"), "audio/ogg; codecs=opus");
        assert_eq!(content_type_for("a.bin"), "application/octet-stream");
        assert_eq!(content_type_for("noext"), "application/octet-stream");
    }
}
