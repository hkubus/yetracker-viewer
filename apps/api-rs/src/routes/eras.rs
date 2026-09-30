//! `/eras`, `/eras/{id}`, `/eras/{id}/songs`. The cover image route lives in
//! `media.rs`.

use axum::extract::{RawQuery, State};
use axum::response::Response;
use rusqlite::Row;
use serde_json::{Value, json};

use super::songs::{dominant_color, era_songs};
use super::{EraId, Params, json_cached};
use crate::db;
use crate::error::ApiError;
use crate::request::ERA_SONG_PARAMS;
use crate::state::SharedState;

/// Columns of an era payload over `eras e`, in the order [`era_json`] reads.
const ERA_COLUMNS: &str = "e.id, coalesce(e.position, e.id), coalesce(e.name, ''), e.subtitle, \
     coalesce(e.notes, ''), coalesce(e.description, ''), e.dominant_color, e.cover_version, \
     (SELECT count(*) FROM songs s WHERE s.catalog_id = 'unreleased' AND s.era = e.id)";

/// The contract's `Era` object.
fn era_json(row: &Row<'_>) -> rusqlite::Result<Value> {
    let cover_version = row
        .get::<_, Option<String>>(7)?
        .filter(|version| !version.is_empty());
    Ok(json!({
        "id": row.get::<_, i64>(0)?,
        "position": row.get::<_, i64>(1)?,
        "name": row.get::<_, String>(2)?,
        "subtitle": row.get::<_, Option<String>>(3)?,
        "notes": row.get::<_, String>(4)?,
        "description": row.get::<_, String>(5)?,
        "dominantColor": dominant_color(row.get::<_, Option<String>>(6)?.as_deref()),
        "hasCover": cover_version.is_some(),
        "coverVersion": cover_version,
        "songsCount": row.get::<_, i64>(8)?,
    }))
}

/// The main eras in catalog order.
pub async fn list_eras(State(state): State<SharedState>) -> Result<Response, ApiError> {
    let eras = db::call(&state.pool, |conn| {
        let sql = format!(
            "SELECT {ERA_COLUMNS} FROM eras e WHERE e.is_main = 1 ORDER BY e.position, e.id"
        );
        let mut statement = conn.prepare_cached(&sql)?;
        let rows = statement.query_map([], era_json)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    })
    .await?;
    Ok(json_cached(Value::Array(eras)))
}

pub async fn get_era(
    State(state): State<SharedState>,
    EraId(id): EraId,
) -> Result<Response, ApiError> {
    let era = db::call(&state.pool, move |conn| {
        let sql = format!("SELECT {ERA_COLUMNS} FROM eras e WHERE e.id = ?1");
        let mut statement = conn.prepare_cached(&sql)?;
        let mut rows = statement.query_map([id], era_json)?;
        Ok(rows.next().transpose()?)
    })
    .await?;
    era.map(json_cached)
        .ok_or_else(|| ApiError::not_found("Era does not exist"))
}

pub async fn list_era_songs(
    State(state): State<SharedState>,
    era: EraId,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let params = Params::parse(query.as_deref(), ERA_SONG_PARAMS)?;
    era_songs(state, era, &params).await
}
