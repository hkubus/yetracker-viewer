//! Binary entry point: load config, open SQLite, run startup repair, build the
//! router and serve it. Mirrors the boot/shutdown sequence in
//! `apps/api/src/index.ts`, including the sequential background sync phases.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{
    Extensions, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version, header,
};
use axum::middleware::{self, Next};
use axum::response::Response;
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::Predicate;
use tower_http::limit::RequestBodyLimitLayer;

use yetracker_api::backfill;
use yetracker_api::config::Config;
use yetracker_api::db;
use yetracker_api::downloader;
use yetracker_api::importer;
use yetracker_api::repair;
use yetracker_api::routes;
use yetracker_api::state::{AppState, SharedState};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("API startup failed {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let config = Config::load()?;
    let api_host = config.api_host.clone();
    let api_port = config.api_port;

    let db_path = config.storage_path.join("db.sqlite3");
    let pool = db::create_pool(&db_path).map_err(|error| format!("{error:?}"))?;
    db::run_migrations(&pool).map_err(|error| format!("{error:?}"))?;
    {
        let connection = pool.get().map_err(|error| error.to_string())?;
        repair::repair_era_duplicates(&connection).map_err(|error| format!("{error:?}"))?;
    }

    let state = AppState::new(config, pool);
    state
        .playable
        .refresh(&state.config.songs_path)
        .await
        .map_err(|error| format!("{error:?}"))?;

    let shutdown_requested = Arc::new(AtomicBool::new(false));

    if state.config.sync_on_start {
        // Import blocks startup (as in the Node original); a failure aborts boot.
        importer::import_data(state.clone())
            .await
            .map_err(|error| format!("{error:?}"))?;
        // Kick the cover/song pipeline off right away; the periodic loop below
        // re-runs the whole cycle on its own cadence.
        spawn_background_sync(state.clone(), shutdown_requested.clone());
    }

    // Syncing also repeats in the background in every environment, independent
    // of SYNC_ON_START, so a long-running server picks up new catalog entries
    // and media without a restart.
    spawn_periodic_sync(state.clone(), shutdown_requested.clone());

    let app = routes::router(state.clone())
        .layer(CompressionLayer::new().compress_when(compression_predicate()))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            cors_and_secure_headers,
        ))
        // Mirrors Hono's global `bodyLimit({ maxSize: 64 * 1024 })`.
        .layer(RequestBodyLimitLayer::new(64 * 1024));

    let listener = tokio::net::TcpListener::bind((api_host.as_str(), api_port))
        .await
        .map_err(|error| error.to_string())?;
    println!("API listening on http://{api_host}:{api_port}");

    let signal_shutdown = shutdown_requested.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            signal_shutdown.store(true, Ordering::SeqCst);
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// How often the always-on background sync loop re-imports the catalogs and
/// retries missing media. Deliberately a constant rather than an environment
/// variable so every deployment keeps syncing.
const SYNC_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Sequential background chain mirroring `index.ts`: covers -> backfill ->
/// songs -> backfill, bailing out between phases once shutdown is requested.
async fn run_sync_chain(state: &AppState, shutdown: &AtomicBool) {
    if let Err(error) = downloader::download_covers(state).await {
        if !shutdown.load(Ordering::SeqCst) {
            eprintln!("background song processing failed {error:?}");
        }
        return;
    }
    if shutdown.load(Ordering::SeqCst) {
        return;
    }
    if let Err(error) = backfill::backfill_durations(state).await {
        if !shutdown.load(Ordering::SeqCst) {
            eprintln!("background song processing failed {error:?}");
        }
        return;
    }
    if shutdown.load(Ordering::SeqCst) {
        return;
    }
    if let Err(error) = downloader::download_songs(state).await {
        if !shutdown.load(Ordering::SeqCst) {
            eprintln!("background song processing failed {error:?}");
        }
        return;
    }
    if shutdown.load(Ordering::SeqCst) {
        return;
    }
    if let Err(error) = backfill::backfill_durations(state).await {
        if !shutdown.load(Ordering::SeqCst) {
            eprintln!("background song processing failed {error:?}");
        }
    }
}

fn spawn_background_sync(state: SharedState, shutdown: Arc<AtomicBool>) {
    tokio::spawn(async move {
        run_sync_chain(&state, &shutdown).await;
    });
}

/// Re-imports the catalogs and re-runs the media pipeline every
/// [`SYNC_INTERVAL`], regardless of `SYNC_ON_START`, until shutdown. The first
/// run waits one interval so boot stays fast and offline-capable.
fn spawn_periodic_sync(state: SharedState, shutdown: Arc<AtomicBool>) {
    tokio::spawn(async move {
        loop {
            if !sleep_until_next_sync(&shutdown).await {
                return;
            }
            println!("background sync starting");
            if let Err(error) = importer::import_data(state.clone()).await {
                if !shutdown.load(Ordering::SeqCst) {
                    eprintln!("background sync import failed {error:?}");
                }
            }
            // Retry pending media even when the import failed: the backfill and
            // download phases still make progress on the existing catalog.
            run_sync_chain(&state, &shutdown).await;
            if shutdown.load(Ordering::SeqCst) {
                return;
            }
        }
    });
}

/// Sleeps for [`SYNC_INTERVAL`], waking early if shutdown is requested. Returns
/// `false` when the server is shutting down.
async fn sleep_until_next_sync(shutdown: &AtomicBool) -> bool {
    const TICK: Duration = Duration::from_secs(1);
    let mut remaining = SYNC_INTERVAL;
    while !remaining.is_zero() {
        if shutdown.load(Ordering::SeqCst) {
            return false;
        }
        let nap = remaining.min(TICK);
        tokio::time::sleep(nap).await;
        remaining = remaining.saturating_sub(nap);
    }
    !shutdown.load(Ordering::SeqCst)
}

const SECURE_HEADERS: [(&str, &str); 11] = [
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-resource-policy", "cross-origin"),
    ("origin-agent-cluster", "?1"),
    ("referrer-policy", "no-referrer"),
    (
        "strict-transport-security",
        "max-age=15552000; includeSubDomains",
    ),
    ("x-content-type-options", "nosniff"),
    ("x-dns-prefetch-control", "off"),
    ("x-download-options", "noopen"),
    ("x-frame-options", "SAMEORIGIN"),
    ("x-permitted-cross-domain-policies", "none"),
    ("x-xss-protection", "0"),
];

fn apply_security_headers(response: &mut Response) {
    for (name, value) in SECURE_HEADERS {
        response.headers_mut().insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
}

fn apply_cors(state: &AppState, response: &mut Response, origin: Option<&str>) {
    response.headers_mut().insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("X-Total-Count"),
    );
    response
        .headers_mut()
        .insert(header::VARY, HeaderValue::from_static("Origin"));

    let wildcard = state
        .config
        .cors_origins
        .iter()
        .any(|allowed| allowed == "*");
    let allowed = if wildcard {
        Some("*")
    } else {
        origin.filter(|origin| {
            state
                .config
                .cors_origins
                .iter()
                .any(|allowed| allowed == origin)
        })
    };
    if let Some(allowed) = allowed {
        if let Ok(value) = HeaderValue::from_str(allowed) {
            response
                .headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        }
    }
}

/// Hono's `compress` only compresses compressible content types (JSON/text);
/// it does not apply a size threshold in practice because its responses carry
/// no `Content-Length` at middleware time, so neither do we.
fn compression_predicate() -> impl Predicate {
    |_status: StatusCode, _version: Version, headers: &HeaderMap, _extensions: &Extensions| {
        let content_type = headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        content_type.starts_with("application/json") || content_type.starts_with("text/")
    }
}

async fn cors_and_secure_headers(
    State(state): State<SharedState>,
    request: Request,
    next: Next,
) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    if request.method() == Method::OPTIONS {
        let requested_headers = request
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);

        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NO_CONTENT;
        apply_security_headers(&mut response);
        apply_cors(&state, &mut response, origin.as_deref());
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET,HEAD,OPTIONS"),
        );
        // Hono echoes the requested headers and widens `Vary` accordingly.
        if let Some(requested_headers) = requested_headers {
            if let Ok(value) = HeaderValue::from_str(&requested_headers) {
                response
                    .headers_mut()
                    .insert(header::ACCESS_CONTROL_ALLOW_HEADERS, value);
            }
            response.headers_mut().insert(
                header::VARY,
                HeaderValue::from_static("Origin, Access-Control-Request-Headers"),
            );
        }
        return response;
    }

    let mut response = next.run(request).await;
    apply_security_headers(&mut response);
    apply_cors(&state, &mut response, origin.as_deref());
    response
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    println!("shutdown signal received, closing HTTP server");
}
