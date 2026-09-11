//! `/categories`, `/categories/{id}` and `/categories/{id}/songs`.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::response::Response;
use serde_json::json;

use super::{json_cached, songs};
use crate::catalogs::{catalog_source_url, get_catalog, get_category_catalogs, PRIMARY_CATALOG_ID};
use crate::db;
use crate::error::ApiError;
use crate::routes::songs::SongScope;
use crate::state::SharedState;

pub async fn list_categories(State(state): State<SharedState>) -> Result<Response, ApiError> {
    let counts = db::call(&state.pool, |conn| {
        let mut statement = conn.prepare("SELECT catalog_id, count(id) FROM songs GROUP BY catalog_id")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await?;

    let counts_by_catalog: HashMap<String, i64> = counts
        .into_iter()
        .filter_map(|(catalog_id, count)| catalog_id.map(|catalog_id| (catalog_id, count)))
        .collect();

    let categories: Vec<serde_json::Value> = get_category_catalogs()
        .map(|catalog| {
            json!({
                "id": catalog.id,
                "name": catalog.name,
                "description": catalog.description,
                "songsCount": counts_by_catalog.get(catalog.id).copied().unwrap_or(0),
                "sourceUrl": catalog_source_url(catalog.gid),
            })
        })
        .collect();

    Ok(json_cached(serde_json::Value::Array(categories)))
}

fn ensure_category(id: &str) -> Result<(), ApiError> {
    match get_catalog(id) {
        Some(catalog) if catalog.id != PRIMARY_CATALOG_ID => Ok(()),
        _ => Err(ApiError::not_found("Category does not exist")),
    }
}

pub async fn get_category(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    ensure_category(&id)?;

    let catalog_id = id.clone();
    let count = db::call(&state.pool, move |conn| {
        let mut statement = conn.prepare("SELECT count(id) FROM songs WHERE catalog_id = ?1")?;
        let count: i64 = statement.query_row([catalog_id], |row| row.get(0))?;
        Ok(count)
    })
    .await?;

    let catalog = get_catalog(&id).expect("category validated");
    Ok(json_cached(json!({
        "id": catalog.id,
        "name": catalog.name,
        "description": catalog.description,
        "songsCount": count,
        "sourceUrl": catalog_source_url(catalog.gid),
    })))
}

pub async fn list_category_songs(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    ensure_category(&id)?;
    songs::paginated_songs(&state, SongScope::Catalog(id), &params).await
}
