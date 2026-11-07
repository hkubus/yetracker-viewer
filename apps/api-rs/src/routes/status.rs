//! `/health` (liveness plus a trivial database query) and `/status` (catalog
//! freshness and counts for the home page).

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde_json::json;

use super::{NO_STORE, json_cached, json_response, set_header};
use crate::db::{self, meta_keys};
use crate::error::ApiError;
use crate::state::SharedState;

/// How long `/health` waits for a pooled connection and its query before
/// reporting the database as failing.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);

/// How long one read of the catalog's side of `/status` (the import state,
/// the counts and every downloaded song's filename: a scan of the whole
/// catalog) answers every request. Which of those files are playable is
/// checked on each request (in memory), so a file found missing drops out at
/// once.
const STATUS_TTL: Duration = Duration::from_secs(5);

/// What `/status` says about a failed import. The recorded error can name
/// storage paths, SQLite errors or upstream URLs, so it stays in the logs and
/// the database (`meta.last_import_error`) and is never served publicly.
const PUBLIC_IMPORT_ERROR: &str = "Catalog update failed";

/// `{"status":"ok"}` once `SELECT 1` succeeds, else 503 `{"status":"error"}`
/// (also when no connection frees up within [`HEALTH_TIMEOUT`]).
pub async fn health(State(state): State<SharedState>) -> Response {
    let pool = state.pool.clone();
    let check = tokio::task::spawn_blocking(move || {
        let conn = pool.get_timeout(HEALTH_TIMEOUT).ok()?;
        conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .ok()
    });
    let healthy = matches!(
        tokio::time::timeout(HEALTH_TIMEOUT, check).await,
        Ok(Ok(Some(1)))
    );
    let mut response = json_response(json!({ "status": if healthy { "ok" } else { "error" } }));
    if !healthy {
        *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
    }
    set_header(&mut response, header::CACHE_CONTROL, NO_STORE);
    response
}

/// The database's side of `/status`, shared for [`STATUS_TTL`].
pub struct StatusSnapshot {
    last_import_at: Option<i64>,
    last_import_ok: Option<bool>,
    last_import_failed: bool,
    eras: i64,
    songs: i64,
    /// The files of the songs that have been downloaded.
    filenames: Arc<Vec<String>>,
}

/// Catalog status: the last successful import (`lastImportAt`, Unix
/// seconds), whether the latest attempt succeeded (with a generic error
/// message when it did not, see [`PUBLIC_IMPORT_ERROR`]), and the
/// era, song and playable-song counts.
pub async fn status(State(state): State<SharedState>) -> Result<Response, ApiError> {
    let snapshot = {
        // Concurrent requests wait for the one reading the database.
        let mut cached = state.status_cache.lock().await;
        match cached.as_ref() {
            Some((read_at, snapshot)) if read_at.elapsed() < STATUS_TTL => snapshot.clone(),
            _ => {
                let snapshot = Arc::new(read_snapshot(&state).await?);
                *cached = Some((Instant::now(), snapshot.clone()));
                snapshot
            }
        }
    };
    let shared = state.clone();
    let filenames = snapshot.filenames.clone();
    // Only songs whose file has been downloaded can be playable; the disk
    // (through the playable set) decides which of those still are.
    let playable_songs = tokio::task::spawn_blocking(move || {
        filenames
            .iter()
            .filter(|filename| {
                shared
                    .playable
                    .resolve_playable_blocking(&shared.config.songs_path, Some(filename))
            })
            .count()
    })
    .await
    .map_err(ApiError::unexpected)?;
    Ok(json_cached(json!({
        "status": "ok",
        "lastImportAt": snapshot.last_import_at,
        "lastImportOk": snapshot.last_import_ok,
        "lastImportError": snapshot.last_import_failed.then_some(PUBLIC_IMPORT_ERROR),
        "eras": snapshot.eras,
        "songs": snapshot.songs,
        "playableSongs": playable_songs,
    })))
}

async fn read_snapshot(state: &SharedState) -> Result<StatusSnapshot, ApiError> {
    db::call(&state.pool, move |conn| {
        let last_import_ok =
            db::meta_get(conn, meta_keys::LAST_IMPORT_OK)?.and_then(|value| match value.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            });
        let mut statement = conn.prepare_cached(
            "SELECT f.filename FROM songs s JOIN files f ON f.url = s.url \
             WHERE s.catalog_id = 'unreleased' AND f.filename IS NOT NULL",
        )?;
        let filenames = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StatusSnapshot {
            last_import_at: db::meta_get_i64(conn, meta_keys::LAST_IMPORT_AT)?,
            last_import_ok,
            last_import_failed: db::meta_get(conn, meta_keys::LAST_IMPORT_ERROR)?.is_some(),
            eras: conn
                .prepare_cached("SELECT count(*) FROM eras WHERE is_main = 1")?
                .query_row([], |row| row.get(0))?,
            songs: conn
                .prepare_cached("SELECT count(*) FROM songs WHERE catalog_id = 'unreleased'")?
                .query_row([], |row| row.get(0))?,
            filenames: Arc::new(filenames),
        })
    })
    .await
}
