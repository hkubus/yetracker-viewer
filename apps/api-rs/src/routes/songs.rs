//! `/songs` routes plus the paginated era song list.
//!
//! Mirrors `apps/api/src/routes/songs/**`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::stream;
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use serde_json::{Value as JsonValue, json};
use tokio::io::AsyncReadExt;
use tokio::sync::OwnedSemaphorePermit;

use super::{DURATION_CACHE, JSON_CACHE, MEDIA_CACHE, js_float, json_cached, set_header};
use crate::db;
use crate::error::ApiError;
use crate::media::{
    InvalidReason, ProbeError, delete_invalid_file, get_duration, probe_audio_file,
};
use crate::playable::{mtime_ms_of, stored_song_path};
use crate::rank::{Searchable, rank_song_search};
use crate::request::{
    escape_like_pattern, normalize_query, pagination_value, positive_integer, sort_value,
};
use crate::serve::{file_body, parse_range};
use crate::state::{AppState, SharedState};
use crate::text;

const QUALITY_FILTERS: [&str; 6] = [
    "Low Quality",
    "High Quality",
    "CD Quality",
    "Lossless",
    "Not Available",
    "Recording",
];
const AVAILABILITY_FILTERS: [&str; 10] = [
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

/// Emoji category markers that prefix `unreleased` song names. `id` is the
/// public filter value; `emoji` is matched with `instr` (use the base codepoint
/// so variation-selector forms also match).
const SONG_CATEGORIES: [(&str, &str); 6] = [
    ("best-of", "⭐"),
    ("special", "✨"),
    ("grails", "🏆"),
    ("wanted", "🏅"),
    ("worst-of", "🗑"),
    ("ai", "🤖"),
];

fn category_emoji(id: &str) -> Option<&'static str> {
    SONG_CATEGORIES.iter().find(|(key, _)| *key == id).map(|(_, emoji)| *emoji)
}

/// `ORDER BY` fragment for a validated sort key (see `request::sort_value`).
///
/// Every fragment ends with `songs.id ASC` so equal keys keep a stable,
/// import-ordered tie-break, and the date sorts push rows with a missing or
/// zero date to the end instead of pretending they leaked in 1970.
fn sort_order_by(sort: &str) -> &'static str {
    match sort {
        "leak-newest" => {
            "songs.leak_date IS NULL OR songs.leak_date = 0, songs.leak_date DESC, songs.id ASC"
        }
        "leak-oldest" => {
            "songs.leak_date IS NULL OR songs.leak_date = 0, songs.leak_date ASC, songs.id ASC"
        }
        "file-newest" => {
            "songs.file_date IS NULL OR songs.file_date = 0, songs.file_date DESC, songs.id ASC"
        }
        "name" => "songs.name COLLATE NOCASE ASC, songs.id ASC",
        _ => "songs.id ASC",
    }
}

fn enum_filter(
    value: Option<&str>,
    allowed: &[&str],
    label: &str,
) -> Result<Option<String>, ApiError> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if !allowed.contains(&value) {
        return Err(ApiError::bad_request(format!("Invalid {label} filter")));
    }
    Ok(Some(value.to_string()))
}

fn is_ascii_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

struct PlainSong {
    id: i64,
    era: Option<i64>,
    catalog_id: Option<String>,
    name: Option<String>,
    notes: Option<String>,
    file_date: Option<i64>,
    leak_date: Option<i64>,
    available_length: Option<String>,
    track_length: Option<i64>,
    quality: Option<String>,
    url: Option<String>,
}

fn plain_song_json(song: &PlainSong) -> JsonValue {
    json!({
        "id": song.id,
        "eraId": song.era,
        "catalogId": song.catalog_id,
        "name": song.name,
        "notes": song.notes,
        "fileDate": song.file_date,
        "leakDate": song.leak_date,
        "availableLength": song.available_length,
        "trackLength": song.track_length,
        "quality": song.quality,
        "url": song.url,
    })
}

#[derive(Clone)]
pub struct SearchSong {
    pub id: i64,
    pub era_id: Option<i64>,
    pub name: Option<String>,
    pub notes: Option<String>,
    pub quality: Option<String>,
    pub available_length: Option<String>,
    pub era_name: Option<String>,
    pub dominant_color: Option<String>,
    pub era_position: Option<i64>,
    pub leak_date: Option<i64>,
    pub filename: Option<String>,
    pub playable: bool,
}

impl Searchable for SearchSong {
    fn id(&self) -> i64 {
        self.id
    }
    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    fn notes(&self) -> Option<&str> {
        self.notes.as_deref()
    }
    fn quality(&self) -> Option<&str> {
        self.quality.as_deref()
    }
    fn available_length(&self) -> Option<&str> {
        self.available_length.as_deref()
    }
    fn era_name(&self) -> Option<&str> {
        self.era_name.as_deref()
    }
    fn playable(&self) -> bool {
        self.playable
    }
}

impl SearchSong {
    fn to_json(&self) -> JsonValue {
        json!({
            "id": self.id,
            "eraId": self.era_id,
            "name": self.name,
            "notes": self.notes,
            "quality": self.quality,
            "availableLength": self.available_length,
            "eraName": self.era_name,
            "dominantColor": self.dominant_color,
            "eraPosition": self.era_position.unwrap_or(1),
            "leakDate": self.leak_date,
            "playable": self.playable,
        })
    }
}

pub async fn list_songs(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let query = params.get("q").map(|value| normalize_query(value));
    if let Some(query) = &query {
        if !query.is_empty() && text::utf16_len(query) > 100 {
            return Err(ApiError::bad_request("Search query is too long"));
        }
    }

    let era_id = match params.get("era").filter(|value| !value.is_empty()) {
        Some(value) => Some(positive_integer(Some(value), "era filter")?),
        None => None,
    };
    let era_from_id = match params.get("eraFrom").filter(|value| !value.is_empty()) {
        Some(value) => Some(positive_integer(Some(value), "starting era filter")?),
        None => None,
    };
    let era_to_id = match params.get("eraTo").filter(|value| !value.is_empty()) {
        Some(value) => Some(positive_integer(Some(value), "ending era filter")?),
        None => None,
    };
    if let (Some(from), Some(to)) = (era_from_id, era_to_id) {
        if from > to {
            return Err(ApiError::bad_request(
                "Starting era must not be after ending era",
            ));
        }
    }

    let quality_filter = enum_filter(
        params.get("quality").map(String::as_str),
        &QUALITY_FILTERS,
        "quality",
    )?;
    let availability_filter = enum_filter(
        params.get("availability").map(String::as_str),
        &AVAILABILITY_FILTERS,
        "availability",
    )?;
    let playable_raw = params
        .get("playable")
        .map(String::as_str)
        .filter(|value| !value.is_empty());
    if let Some(value) = playable_raw {
        if value != "true" && value != "false" {
            return Err(ApiError::bad_request("Invalid playable filter"));
        }
    }
    let playable_filter = playable_raw.map(|value| value == "true");

    let has_filters = era_id.is_some()
        || era_from_id.is_some()
        || era_to_id.is_some()
        || quality_filter.is_some()
        || availability_filter.is_some()
        || playable_filter.is_some();

    let is_search = query
        .as_deref()
        .map(|value| !value.is_empty())
        .unwrap_or(false)
        || has_filters;
    if is_search {
        return search_songs(
            &state,
            query.as_deref().unwrap_or(""),
            era_id,
            era_from_id,
            era_to_id,
            quality_filter,
            availability_filter,
            playable_filter,
            &params,
        )
        .await;
    }

    let requested_limit =
        pagination_value(params.get("limit").map(String::as_str), 100, 500, "limit")?;
    let requested_offset = pagination_value(
        params.get("offset").map(String::as_str),
        0,
        10_000,
        "offset",
    )?;
    let order_by = sort_order_by(sort_value(params.get("sort").map(String::as_str))?);

    let songs = db::call(&state.pool, move |conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT id, era, catalog_id, name, notes, file_date, leak_date, available_length, track_length, quality, url \
             FROM songs WHERE catalog_id = 'unreleased' ORDER BY {order_by} LIMIT ?1 OFFSET ?2",
        ))?;
        let rows = statement.query_map([requested_limit, requested_offset], |row| {
            Ok(PlainSong {
                id: row.get(0)?,
                era: row.get(1)?,
                catalog_id: row.get(2)?,
                name: row.get(3)?,
                notes: row.get(4)?,
                file_date: row.get(5)?,
                leak_date: row.get(6)?,
                available_length: row.get(7)?,
                track_length: row.get(8)?,
                quality: row.get(9)?,
                url: row.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await?;

    let body: Vec<JsonValue> = songs.iter().map(plain_song_json).collect();
    let mut response = json_cached(JsonValue::Array(body));
    set_header(&mut response, header::CACHE_CONTROL, JSON_CACHE);
    Ok(response)
}

#[allow(clippy::too_many_arguments)]
async fn search_songs(
    state: &AppState,
    query: &str,
    era_id: Option<i64>,
    era_from_id: Option<i64>,
    era_to_id: Option<i64>,
    quality_filter: Option<String>,
    availability_filter: Option<String>,
    playable_filter: Option<bool>,
    params: &HashMap<String, String>,
) -> Result<Response, ApiError> {
    let requested_limit =
        pagination_value(params.get("limit").map(String::as_str), 50, 50, "limit")?;
    let order_by = sort_order_by(sort_value(params.get("sort").map(String::as_str))?);

    // `eraPosition` must be the song's index inside its *unfiltered* era: the web
    // client turns it into a page number for `/eras/{id}?page=N#song-{id}`. A
    // window function in the outer query numbers only the filtered rows, which
    // sent deep links to the wrong page whenever a query or filter was set.
    let mut sql = String::from(
        "WITH era_positions AS (SELECT id, row_number() OVER (PARTITION BY era ORDER BY id) AS position \
         FROM songs WHERE catalog_id = 'unreleased') \
         SELECT songs.id, songs.era, songs.name, songs.notes, songs.quality, songs.available_length, \
         eras.name, eras.dominant_color, files.filename, era_positions.position, songs.leak_date \
         FROM songs LEFT JOIN eras ON songs.era = eras.id LEFT JOIN files ON songs.url = files.url \
         LEFT JOIN era_positions ON era_positions.id = songs.id \
         WHERE songs.catalog_id = 'unreleased'",
    );
    let mut values: Vec<Value> = Vec::new();
    if !query.is_empty() {
        sql.push_str(
            " AND instr(lower(replace(replace(coalesce(songs.name,'') || ' ' || coalesce(songs.notes,'') || ' ' || \
             coalesce(eras.name,'') || ' ' || coalesce(songs.quality,'') || ' ' || coalesce(songs.available_length,''), \
             char(13), ' '), char(10), ' ')), ?) > 0",
        );
        values.push(Value::Text(query.to_string()));
    }
    if let Some(era_id) = era_id {
        sql.push_str(" AND songs.era = ?");
        values.push(Value::Integer(era_id));
    }
    if let Some(era_from_id) = era_from_id {
        sql.push_str(" AND songs.era >= ?");
        values.push(Value::Integer(era_from_id));
    }
    if let Some(era_to_id) = era_to_id {
        sql.push_str(" AND songs.era <= ?");
        values.push(Value::Integer(era_to_id));
    }
    if let Some(quality) = &quality_filter {
        sql.push_str(" AND songs.quality = ?");
        values.push(Value::Text(quality.clone()));
    }
    if let Some(availability) = &availability_filter {
        sql.push_str(" AND songs.available_length = ?");
        values.push(Value::Text(availability.clone()));
    }
    // Shapes the candidate window; the relevance ranking below still decides the
    // final order whenever a query is present.
    sql.push_str(" ORDER BY ");
    sql.push_str(order_by);
    sql.push_str(" LIMIT 1000");

    let rows = db::call(&state.pool, move |conn| {
        let mut statement = conn.prepare(&sql)?;
        let mapped = statement.query_map(params_from_iter(values), |row| {
            let filename: Option<String> = row.get(8)?;
            Ok(SearchSong {
                id: row.get(0)?,
                era_id: row.get(1)?,
                name: row.get(2)?,
                notes: row.get(3)?,
                quality: row.get(4)?,
                available_length: row.get(5)?,
                era_name: row.get(6)?,
                dominant_color: row.get(7)?,
                era_position: row.get(9)?,
                leak_date: row.get(10)?,
                playable: false,
                filename,
            })
        })?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(ApiError::from)
    })
    .await?;

    let mut matches: Vec<SearchSong> = rows
        .into_iter()
        .map(|mut song| {
            song.playable = state.playable.is_playable(song.filename.as_deref());
            song
        })
        .filter(|song| {
            playable_filter
                .map(|wanted| song.playable == wanted)
                .unwrap_or(true)
        })
        .collect();

    let total = matches.len();
    let ranked = if query.is_empty() {
        matches.truncate(requested_limit as usize);
        matches
    } else {
        rank_song_search(matches, query, requested_limit as usize, &state.rank_cache)
    };

    let songs: Vec<JsonValue> = ranked.iter().map(SearchSong::to_json).collect();
    let mut response = json_cached(json!({ "songs": songs, "total": total }));
    set_header(&mut response, header::CACHE_CONTROL, JSON_CACHE);
    Ok(response)
}

pub async fn get_song(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = positive_integer(Some(&id), "song id")?;

    let song = db::call(&state.pool, move |conn| {
        let mut statement = conn.prepare(
            "SELECT id, era, catalog_id, name, notes, file_date, leak_date, available_length, track_length, quality, url \
             FROM songs WHERE id = ?1 LIMIT 1",
        )?;
        let mut rows = statement.query_map([id], |row| {
            Ok(PlainSong {
                id: row.get(0)?,
                era: row.get(1)?,
                catalog_id: row.get(2)?,
                name: row.get(3)?,
                notes: row.get(4)?,
                file_date: row.get(5)?,
                leak_date: row.get(6)?,
                available_length: row.get(7)?,
                track_length: row.get(8)?,
                quality: row.get(9)?,
                url: row.get(10)?,
            })
        })?;
        match rows.next() {
            Some(row) => row.map(Some).map_err(ApiError::from),
            None => Ok(None),
        }
    })
    .await?;

    let Some(song) = song else {
        return Err(ApiError::not_found("Song not found"));
    };
    let mut response = json_cached(plain_song_json(&song));
    set_header(&mut response, header::CACHE_CONTROL, JSON_CACHE);
    Ok(response)
}

/// A song row for a paginated era listing.
struct ScopedSong {
    id: i64,
    era: Option<i64>,
    catalog_id: Option<String>,
    name: Option<String>,
    notes: Option<String>,
    file_date: Option<i64>,
    leak_date: Option<i64>,
    available_length: Option<String>,
    track_length: Option<i64>,
    quality: Option<String>,
    url: Option<String>,
    downloaded: Option<i64>,
    filename: Option<String>,
    file_duration: Option<f64>,
    total: i64,
}

pub async fn paginated_songs(
    state: &AppState,
    era_id: i64,
    params: &HashMap<String, String>,
) -> Result<Response, ApiError> {
    let normalized_query = params
        .get("q")
        .map(|value| text::collapse_whitespace(value))
        .unwrap_or_default();
    if !normalized_query.is_empty() && text::utf16_len(&normalized_query) > 100 {
        return Err(ApiError::bad_request("Search query is too long"));
    }
    let requested_limit =
        pagination_value(params.get("limit").map(String::as_str), 100, 500, "limit")?;
    let requested_offset = pagination_value(
        params.get("offset").map(String::as_str),
        0,
        10_000,
        "offset",
    )?;
    let order_by = sort_order_by(sort_value(params.get("sort").map(String::as_str))?);
    let category_filter = params.get("category").map(|value| value.trim()).filter(|value| !value.is_empty());
    let category_emoji_value = match category_filter {
        Some(id) => match category_emoji(id) {
            Some(emoji) => Some(emoji),
            None => return Err(ApiError::bad_request("Invalid category filter")),
        },
        None => None,
    };

    let mut sql = String::from(
        "SELECT songs.id, songs.era, songs.catalog_id, songs.name, songs.notes, songs.file_date, songs.leak_date, \
         songs.available_length, songs.track_length, songs.quality, songs.url, \
         files.downloaded, files.filename, files.duration, count(*) OVER() AS total \
         FROM songs LEFT JOIN files ON songs.url = files.url WHERE ",
    );
    let mut values: Vec<Value> = Vec::new();
    sql.push_str("songs.era = ? AND songs.catalog_id = 'unreleased'");
    values.push(Value::Integer(era_id));
    if !normalized_query.is_empty() {
        let pattern = format!(
            "%{}%",
            escape_like_pattern(&normalized_query.to_lowercase())
        );
        sql.push_str(
            " AND (lower(coalesce(songs.name,'')) LIKE ? ESCAPE '\\' \
             OR lower(coalesce(songs.notes,'')) LIKE ? ESCAPE '\\' \
             OR lower(coalesce(songs.quality,'')) LIKE ? ESCAPE '\\' \
             OR lower(coalesce(songs.available_length,'')) LIKE ? ESCAPE '\\')",
        );
        for _ in 0..4 {
            values.push(Value::Text(pattern.clone()));
        }
    }
    if let Some(emoji) = category_emoji_value {
        sql.push_str(" AND instr(coalesce(songs.name, ''), ?) > 0");
        values.push(Value::Text(emoji.to_string()));
    }
    sql.push_str(" ORDER BY ");
    sql.push_str(order_by);
    sql.push_str(" LIMIT ? OFFSET ?");
    values.push(Value::Integer(requested_limit));
    values.push(Value::Integer(requested_offset));

    let songs = db::call(&state.pool, move |conn| {
        let mut statement = conn.prepare(&sql)?;
        let mapped = statement.query_map(params_from_iter(values), |row| {
            Ok(ScopedSong {
                id: row.get(0)?,
                era: row.get(1)?,
                catalog_id: row.get(2)?,
                name: row.get(3)?,
                notes: row.get(4)?,
                file_date: row.get(5)?,
                leak_date: row.get(6)?,
                available_length: row.get(7)?,
                track_length: row.get(8)?,
                quality: row.get(9)?,
                url: row.get(10)?,
                downloaded: row.get(11)?,
                filename: row.get(12)?,
                file_duration: row.get(13)?,
                total: row.get(14)?,
            })
        })?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(ApiError::from)
    })
    .await?;

    let total = songs.first().map(|song| song.total).unwrap_or(0);
    let body: Vec<JsonValue> = songs
        .iter()
        .map(|song| {
            let playable = state.playable.is_playable(song.filename.as_deref());
            let duration = if playable {
                song.file_duration.map(js_float).unwrap_or(JsonValue::Null)
            } else {
                JsonValue::Null
            };
            json!({
                "id": song.id,
                "eraId": song.era,
                "catalogId": song.catalog_id,
                "name": song.name,
                "notes": song.notes,
                "fileDate": song.file_date,
                "leakDate": song.leak_date,
                "availableLength": song.available_length,
                "trackLength": song.track_length,
                "quality": song.quality,
                "url": song.url,
                "downloaded": song.downloaded,
                "playable": playable,
                "duration": duration,
            })
        })
        .collect();

    let mut response = json_cached(JsonValue::Array(body));
    set_header(&mut response, header::CACHE_CONTROL, JSON_CACHE);
    set_header(
        &mut response,
        header::HeaderName::from_static("x-total-count"),
        &total.to_string(),
    );
    Ok(response)
}

struct SongFile {
    name: Option<String>,
    filename: Option<String>,
    url: Option<String>,
    duration: Option<f64>,
}

async fn lookup_song_file(state: &AppState, song_id: i64) -> Result<Option<SongFile>, ApiError> {
    db::call(&state.pool, move |conn| {
        let mut statement = conn.prepare(
            "SELECT songs.name, files.filename, files.url, files.duration FROM songs \
             LEFT JOIN files ON songs.url = files.url WHERE songs.id = ?1 LIMIT 1",
        )?;
        let mut rows = statement.query_map([song_id], |row| {
            Ok(SongFile {
                name: row.get(0)?,
                filename: row.get(1)?,
                url: row.get(2)?,
                duration: row.get(3)?,
            })
        })?;
        match rows.next() {
            Some(row) => row.map(Some).map_err(ApiError::from),
            None => Ok(None),
        }
    })
    .await
}

/// Resolves size/mtime for `filename`, replicating the empty/missing cleanup.
async fn resolve_file_meta(
    state: &AppState,
    path: &std::path::Path,
    filename: &str,
    url: Option<&str>,
) -> Result<(u64, i64), ApiError> {
    if let Some(meta) = state.playable.get_meta(filename) {
        if meta.size == 0 {
            delete_invalid_file(state, filename, url, InvalidReason::Empty).await;
            return Err(ApiError::not_found("Song file not found"));
        }
        return Ok((meta.size, meta.mtime_ms));
    }

    match tokio::fs::metadata(path).await {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() == 0 {
                delete_invalid_file(state, filename, url, InvalidReason::Empty).await;
                return Err(ApiError::not_found("Song file not found"));
            }
            Ok((metadata.len(), mtime_ms_of(&metadata)))
        }
        Err(error) => {
            if error.kind() == std::io::ErrorKind::NotFound {
                delete_invalid_file(state, filename, url, InvalidReason::Missing).await;
            }
            Err(ApiError::not_found("Song file not found"))
        }
    }
}

fn file_etag(size: u64, mtime_ms: i64) -> String {
    format!("\"{size:x}-{mtime_ms:x}\"")
}

fn mime_for(filename: &str) -> &'static str {
    let extension = std::path::Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "mp3" => "audio/mpeg",
        "opus" => "audio/opus",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "aif" | "aiff" => "audio/aiff",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

pub async fn stream_song(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let quality = params
        .get("quality")
        .map(String::as_str)
        .filter(|value| !value.is_empty());
    let song_id = positive_integer(Some(&id), "song id")?;

    let song = lookup_song_file(&state, song_id).await?;
    let Some(song) = song else {
        return Err(ApiError::not_found("Song not found"));
    };
    let Some(filename) = song.filename.clone() else {
        return Err(ApiError::internal("Could not find file for song"));
    };

    let path = stored_song_path(&state.config.songs_path, &filename)?;
    if song.duration == Some(0.0) {
        delete_invalid_file(
            &state,
            &filename,
            song.url.as_deref(),
            InvalidReason::NoDuration,
        )
        .await;
        return Err(ApiError::not_found("Song file not found"));
    }

    let (file_size, mtime_ms) =
        resolve_file_meta(&state, &path, &filename, song.url.as_deref()).await?;
    let etag = file_etag(file_size, mtime_ms);
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(etag.as_str())
        && quality.is_none()
    {
        return Ok(super::empty(StatusCode::NOT_MODIFIED));
    }

    if let Some(quality) = quality {
        return stream_transcoded(state, path, quality, filename, song.url);
    }

    let mut response = super::empty(StatusCode::OK);
    set_header(&mut response, header::CONTENT_TYPE, mime_for(&filename));
    set_header(&mut response, header::ETAG, &etag);
    set_header(&mut response, header::CACHE_CONTROL, MEDIA_CACHE);
    set_header(&mut response, header::ACCEPT_RANGES, "bytes");

    if let Some(range_header) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        let if_range = headers
            .get(header::IF_RANGE)
            .and_then(|value| value.to_str().ok());
        if let Some(if_range) = if_range {
            if if_range != etag {
                return full_file_response(&path, file_size, response).await;
            }
        }
        return range_file_response(&path, file_size, range_header, response).await;
    }

    full_file_response(&path, file_size, response).await
}

async fn full_file_response(
    path: &std::path::Path,
    file_size: u64,
    mut response: Response,
) -> Result<Response, ApiError> {
    set_header(
        &mut response,
        header::CONTENT_LENGTH,
        &file_size.to_string(),
    );
    let body = file_body(path, None).await.map_err(|error| {
        eprintln!("failed to stream song {error}");
        ApiError::internal("Could not stream song")
    })?;
    *response.body_mut() = body;
    Ok(response)
}

async fn range_file_response(
    path: &std::path::Path,
    file_size: u64,
    range_header: &str,
    mut response: Response,
) -> Result<Response, ApiError> {
    let Some(range) = parse_range(range_header, file_size) else {
        set_header(
            &mut response,
            header::CONTENT_RANGE,
            &format!("bytes */{file_size}"),
        );
        *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
        // An unknown-length (chunked) empty body matches Hono, whose 416 has no
        // `Content-Length`; a known empty body would gain `content-length: 0`.
        *response.body_mut() =
            Body::from_stream(futures_util::stream::empty::<Result<Bytes, std::io::Error>>());
        return Ok(response);
    };

    set_header(
        &mut response,
        header::CONTENT_RANGE,
        &format!("bytes {}-{}/{file_size}", range.start, range.end),
    );
    set_header(
        &mut response,
        header::CONTENT_LENGTH,
        &(range.end - range.start + 1).to_string(),
    );
    *response.status_mut() = StatusCode::PARTIAL_CONTENT;
    let body = file_body(path, Some(range)).await.map_err(|error| {
        eprintln!("failed to stream song {error}");
        ApiError::internal("Could not stream song")
    })?;
    *response.body_mut() = body;
    Ok(response)
}

fn stream_transcoded(
    state: SharedState,
    path: PathBuf,
    quality: &str,
    filename: String,
    url: Option<String>,
) -> Result<Response, ApiError> {
    if !is_ascii_digits(quality) {
        return Err(ApiError::bad_request("Invalid quality for file"));
    }
    let parsed: i64 = quality
        .parse()
        .map_err(|_| ApiError::bad_request("Invalid quality for file"))?;
    if !(8..=320).contains(&parsed) {
        return Err(ApiError::bad_request("Invalid quality for file"));
    }

    let permit = match state.transcode_slots.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            let mut response = (
                StatusCode::SERVICE_UNAVAILABLE,
                "Transcoding capacity reached; try again shortly",
            )
                .into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("text/plain;charset=UTF-8"),
            );
            set_header(&mut response, header::RETRY_AFTER, "5");
            return Ok(response);
        }
    };

    let body = transcode_body(state, permit, path, format!("{parsed}k"), filename, url);

    let mut response = super::empty(StatusCode::OK);
    set_header(&mut response, header::CONTENT_TYPE, "audio/opus");
    set_header(&mut response, header::ACCEPT_RANGES, "none");
    set_header(&mut response, header::CACHE_CONTROL, "no-store");
    *response.body_mut() = body;
    Ok(response)
}

fn transcode_body(
    state: SharedState,
    permit: OwnedSemaphorePermit,
    path: PathBuf,
    quality: String,
    filename: String,
    url: Option<String>,
) -> Body {
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);

    tokio::spawn(async move {
        let _permit = permit;
        let mut command = tokio::process::Command::new("ffmpeg");
        command
            .arg("-i")
            .arg(&path)
            .args(["-map_metadata", "0", "-f", "ogg", "-c:a", "libopus", "-b:a"])
            .arg(&quality)
            .arg("pipe:1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                eprintln!("failed to transcode song {error}");
                let _ = sender
                    .send(Err(std::io::Error::other(error.to_string())))
                    .await;
                return;
            }
        };

        let mut stdout = child.stdout.take().expect("ffmpeg stdout is piped");
        let mut buffer = vec![0u8; 64 * 1024];
        let mut client_gone = false;
        let mut errored = false;
        loop {
            match stdout.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    if sender
                        .send(Ok(Bytes::copy_from_slice(&buffer[..read])))
                        .await
                        .is_err()
                    {
                        client_gone = true;
                        let _ = child.kill().await;
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                    errored = true;
                    break;
                }
            }
        }

        if let Ok(status) = child.wait().await {
            if !status.success() && !client_gone {
                // Surface a failed transcode as a broken body (Hono destroys the
                // converter output with the error) instead of a clean truncated 200.
                if !errored {
                    let _ = sender
                        .send(Err(std::io::Error::other("transcode failed")))
                        .await;
                }
                let probe = probe_audio_file(&state, &filename).await;
                if !probe.valid {
                    delete_invalid_file(
                        &state,
                        &filename,
                        url.as_deref(),
                        probe.reason.unwrap_or(InvalidReason::Unreadable),
                    )
                    .await;
                }
            }
        }
    });

    let body_stream = stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    });
    Body::from_stream(body_stream)
}

fn download_name(name: Option<&str>, id: i64, extension: &str) -> String {
    let fallback = format!("song-{id}");
    let source = name.unwrap_or(&fallback);

    let mut replaced = String::with_capacity(source.len());
    for character in source.chars() {
        if character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
        {
            replaced.push(' ');
        } else {
            replaced.push(character);
        }
    }
    let collapsed = text::collapse_whitespace(&replaced);
    let truncated: String = collapsed.chars().take(120).collect();
    let safe_name = if truncated.is_empty() {
        fallback
    } else {
        truncated
    };
    format!("{safe_name}{extension}")
}

fn encode_header_filename(filename: &str) -> String {
    let mut encoded = String::with_capacity(filename.len());
    for character in filename.chars() {
        let unreserved = character.is_ascii_alphanumeric()
            || matches!(
                character,
                '-' | '_' | '.' | '~' | '!' | '*' | '\'' | '(' | ')'
            );
        if unreserved {
            encoded.push(character);
        } else {
            let mut buffer = [0u8; 4];
            for byte in character.encode_utf8(&mut buffer).bytes() {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    encoded
        .replace('!', "%21")
        .replace('\'', "%27")
        .replace('(', "%28")
        .replace(')', "%29")
        .replace('*', "%2A")
}

pub async fn download_song(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let song_id = positive_integer(Some(&id), "song id")?;

    let song = lookup_song_file(&state, song_id).await?;
    let Some(song) = song else {
        return Err(ApiError::not_found("Song not found"));
    };
    let Some(filename) = song.filename.clone() else {
        return Err(ApiError::not_found("Song file not found"));
    };

    let path = stored_song_path(&state.config.songs_path, &filename)?;
    if song.duration == Some(0.0) {
        delete_invalid_file(
            &state,
            &filename,
            song.url.as_deref(),
            InvalidReason::NoDuration,
        )
        .await;
        return Err(ApiError::not_found("Song file not found"));
    }

    let (file_size, mtime_ms) =
        resolve_file_meta(&state, &path, &filename, song.url.as_deref()).await?;
    let etag = file_etag(file_size, mtime_ms);
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(etag.as_str())
    {
        return Ok(super::empty(StatusCode::NOT_MODIFIED));
    }

    let extension = std::path::Path::new(&filename)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    let name = download_name(song.name.as_deref(), song_id, &extension);

    let disposition = format!(
        "attachment; filename=\"song-{song_id}{extension}\"; filename*=UTF-8''{}",
        encode_header_filename(&name)
    );

    let mut response = super::empty(StatusCode::OK);
    set_header(
        &mut response,
        header::CONTENT_TYPE,
        "application/octet-stream",
    );
    set_header(&mut response, header::ETAG, &etag);
    set_header(&mut response, header::ACCEPT_RANGES, "bytes");
    set_header(&mut response, header::CACHE_CONTROL, MEDIA_CACHE);
    set_header(&mut response, header::CONTENT_DISPOSITION, &disposition);

    if let Some(range_header) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        let if_range = headers
            .get(header::IF_RANGE)
            .and_then(|value| value.to_str().ok());
        if if_range.is_none() || if_range == Some(etag.as_str()) {
            if let Some(range) = parse_range(range_header, file_size) {
                set_header(
                    &mut response,
                    header::CONTENT_RANGE,
                    &format!("bytes {}-{}/{file_size}", range.start, range.end),
                );
                set_header(
                    &mut response,
                    header::CONTENT_LENGTH,
                    &(range.end - range.start + 1).to_string(),
                );
                *response.status_mut() = StatusCode::PARTIAL_CONTENT;
                let body = file_body(&path, Some(range)).await.map_err(|error| {
                    eprintln!("failed to stream song {error}");
                    ApiError::internal("Could not stream song")
                })?;
                *response.body_mut() = body;
                return Ok(response);
            }
        }
    }

    full_file_response(&path, file_size, response).await
}

pub async fn get_song_duration(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let song_id = positive_integer(Some(&id), "song id")?;

    let song = lookup_song_file(&state, song_id).await?;
    let Some(song) = song else {
        return Err(ApiError::not_found("Song not found"));
    };
    let Some(filename) = song.filename.clone() else {
        return Err(ApiError::not_found("Could not find file for song"));
    };

    if let Some(duration) = song.duration {
        if duration == 0.0 {
            delete_invalid_file(
                &state,
                &filename,
                song.url.as_deref(),
                InvalidReason::NoDuration,
            )
            .await;
            return Err(ApiError::unprocessable("Could not determine file duration"));
        }
        let mut response = json_cached(json!({ "duration": js_float(duration) }));
        set_header(&mut response, header::CACHE_CONTROL, DURATION_CACHE);
        return Ok(response);
    }

    let path = stored_song_path(&state.config.songs_path, &filename)?;
    let mtime_ms = match tokio::fs::metadata(&path).await {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(ApiError::not_found("Song file not found"));
            }
            mtime_ms_of(&metadata)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::not_found("Song file not found"));
        }
        Err(error) => {
            eprintln!("failed to read song duration {error}");
            return Err(ApiError::internal("Could not determine file duration"));
        }
    };

    let duration = match get_duration(&state, &path, Some(mtime_ms)).await {
        Ok(Some(duration)) => duration,
        Ok(None) => {
            delete_invalid_file(
                &state,
                &filename,
                song.url.as_deref(),
                InvalidReason::NoDuration,
            )
            .await;
            return Err(ApiError::unprocessable("Could not determine file duration"));
        }
        Err(ProbeError::BinaryMissing) => {
            return Err(ApiError::not_found("Song file not found"));
        }
        Err(ProbeError::Exit(_)) => {
            if song.url.is_some() {
                delete_invalid_file(
                    &state,
                    &filename,
                    song.url.as_deref(),
                    InvalidReason::Unreadable,
                )
                .await;
                return Err(ApiError::unprocessable("Could not determine file duration"));
            }
            eprintln!("failed to read song duration ffprobe exited non-zero");
            return Err(ApiError::internal("Could not determine file duration"));
        }
        Err(error) => {
            eprintln!("failed to read song duration {error:?}");
            return Err(ApiError::internal("Could not determine file duration"));
        }
    };

    if let Some(url) = song.url.clone() {
        db::call(&state.pool, move |conn| {
            conn.execute(
                "UPDATE files SET duration = ?1 WHERE url = ?2",
                rusqlite::params![duration, url],
            )?;
            Ok(())
        })
        .await?;
    }

    let mut response = json_cached(json!({ "duration": js_float(duration) }));
    set_header(&mut response, header::CACHE_CONTROL, DURATION_CACHE);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_emoji_resolves_known_ids() {
        assert_eq!(category_emoji("best-of"), Some("⭐"));
        assert_eq!(category_emoji("worst-of"), Some("🗑"));
        assert!(category_emoji("bogus").is_none());
    }
}
