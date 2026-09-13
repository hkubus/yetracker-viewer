//! ffprobe/ffmpeg plumbing: duration probing with an LRU keyed by path+mtime,
//! validity probing and invalid-file deletion. Ports `util/getDuration.ts` and
//! `util/invalidFiles.ts`.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use tokio::process::Command;

use crate::db;
use crate::playable::{is_safe_filename, mtime_ms_of};
use crate::state::{AppState, DurationEntry};

const FFPROBE_TIMEOUT: Duration = Duration::from_secs(10);
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeError {
    /// `spawn ENOENT` — the ffprobe binary itself is unavailable (fail open).
    BinaryMissing,
    Timeout,
    /// ffprobe ran and exited non-zero (Node's numeric `error.code`).
    Exit(i32),
    Other(String),
}

pub type DurationFuture = Shared<BoxFuture<'static, Result<Option<f64>, ProbeError>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidReason {
    Missing,
    Empty,
    NoDuration,
    NoAudioStream,
    Unreadable,
    UnsafeName,
}

impl InvalidReason {
    pub fn as_str(self) -> &'static str {
        match self {
            InvalidReason::Missing => "missing",
            InvalidReason::Empty => "empty",
            InvalidReason::NoDuration => "no-duration",
            InvalidReason::NoAudioStream => "no-audio-stream",
            InvalidReason::Unreadable => "unreadable",
            InvalidReason::UnsafeName => "unsafe-name",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProbeOutcome {
    pub valid: bool,
    pub reason: Option<InvalidReason>,
    pub mtime_ms: Option<i64>,
}

async fn ffprobe_duration(path: &Path) -> Result<String, ProbeError> {
    let mut command = Command::new("ffprobe");
    command
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    match tokio::time::timeout(FFPROBE_TIMEOUT, command.output()).await {
        Err(_) => Err(ProbeError::Timeout),
        Ok(Err(error)) => {
            if error.kind() == std::io::ErrorKind::NotFound {
                Err(ProbeError::BinaryMissing)
            } else {
                Err(ProbeError::Other(error.to_string()))
            }
        }
        Ok(Ok(output)) => {
            if !output.status.success() {
                return Err(ProbeError::Exit(output.status.code().unwrap_or(-1)));
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
    }
}

async fn has_audio_stream(path: &Path) -> Option<bool> {
    let mut command = Command::new("ffprobe");
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    match tokio::time::timeout(FFPROBE_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => Some(!String::from_utf8_lossy(&output.stdout).trim().is_empty()),
        Ok(Err(error)) => {
            if error.kind() == std::io::ErrorKind::NotFound {
                None
            } else {
                Some(false)
            }
        }
        Err(_) => Some(false),
    }
}

async fn stat_mtime_ms(path: &Path) -> Option<i64> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    Some(mtime_ms_of(&metadata))
}

/// Returns the (shared, deduplicated) duration probe for `path`, caching it
/// against the file's mtime.
pub async fn duration_future(
    state: &AppState,
    path: &Path,
    known_mtime_ms: Option<i64>,
) -> DurationFuture {
    let mtime_ms = match known_mtime_ms {
        Some(value) => Some(value),
        None => stat_mtime_ms(path).await,
    };
    let key = path.to_string_lossy().into_owned();

    {
        let mut cache = state
            .duration_cache
            .lock()
            .expect("duration cache poisoned");
        if let Some(entry) = cache.get(&key) {
            if let Some(mtime_ms) = mtime_ms {
                if entry.mtime_ms == mtime_ms {
                    return entry.future.clone();
                }
            }
        }
        if mtime_ms.is_none() {
            cache.pop(&key);
        }
    }

    let cache = state.duration_cache.clone();
    let path_owned = path.to_path_buf();
    let error_key = key.clone();
    let future: DurationFuture = async move {
        let outcome = async {
            let stdout = ffprobe_duration(&path_owned).await?;
            let duration: f64 = stdout.trim().parse().unwrap_or(f64::NAN);
            Ok(if duration.is_finite() && duration > 0.0 {
                Some(duration)
            } else {
                None
            })
        }
        .await;

        if outcome.is_err() {
            cache
                .lock()
                .expect("duration cache poisoned")
                .pop(&error_key);
        }
        outcome
    }
    .boxed()
    .shared();

    state
        .duration_cache
        .lock()
        .expect("duration cache poisoned")
        .put(
            key,
            DurationEntry {
                mtime_ms: mtime_ms.unwrap_or(-1),
                future: future.clone(),
            },
        );
    future
}

/// `getDuration`: probes and awaits the shared future.
pub async fn get_duration(
    state: &AppState,
    path: &Path,
    known_mtime_ms: Option<i64>,
) -> Result<Option<f64>, ProbeError> {
    duration_future(state, path, known_mtime_ms).await.await
}

/// `probeAudioFile`: exists, non-empty, ffprobe-readable and has an audio stream.
pub async fn probe_audio_file(state: &AppState, filename: &str) -> ProbeOutcome {
    let invalid = |reason: InvalidReason, mtime_ms: Option<i64>| ProbeOutcome {
        valid: false,
        reason: Some(reason),
        mtime_ms,
    };
    let valid = |mtime_ms: Option<i64>| ProbeOutcome {
        valid: true,
        reason: None,
        mtime_ms,
    };

    if !is_safe_filename(filename) {
        return invalid(InvalidReason::UnsafeName, None);
    }
    let path = state.config.songs_path.join(filename);
    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) => metadata,
        Err(_) => return invalid(InvalidReason::Missing, None),
    };
    if !metadata.is_file() {
        return invalid(InvalidReason::Missing, None);
    }
    if metadata.len() == 0 {
        return invalid(InvalidReason::Empty, Some(mtime_ms_of(&metadata)));
    }
    let mtime_ms = mtime_ms_of(&metadata);

    match get_duration(state, &path, Some(mtime_ms)).await {
        Err(ProbeError::BinaryMissing) => valid(Some(mtime_ms)),
        Err(_) => invalid(InvalidReason::Unreadable, Some(mtime_ms)),
        Ok(None) => invalid(InvalidReason::NoDuration, Some(mtime_ms)),
        Ok(Some(_)) => match has_audio_stream(&path).await {
            None | Some(true) => valid(Some(mtime_ms)),
            Some(false) => invalid(InvalidReason::NoAudioStream, Some(mtime_ms)),
        },
    }
}

/// `deleteInvalidFile`: unlink, drop from the playable cache, reset the DB row.
pub async fn delete_invalid_file(
    state: &AppState,
    filename: &str,
    url: Option<&str>,
    reason: InvalidReason,
) {
    if is_safe_filename(filename) {
        let _ = tokio::fs::remove_file(state.config.songs_path.join(filename)).await;
        state.playable.set_playable(filename, false);
    }

    if let Some(url) = url {
        let url_owned = url.to_string();
        let result = db::call(&state.pool, move |conn| {
            conn.execute(
                "UPDATE files SET downloaded = 0, duration = NULL WHERE url = ?1",
                [url_owned],
            )?;
            Ok(())
        })
        .await;
        if let Err(error) = result {
            eprintln!("failed to reset db row after deleting invalid file {filename} {error:?}");
        }
    }

    eprintln!(
        "deleted invalid file {filename} (reason: {})",
        reason.as_str()
    );
}
