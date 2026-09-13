//! `/eras`, `/eras/{id}`, `/eras/{id}/songs`, `/eras/{id}/cover`.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use serde_json::json;

use super::{COVER_CACHE, empty, json_cached, set_header, songs};
use crate::db;
use crate::error::ApiError;
use crate::playable::mtime_ms_of;
use crate::request::positive_integer;
use crate::serve::file_body;
use crate::state::SharedState;

pub async fn list_eras(State(state): State<SharedState>) -> Result<Response, ApiError> {
    let rows = db::call(&state.pool, |conn| {
        let mut statement = conn.prepare(
            "SELECT eras.id, eras.name, eras.notes, eras.description, eras.dominant_color, eras.image_url, \
             count(songs.id) AS songsCount \
             FROM eras LEFT JOIN songs ON (eras.id = songs.era) AND (songs.catalog_id = 'unreleased') \
             WHERE eras.is_main = 1 GROUP BY eras.id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await?;

    let eras: Vec<serde_json::Value> = rows
        .into_iter()
        .map(
            |(id, name, notes, description, dominant_color, image_url, songs_count)| {
                json!({
                    "id": id,
                    "name": name,
                    "notes": notes,
                    "description": description,
                    "dominantColor": dominant_color,
                    "songsCount": songs_count,
                    "coverVersion": state.cover_versions.get(image_url.as_deref()),
                })
            },
        )
        .collect();

    Ok(json_cached(serde_json::Value::Array(eras)))
}

pub async fn get_era(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = positive_integer(Some(&id), "era id")?;

    let row = db::call(&state.pool, move |conn| {
        let mut statement = conn.prepare(
            "SELECT eras.id, eras.name, eras.notes, eras.description, eras.dominant_color, eras.image_url \
             FROM eras WHERE eras.id = ?1 LIMIT 1",
        )?;
        let mut rows = statement.query_map([id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        match rows.next() {
            Some(row) => row.map(Some).map_err(ApiError::from),
            None => Ok(None),
        }
    })
    .await?;

    let Some((id, name, notes, description, dominant_color, image_url)) = row else {
        return Err(ApiError::not_found("Era does not exist"));
    };

    Ok(json_cached(json!({
        "id": id,
        "name": name,
        "notes": notes,
        "description": description,
        "dominantColor": dominant_color,
        "coverVersion": state.cover_versions.get(image_url.as_deref()),
    })))
}

pub async fn list_era_songs(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let id = positive_integer(Some(&id), "era id")?;

    let exists = db::call(&state.pool, move |conn| {
        let mut statement = conn.prepare("SELECT eras.id FROM eras WHERE eras.id = ?1 LIMIT 1")?;
        let mut rows = statement.query([id])?;
        Ok(rows.next()?.is_some())
    })
    .await?;
    if !exists {
        return Err(ApiError::not_found("Era does not exist"));
    }

    songs::paginated_songs(&state, id, &params).await
}

pub async fn get_era_cover(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let id = positive_integer(Some(&id), "era id")?;
    let path = state
        .config
        .storage_path
        .join("covers")
        .join(format!("{id}.avif"));

    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::not_found("Cover not found"));
        }
        Err(error) => {
            eprintln!("failed to read cover {error}");
            return Err(ApiError::internal("Could not load cover"));
        }
    };
    if !metadata.is_file() {
        return Err(ApiError::not_found("Cover not found"));
    }

    // The era cover ETag is derived from the era id, not the stored image URL.
    let etag = format!(
        "\"{}-{:x}-{:x}\"",
        state.cover_versions.get(Some(&id.to_string())),
        metadata.len(),
        mtime_ms_of(&metadata)
    );

    let mut response = empty(StatusCode::OK);
    set_header(&mut response, header::CACHE_CONTROL, COVER_CACHE);
    set_header(&mut response, header::ETAG, &etag);
    if let Ok(modified) = metadata.modified() {
        set_header(
            &mut response,
            header::LAST_MODIFIED,
            &httpdate::fmt_http_date(modified),
        );
    }

    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(etag.as_str())
    {
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        return Ok(response);
    }

    set_header(&mut response, header::CONTENT_TYPE, "image/avif");
    set_header(
        &mut response,
        header::CONTENT_LENGTH,
        &metadata.len().to_string(),
    );
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));

    let body = file_body(&path, None).await.map_err(|error| {
        eprintln!("failed to read cover {error}");
        ApiError::internal("Could not load cover")
    })?;
    *response.body_mut() = body;
    Ok(response)
}
