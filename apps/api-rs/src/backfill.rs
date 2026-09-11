//! Duration backfill for downloaded files, ported from
//! `util/backfillDurations.ts`.

use futures_util::stream::{self, StreamExt};

use crate::db;
use crate::error::ApiError;
use crate::media::{delete_invalid_file, get_duration, InvalidReason, ProbeError};
use crate::playable::{mtime_ms_of, stored_song_path};
use crate::state::AppState;

fn backfill_concurrency() -> usize {
    std::env::var("BACKFILL_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(8)
}

async fn reset_row(state: &AppState, url: &str) -> Result<(), ApiError> {
    let url = url.to_string();
    db::call(&state.pool, move |conn| {
        conn.execute(
            "UPDATE files SET downloaded = 0, duration = NULL WHERE url = ?1",
            [url],
        )?;
        Ok(())
    })
    .await
}

/// Probes one downloaded file and caches its duration, deleting files that are
/// missing, empty or unreadable so the downloader retries them.
pub async fn cache_file_duration(state: &AppState, url: &str, filename: &str) -> Result<(), ApiError> {
    let path = match stored_song_path(&state.config.songs_path, filename) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("backfill: unsafe stored filename for {url} ({filename})");
            return Err(error);
        }
    };

    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("backfill: missing file for {url} ({filename}), resetting for re-download {error}");
            state.playable.set_playable(filename, false);
            return reset_row(state, url).await;
        }
    };
    if !metadata.is_file() || metadata.len() == 0 {
        eprintln!("backfill: empty file for {url} ({filename}), deleting for re-download");
        delete_invalid_file(state, filename, Some(url), InvalidReason::Empty).await;
        return Ok(());
    }

    match get_duration(state, &path, Some(mtime_ms_of(&metadata))).await {
        Err(ProbeError::BinaryMissing) => {
            // Fail open when ffprobe itself is unavailable: never mass-delete.
            eprintln!("backfill: ffprobe unavailable, skipping {url} ({filename})");
        }
        Err(_) => {
            eprintln!("backfill: unreadable file for {url} ({filename}), deleting for re-download");
            delete_invalid_file(state, filename, Some(url), InvalidReason::Unreadable).await;
        }
        Ok(None) => {
            eprintln!("backfill: unprobable file for {url} ({filename}), deleting for re-download");
            delete_invalid_file(state, filename, Some(url), InvalidReason::NoDuration).await;
        }
        Ok(Some(duration)) => {
            let url = url.to_string();
            db::call(&state.pool, move |conn| {
                conn.execute("UPDATE files SET duration = ?1 WHERE url = ?2", rusqlite::params![duration, url])?;
                Ok(())
            })
            .await?;
        }
    }
    Ok(())
}

pub async fn backfill_durations(state: &AppState) -> Result<(), ApiError> {
    let files = db::call(&state.pool, |conn| {
        let mut statement = conn.prepare(
            "SELECT url, filename FROM files \
             WHERE downloaded = 1 AND filename IS NOT NULL AND duration IS NULL",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await?;

    if files.is_empty() {
        return Ok(());
    }
    println!("backfilling duration of {} files", files.len());

    stream::iter(files)
        .for_each_concurrent(backfill_concurrency(), |(url, filename)| async move {
            let Some(filename) = filename else { return };
            if let Err(error) = cache_file_duration(state, &url, &filename).await {
                eprintln!("backfill failed for {url} ({filename}) {error:?}");
            }
        })
        .await;
    Ok(())
}
