//! The background sync: one task runs import → covers → downloads →
//! backfill → cleanup, first right after boot (`SYNC_ON_START`, while the
//! listener already serves the stored catalog) and then
//! `SYNC_INTERVAL_MINUTES` after the previous run finished. Runs never
//! overlap; a failing phase is logged and the next one still runs; a panic
//! is logged and the schedule continues. Shutdown interrupts the wait at once
//! and stops a running sync (dropping its downloads and child processes).

use std::future::Future;
use std::time::Instant;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::backfill;
use crate::downloader;
use crate::error::ApiError;
use crate::importer;
use crate::media::{Tool, ToolSet};
use crate::routes;
use crate::state::SharedState;

/// Starts the sync loop.
pub fn spawn(state: SharedState, shutdown: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        if state.config.sync_interval.is_none() {
            info!("periodic sync disabled (SYNC_INTERVAL_MINUTES=0)");
        }
        let mut run_now = state.config.sync_on_start;
        loop {
            if !run_now {
                let Some(interval) = state.config.sync_interval else {
                    return;
                };
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = tokio::time::sleep(interval) => {}
                }
            }
            run_now = false;
            supervise(run_sync(state.clone(), shutdown.clone())).await;
            if shutdown.is_cancelled() {
                return;
            }
        }
    })
}

/// Runs one sync in its own task, so that a panic in any phase is logged
/// here instead of ending the schedule.
async fn supervise(run: impl Future<Output = ()> + Send + 'static) {
    if let Err(failure) = tokio::spawn(run).await {
        if failure.is_panic() {
            error!(panic = %panic_message(failure.into_panic()), "sync panicked; the schedule continues");
        } else {
            error!(error = %failure, "sync task failed");
        }
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Runs one phase unless shutdown is requested first; a failure is logged
/// with the phase name. Returns whether the sync should go on.
async fn phase<T>(
    shutdown: &CancellationToken,
    name: &'static str,
    work: impl Future<Output = Result<T, ApiError>>,
) -> bool {
    tokio::select! {
        () = shutdown.cancelled() => false,
        result = work => {
            if let Err(error) = result {
                error!(phase = name, %error, "sync phase failed");
            }
            !shutdown.is_cancelled()
        }
    }
}

/// One warning per sync naming the missing tools and what is skipped.
fn warn_about_missing_tools(tools: ToolSet) {
    let missing = tools.missing();
    if missing.is_empty() {
        return;
    }
    let mut skipped = Vec::new();
    if !tools.ffprobe {
        skipped.push("duration backfill and media verification");
    }
    if !tools.ffmpeg {
        skipped.push("cover encoding (originals are stored) and transcoding");
    }
    if !tools.ffmpeg || !tools.yt_dlp {
        skipped.push("YouTube/Instagram/X downloads");
    }
    let names: Vec<&str> = missing.iter().map(|tool| tool.binary()).collect();
    warn!(
        missing = %names.join(", "),
        skipped = %skipped.join("; "),
        "media tools are missing; skipping what needs them"
    );
}

async fn run_sync(state: SharedState, shutdown: CancellationToken) {
    let started = Instant::now();
    info!("sync started");
    let tools = state.tools.detect(&Tool::ALL).await;
    warn_about_missing_tools(tools);

    // A failed or refused import leaves the stored catalog in place; the
    // media phases still make progress on it.
    let import = async { importer::import_data(state.clone()).await };
    if !phase(&shutdown, "import", import).await {
        return;
    }
    if let Err(error) = routes::songs::warm_search_index(&state).await {
        warn!(%error, "building the search index failed");
    }
    // The playable set also has to notice media that changed out of band.
    if let Err(error) = state.playable.refresh(&state.config.songs_path).await {
        warn!(%error, "playable file rescan failed");
    }
    if !phase(
        &shutdown,
        "covers",
        downloader::sync_covers(&state, &shutdown, tools),
    )
    .await
    {
        return;
    }
    let downloads = async {
        backfill::reverify_queued(&state).await?;
        downloader::download_songs(&state, &shutdown).await
    };
    if !phase(&shutdown, "downloads", downloads).await {
        return;
    }
    if tools.ffprobe
        && !phase(
            &shutdown,
            "backfill",
            backfill::backfill_durations(&state, &shutdown),
        )
        .await
    {
        return;
    }
    if !phase(&shutdown, "cleanup", backfill::cleanup(&state)).await {
        return;
    }
    info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        "sync finished"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_messages_are_extracted() {
        assert_eq!(panic_message(Box::new("boom")), "boom");
        assert_eq!(panic_message(Box::new(String::from("bang"))), "bang");
        assert_eq!(panic_message(Box::new(42)), "unknown panic payload");
    }

    #[tokio::test]
    async fn a_panicking_sync_does_not_end_the_schedule() {
        supervise(async { panic!("boom") }).await;
        // Still running: the next sync can be supervised as well.
        supervise(async {}).await;
    }

    #[tokio::test]
    async fn phases_stop_on_shutdown() {
        let shutdown = CancellationToken::new();
        assert!(phase(&shutdown, "ok", async { Ok::<_, ApiError>(()) }).await);
        assert!(
            phase(&shutdown, "failing", async {
                Err::<(), _>(ApiError::unexpected("boom"))
            })
            .await
        );
        shutdown.cancel();
        assert!(
            !phase(
                &shutdown,
                "never",
                std::future::pending::<Result<(), ApiError>>()
            )
            .await
        );
    }
}
