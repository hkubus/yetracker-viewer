//! `/songs` (plain list and search/filter mode), `/songs/{id}` and the song
//! list of `/eras/{id}/songs`. The media endpoints (stream, download,
//! duration, cover) live in `media.rs`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{RawQuery, State};
use axum::response::Response;
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, Row, params_from_iter};
use serde_json::{Map, Value, json};
use tokio::sync::OwnedSemaphorePermit;
use url::Url;

use super::{EraId, Params, SongId, js_float, json_cached, json_list};
use crate::config::Config;
use crate::db::{self, meta_keys};
use crate::downloader::{Source, source_of};
use crate::error::ApiError;
use crate::importer::is_downloadable_url;
use crate::rank::{CatalogIndex, SearchQuery, SongTexts};
use crate::request::{
    SONG_LIST_PARAMS, SongSort, limit_value, offset_value, positive_integer, search_query,
    sort_value,
};
use crate::search_text::SONG_CATEGORIES;
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

/// Quality of songs that have no audio anywhere.
const NOT_AVAILABLE: &str = "Not Available";

/// Fallback era color: six hex digits, never null in payloads.
const DEFAULT_COLOR: &str = "666666";

/// Page sizes: default and maximum.
const ERA_PAGE_LIMIT: (i64, i64) = (100, 500);
const SEARCH_PAGE_LIMIT: (i64, i64) = (50, 50);

/// How long a search waits for one of the `SEARCH_CONCURRENCY` slots before
/// answering 503.
const SEARCH_SLOT_WAIT: Duration = Duration::from_secs(10);

/// Pages longer than this take a search slot like text searches do; smaller
/// pages and filter-only requests are cheap and never wait for one.
const HEAVY_PAGE: i64 = 100;

/// The folded text a global search matches: the song's own text plus its
/// era's name and subtitle.
const SEARCH_TEXT: &str = "s.search_text";
/// The folded text an era's song list matches: the song's own text only, so a
/// token from the era's name doesn't match every song of the era (rows written
/// before the column existed fall back to the full text).
const OWN_SEARCH_TEXT: &str = "coalesce(s.song_search_text, s.search_text)";

/// Columns of a full song payload over `songs s LEFT JOIN files f`, in the
/// order [`SongRow::read`] expects.
const SONG_COLUMNS: &str = "s.id, s.era, s.era_position, s.catalog_id, s.name, s.title, \
     s.sub_era, s.notes, s.notes_links, s.file_date, s.file_date_precision, s.leak_date, \
     s.leak_date_precision, s.available_length, s.track_length, s.track_length_approx, \
     s.quality, s.url, s.links, f.status, f.filename, f.duration";
const SONG_SOURCE: &str = "songs s LEFT JOIN files f ON f.url = s.url";

/// `ORDER BY` for a sort key; every order ends in catalog position, and the
/// date sorts put songs without a date last.
fn order_by(sort: SongSort) -> &'static str {
    match sort {
        SongSort::Catalog => "s.position, s.id",
        SongSort::Category => "s.category_rank, s.position, s.id",
        SongSort::LeakNewest => "s.leak_date IS NULL, s.leak_date DESC, s.position, s.id",
        SongSort::LeakOldest => "s.leak_date IS NULL, s.leak_date, s.position, s.id",
        SongSort::FileNewest => "s.file_date IS NULL, s.file_date DESC, s.position, s.id",
        // Titles without letters or digits (`???`) fold to an empty key; they
        // go last instead of first.
        SongSort::Name => "coalesce(s.sort_title, '') = '', s.sort_title, s.position, s.id",
    }
}

/// `category`: the marker of a category id; `None` → no filter, unknown →
/// 400 `Invalid category filter`.
fn category_marker(value: Option<&str>) -> Result<Option<&'static str>, ApiError> {
    let Some(id) = value else {
        return Ok(None);
    };
    SONG_CATEGORIES
        .iter()
        .find(|category| category.id == id)
        .map(|category| Some(category.marker))
        .ok_or_else(|| ApiError::bad_request("Invalid category filter"))
}

/// What this server downloads, as far as `downloadState` is concerned.
#[derive(Debug, Clone, Copy)]
struct DownloadPolicy {
    /// `YOUTUBE_DOWNLOAD`: YouTube links are fetched with yt-dlp.
    youtube: bool,
}

impl DownloadPolicy {
    fn of(config: &Config) -> Self {
        Self {
            youtube: config.youtube_download,
        }
    }

    /// Whether the downloader would ever fetch `url`.
    fn downloads(self, url: &str) -> bool {
        if !is_downloadable_url(url) {
            return false;
        }
        self.youtube
            || !matches!(
                Url::parse(url).ok().and_then(|url| source_of(&url)),
                Some(Source::YouTube)
            )
    }
}

/// A `WHERE` clause over `songs s` with its parameters.
struct Filter {
    sql: String,
    values: Vec<SqlValue>,
}

impl Filter {
    fn catalog() -> Self {
        Self {
            sql: "s.catalog_id = 'unreleased'".to_string(),
            values: Vec::new(),
        }
    }

    fn and(&mut self, clause: &str, values: impl IntoIterator<Item = SqlValue>) {
        self.sql.push_str(" AND ");
        self.sql.push_str(clause);
        self.values.extend(values);
    }

    /// Songs whose folded `text` ([`SEARCH_TEXT`] or [`OWN_SEARCH_TEXT`])
    /// contains every token (fold/substring semantics; the tokens are folded
    /// already).
    fn matching(&mut self, text: &str, tokens: &[String]) {
        let clause = format!("instr({text}, ?) > 0");
        for token in tokens {
            self.and(&clause, [SqlValue::Text(token.clone())]);
        }
    }

    /// Songs carrying every one of `markers`. Markers only ever lead the
    /// title, and a song can carry several (`🗑️🤖`), so a song is in each
    /// category whose marker it has.
    fn categories(&mut self, markers: &[&str]) {
        for marker in markers {
            self.and(
                "instr(s.title, ?) > 0",
                [SqlValue::Text((*marker).to_string())],
            );
        }
    }

    fn count(&self, conn: &Connection) -> Result<i64, ApiError> {
        let sql = format!("SELECT count(*) FROM songs s WHERE {}", self.sql);
        Ok(conn
            .prepare_cached(&sql)?
            .query_row(params_from_iter(&self.values), |row| row.get(0))?)
    }

    /// One page in `sort` order. The ids are sorted on `songs` alone and the
    /// full rows (with their `files` row) fetched for the page only.
    fn page(
        &self,
        conn: &Connection,
        sort: SongSort,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<SongRow>, ApiError> {
        let sql = format!(
            "SELECT s.id FROM songs s WHERE {} ORDER BY {} LIMIT ? OFFSET ?",
            self.sql,
            order_by(sort)
        );
        let values = self
            .values
            .iter()
            .cloned()
            .chain([SqlValue::Integer(limit), SqlValue::Integer(offset)]);
        let mut statement = conn.prepare_cached(&sql)?;
        let ids = statement
            .query_map(params_from_iter(values), |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows_by_id(conn, &ids)
    }
}

/// A `songs` row joined with its `files` row, as selected by [`SONG_COLUMNS`].
struct SongRow {
    id: i64,
    era_id: Option<i64>,
    era_position: Option<i64>,
    catalog_id: Option<String>,
    name: Option<String>,
    title: Option<String>,
    sub_era: Option<String>,
    notes: Option<String>,
    notes_links: Option<String>,
    file_date: Option<i64>,
    file_date_precision: Option<String>,
    leak_date: Option<i64>,
    leak_date_precision: Option<String>,
    available_length: Option<String>,
    track_length: Option<i64>,
    track_length_approx: Option<i64>,
    quality: Option<String>,
    url: Option<String>,
    links: Option<String>,
    file_status: Option<String>,
    filename: Option<String>,
    duration: Option<f64>,
}

impl SongRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            era_id: row.get(1)?,
            era_position: row.get(2)?,
            catalog_id: row.get(3)?,
            name: row.get(4)?,
            title: row.get(5)?,
            sub_era: row.get(6)?,
            notes: row.get(7)?,
            notes_links: row.get(8)?,
            file_date: row.get(9)?,
            file_date_precision: row.get(10)?,
            leak_date: row.get(11)?,
            leak_date_precision: row.get(12)?,
            available_length: row.get(13)?,
            track_length: row.get(14)?,
            track_length_approx: row.get(15)?,
            quality: row.get(16)?,
            url: row.get(17)?,
            links: row.get(18)?,
            file_status: row.get(19)?,
            filename: row.get(20)?,
            duration: row.get(21)?,
        })
    }

    fn url(&self) -> Option<&str> {
        self.url.as_deref().filter(|url| !url.is_empty())
    }

    /// Whether `/songs/{id}/stream` can serve the song: its file was
    /// downloaded and is still on disk.
    fn playable(&self, state: &AppState) -> bool {
        state
            .playable
            .resolve_playable_blocking(&state.config.songs_path, self.filename.as_deref())
    }

    /// Where the song's audio stands (see the `DownloadState` type). A link
    /// this server doesn't download — an unsupported host, a host disabled by
    /// the configuration (`YOUTUBE_DOWNLOAD=false`), or a song without audio
    /// anywhere — is `unsupported` rather than waiting forever as `pending`.
    fn download_state(&self, playable: bool, policy: DownloadPolicy) -> &'static str {
        if playable {
            return "downloaded";
        }
        let Some(url) = self.url() else {
            return "none";
        };
        if self.quality.as_deref() == Some(NOT_AVAILABLE) || !policy.downloads(url) {
            return "unsupported";
        }
        match self.file_status.as_deref() {
            Some("failed") => "failed",
            _ => "pending",
        }
    }

    /// The contract's `Song` object, as `state` sees it.
    fn json(&self, state: &AppState) -> Map<String, Value> {
        self.to_json(self.playable(state), DownloadPolicy::of(&state.config))
    }

    /// The contract's `Song` object.
    fn to_json(&self, playable: bool, policy: DownloadPolicy) -> Map<String, Value> {
        let name = self.name.clone().unwrap_or_default();
        let title = self
            .title
            .clone()
            .unwrap_or_else(|| text::first_line(&name).to_string());
        let links = json_array(self.links.as_deref())
            .unwrap_or_else(|| self.url().map(|url| json!([url])).unwrap_or(json!([])));
        let (file_date, file_date_precision) =
            dated(self.file_date, self.file_date_precision.as_deref());
        let (leak_date, leak_date_precision) =
            dated(self.leak_date, self.leak_date_precision.as_deref());
        let duration = self
            .duration
            .filter(|duration| playable && duration.is_finite() && *duration > 0.0)
            .map_or(Value::Null, js_float);

        let mut song = Map::new();
        song.insert("id".into(), json!(self.id));
        song.insert("eraId".into(), json!(self.era_id));
        song.insert("eraPosition".into(), json!(self.era_position.unwrap_or(1)));
        song.insert("catalogId".into(), json!(self.catalog_id));
        song.insert("name".into(), json!(name));
        song.insert("title".into(), json!(title));
        song.insert("subEra".into(), json!(self.sub_era));
        song.insert(
            "notes".into(),
            json!(self.notes.as_deref().unwrap_or_default()),
        );
        song.insert(
            "notesLinks".into(),
            json_array(self.notes_links.as_deref()).unwrap_or(json!([])),
        );
        song.insert("fileDate".into(), file_date);
        song.insert("fileDatePrecision".into(), file_date_precision);
        song.insert("leakDate".into(), leak_date);
        song.insert("leakDatePrecision".into(), leak_date_precision);
        song.insert("availableLength".into(), json!(self.available_length));
        song.insert("trackLength".into(), json!(self.track_length));
        song.insert(
            "trackLengthApprox".into(),
            json!(self.track_length_approx.unwrap_or(0) != 0),
        );
        song.insert("quality".into(), json!(self.quality));
        song.insert("url".into(), json!(self.url()));
        song.insert("links".into(), links);
        song.insert(
            "downloadState".into(),
            json!(self.download_state(playable, policy)),
        );
        song.insert("playable".into(), json!(playable));
        song.insert("duration".into(), duration);
        song
    }
}

/// A stored JSON array (`links`, `notes_links`); `None` when missing or not
/// an array.
fn json_array(text: Option<&str>) -> Option<Value> {
    serde_json::from_str::<Value>(text?)
        .ok()
        .filter(Value::is_array)
}

/// A date and its precision; the precision is null exactly when the date is.
fn dated(date: Option<i64>, precision: Option<&str>) -> (Value, Value) {
    match date {
        None => (Value::Null, Value::Null),
        Some(date) => {
            let precision = precision
                .filter(|precision| matches!(*precision, "day" | "month" | "year"))
                .unwrap_or("day");
            (json!(date), json!(precision))
        }
    }
}

/// `eras.dominant_color` if it is six hex digits, else [`DEFAULT_COLOR`].
pub(super) fn dominant_color(value: Option<&str>) -> String {
    value
        .filter(|color| color.len() == 6 && color.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .unwrap_or(DEFAULT_COLOR)
        .to_string()
}

/// A full-row query → the song payloads, in row order.
fn songs_json(state: &AppState, rows: &[SongRow]) -> Vec<Value> {
    rows.iter()
        .map(|row| Value::Object(row.json(state)))
        .collect()
}

pub async fn get_song(
    State(state): State<SharedState>,
    SongId(id): SongId,
) -> Result<Response, ApiError> {
    let shared = state.clone();
    let song = db::call(&state.pool, move |conn| {
        let sql = format!("SELECT {SONG_COLUMNS} FROM {SONG_SOURCE} WHERE s.id = ?1");
        let mut statement = conn.prepare_cached(&sql)?;
        let mut rows = statement.query_map([id], SongRow::read)?;
        match rows.next() {
            Some(row) => Ok(Some(row?.json(&shared))),
            None => Ok(None),
        }
    })
    .await?;
    match song {
        Some(song) => Ok(json_cached(Value::Object(song))),
        None => Err(ApiError::not_found("Song not found")),
    }
}

/// What `q` asks for: the folded text to match, and the category markers it
/// contains (`⭐`, `🗑️`, with or without a variation selector), which filter
/// like `category` does. A `q` with neither (`???`) is treated as blank.
#[derive(Debug, Default)]
struct SearchInput {
    /// `None` when nothing searchable is left after folding.
    text: Option<SearchQuery>,
    markers: Vec<&'static str>,
}

impl SearchInput {
    fn parse(params: &Params) -> Result<Self, ApiError> {
        let Some(query) = search_query(params.get("q"))? else {
            return Ok(Self::default());
        };
        let markers = SONG_CATEGORIES
            .iter()
            .filter(|category| query.contains(category.marker))
            .map(|category| category.marker)
            .collect();
        Ok(Self {
            text: SearchQuery::new(&query),
            markers,
        })
    }

    fn is_blank(&self) -> bool {
        self.text.is_none() && self.markers.is_empty()
    }

    fn tokens(&self) -> &[String] {
        self.text.as_ref().map_or(&[], SearchQuery::tokens)
    }

    /// `markers` (of the `category` filter) plus those of `q`, each once.
    fn with_markers(&self, mut markers: Vec<&'static str>) -> Vec<&'static str> {
        for marker in &self.markers {
            if !markers.contains(marker) {
                markers.push(marker);
            }
        }
        markers
    }
}

/// Whether a request needs a search slot: text searches and pages longer
/// than [`HEAVY_PAGE`] do; filter-only requests and ordinary pages are cheap.
fn takes_search_slot(text_search: bool, limit: i64) -> bool {
    text_search || limit > HEAVY_PAGE
}

/// One of the `SEARCH_CONCURRENCY` slots, when [`takes_search_slot`]. Waits
/// at most [`SEARCH_SLOT_WAIT`], then answers 503.
async fn search_slot(
    state: &AppState,
    text_search: bool,
    limit: i64,
) -> Result<Option<OwnedSemaphorePermit>, ApiError> {
    if !takes_search_slot(text_search, limit) {
        return Ok(None);
    }
    match tokio::time::timeout(SEARCH_SLOT_WAIT, state.search_slots.clone().acquire_owned()).await {
        Ok(Ok(permit)) => Ok(Some(permit)),
        _ => Err(ApiError::busy("Too many searches right now, try again", 2)),
    }
}

/// `/eras/{id}/songs`: one page of an era in `sort` order (catalog order by
/// default), optionally narrowed by `q` (every folded token must occur in the
/// song's own text; category markers in `q` filter like `category`) and
/// `category`. `X-Total-Count` is the number of matching songs.
pub async fn era_songs(
    state: SharedState,
    EraId(era_id): EraId,
    params: &Params,
) -> Result<Response, ApiError> {
    let input = SearchInput::parse(params)?;
    let (default_limit, max_limit) = ERA_PAGE_LIMIT;
    let limit = limit_value(params.get("limit"), default_limit, max_limit)?;
    let offset = offset_value(params.get("offset"))?;
    let sort = sort_value(params.get("sort"))?;
    let markers = input.with_markers(
        category_marker(params.get("category"))?
            .into_iter()
            .collect(),
    );
    let tokens = input.tokens().to_vec();

    let permit = search_slot(&state, !tokens.is_empty(), limit).await?;
    let shared = state.clone();
    let (total, songs) = db::call(&state.pool, move |conn| {
        // Held until the query is done, even if the client goes away.
        let _permit = permit;
        let exists = conn
            .prepare_cached("SELECT 1 FROM eras WHERE id = ?1")?
            .exists([era_id])?;
        if !exists {
            return Err(ApiError::not_found("Era does not exist"));
        }
        let mut filter = Filter::catalog();
        filter.and("s.era = ?", [SqlValue::Integer(era_id)]);
        filter.matching(OWN_SEARCH_TEXT, &tokens);
        filter.categories(&markers);
        let total = filter.count(conn)?;
        let rows = filter.page(conn, sort, limit, offset)?;
        Ok((total, songs_json(&shared, &rows)))
    })
    .await?;
    Ok(json_list(Value::Array(songs), total))
}

/// Search/filter parameters of `/songs`.
#[derive(Debug, Clone, Default)]
struct SearchFilters {
    era: Option<i64>,
    era_from: Option<i64>,
    era_to: Option<i64>,
    quality: Option<String>,
    available_length: Option<String>,
    playable: Option<bool>,
    /// Category markers a song must all carry (`category`, and those in `q`).
    categories: Vec<&'static str>,
}

impl SearchFilters {
    /// Blank values count as absent, like for every other parameter.
    fn parse(params: &Params) -> Result<Self, ApiError> {
        let id = |name: &str, label: &str| {
            params
                .get(name)
                .map(|value| positive_integer(Some(value), label))
                .transpose()
        };
        let choice = |name: &str, allowed: &[&str], label: &str| {
            params
                .get(name)
                .map(|value| {
                    allowed
                        .contains(&value)
                        .then(|| value.to_string())
                        .ok_or_else(|| ApiError::bad_request(format!("Invalid {label} filter")))
                })
                .transpose()
        };
        let playable = match params.get("playable") {
            None => None,
            Some("true") => Some(true),
            Some("false") => Some(false),
            Some(_) => return Err(ApiError::bad_request("Invalid playable filter")),
        };
        Ok(Self {
            era: id("era", "era filter")?,
            era_from: id("eraFrom", "starting era filter")?,
            era_to: id("eraTo", "ending era filter")?,
            quality: choice("quality", &QUALITY_FILTERS, "quality")?,
            available_length: choice("availability", &AVAILABILITY_FILTERS, "availability")?,
            playable,
            categories: category_marker(params.get("category"))?
                .into_iter()
                .collect(),
        })
    }

    fn any(&self) -> bool {
        self.era.is_some()
            || self.era_from.is_some()
            || self.era_to.is_some()
            || self.quality.is_some()
            || self.available_length.is_some()
            || self.playable.is_some()
            || !self.categories.is_empty()
    }
}

/// `/songs`: the plain catalog list, or search/filter mode when `q` is not
/// blank (it has text to match or category markers) or any filter is
/// present.
pub async fn list_songs(
    State(state): State<SharedState>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let params = Params::parse(query.as_deref(), SONG_LIST_PARAMS)?;
    let input = SearchInput::parse(&params)?;
    let filters = SearchFilters::parse(&params)?;
    if input.is_blank() && !filters.any() {
        return plain_list(state, &params).await;
    }
    search_songs(state, input, filters, &params).await
}

/// Plain mode: a bare array of songs in `sort` order, with `X-Total-Count`.
async fn plain_list(state: SharedState, params: &Params) -> Result<Response, ApiError> {
    let (default_limit, max_limit) = ERA_PAGE_LIMIT;
    let limit = limit_value(params.get("limit"), default_limit, max_limit)?;
    let offset = offset_value(params.get("offset"))?;
    let sort = sort_value(params.get("sort"))?;
    let permit = search_slot(&state, false, limit).await?;
    let shared = state.clone();
    let (total, songs) = db::call(&state.pool, move |conn| {
        let _permit = permit;
        let filter = Filter::catalog();
        let total = filter.count(conn)?;
        let rows = filter.page(conn, sort, limit, offset)?;
        Ok((total, songs_json(&shared, &rows)))
    })
    .await?;
    Ok(json_list(Value::Array(songs), total))
}

/// An era as search results show it.
struct EraInfo {
    position: i64,
    name: String,
    dominant_color: String,
    cover_version: Option<String>,
}

fn load_eras(conn: &Connection) -> Result<HashMap<i64, EraInfo>, ApiError> {
    let mut statement = conn.prepare_cached(
        "SELECT id, coalesce(position, id), coalesce(name, ''), dominant_color, cover_version \
         FROM eras",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            EraInfo {
                position: row.get(1)?,
                name: row.get(2)?,
                dominant_color: dominant_color(row.get::<_, Option<String>>(3)?.as_deref()),
                cover_version: row
                    .get::<_, Option<String>>(4)?
                    .filter(|version| !version.is_empty()),
            },
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// A song matching a search, and its downloaded file if it has one.
struct Match {
    id: i64,
    filename: Option<String>,
}

/// Search/filter mode: `{songs, total, offset, limit}`. With query tokens
/// the matches are ranked by relevance (see `rank.rs`), otherwise ordered by
/// `sort`. `total` counts every match after all filters.
async fn search_songs(
    state: SharedState,
    input: SearchInput,
    mut filters: SearchFilters,
    params: &Params,
) -> Result<Response, ApiError> {
    let (default_limit, max_limit) = SEARCH_PAGE_LIMIT;
    let limit = limit_value(params.get("limit"), default_limit, max_limit)?;
    let offset = offset_value(params.get("offset"))?;
    let sort = sort_value(params.get("sort"))?;
    filters.categories = input.with_markers(std::mem::take(&mut filters.categories));
    let search = input.text;

    let permit = search_slot(&state, search.is_some(), limit).await?;
    let shared = state.clone();
    let (total, songs) = db::call(&state.pool, move |conn| {
        // Held until the query is done, even if the client goes away.
        let _permit = permit;
        // One read transaction: the ranking index, the matches and the page
        // all see the same catalog, even while an import commits.
        let snapshot = conn.unchecked_transaction()?;
        let found = run_search(
            &shared,
            &snapshot,
            search.as_ref(),
            &filters,
            sort,
            limit,
            offset,
        );
        snapshot.finish()?;
        found
    })
    .await?;
    let body = json!({ "songs": songs, "total": total, "offset": offset, "limit": limit });
    Ok(json_list(body, total))
}

fn run_search(
    state: &AppState,
    conn: &Connection,
    search: Option<&SearchQuery>,
    filters: &SearchFilters,
    sort: SongSort,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Value>), ApiError> {
    let eras = load_eras(conn)?;
    let era_position = |id: Option<i64>, label: &str| -> Result<Option<i64>, ApiError> {
        id.map(|id| {
            eras.get(&id)
                .map(|era| era.position)
                .ok_or_else(|| ApiError::bad_request(format!("Invalid {label}")))
        })
        .transpose()
    };
    let from = era_position(filters.era_from, "starting era filter")?;
    let to = era_position(filters.era_to, "ending era filter")?;
    if let (Some(from), Some(to)) = (from, to)
        && from > to
    {
        return Err(ApiError::bad_request(
            "Starting era must not be after ending era",
        ));
    }

    let mut filter = Filter::catalog();
    if let Some(tokens) = search.map(SearchQuery::tokens) {
        filter.matching(SEARCH_TEXT, tokens);
    }
    if let Some(era) = filters.era {
        filter.and("s.era = ?", [SqlValue::Integer(era)]);
    }
    if from.is_some() || to.is_some() {
        filter.and(
            "s.era IN (SELECT id FROM eras WHERE coalesce(position, id) BETWEEN ? AND ?)",
            [
                SqlValue::Integer(from.unwrap_or(i64::MIN)),
                SqlValue::Integer(to.unwrap_or(i64::MAX)),
            ],
        );
    }
    if let Some(quality) = &filters.quality {
        filter.and("s.quality = ?", [SqlValue::Text(quality.clone())]);
    }
    if let Some(available_length) = &filters.available_length {
        filter.and(
            "s.available_length = ?",
            [SqlValue::Text(available_length.clone())],
        );
    }
    filter.categories(&filters.categories);
    if filters.playable == Some(true) {
        // Only downloaded files can be playable; the disk check follows.
        filter.and(DOWNLOADED, []);
    }

    let (total, rows) = if search.is_none() && filters.playable.is_none() {
        // Plain SQL paging: nothing to rank and nothing to check on disk.
        (filter.count(conn)?, filter.page(conn, sort, limit, offset)?)
    } else {
        let mut matches: Vec<(i64, bool)> =
            find_matches(conn, &filter, search.is_none().then_some(sort))?
                .into_iter()
                .map(|found| {
                    let playable = state.playable.resolve_playable_blocking(
                        &state.config.songs_path,
                        found.filename.as_deref(),
                    );
                    (found.id, playable)
                })
                .collect();
        if let Some(wanted) = filters.playable {
            matches.retain(|(_, playable)| *playable == wanted);
        }
        let total = matches.len() as i64;
        let wanted = usize::try_from(offset + limit).unwrap_or(usize::MAX);
        let order: Vec<usize> = match search {
            Some(search) => catalog_index(state, conn)?.rank(search, &matches, wanted),
            None => (0..matches.len().min(wanted)).collect(),
        };
        let page: Vec<i64> = order
            .into_iter()
            .skip(offset as usize)
            .map(|index| matches[index].0)
            .collect();
        (total, rows_by_id(conn, &page)?)
    };

    let songs = rows
        .iter()
        .map(|row| {
            let mut song = row.json(state);
            let era = row.era_id.and_then(|id| eras.get(&id));
            song.insert(
                "eraName".into(),
                json!(era.map(|era| era.name.as_str()).unwrap_or_default()),
            );
            song.insert(
                "dominantColor".into(),
                json!(era.map_or(DEFAULT_COLOR, |era| era.dominant_color.as_str())),
            );
            let cover_version = era.and_then(|era| era.cover_version.as_deref());
            song.insert("eraHasCover".into(), json!(cover_version.is_some()));
            song.insert("eraCoverVersion".into(), json!(cover_version));
            Value::Object(song)
        })
        .collect();
    Ok((total, songs))
}

/// Songs whose link has a downloaded file (a membership test against the
/// few downloaded links is much cheaper than joining `files` for every row).
pub(super) const DOWNLOADED: &str = "s.url IN (SELECT url FROM files WHERE filename IS NOT NULL)";

/// Every song passing `filter` with its downloaded file, in `sort` order
/// when given.
fn find_matches(
    conn: &Connection,
    filter: &Filter,
    sort: Option<SongSort>,
) -> Result<Vec<Match>, ApiError> {
    let mut sql = format!(
        "SELECT s.id, CASE WHEN {DOWNLOADED} THEN (SELECT filename FROM files WHERE url = s.url) END \
         FROM songs s WHERE {}",
        filter.sql
    );
    if let Some(sort) = sort {
        sql.push_str(" ORDER BY ");
        sql.push_str(order_by(sort));
    }
    let mut statement = conn.prepare_cached(&sql)?;
    let rows = statement.query_map(params_from_iter(&filter.values), |row| {
        Ok(Match {
            id: row.get(0)?,
            filename: row.get(1)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The ranking index of the current catalog, rebuilt after an import changed
/// it (the catalog fingerprint in `meta` is its version).
fn catalog_index(state: &AppState, conn: &Connection) -> Result<Arc<CatalogIndex>, ApiError> {
    let version = db::meta_get(conn, meta_keys::LAST_SHEET_SHA256)?;
    state.rank_cache.index(version.as_deref(), || {
        let mut index = CatalogIndex::new(version.clone());
        let mut eras = conn.prepare_cached("SELECT id, coalesce(name, ''), subtitle FROM eras")?;
        let mut rows = eras.query([])?;
        while let Some(row) = rows.next()? {
            index.add_era(
                row.get(0)?,
                row.get_ref(1)?.as_str().map_err(rusqlite::Error::from)?,
                row.get_ref(2)?
                    .as_str_or_null()
                    .map_err(rusqlite::Error::from)?,
            );
        }
        let mut songs = conn.prepare_cached(
            "SELECT id, coalesce(position, id), era, coalesce(name, ''), \
             coalesce(search_text, ''), sub_era, quality, available_length, \
             coalesce(category_rank, 4) FROM songs WHERE catalog_id = 'unreleased'",
        )?;
        let mut rows = songs.query([])?;
        while let Some(row) = rows.next()? {
            let text = |column: usize| -> rusqlite::Result<Option<&str>> {
                Ok(row.get_ref(column)?.as_str_or_null()?)
            };
            index.add_song(
                row.get(0)?,
                &SongTexts {
                    position: row.get(1)?,
                    era: row.get(2)?,
                    name: text(3)?.unwrap_or_default(),
                    search_text: text(4)?.unwrap_or_default(),
                    sub_era: text(5)?,
                    quality: text(6)?,
                    available_length: text(7)?,
                    category_rank: row.get(8)?,
                },
            );
        }
        Ok::<_, ApiError>(index)
    })
}

/// Full rows for `ids`, in that order.
fn rows_by_id(conn: &Connection, ids: &[i64]) -> Result<Vec<SongRow>, ApiError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT {SONG_COLUMNS} FROM {SONG_SOURCE} \
         WHERE s.id IN (SELECT value FROM json_each(?1))"
    );
    let ids_json = serde_json::to_string(ids).expect("a list of integers serialises");
    let mut statement = conn.prepare_cached(&sql)?;
    let rows = statement.query_map([ids_json], SongRow::read)?;
    let mut by_id: HashMap<i64, SongRow> = HashMap::with_capacity(ids.len());
    for row in rows {
        let row = row?;
        by_id.insert(row.id, row);
    }
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::Query;

    /// Every supported host is downloaded.
    const EVERYTHING: DownloadPolicy = DownloadPolicy { youtube: true };

    fn params(query: &str) -> Params {
        Params(Query::parse(Some(query)))
    }

    #[test]
    fn category_filters_resolve_known_ids() {
        assert_eq!(category_marker(Some("best-of")).unwrap(), Some("⭐"));
        assert_eq!(category_marker(Some("grails")).unwrap(), Some("🏆"));
        assert_eq!(category_marker(Some("worst-of")).unwrap(), Some("🗑"));
        assert_eq!(category_marker(None).unwrap(), None);
        assert_eq!(
            category_marker(Some("bogus")).unwrap_err().to_string(),
            "400 Invalid category filter"
        );
    }

    #[test]
    fn dates_carry_their_precision_or_neither() {
        assert_eq!(dated(None, Some("day")), (Value::Null, Value::Null));
        assert_eq!(
            dated(Some(86400), Some("month")),
            (json!(86400), json!("month"))
        );
        assert_eq!(dated(Some(86400), None), (json!(86400), json!("day")));
        assert_eq!(dated(Some(0), Some("bogus")), (json!(0), json!("day")));
    }

    #[test]
    fn colors_fall_back_to_gray() {
        assert_eq!(dominant_color(Some("1a2B3c")), "1a2B3c");
        assert_eq!(dominant_color(Some("#1a2b3c")), DEFAULT_COLOR);
        assert_eq!(dominant_color(Some("")), DEFAULT_COLOR);
        assert_eq!(dominant_color(None), DEFAULT_COLOR);
    }

    fn row(url: Option<&str>, quality: Option<&str>, status: Option<&str>) -> SongRow {
        SongRow {
            id: 1,
            era_id: Some(2),
            era_position: Some(3),
            catalog_id: Some("unreleased".into()),
            name: Some("⭐ Song [V2]\n(feat. X)".into()),
            title: None,
            sub_era: None,
            notes: None,
            notes_links: Some(r#"[{"text":"a","url":"https://x.test/a"}]"#.into()),
            file_date: None,
            file_date_precision: Some("day".into()),
            leak_date: Some(1_509_494_400),
            leak_date_precision: Some("month".into()),
            available_length: Some("Full".into()),
            track_length: Some(185),
            track_length_approx: Some(1),
            quality: quality.map(Into::into),
            url: url.map(Into::into),
            links: None,
            file_status: status.map(Into::into),
            filename: None,
            duration: Some(185.25),
        }
    }

    #[test]
    fn download_state_follows_the_link_and_the_file() {
        let pillows = Some("https://pillows.su/f/abc");
        let state = |song: SongRow, playable: bool| song.download_state(playable, EVERYTHING);
        assert_eq!(state(row(None, None, None), false), "none");
        assert_eq!(state(row(Some(""), None, None), false), "none");
        assert_eq!(
            state(row(Some("https://example.com/x"), None, None), false),
            "unsupported"
        );
        assert_eq!(
            state(row(pillows, Some(NOT_AVAILABLE), Some("pending")), false),
            "unsupported"
        );
        assert_eq!(state(row(pillows, None, None), false), "pending");
        assert_eq!(
            state(row(pillows, None, Some("downloaded")), false),
            "pending"
        );
        assert_eq!(state(row(pillows, None, Some("failed")), false), "failed");
        assert_eq!(
            state(row(pillows, Some(NOT_AVAILABLE), Some("downloaded")), true),
            "downloaded"
        );
    }

    /// With `YOUTUBE_DOWNLOAD=false` YouTube links are never fetched, so they
    /// are `unsupported` instead of `pending` forever; other hosts and files
    /// already on disk are unaffected.
    #[test]
    fn disabled_youtube_downloads_are_unsupported() {
        let no_youtube = DownloadPolicy { youtube: false };
        for url in [
            "https://youtu.be/abc",
            "https://www.youtube.com/watch?v=abc",
            "https://music.youtube.com/watch?v=abc",
        ] {
            for status in [None, Some("pending"), Some("failed")] {
                let song = row(Some(url), Some("CD Quality"), status);
                assert_eq!(
                    song.download_state(false, no_youtube),
                    "unsupported",
                    "{url}"
                );
                assert_ne!(
                    song.download_state(false, EVERYTHING),
                    "unsupported",
                    "{url}"
                );
            }
            let song = row(Some(url), None, Some("downloaded"));
            assert_eq!(song.download_state(true, no_youtube), "downloaded");
        }
        let pillows = row(Some("https://pillows.su/f/abc"), None, None);
        assert_eq!(pillows.download_state(false, no_youtube), "pending");
        let instagram = row(Some("https://www.instagram.com/p/abc/"), None, None);
        assert_eq!(instagram.download_state(false, no_youtube), "pending");
    }

    #[test]
    fn search_input_splits_text_and_category_markers() {
        let input = SearchInput::parse(&params("q=%E2%AD%90%EF%B8%8F+glory")).unwrap();
        assert_eq!(input.tokens(), ["glory"]);
        assert_eq!(input.markers, ["⭐"]);

        // Markers alone filter, with or without a variation selector.
        for q in ["🗑️", "🗑", " 🗑 ???", "🗑️🤖"] {
            let input = SearchInput::parse(&params(&format!("q={q}"))).unwrap();
            assert!(input.text.is_none(), "{q}");
            assert!(!input.is_blank(), "{q}");
            assert!(input.markers.contains(&"🗑"), "{q}");
        }
        let both = SearchInput::parse(&params("q=🗑️🤖")).unwrap();
        assert_eq!(both.markers, ["🗑", "🤖"]);

        // Nothing searchable and no marker: the same as no `q` at all.
        for q in ["???", "%20%20", "-+.", "★", ""] {
            let input = SearchInput::parse(&params(&format!("q={q}"))).unwrap();
            assert!(input.is_blank(), "{q:?}");
            assert!(input.tokens().is_empty());
        }

        // The `category` filter and the markers of `q` add up, each once.
        let input = SearchInput::parse(&params("q=⭐+✨")).unwrap();
        assert_eq!(input.with_markers(vec!["✨"]), ["✨", "⭐"]);
        assert_eq!(input.with_markers(Vec::new()), ["⭐", "✨"]);

        let long = format!("q={}", "⭐".repeat(101));
        assert_eq!(
            SearchInput::parse(&params(&long)).unwrap_err().to_string(),
            "400 Search query is too long"
        );
    }

    #[test]
    fn only_text_searches_and_big_pages_take_a_search_slot() {
        assert!(takes_search_slot(true, 1));
        assert!(takes_search_slot(true, 50));
        assert!(!takes_search_slot(false, 50));
        assert!(!takes_search_slot(false, HEAVY_PAGE));
        assert!(takes_search_slot(false, HEAVY_PAGE + 1));
        assert!(takes_search_slot(false, 500));
    }

    /// An in-memory catalog with the columns [`Filter`] reads: two eras'
    /// worth of songs whose search text includes the era name.
    fn catalog() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE songs (id INTEGER PRIMARY KEY, catalog_id TEXT, era INTEGER, \
               position INTEGER, title TEXT, search_text TEXT, song_search_text TEXT, \
               category_rank INTEGER, sort_title TEXT, leak_date INTEGER, file_date INTEGER);
             INSERT INTO songs (id, catalog_id, era, position, title, search_text, song_search_text) VALUES
               (1, 'unreleased', 7, 1, '⭐ Donda [V1]', 'donda v1 donda', 'donda v1'),
               (2, 'unreleased', 7, 2, 'Jail [V2]', 'jail v2 donda', 'jail v2'),
               (3, 'unreleased', 7, 3, '🗑️🤖 Jail [V3]', 'jail v3 donda', 'jail v3'),
               (4, 'unreleased', 7, 4, 'Old Row', 'old row donda', NULL),
               (5, 'unreleased', 8, 5, '⭐ Donda Chant', 'donda chant graduation', 'donda chant');",
        )
        .unwrap();
        conn
    }

    /// The ids `filter` selects, in catalog order.
    fn ids(conn: &Connection, filter: &Filter) -> Vec<i64> {
        let sql = format!(
            "SELECT s.id FROM songs s WHERE {} ORDER BY {}",
            filter.sql,
            order_by(SongSort::Catalog)
        );
        let mut statement = conn.prepare(&sql).unwrap();
        statement
            .query_map(params_from_iter(&filter.values), |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn era_lists_match_the_songs_own_text() {
        let conn = catalog();
        let tokens = vec!["donda".to_string()];

        // A global search matches the era name too.
        let mut global = Filter::catalog();
        global.matching(SEARCH_TEXT, &tokens);
        assert_eq!(ids(&conn, &global), [1, 2, 3, 4, 5]);
        assert_eq!(global.count(&conn).unwrap(), 5);

        // An era's list only matches what the songs themselves say; rows
        // without the column yet fall back to the full text.
        let mut scoped = Filter::catalog();
        scoped.and("s.era = ?", [SqlValue::Integer(7)]);
        scoped.matching(OWN_SEARCH_TEXT, &tokens);
        assert_eq!(ids(&conn, &scoped), [1, 4]);
        assert_eq!(scoped.count(&conn).unwrap(), 2);
    }

    #[test]
    fn category_markers_combine_with_and() {
        let conn = catalog();
        let mut best = Filter::catalog();
        best.categories(&["⭐"]);
        assert_eq!(ids(&conn, &best), [1, 5]);

        let mut both = Filter::catalog();
        both.categories(&["🗑", "🤖"]);
        assert_eq!(ids(&conn, &both), [3]);

        let mut none = Filter::catalog();
        none.categories(&["⭐", "🤖"]);
        assert!(ids(&conn, &none).is_empty());
    }

    #[test]
    fn song_payload_has_every_key() {
        let song = row(Some("https://pillows.su/f/abc"), Some("CD Quality"), None)
            .to_json(false, EVERYTHING);
        let keys: Vec<&str> = song.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "id",
                "eraId",
                "eraPosition",
                "catalogId",
                "name",
                "title",
                "subEra",
                "notes",
                "notesLinks",
                "fileDate",
                "fileDatePrecision",
                "leakDate",
                "leakDatePrecision",
                "availableLength",
                "trackLength",
                "trackLengthApprox",
                "quality",
                "url",
                "links",
                "downloadState",
                "playable",
                "duration",
            ]
        );
        assert_eq!(song["title"], "⭐ Song [V2]");
        assert_eq!(song["notes"], "");
        assert_eq!(song["notesLinks"][0]["url"], "https://x.test/a");
        assert_eq!(song["fileDatePrecision"], Value::Null);
        assert_eq!(song["leakDatePrecision"], "month");
        assert_eq!(song["trackLengthApprox"], true);
        assert_eq!(song["links"], json!(["https://pillows.su/f/abc"]));
        assert_eq!(song["duration"], Value::Null, "no duration without a file");
        assert_eq!(
            row(None, None, None).to_json(true, EVERYTHING)["duration"],
            json!(185.25)
        );
    }
}
