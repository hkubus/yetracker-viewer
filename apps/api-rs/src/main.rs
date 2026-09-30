//! Binary entry point: configuration, logging and the database; then the
//! HTTP listener binds and serves the stored catalog while the background
//! sync (`sync.rs`) imports and fetches media. See `http.rs` for the server.

use std::io::IsTerminal;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

use yetracker_api::backfill;
use yetracker_api::config::{self, Config};
use yetracker_api::db;
use yetracker_api::http;
use yetracker_api::media::Tool;
use yetracker_api::state::AppState;
use yetracker_api::sync;

/// How long the background sync may take to wind down after the server
/// stopped.
const SYNC_STOP_WAIT: Duration = Duration::from_secs(5);
/// Upper bound for the runtime shutdown. Tasks still running then (a
/// download, a transcode) are dropped, which kills their child processes.
const RUNTIME_STOP_WAIT: Duration = Duration::from_secs(5);

fn main() {
    // `.env` may set `RUST_LOG`, so load it before the subscriber reads it.
    config::load_env_file();
    init_tracing();
    // Before the configuration: the default `MAX_CONNECTIONS` derives from
    // the (raised) limit.
    match config::raise_open_file_limit() {
        Ok((before, after)) if after > before => {
            info!(from = before, to = after, "raised the open-file limit");
        }
        Ok(_) => {}
        Err(error) => warn!(%error, "could not raise the open-file limit"),
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            error!("could not start the async runtime: {error}");
            std::process::exit(1);
        }
    };
    let result = runtime.block_on(run());
    runtime.shutdown_timeout(RUNTIME_STOP_WAIT);
    if let Err(error) = result {
        error!("API startup failed: {error}");
        std::process::exit(1);
    }
}

/// Timestamped, levelled log lines on stdout. `RUST_LOG` replaces the default
/// `info` filter (e.g. `RUST_LOG=yetracker_api=debug,info`).
fn init_tracing() {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(std::io::stdout().is_terminal())
        .init();
}

async fn run() -> Result<(), String> {
    let config = Config::load()?;
    info!(
        open_file_limit = config::open_file_limit(),
        max_connections = config.max_connections,
        "connection limit"
    );

    let db_path = config.storage_path.join("db.sqlite3");
    let pool = db::create_pool(&db_path).map_err(|error| error.to_string())?;
    db::run_migrations(&pool, &config).map_err(|error| error.to_string())?;

    let state = AppState::new(config, pool);
    let removed = backfill::remove_stale_temp_files(&state.config).await;
    if removed > 0 {
        info!(removed, "removed temporary files left by a previous run");
    }
    state.tools.detect(&Tool::ALL).await;
    // A SONGS_DIR outside STORAGE_DIR is never created on demand; when it is
    // missing (already reported while loading the config) start with an empty
    // playable set rather than refusing to boot.
    if tokio::fs::metadata(&state.config.songs_path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
    {
        state
            .playable
            .refresh(&state.config.songs_path)
            .await
            .map_err(|error| error.to_string())?;
    }

    // Bind before anything else runs: the stored catalog is served while the
    // first sync imports in the background.
    let listener = http::bind(&state.config).await?;
    let shutdown = CancellationToken::new();
    let sync = sync::spawn(state.clone(), shutdown.clone());
    let served = http::serve(listener, state, shutdown.clone()).await;
    shutdown.cancel();
    if tokio::time::timeout(SYNC_STOP_WAIT, sync).await.is_err() {
        warn!("the background sync did not stop in time");
    }
    served
}
