//! Duration backfill for downloaded files, ported from
//! `util/backfillDurations.ts`.

use futures_util::stream::{self, StreamExt};

use crate::db;
use crate::error::ApiError;
use crate::media::{InvalidReason, ProbeError, delete_invalid_file, get_duration};
use crate::playable::{mtime_ms_of, stored_song_path};
use crate::state::AppState;

fn backfill_concurrency_from(raw: Option<&str>) -> usize {
    // Mirrors `Number(process.env.BACKFILL_CONCURRENCY ?? 8) || 8`: missing,
    // empty, zero and unparsable values fall back to 8. Negative values become
    // 0, matching `Array.from({ length })`'s ToLength coercion.
    let parsed = match raw.map(str::trim) {
        None | Some("") => 8.0,
        Some(value) => value.parse::<f64>().unwrap_or(f64::NAN),
    };
    let effective = if parsed == 0.0 || parsed.is_nan() {
        8.0
    } else {
        parsed
    };
    if effective < 1.0 {
        0
    } else {
        effective.floor() as usize
    }
}

fn backfill_concurrency() -> usize {
    backfill_concurrency_from(std::env::var("BACKFILL_CONCURRENCY").ok().as_deref())
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
pub async fn cache_file_duration(
    state: &AppState,
    url: &str,
    filename: &str,
) -> Result<(), ApiError> {
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
            eprintln!(
                "backfill: missing file for {url} ({filename}), resetting for re-download {error}"
            );
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
                conn.execute(
                    "UPDATE files SET duration = ?1 WHERE url = ?2",
                    rusqlite::params![duration, url],
                )?;
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

    let concurrency = backfill_concurrency().min(files.len());
    if concurrency == 0 {
        return Ok(());
    }
    stream::iter(files)
        .for_each_concurrent(concurrency, |(url, filename)| async move {
            let Some(filename) = filename else { return };
            if let Err(error) = cache_file_duration(state, &url, &filename).await {
                eprintln!("backfill failed for {url} ({filename}) {error:?}");
            }
        })
        .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrency_matches_node_number_or_fallback() {
        assert_eq!(backfill_concurrency_from(None), 8);
        assert_eq!(backfill_concurrency_from(Some("")), 8);
        assert_eq!(backfill_concurrency_from(Some("0")), 8);
        assert_eq!(backfill_concurrency_from(Some("abc")), 8);
        assert_eq!(backfill_concurrency_from(Some("4")), 4);
        assert_eq!(backfill_concurrency_from(Some("3.9")), 3);
        assert_eq!(backfill_concurrency_from(Some("-3")), 0);
    }
}
