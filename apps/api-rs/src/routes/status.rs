//! `/health` (liveness plus a trivial database query) and `/status` (catalog
//! freshness and counts for the home page).

use std::time::Duration;

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde_json::json;

use super::songs::DOWNLOADED;
use super::{NO_STORE, json_cached, json_response, set_header};
use crate::db::{self, meta_keys};
use crate::error::ApiError;
use crate::state::SharedState;

/// How long `/health` waits for a pooled connection and its query before
/// reporting the database as failing.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);

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

/// Catalog status: the last successful import (`lastImportAt`, Unix
/// seconds), whether the latest attempt succeeded (with a generic error
/// message when it did not, see [`PUBLIC_IMPORT_ERROR`]), and the
/// era, song and playable-song counts.
pub async fn status(State(state): State<SharedState>) -> Result<Response, ApiError> {
    let songs_path = state.config.songs_path.clone();
    let shared = state.clone();
    let body = db::call(&state.pool, move |conn| {
        let last_import_at = db::meta_get_i64(conn, meta_keys::LAST_IMPORT_AT)?;
        let last_import_ok =
            db::meta_get(conn, meta_keys::LAST_IMPORT_OK)?.and_then(|value| match value.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            });
        let last_import_error =
            db::meta_get(conn, meta_keys::LAST_IMPORT_ERROR)?.map(|_| PUBLIC_IMPORT_ERROR);
        let eras: i64 = conn
            .prepare_cached("SELECT count(*) FROM eras WHERE is_main = 1")?
            .query_row([], |row| row.get(0))?;
        let songs: i64 = conn
            .prepare_cached("SELECT count(*) FROM songs WHERE catalog_id = 'unreleased'")?
            .query_row([], |row| row.get(0))?;
        // Only songs whose file has been downloaded can be playable; the disk
        // decides which of those still are.
        let sql = format!(
            "SELECT (SELECT filename FROM files WHERE url = s.url) FROM songs s \
             WHERE s.catalog_id = 'unreleased' AND {DOWNLOADED}"
        );
        let mut statement = conn.prepare_cached(&sql)?;
        let filenames = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let playable_songs = filenames
            .iter()
            .filter(|filename| {
                shared
                    .playable
                    .resolve_playable_blocking(&songs_path, Some(filename))
            })
            .count();
        Ok(json!({
            "status": "ok",
            "lastImportAt": last_import_at,
            "lastImportOk": last_import_ok,
            "lastImportError": last_import_error,
            "eras": eras,
            "songs": songs,
            "playableSongs": playable_songs,
        }))
    })
    .await?;
    Ok(json_cached(body))
}
