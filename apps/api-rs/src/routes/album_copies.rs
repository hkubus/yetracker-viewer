//! `GET /album-copies`: groups unreleased-tracking album copies by name.

use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value as JsonValue};

use super::{js_float, json_cached};
use crate::db;
use crate::error::ApiError;
use crate::state::SharedState;
use crate::text;

struct CopySong {
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
    era_name: Option<String>,
    era_image_url: Option<String>,
    filename: Option<String>,
    file_duration: Option<f64>,
}

pub async fn list_album_copies(State(state): State<SharedState>) -> Result<Response, ApiError> {
    let songs = db::call(&state.pool, |conn| {
        let mut statement = conn.prepare(
            "SELECT songs.id, songs.era, songs.catalog_id, songs.name, songs.notes, songs.file_date, \
             songs.leak_date, songs.available_length, songs.track_length, songs.quality, songs.url, \
             eras.name, eras.image_url, files.filename, files.duration \
             FROM songs LEFT JOIN eras ON songs.era = eras.id LEFT JOIN files ON songs.url = files.url \
             WHERE songs.catalog_id = 'album-copies' ORDER BY songs.id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(CopySong {
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
                era_name: row.get(11)?,
                era_image_url: row.get(12)?,
                filename: row.get(13)?,
                file_duration: row.get(14)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await?;

    let mut groups: Vec<(String, String, Vec<JsonValue>)> = Vec::new();
    for song in &songs {
        let display_name = match song.name.as_deref() {
            Some(name) if !text::trim(name).is_empty() => text::trim(name).to_string(),
            _ => "Untitled album copy".to_string(),
        };
        let key = text::normalize(&display_name);

        let playable = state.playable.is_playable(song.filename.as_deref());
        let duration = if playable {
            song.file_duration.map(js_float).unwrap_or(JsonValue::Null)
        } else {
            JsonValue::Null
        };
        let copy = json!({
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
            "eraName": song.era_name,
            "coverVersion": state.cover_versions.get(song.era_image_url.as_deref()),
            "playable": playable,
            "duration": duration,
        });

        match groups.iter_mut().find(|(existing, _, _)| *existing == key) {
            Some((_, _, copies)) => copies.push(copy),
            None => groups.push((key, display_name, vec![copy])),
        }
    }

    let body: Vec<JsonValue> = groups
        .into_iter()
        .map(|(_, name, copies)| json!({ "name": name, "copies": copies }))
        .collect();
    Ok(json_cached(JsonValue::Array(body)))
}
