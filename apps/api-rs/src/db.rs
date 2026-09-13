//! SQLite pool, PRAGMAs and the startup DDL, mirroring `db/client.ts` and the
//! DDL block in `index.ts` statement-for-statement.

use std::path::Path;

use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;

use crate::error::ApiError;

pub type Pool = r2d2::Pool<SqliteConnectionManager>;

const PRAGMAS: &str = "\
PRAGMA busy_timeout = 5000;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA cache_size = -64000;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 67108864;
PRAGMA journal_size_limit = 67108864;";

const DDL: &str = "\
CREATE TABLE IF NOT EXISTS eras (
  id INTEGER PRIMARY KEY,
  name TEXT,
  notes TEXT,
  image_url TEXT,
  description TEXT,
  dominant_color TEXT,
  cover_source TEXT,
  is_main INTEGER NOT NULL DEFAULT 1
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
  url TEXT
);
CREATE TABLE IF NOT EXISTS files (
  url TEXT PRIMARY KEY,
  downloaded INTEGER,
  filename TEXT,
  duration REAL
);
CREATE INDEX IF NOT EXISTS songs_era_index ON songs (era);
CREATE INDEX IF NOT EXISTS songs_url_index ON songs (url);
CREATE INDEX IF NOT EXISTS songs_catalog_era_id_index ON songs (catalog_id, era, id);
CREATE INDEX IF NOT EXISTS songs_catalog_id_index ON songs (catalog_id, id);
CREATE INDEX IF NOT EXISTS songs_quality_index ON songs (quality);
CREATE INDEX IF NOT EXISTS songs_available_length_index ON songs (available_length);
CREATE INDEX IF NOT EXISTS eras_is_main_index ON eras (is_main);
CREATE INDEX IF NOT EXISTS songs_catalog_index ON songs (catalog_id);
UPDATE eras SET dominant_color = '666666' WHERE dominant_color IS NULL OR trim(dominant_color) = '';";

pub fn create_pool(path: &Path) -> Result<Pool, ApiError> {
    let manager = SqliteConnectionManager::file(path).with_init(|conn: &mut Connection| {
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch(PRAGMAS)
    });
    r2d2::Pool::builder()
        .max_size(8)
        .build(manager)
        .map_err(ApiError::unexpected)
}

/// Runs the startup DDL and the conditional column migrations.
pub fn run_migrations(pool: &Pool) -> Result<(), ApiError> {
    let conn = pool.get().map_err(ApiError::unexpected)?;
    conn.execute_batch(DDL)?;

    if !has_column(&conn, "files", "duration")? {
        conn.execute_batch("ALTER TABLE files ADD COLUMN duration REAL")?;
    }
    if !has_column(&conn, "eras", "is_main")? {
        conn.execute_batch("ALTER TABLE eras ADD COLUMN is_main INTEGER NOT NULL DEFAULT 1")?;
    }
    if !has_column(&conn, "eras", "cover_source")? {
        conn.execute_batch("ALTER TABLE eras ADD COLUMN cover_source TEXT")?;
    }
    if !has_column(&conn, "songs", "catalog_id")? {
        conn.execute_batch(
            "ALTER TABLE songs ADD COLUMN catalog_id TEXT NOT NULL DEFAULT 'unreleased'",
        )?;
    }
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
