//! SQLite pool, PRAGMAs, the startup DDL and the schema migrations.
//!
//! Schema v2 (see `migrate`): eras and songs carry their sheet order
//! (`position`), derived search/sort columns and a content key used to keep
//! ids stable across imports; `files` tracks download state explicitly; a
//! `meta` table holds the schema version, id high-water marks and the last
//! import's outcome; tombstones remember the ids of deleted songs and eras
//! for a while so that rows restored upstream get their old ids back.
//! Databases written by v1 are upgraded in place and backfilled so the API
//! keeps working before the first v2 import.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use tracing::{info, warn};

use crate::config::Config;
use crate::cover_version;
use crate::downloader::FAILED_RETRY_SECS;
use crate::error::ApiError;
use crate::importer;
use crate::playable::is_safe_filename;
use crate::repair;
use crate::search_text::{self, SearchFields};
use crate::text;

pub type Pool = r2d2::Pool<SqliteConnectionManager>;

/// Current schema version, stored in `meta.schema_version`.
pub const SCHEMA_VERSION: i64 = 2;

/// Keys of the `meta` table.
pub mod meta_keys {
    pub const SCHEMA_VERSION: &str = "schema_version";
    /// Next id for a new song; ids of deleted songs are never reused.
    pub const NEXT_SONG_ID: &str = "next_song_id";
    /// Next id for a new era; ids of deleted eras are never reused.
    pub const NEXT_ERA_ID: &str = "next_era_id";
    /// Unix seconds of the last successful import (including imports that
    /// found the catalog unchanged); absent before the first one.
    pub const LAST_IMPORT_AT: &str = "last_import_at";
    /// `true` / `false`: outcome of the most recent import attempt.
    pub const LAST_IMPORT_OK: &str = "last_import_ok";
    /// Error of the most recent import attempt; NULL after a success.
    pub const LAST_IMPORT_ERROR: &str = "last_import_error";
    /// Fingerprint of the last imported catalog (see `importer`).
    pub const LAST_SHEET_SHA256: &str = "last_sheet_sha256";
    /// Unix seconds of the last cover refresh (see `downloader`).
    pub const LAST_COVER_REFRESH_AT: &str = "last_cover_refresh_at";
    /// How many unseen `files` rows the last cleanup kept because they hold
    /// downloaded media (see `backfill`); an increase is reported.
    pub const CLEANUP_KEPT_DOWNLOADED: &str = "cleanup_kept_downloaded";
}

const PRAGMAS: &str = "\
PRAGMA busy_timeout = 5000;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA cache_size = -64000;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 67108864;
PRAGMA journal_size_limit = 67108864;";

/// Full v2 table shapes for new databases. Existing databases get the missing
/// columns from `COLUMNS` instead.
const TABLES: &str = "\
CREATE TABLE IF NOT EXISTS eras (
  id INTEGER PRIMARY KEY,
  name TEXT,
  notes TEXT,
  image_url TEXT,
  description TEXT,
  dominant_color TEXT,
  cover_source TEXT,
  is_main INTEGER NOT NULL DEFAULT 1,
  key TEXT,
  position INTEGER,
  subtitle TEXT,
  cover_version TEXT,
  cover_attempts INTEGER NOT NULL DEFAULT 0,
  cover_next_attempt_at INTEGER,
  cover_last_error TEXT
);
CREATE TABLE IF NOT EXISTS songs (
  id INTEGER PRIMARY KEY,
  era INTEGER,
  catalog_id TEXT NOT NULL DEFAULT 'unreleased',
  name TEXT,
  notes TEXT,
  file_date INTEGER,
  leak_date INTEGER,
  available_length TEXT,
  track_length INTEGER,
  quality TEXT,
  url TEXT,
  position INTEGER,
  era_position INTEGER,
  title TEXT,
  sub_era TEXT,
  links TEXT,
  notes_links TEXT,
  file_date_precision TEXT,
  leak_date_precision TEXT,
  track_length_approx INTEGER NOT NULL DEFAULT 0,
  search_text TEXT,
  sort_title TEXT,
  category_rank INTEGER,
  song_key TEXT,
  song_search_text TEXT
);
CREATE TABLE IF NOT EXISTS files (
  url TEXT PRIMARY KEY,
  downloaded INTEGER,
  filename TEXT,
  duration REAL,
  status TEXT NOT NULL DEFAULT 'pending',
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER,
  last_error TEXT,
  last_seen_at INTEGER,
  transient_failures INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT
);
CREATE TABLE IF NOT EXISTS song_tombstones (
  id INTEGER PRIMARY KEY,
  song_key TEXT,
  era_key TEXT,
  name TEXT,
  url TEXT,
  deleted_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS era_tombstones (
  id INTEGER PRIMARY KEY,
  key TEXT NOT NULL,
  deleted_at INTEGER NOT NULL
);";

/// Every column added after the original schema, as `(table, column,
/// definition)`. Applied with `ALTER TABLE ... ADD COLUMN` when missing.
const COLUMNS: &[(&str, &str, &str)] = &[
    ("files", "duration", "REAL"),
    ("eras", "is_main", "INTEGER NOT NULL DEFAULT 1"),
    ("eras", "cover_source", "TEXT"),
    ("songs", "catalog_id", "TEXT NOT NULL DEFAULT 'unreleased'"),
    // v2
    ("eras", "key", "TEXT"),
    ("eras", "position", "INTEGER"),
    ("eras", "subtitle", "TEXT"),
    ("eras", "cover_version", "TEXT"),
    ("eras", "cover_attempts", "INTEGER NOT NULL DEFAULT 0"),
    ("eras", "cover_next_attempt_at", "INTEGER"),
    ("eras", "cover_last_error", "TEXT"),
    ("songs", "position", "INTEGER"),
    ("songs", "era_position", "INTEGER"),
    ("songs", "title", "TEXT"),
    ("songs", "sub_era", "TEXT"),
    ("songs", "links", "TEXT"),
    ("songs", "notes_links", "TEXT"),
    ("songs", "file_date_precision", "TEXT"),
    ("songs", "leak_date_precision", "TEXT"),
    ("songs", "track_length_approx", "INTEGER NOT NULL DEFAULT 0"),
    ("songs", "search_text", "TEXT"),
    ("songs", "sort_title", "TEXT"),
    ("songs", "category_rank", "INTEGER"),
    ("songs", "song_key", "TEXT"),
    ("files", "status", "TEXT NOT NULL DEFAULT 'pending'"),
    ("files", "attempts", "INTEGER NOT NULL DEFAULT 0"),
    ("files", "next_attempt_at", "INTEGER"),
    ("files", "last_error", "TEXT"),
    ("files", "last_seen_at", "INTEGER"),
    ("songs", "song_search_text", "TEXT"),
    ("files", "transient_failures", "INTEGER NOT NULL DEFAULT 0"),
];

/// v1 indexes nothing reads any more (single-column filters over ~10k rows
/// are cheaper as scans) or that duplicate a composite index's prefix.
const DROPPED_INDEXES: &[&str] = &[
    "songs_catalog_index",
    "songs_url_index",
    "songs_quality_index",
    "songs_available_length_index",
    "songs_era_index",
];

/// `(catalog_id, position)` serves catalog-ordered listings, `(catalog_id,
/// era, position)` era pages and per-era counts. The `id` variants serve
/// `sort=id` and id-ordered era listings. `songs_search_scan` covers the
/// catalog-wide search, which then scans the index instead of the wider
/// table rows.
const INDEXES: &str = "\
CREATE INDEX IF NOT EXISTS songs_catalog_era_id_index ON songs (catalog_id, era, id);
CREATE INDEX IF NOT EXISTS songs_catalog_id_index ON songs (catalog_id, id);
CREATE INDEX IF NOT EXISTS songs_catalog_position_index ON songs (catalog_id, position);
CREATE INDEX IF NOT EXISTS songs_catalog_era_position_index ON songs (catalog_id, era, position);
CREATE INDEX IF NOT EXISTS songs_search_scan ON songs (catalog_id, search_text, url);
CREATE INDEX IF NOT EXISTS eras_is_main_index ON eras (is_main);
CREATE UNIQUE INDEX IF NOT EXISTS eras_key_index ON eras (key);";

pub fn create_pool(path: &Path) -> Result<Pool, ApiError> {
    // Switch the file to WAL once, before the pool opens its connections in
    // parallel: on a fresh database several of them racing to change the
    // journal mode fail with "database is locked". The mode is persistent, so
    // the per-connection pragma below is then a no-op.
    Connection::open(path)?.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    let manager = SqliteConnectionManager::file(path).with_init(|conn: &mut Connection| {
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch(PRAGMAS)
    });
    r2d2::Pool::builder()
        .max_size(8)
        .build(manager)
        .map_err(ApiError::unexpected)
}

/// Runs the startup DDL and the schema migrations (see [`migrate`]).
pub fn run_migrations(pool: &Pool, config: &Config) -> Result<(), ApiError> {
    let conn = pool.get().map_err(ApiError::unexpected)?;
    migrate(
        &conn,
        &config.storage_path.join("covers"),
        &config.songs_path,
    )
}

/// Brings the database to [`SCHEMA_VERSION`]; idempotent.
///
/// `covers_dir` and `songs_dir` are only read while upgrading a v1 database:
/// on-disk covers get their `cover_version`, and `files.filename` is cleared
/// for media that is not on disk.
pub fn migrate(conn: &Connection, covers_dir: &Path, songs_dir: &Path) -> Result<(), ApiError> {
    conn.execute_batch(TABLES)?;
    for (table, column, definition) in COLUMNS {
        if !has_column(conn, table, column)? {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {definition}"
            ))?;
        }
    }

    // SQLite lets a TEXT primary key be NULL (old versions wrote such rows).
    // A download row without a link identifies nothing and would trip every
    // reader that expects one, so it goes.
    let linkless = conn.execute("DELETE FROM files WHERE url IS NULL", [])?;
    if linkless > 0 {
        warn!(rows = linkless, "removed download rows without a link");
    }

    let version = meta_get(conn, meta_keys::SCHEMA_VERSION)?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(1);
    if version < 2 {
        let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        backfill_v2(&transaction, covers_dir, songs_dir)?;
        meta_set(
            &transaction,
            meta_keys::SCHEMA_VERSION,
            Some(&SCHEMA_VERSION.to_string()),
        )?;
        transaction.commit()?;
    }
    backfill_song_search_text(conn)?;
    // Downloads that gave up before failed rows got a retry date wait a full
    // cool-down from now.
    conn.execute(
        "UPDATE files SET next_attempt_at = ?1 WHERE status = 'failed' AND next_attempt_at IS NULL",
        [unix_now() + FAILED_RETRY_SECS],
    )?;

    for index in DROPPED_INDEXES {
        conn.execute_batch(&format!("DROP INDEX IF EXISTS {index}"))?;
    }
    conn.execute_batch(INDEXES)?;
    conn.execute(
        "UPDATE eras SET dominant_color = '666666' \
         WHERE dominant_color IS NULL OR trim(dominant_color) = ''",
        [],
    )?;
    Ok(())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, ApiError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get("name")?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Unix seconds now.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Reads a `meta` value (`None` when the key is absent or NULL).
pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>, ApiError> {
    let value: Option<Option<String>> = conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(value.flatten())
}

/// Writes a `meta` value; `None` stores NULL.
pub fn meta_set(conn: &Connection, key: &str, value: Option<&str>) -> Result<(), ApiError> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Reads an integer `meta` value (`None` when absent or not a number).
pub fn meta_get_i64(conn: &Connection, key: &str) -> Result<Option<i64>, ApiError> {
    Ok(meta_get(conn, key)?.and_then(|value| value.parse().ok()))
}

/// Upgrades v1 rows in place so the v2 columns are populated before the first
/// v2 import: sheet order falls back to id order, the derived columns are
/// computed from the stored text, dates stored as 0 become NULL, the download
/// state is derived from the legacy `downloaded` flag and the media on disk,
/// and the id high-water marks start above the existing ids.
fn backfill_v2(conn: &Connection, covers_dir: &Path, songs_dir: &Path) -> Result<(), ApiError> {
    let merged = repair::merge_duplicate_eras(conn)?;
    if merged > 0 {
        info!(merged, "merged duplicate era rows before keying eras");
    }

    // Eras: main eras first in id (= first-seen sheet) order.
    let eras: Vec<(i64, Option<String>)> = {
        let mut statement =
            conn.prepare("SELECT id, name FROM eras ORDER BY is_main DESC, id ASC")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut era_info = std::collections::HashMap::new();
    {
        let mut update = conn
            .prepare("UPDATE eras SET key = ?1, position = ?2, cover_version = ?3 WHERE id = ?4")?;
        for (index, (id, name)) in eras.iter().enumerate() {
            let name = name.as_deref().map(text::clean_line).unwrap_or_default();
            let key = (!name.is_empty()).then(|| importer::era_key(&name));
            let cover = covers_dir.join(format!("{id}.avif"));
            let version = cover_version::cover_version_of_file(&cover).unwrap_or(None);
            update.execute(params![key, index as i64 + 1, version, id])?;
            era_info.insert(*id, (name, key));
        }
    }

    // Songs: sheet order is not known, id order is the closest (v1 assigned
    // ids in sheet order).
    struct V1Song {
        id: i64,
        era: Option<i64>,
        name: String,
        notes: Option<String>,
        url: Option<String>,
        track_length: Option<i64>,
        quality: Option<String>,
        available_length: Option<String>,
    }
    let songs: Vec<V1Song> = {
        let mut statement = conn.prepare(
            "SELECT id, era, name, notes, url, track_length, quality, available_length \
             FROM songs ORDER BY id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(V1Song {
                id: row.get(0)?,
                era: row.get(1)?,
                name: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                notes: row.get(3)?,
                url: row.get(4)?,
                track_length: row.get(5)?,
                quality: row.get(6)?,
                available_length: row.get(7)?,
            })
        })?;
        rows.collect::<Result<_, _>>()?
    };
    {
        let mut era_counters: std::collections::HashMap<Option<i64>, i64> =
            std::collections::HashMap::new();
        let mut update = conn.prepare(
            "UPDATE songs SET position = ?1, era_position = ?2, title = ?3, search_text = ?4, \
             sort_title = ?5, category_rank = ?6, song_key = ?7, links = ?8, notes_links = '[]', \
             file_date = NULLIF(file_date, 0), leak_date = NULLIF(leak_date, 0), \
             file_date_precision = CASE WHEN file_date IS NULL OR file_date = 0 THEN NULL ELSE 'day' END, \
             leak_date_precision = CASE WHEN leak_date IS NULL OR leak_date = 0 THEN NULL ELSE 'day' END \
             WHERE id = ?9",
        )?;
        for song in &songs {
            let era_position = era_counters.entry(song.era).or_insert(0);
            *era_position += 1;
            let (era_name, era_key) = song
                .era
                .and_then(|era| era_info.get(&era))
                .map(|(name, key)| (Some(name.as_str()), key.as_deref()))
                .unwrap_or((None, None));
            let search_text = search_text::search_text_for(&SearchFields {
                name: &song.name,
                notes: song.notes.as_deref(),
                era_name,
                era_subtitle: None,
                sub_era: None,
                quality: song.quality.as_deref(),
                available_length: song.available_length.as_deref(),
            });
            let song_key = importer::song_key(
                era_key.unwrap_or_default(),
                &song.name,
                song.notes.as_deref(),
                song.url.as_deref(),
                song.track_length,
            );
            let links = serde_json::to_string(&song.url.iter().collect::<Vec<_>>())
                .expect("a list of strings serialises");
            update.execute(params![
                song.id,
                *era_position,
                text::first_line(&song.name),
                search_text,
                search_text::sort_title(&song.name),
                search_text::category_rank(&song.name),
                song_key,
                links,
                song.id,
            ])?;
        }
    }

    // Files: `downloaded = 1` means downloaded only when the file is still on
    // disk; a filename that is not on disk is dropped so it reads as "not
    // downloaded yet".
    let files: Vec<(String, Option<i64>, Option<String>)> = {
        let mut statement =
            conn.prepare("SELECT url, downloaded, filename FROM files WHERE url IS NOT NULL")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let now = unix_now();
    {
        let mut update = conn.prepare(
            "UPDATE files SET status = ?1, filename = ?2, downloaded = ?3, last_seen_at = ?4 \
             WHERE url = ?5",
        )?;
        for (url, downloaded, filename) in &files {
            let on_disk = filename
                .as_deref()
                .filter(|name| is_safe_filename(name) && *name != "." && *name != "..")
                .filter(|name| {
                    std::fs::metadata(songs_dir.join(name))
                        .map(|metadata| metadata.is_file() && metadata.len() > 0)
                        .unwrap_or(false)
                });
            let was_downloaded = *downloaded == Some(1);
            let status = if was_downloaded && on_disk.is_some() {
                "downloaded"
            } else {
                "pending"
            };
            // A "downloaded" row whose file vanished must be fetched again.
            let legacy = if was_downloaded && on_disk.is_none() {
                Some(0)
            } else {
                *downloaded
            };
            update.execute(params![status, on_disk, legacy, now, url])?;
        }
    }

    let max_song_id: i64 = conn.query_row("SELECT coalesce(max(id), 0) FROM songs", [], |row| {
        row.get(0)
    })?;
    let max_era_id: i64 = conn.query_row("SELECT coalesce(max(id), 0) FROM eras", [], |row| {
        row.get(0)
    })?;
    raise_high_water_mark(conn, meta_keys::NEXT_SONG_ID, max_song_id + 1)?;
    raise_high_water_mark(conn, meta_keys::NEXT_ERA_ID, max_era_id + 1)?;

    if !songs.is_empty() || !eras.is_empty() {
        info!(
            eras = eras.len(),
            songs = songs.len(),
            files = files.len(),
            "upgraded the catalog to schema v2"
        );
    }
    Ok(())
}

/// Fills `songs.song_search_text` where it is missing (rows written before
/// the column existed), from the stored song fields.
fn backfill_song_search_text(conn: &Connection) -> Result<(), ApiError> {
    let missing: i64 = conn.query_row(
        "SELECT count(*) FROM songs WHERE song_search_text IS NULL",
        [],
        |row| row.get(0),
    )?;
    if missing == 0 {
        return Ok(());
    }
    type Row = (
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    {
        let rows: Vec<Row> = {
            let mut statement = transaction.prepare(
                "SELECT id, name, notes, sub_era, quality, available_length FROM songs \
                 WHERE song_search_text IS NULL",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })?;
            rows.collect::<Result<_, _>>()?
        };
        let mut update =
            transaction.prepare("UPDATE songs SET song_search_text = ?1 WHERE id = ?2")?;
        for (id, name, notes, sub_era, quality, available_length) in &rows {
            let text = search_text::song_search_text_for(&SearchFields {
                name: name.as_deref().unwrap_or_default(),
                notes: notes.as_deref(),
                sub_era: sub_era.as_deref(),
                quality: quality.as_deref(),
                available_length: available_length.as_deref(),
                ..SearchFields::default()
            });
            update.execute(params![text, id])?;
        }
    }
    transaction.commit()?;
    info!(songs = missing, "filled in the era-independent search text");
    Ok(())
}

/// Sets an id high-water mark to at least `value`.
pub fn raise_high_water_mark(conn: &Connection, key: &str, value: i64) -> Result<(), ApiError> {
    match meta_get_i64(conn, key)? {
        Some(current) if current >= value => Ok(()),
        _ => meta_set(conn, key, Some(&value.to_string())),
    }
}

/// Runs a blocking SQLite closure on the blocking pool.
pub async fn call<T, F>(pool: &Pool, f: F) -> Result<T, ApiError>
where
    F: FnOnce(&Connection) -> Result<T, ApiError> + Send + 'static,
    T: Send + 'static,
{
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(ApiError::unexpected)?;
        f(&conn)
    })
    .await
    .map_err(ApiError::unexpected)?
}
