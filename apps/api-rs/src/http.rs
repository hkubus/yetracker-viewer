//! HTTP layer: the middleware around the route table and the server loop.
//!
//! Middleware, outermost first:
//! - request context: a `request` span (method + path) around everything the
//!   request logs, `Accept-Encoding` normalisation (a client that refuses
//!   identity still gets identity rather than 406), a 30 s time limit for
//!   everything but media, CORS, security headers and `Vary`;
//! - the 64 KiB request body limit (inside, so its 413 gets those headers);
//! - compression of JSON/text responses of at least 1 KiB;
//! - weak ETags and 304s for JSON 200 responses.
//!
//! Server: HTTP/1.1 with TCP_NODELAY and at most `MAX_CONNECTIONS` open
//! connections. At the limit a new connection replaces the one that has been
//! idle (no request in flight) the longest or, failing that, the one whose
//! client has read nothing of its response for the longest time (at least
//! 10 s); it is refused only when every connection is busy, and limit events
//! are logged at most every 10 s with a count. 30 s to send a request's
//! headers; idle keep-alive connections are closed after 75 s; a response
//! whose client reads nothing for 60 s is closed (freeing the connection and
//! the file it streams), any other response that moves no bytes after
//! 10 min; a graceful shutdown drains for at most 10 s.

use std::collections::HashMap;
use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{
    Extensions, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version, header,
};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http_body::Body as HttpBody;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{Predicate, SizeAbove};
use tower_http::limit::RequestBodyLimitLayer;
use tracing::{Instrument, debug, error, info, info_span, warn};

use crate::config::Config;
use crate::error::{ApiError, ErrorDetail};
use crate::routes;
use crate::serve::if_none_match_hits;
use crate::state::{AppState, SharedState};

const BODY_LIMIT: usize = 64 * 1024;
const COMPRESSION_MIN_BYTES: u64 = 1024;
/// Time limit for producing a non-media response.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(75);
/// A response that moves no bytes for this long is abandoned.
const STALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// A connection whose client reads nothing for this long (its socket stays
/// unwritable) is closed, with the file its response streams.
const NOT_READING_TIMEOUT: Duration = Duration::from_secs(60);
/// At the connection limit, with no idle connection to close, one whose
/// client has read nothing for this long makes room for a newcomer.
const NOT_READING_EVICTABLE: Duration = Duration::from_secs(10);
/// Connection-limit warnings are logged at most this often, with counts.
const LIMIT_LOG_INTERVAL: Duration = Duration::from_secs(10);
const WATCHDOG_TICK: Duration = Duration::from_secs(1);
const SHUTDOWN_DRAIN: Duration = Duration::from_secs(10);
const MAX_ETAG_BODY: usize = 32 * 1024 * 1024;
/// Response headers scripts on other origins may read.
const EXPOSED_HEADERS: &str = "X-Total-Count, Retry-After";

const SECURITY_HEADERS: [(&str, &str); 10] = [
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-resource-policy", "cross-origin"),
    ("origin-agent-cluster", "?1"),
    ("referrer-policy", "no-referrer"),
    ("x-content-type-options", "nosniff"),
    ("x-dns-prefetch-control", "off"),
    ("x-download-options", "noopen"),
    ("x-frame-options", "SAMEORIGIN"),
    ("x-permitted-cross-domain-policies", "none"),
    ("x-xss-protection", "0"),
];

/// The route table wrapped in the middleware stack.
pub fn app(state: SharedState) -> axum::Router {
    routes::router(state.clone())
        .layer(middleware::from_fn(json_etag))
        .layer(CompressionLayer::new().compress_when(compression_predicate()))
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(state, request_context))
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

fn is_compressible(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|content_type| {
            content_type.starts_with("application/json") || content_type.starts_with("text/")
        })
}

fn compression_predicate() -> impl Predicate {
    SizeAbove::new(COMPRESSION_MIN_BYTES).and(
        |_: StatusCode, _: Version, headers: &HeaderMap, _: &Extensions| is_compressible(headers),
    )
}

/// Media responses stream for as long as the client listens, so only the
/// other routes get the request time limit.
fn is_media_path(path: &str) -> bool {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    matches!(
        segments.as_slice(),
        ["songs", _, "stream" | "download"] | ["eras", _, "cover"]
    )
}

/// Drops `identity;q=0` and `*;q=0` from `Accept-Encoding`: when nothing
/// else is acceptable the compression layer would answer 406, but the API
/// always serves identity instead.
pub fn normalize_accept_encoding(headers: &mut HeaderMap) {
    let Some(value) = headers.get(header::ACCEPT_ENCODING) else {
        return;
    };
    let Ok(text) = value.to_str() else {
        headers.remove(header::ACCEPT_ENCODING);
        return;
    };
    let refuses = |item: &str| {
        let mut parts = item.split(';');
        let coding = parts.next().unwrap_or_default().trim();
        let about_identity = coding.eq_ignore_ascii_case("identity") || coding == "*";
        about_identity
            && parts.any(|parameter| {
                let parameter = parameter.trim();
                parameter.len() > 2
                    && parameter[..2].eq_ignore_ascii_case("q=")
                    && parameter[2..]
                        .trim()
                        .parse::<f32>()
                        .is_ok_and(|quality| quality == 0.0)
            })
    };
    let items: Vec<&str> = text
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect();
    let kept: Vec<&str> = items
        .iter()
        .copied()
        .filter(|item| !refuses(item))
        .collect();
    if kept.len() == items.len() {
        return;
    }
    match HeaderValue::from_str(&kept.join(", ")) {
        Ok(value) if !kept.is_empty() => {
            headers.insert(header::ACCEPT_ENCODING, value);
        }
        _ => {
            headers.remove(header::ACCEPT_ENCODING);
        }
    }
}

/// Adds `names` to `Vary` (case-insensitively deduplicated, `*` respected).
pub fn append_vary(headers: &mut HeaderMap, names: &[&str]) {
    let mut values: Vec<String> = headers
        .get_all(header::VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    if values.iter().any(|value| value == "*") {
        return;
    }
    for name in names {
        if !values.iter().any(|value| value.eq_ignore_ascii_case(name)) {
            values.push((*name).to_string());
        }
    }
    if let Ok(value) = HeaderValue::from_str(&values.join(", ")) {
        headers.insert(header::VARY, value);
    }
}

fn apply_cors(state: &AppState, headers: &mut HeaderMap, origin: Option<&str>) {
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static(EXPOSED_HEADERS),
    );
    let origins = &state.config.cors_origins;
    let allowed = if origins.iter().any(|allowed| allowed == "*") {
        Some("*")
    } else {
        origin.filter(|origin| origins.iter().any(|allowed| allowed == origin))
    };
    if let Some(allowed) = allowed
        && let Ok(value) = HeaderValue::from_str(allowed)
    {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }
}

fn apply_security_headers(headers: &mut HeaderMap) {
    for (name, value) in SECURITY_HEADERS {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
}

fn preflight(state: &AppState, request: &HeaderMap, origin: Option<&str>) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    let headers = response.headers_mut();
    apply_security_headers(headers);
    apply_cors(state, headers, origin);
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET,HEAD,OPTIONS"),
    );
    let mut vary = vec!["Origin"];
    if let Some(requested) = request
        .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .and_then(|value| value.to_str().ok())
    {
        if let Ok(value) = HeaderValue::from_str(requested) {
            headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, value);
        }
        vary.push("Access-Control-Request-Headers");
    }
    append_vary(headers, &vary);
    response
}

/// Outermost middleware: see the module docs.
async fn request_context(
    State(state): State<SharedState>,
    mut request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let span = info_span!("request", method = %method, path = %path);
    async move {
        let origin = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if method == Method::OPTIONS {
            return preflight(&state, request.headers(), origin.as_deref());
        }
        normalize_accept_encoding(request.headers_mut());
        let mut response = if is_media_path(&path) {
            next.run(request).await
        } else {
            match tokio::time::timeout(REQUEST_TIMEOUT, next.run(request)).await {
                Ok(response) => response,
                Err(_) => {
                    warn!("request timed out");
                    ApiError::busy("Request timed out", 5).into_response()
                }
            }
        };
        // Unexpected errors were logged with their detail where they
        // happened (inside this span); report other 500s here.
        if response.status() == StatusCode::INTERNAL_SERVER_ERROR
            && response.extensions().get::<ErrorDetail>().is_none()
        {
            error!(status = 500, "request failed");
        }
        let failed = response.status().is_client_error() || response.status().is_server_error();
        let headers = response.headers_mut();
        // Errors from the middleware itself (e.g. 413) are not cacheable
        // either, like the route errors.
        if failed && !headers.contains_key(header::CACHE_CONTROL) {
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        }
        apply_security_headers(headers);
        apply_cors(&state, headers, origin.as_deref());
        if is_compressible(headers) {
            append_vary(headers, &["Origin", "Accept-Encoding"]);
        } else {
            append_vary(headers, &["Origin"]);
        }
        response
    }
    .instrument(span)
    .await
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|content_type| content_type.starts_with("application/json"))
}

fn is_no_store(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::CACHE_CONTROL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.to_ascii_lowercase().contains("no-store"))
}

/// ETag of a JSON body: 128 bits of its SHA-256. It is computed before
/// compression and sent with the gzip and the identity body alike, so it is
/// weak: the two are the same data, not the same bytes, and a strong
/// validator must differ between them (RFC 9110 §8.8.3). `If-None-Match` uses
/// weak comparison, so 304s work for both.
fn body_etag(body: &[u8]) -> String {
    format!("W/\"{}\"", hex::encode(&Sha256::digest(body)[..16]))
}

fn not_modified(headers: &HeaderMap) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NOT_MODIFIED;
    for name in [
        header::ETAG,
        header::CACHE_CONTROL,
        header::EXPIRES,
        header::VARY,
        HeaderName::from_static("x-total-count"),
    ] {
        for value in headers.get_all(&name) {
            response.headers_mut().append(name.clone(), value.clone());
        }
    }
    // Same `Vary` as the 200 would have (the JSON body was compressible).
    append_vary(response.headers_mut(), &["Accept-Encoding"]);
    response
}

/// Gives cacheable JSON 200s a weak ETag (hash of the uncompressed body, see
/// [`body_etag`]) and answers a matching `If-None-Match` with 304.
async fn json_etag(request: Request, next: Next) -> Response {
    let conditional = matches!(*request.method(), Method::GET | Method::HEAD);
    let if_none_match: Option<String> = {
        let values: Vec<&str> = request
            .headers()
            .get_all(header::IF_NONE_MATCH)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect();
        (!values.is_empty()).then(|| values.join(", "))
    };
    let response = next.run(request).await;
    if !conditional
        || response.status() != StatusCode::OK
        || !is_json(response.headers())
        || is_no_store(response.headers())
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let existing = parts
        .headers
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (etag, body) = match existing {
        Some(etag) => (etag, body),
        None => {
            // JSON bodies are built in memory; anything else passes through.
            if body
                .size_hint()
                .exact()
                .is_none_or(|size| size > MAX_ETAG_BODY as u64)
            {
                return Response::from_parts(parts, body);
            }
            let bytes = match axum::body::to_bytes(body, MAX_ETAG_BODY).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    return ApiError::unexpected(format!("buffering a JSON response: {error}"))
                        .into_response();
                }
            };
            let etag = body_etag(&bytes);
            if let Ok(value) = HeaderValue::from_str(&etag) {
                parts.headers.insert(header::ETAG, value);
            }
            (etag, Body::from(bytes))
        }
    };
    if let Some(value) = if_none_match
        && if_none_match_hits(&value, &etag)
    {
        return not_modified(&parts.headers);
    }
    Response::from_parts(parts, body)
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Why the watchdog closes a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expiry {
    /// A request's headers did not arrive in time.
    HeaderRead,
    /// Nothing happened on a keep-alive connection for too long.
    Idle,
    /// A response moved no bytes for too long.
    Stalled,
    /// The client read nothing (the socket stayed unwritable) for too long.
    NotReading,
}

/// What a connection is doing, for the watchdog. Times are milliseconds since
/// `epoch` plus one, so that 0 can mean "not started".
struct Activity {
    epoch: Instant,
    in_flight: AtomicUsize,
    last_io: AtomicU64,
    /// When the next request's head began (0: nothing received yet since the
    /// last response). A new connection starts waiting for a head at once.
    head_started: AtomicU64,
    /// Since when writes have found the socket full, i.e. the client has
    /// stopped reading (0: the last write went through).
    write_blocked: AtomicU64,
}

impl Activity {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            epoch: Instant::now(),
            in_flight: AtomicUsize::new(0),
            last_io: AtomicU64::new(1),
            head_started: AtomicU64::new(1),
            write_blocked: AtomicU64::new(0),
        })
    }

    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64 + 1
    }

    fn instant(&self, millis: u64) -> Instant {
        self.epoch + Duration::from_millis(millis.saturating_sub(1))
    }

    /// A write found the socket full; the first one starts the clock.
    fn write_blocked(&self) {
        let _ = self.write_blocked.compare_exchange(
            0,
            self.now(),
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    /// A write went through.
    fn wrote(&self) {
        self.write_blocked.store(0, Ordering::Relaxed);
        self.saw_bytes(false);
    }

    /// Since when the client has read nothing of what the server is trying
    /// to send; `None` while writes go through.
    fn not_reading_since(&self) -> Option<Instant> {
        match self.write_blocked.load(Ordering::Relaxed) {
            0 => None,
            since => Some(self.instant(since)),
        }
    }

    fn saw_bytes(&self, read: bool) {
        let now = self.now();
        self.last_io.store(now, Ordering::Relaxed);
        if read && self.in_flight.load(Ordering::Relaxed) == 0 {
            let _ =
                self.head_started
                    .compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
        }
    }

    fn request_started(self: &Arc<Self>) -> InFlight {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        self.head_started.store(0, Ordering::Relaxed);
        self.last_io.store(self.now(), Ordering::Relaxed);
        InFlight(self.clone())
    }

    fn check_at(&self, now: u64) -> Option<Expiry> {
        // Whether or not a request is in flight: a response still sitting in
        // the buffers of a client that stopped reading also holds the socket.
        let blocked_since = self.write_blocked.load(Ordering::Relaxed);
        if blocked_since != 0
            && now.saturating_sub(blocked_since) >= NOT_READING_TIMEOUT.as_millis() as u64
        {
            return Some(Expiry::NotReading);
        }
        let quiet_for = now.saturating_sub(self.last_io.load(Ordering::Relaxed));
        if self.in_flight.load(Ordering::Relaxed) > 0 {
            return (quiet_for >= STALL_TIMEOUT.as_millis() as u64).then_some(Expiry::Stalled);
        }
        let head_started = self.head_started.load(Ordering::Relaxed);
        if head_started != 0 {
            let waited = now.saturating_sub(head_started);
            return (waited >= HEADER_READ_TIMEOUT.as_millis() as u64)
                .then_some(Expiry::HeaderRead);
        }
        (quiet_for >= IDLE_TIMEOUT.as_millis() as u64).then_some(Expiry::Idle)
    }

    fn check(&self) -> Option<Expiry> {
        self.check_at(self.now())
    }

    /// Since when the connection has had no request in flight: when it was
    /// accepted or began its current request head, or else its last traffic.
    /// `None` while a request is in flight.
    fn idle_since(&self) -> Option<Instant> {
        if self.in_flight.load(Ordering::Relaxed) > 0 {
            return None;
        }
        let since = match self.head_started.load(Ordering::Relaxed) {
            0 => self.last_io.load(Ordering::Relaxed),
            started => started,
        };
        Some(self.instant(since))
    }
}

/// An open connection, as the accept loop sees it.
struct OpenConnection {
    activity: Arc<Activity>,
    /// Closes the connection at once.
    evict: CancellationToken,
}

/// The connection to close for a newcomer when the limit is reached: the one
/// idle the longest or, when every connection has a request in flight, the
/// one whose client has read nothing for the longest time (at least
/// [`NOT_READING_EVICTABLE`]). A flood of silent sockets or of requests
/// whose responses are never read can't lock real clients out (they send
/// their request right away and read the answer), while a client that
/// keeps reading is never cut off.
fn eviction_victim(open: &HashMap<u64, OpenConnection>, now: Instant) -> Option<u64> {
    longest_idle(open).or_else(|| longest_not_reading(open, now))
}

fn longest_idle(open: &HashMap<u64, OpenConnection>) -> Option<u64> {
    open.iter()
        .filter_map(|(id, connection)| Some((connection.activity.idle_since()?, *id)))
        .min()
        .map(|(_, id)| id)
}

fn longest_not_reading(open: &HashMap<u64, OpenConnection>, now: Instant) -> Option<u64> {
    open.iter()
        .filter_map(|(id, connection)| {
            let since = connection.activity.not_reading_since()?;
            (now.saturating_duration_since(since) >= NOT_READING_EVICTABLE).then_some((since, *id))
        })
        .min()
        .map(|(_, id)| id)
}

/// Rate-limited warnings about the connection limit: the first event of a
/// kind is logged at once, later ones at most every [`LIMIT_LOG_INTERVAL`]
/// with the number of events since the previous line.
#[derive(Default)]
struct LimitLog {
    refused: LimitEvents,
    closed: LimitEvents,
}

#[derive(Default)]
struct LimitEvents {
    /// Events not logged yet.
    pending: u64,
    last_peer: Option<SocketAddr>,
    logged_at: Option<Instant>,
}

impl LimitEvents {
    /// Counts an event; returns the count to log when a line is due.
    fn record(&mut self, peer: SocketAddr, now: Instant) -> Option<(u64, SocketAddr)> {
        self.pending += 1;
        self.last_peer = Some(peer);
        self.take_due(now)
    }

    /// The events not logged yet, when there are some and a line is due.
    fn take_due(&mut self, now: Instant) -> Option<(u64, SocketAddr)> {
        let due = self.pending > 0
            && self
                .logged_at
                .is_none_or(|at| now.saturating_duration_since(at) >= LIMIT_LOG_INTERVAL);
        if !due {
            return None;
        }
        self.logged_at = Some(now);
        let peer = self.last_peer.take()?;
        Some((std::mem::take(&mut self.pending), peer))
    }

    /// When the events not logged yet are due.
    fn due_at(&self) -> Option<Instant> {
        if self.pending == 0 {
            return None;
        }
        Some(
            self.logged_at
                .map_or_else(Instant::now, |at| at + LIMIT_LOG_INTERVAL),
        )
    }
}

impl LimitLog {
    fn refused(&mut self, max_connections: usize, peer: SocketAddr, now: Instant) {
        if let Some((count, peer)) = self.refused.record(peer, now) {
            log_refused(max_connections, count, peer);
        }
    }

    fn closed(&mut self, max_connections: usize, peer: SocketAddr, now: Instant) {
        if let Some((count, peer)) = self.closed.record(peer, now) {
            log_closed(max_connections, count, peer);
        }
    }

    fn due_at(&self) -> Option<Instant> {
        match (self.refused.due_at(), self.closed.due_at()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Logs the counts that are due.
    fn flush(&mut self, max_connections: usize, now: Instant) {
        if let Some((count, peer)) = self.refused.take_due(now) {
            log_refused(max_connections, count, peer);
        }
        if let Some((count, peer)) = self.closed.take_due(now) {
            log_closed(max_connections, count, peer);
        }
    }
}

fn log_refused(max_connections: usize, refused: u64, last_peer: SocketAddr) {
    warn!(
        max_connections,
        refused,
        %last_peer,
        "connection limit reached and every connection is busy; refused new connections"
    );
}

fn log_closed(max_connections: usize, closed: u64, last_peer: SocketAddr) {
    warn!(
        max_connections,
        closed,
        %last_peer,
        "connection limit reached; closed idle or stalled connections to make room"
    );
}

/// Sleeps until `deadline`, or forever without one.
async fn sleep_until_or_forever(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}

/// Marks a request as in flight until its response body is done.
struct InFlight(Arc<Activity>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.last_io.store(self.0.now(), Ordering::Relaxed);
        self.0.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The TCP stream, reporting traffic to the connection's [`Activity`].
struct TrackedIo {
    inner: TcpStream,
    activity: Arc<Activity>,
}

impl AsyncRead for TrackedIo {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(context, buffer);
        if matches!(result, Poll::Ready(Ok(()))) && buffer.filled().len() > before {
            this.activity.saw_bytes(true);
        }
        result
    }
}

impl TrackedIo {
    fn track_write(&self, result: &Poll<io::Result<usize>>) {
        match result {
            Poll::Ready(Ok(written)) if *written > 0 => self.activity.wrote(),
            // The socket buffer is full: the client is not reading.
            Poll::Pending => self.activity.write_blocked(),
            _ => {}
        }
    }
}

impl AsyncWrite for TrackedIo {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_write(context, data);
        this.track_write(&result);
        result
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_write_vectored(context, buffers);
        this.track_write(&result);
        result
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(context)
    }
}

/// A response body that keeps its request marked in flight.
struct TrackedBody {
    inner: Body,
    _in_flight: InFlight,
}

impl HttpBody for TrackedBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, axum::Error>>> {
        Pin::new(&mut self.get_mut().inner).poll_frame(context)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

/// Serves one connection until it closes, is evicted or shuts down; returns
/// its id.
async fn serve_connection(
    id: u64,
    stream: TcpStream,
    app: axum::Router,
    mut closing: watch::Receiver<bool>,
    activity: Arc<Activity>,
    evict: CancellationToken,
) -> u64 {
    let io = TokioIo::new(TrackedIo {
        inner: stream,
        activity: activity.clone(),
    });
    let requests = activity.clone();
    let service = hyper::service::service_fn(move |request: hyper::Request<Incoming>| {
        let in_flight = requests.request_started();
        let app = app.clone();
        async move {
            let response = app
                .oneshot(request.map(Body::new))
                .await
                .unwrap_or_else(|never| match never {});
            Ok::<_, Infallible>(response.map(|inner| TrackedBody {
                inner,
                _in_flight: in_flight,
            }))
        }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    // The watchdog below enforces the header and idle limits.
    builder.keep_alive(true).header_read_timeout(None);
    let connection = builder.serve_connection(io, service);
    tokio::pin!(connection);
    let mut watchdog = tokio::time::interval(WATCHDOG_TICK);
    watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut shutting_down = false;
    loop {
        tokio::select! {
            result = connection.as_mut() => {
                if let Err(error) = result {
                    debug!(%error, "connection closed with an error");
                }
                break;
            }
            _ = closing.changed(), if !shutting_down => {
                shutting_down = true;
                connection.as_mut().graceful_shutdown();
            }
            () = evict.cancelled() => {
                debug!("closing an idle connection to make room for a new one");
                break;
            }
            _ = watchdog.tick() => match activity.check() {
                Some(Expiry::HeaderRead) => {
                    debug!("closing a connection that sent no complete request in time");
                    break;
                }
                Some(Expiry::Stalled) => {
                    debug!("closing a connection whose response stalled");
                    break;
                }
                Some(Expiry::NotReading) => {
                    debug!("closing a connection whose client stopped reading");
                    break;
                }
                Some(Expiry::Idle) if !shutting_down => {
                    shutting_down = true;
                    connection.as_mut().graceful_shutdown();
                }
                _ => {}
            }
        }
    }
    id
}

/// Binds the configured address.
pub async fn bind(config: &Config) -> Result<TcpListener, String> {
    let (host, port) = (config.api_host.as_str(), config.api_port);
    TcpListener::bind((host, port))
        .await
        .map_err(|error| format!("failed to bind {host}:{port}: {error}"))
}

/// Serves [`app`] on `listener` until SIGINT/SIGTERM (or `shutdown`), then
/// cancels `shutdown` so background work stops, lets open connections
/// finish for up to 10 s and closes whatever is left.
pub async fn serve(
    listener: TcpListener,
    state: SharedState,
    shutdown: CancellationToken,
) -> Result<(), String> {
    if let Ok(address) = listener.local_addr() {
        info!("API listening on http://{address}");
    }
    let max_connections = state.config.max_connections;
    let app = app(state);
    let (close_sender, close_receiver) = watch::channel(false);
    let mut connections = JoinSet::new();
    // The connections counting against the limit (evicted ones leave at once).
    let mut open: HashMap<u64, OpenConnection> = HashMap::new();
    let mut next_id: u64 = 0;
    let mut limit_log = LimitLog::default();
    let signal = shutdown_signal();
    tokio::pin!(signal);
    loop {
        let log_due = limit_log.due_at();
        tokio::select! {
            () = &mut signal => break,
            () = shutdown.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    if open.len() >= max_connections {
                        let now = Instant::now();
                        let Some(victim) = eviction_victim(&open, now).and_then(|id| open.remove(&id)) else {
                            limit_log.refused(max_connections, peer, now);
                            drop(stream);
                            continue;
                        };
                        victim.evict.cancel();
                        limit_log.closed(max_connections, peer, now);
                    }
                    if let Err(error) = stream.set_nodelay(true) {
                        debug!(%error, "could not set TCP_NODELAY");
                    }
                    let (id, activity, evict) = (next_id, Activity::new(), CancellationToken::new());
                    next_id += 1;
                    open.insert(id, OpenConnection { activity: activity.clone(), evict: evict.clone() });
                    connections.spawn(serve_connection(
                        id,
                        stream,
                        app.clone(),
                        close_receiver.clone(),
                        activity,
                        evict,
                    ));
                }
                Err(error) => {
                    // E.g. out of file descriptors: back off instead of spinning.
                    warn!(%error, "accepting a connection failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            () = sleep_until_or_forever(log_due) => limit_log.flush(max_connections, Instant::now()),
            Some(joined) = connections.join_next(), if !connections.is_empty() => match joined {
                Ok(id) => {
                    open.remove(&id);
                }
                // A connection task that panicked: forget every connection
                // whose task is gone (only this map still holds its activity).
                Err(_) => open.retain(|_, connection| Arc::strong_count(&connection.activity) > 1),
            },
        }
    }
    shutdown.cancel();
    drop(listener);
    let _ = close_sender.send(true);
    if !connections.is_empty() {
        info!(connections = connections.len(), "draining open connections");
    }
    let drained = tokio::time::timeout(SHUTDOWN_DRAIN, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        warn!(
            remaining = connections.len(),
            "shutdown deadline reached; closing the remaining connections"
        );
        connections.shutdown().await;
    }
    info!("HTTP server stopped");
    Ok(())
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
    info!("shutdown signal received, closing HTTP server");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accept_encoding(value: &str) -> Option<String> {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, value.parse().unwrap());
        normalize_accept_encoding(&mut headers);
        headers
            .get(header::ACCEPT_ENCODING)
            .map(|value| value.to_str().unwrap().to_string())
    }

    #[test]
    fn refusing_identity_never_leads_to_406() {
        assert_eq!(accept_encoding("gzip, br"), Some("gzip, br".to_string()));
        assert_eq!(accept_encoding("identity;q=0"), None);
        assert_eq!(accept_encoding("*;q=0"), None);
        assert_eq!(
            accept_encoding("gzip;q=0, identity;q=0"),
            Some("gzip;q=0".to_string())
        );
        assert_eq!(
            accept_encoding("gzip, *; q=0.000"),
            Some("gzip".to_string())
        );
        assert_eq!(accept_encoding("br, IDENTITY;Q=0"), Some("br".to_string()));
        // A positive quality keeps the entry.
        assert_eq!(
            accept_encoding("identity;q=0.5, *;q=0.1"),
            Some("identity;q=0.5, *;q=0.1".to_string())
        );
    }

    #[test]
    fn vary_values_are_appended_once() {
        let mut headers = HeaderMap::new();
        append_vary(&mut headers, &["Origin"]);
        assert_eq!(headers[header::VARY], "Origin");
        headers.insert(header::VARY, "accept-encoding".parse().unwrap());
        append_vary(&mut headers, &["Origin", "Accept-Encoding"]);
        assert_eq!(headers[header::VARY], "accept-encoding, Origin");
        headers.insert(header::VARY, "*".parse().unwrap());
        append_vary(&mut headers, &["Origin"]);
        assert_eq!(headers[header::VARY], "*");
    }

    #[test]
    fn media_paths_skip_the_request_timeout() {
        assert!(is_media_path("/songs/12/stream"));
        assert!(is_media_path("/songs/12/download"));
        assert!(is_media_path("/eras/3/cover"));
        assert!(!is_media_path("/songs/12/duration"));
        assert!(!is_media_path("/songs"));
        assert!(!is_media_path("/eras/3/songs"));
    }

    #[test]
    fn watchdog_expiries() {
        let activity = Activity::new();
        let seconds = |value: u64| value * 1000 + 1;
        // A new connection must send its first request head within 30 s.
        assert_eq!(activity.check_at(seconds(29)), None);
        assert_eq!(activity.check_at(seconds(31)), Some(Expiry::HeaderRead));

        let in_flight = activity.request_started();
        let started = activity.last_io.load(Ordering::Relaxed);
        assert_eq!(activity.check_at(started + 9 * 60 * 1000), None);
        assert_eq!(
            activity.check_at(started + 11 * 60 * 1000),
            Some(Expiry::Stalled)
        );
        drop(in_flight);
        let done = activity.last_io.load(Ordering::Relaxed);
        assert_eq!(activity.check_at(done + 74_000), None);
        assert_eq!(activity.check_at(done + 76_000), Some(Expiry::Idle));

        // Bytes of a new request start the header clock.
        activity.saw_bytes(true);
        let head = activity.head_started.load(Ordering::Relaxed);
        assert_ne!(head, 0);
        assert_eq!(activity.check_at(head + 31_000), Some(Expiry::HeaderRead));

        // A client that stops reading its response loses the connection
        // after a minute, long before the general stall limit.
        let activity = Activity::new();
        let in_flight = activity.request_started();
        activity.write_blocked.store(5_000, Ordering::Relaxed);
        assert_eq!(activity.check_at(64_999), None);
        assert_eq!(activity.check_at(65_000), Some(Expiry::NotReading));
        // Also once the whole response sits in the buffers.
        drop(in_flight);
        assert_eq!(activity.check_at(65_000), Some(Expiry::NotReading));
        // A write that goes through resets the clock.
        activity.wrote();
        assert_eq!(activity.check_at(65_000), None);
    }

    #[test]
    fn the_longest_idle_connection_makes_room_and_busy_ones_never_do() {
        let connection = |activity: &Arc<Activity>| OpenConnection {
            activity: activity.clone(),
            evict: CancellationToken::new(),
        };
        // Accepted, no request yet: idle since it was accepted.
        let silent = Activity::new();
        let busy = Activity::new();
        let _in_flight = busy.request_started();
        // A keep-alive connection whose last response went out 5 s in.
        let kept_alive = Activity::new();
        kept_alive.head_started.store(0, Ordering::Relaxed);
        kept_alive.last_io.store(5_001, Ordering::Relaxed);

        let mut open = HashMap::from([
            (1, connection(&silent)),
            (2, connection(&busy)),
            (3, connection(&kept_alive)),
        ]);
        assert_eq!(longest_idle(&open), Some(1));
        open.remove(&1);
        assert_eq!(longest_idle(&open), Some(3));
        open.remove(&3);
        assert_eq!(
            longest_idle(&open),
            None,
            "a request in flight is never cut off"
        );
    }

    #[test]
    fn clients_that_stopped_reading_make_room_when_nothing_is_idle() {
        let connection = |activity: &Arc<Activity>| OpenConnection {
            activity: activity.clone(),
            evict: CancellationToken::new(),
        };
        let reading = Activity::new();
        let _reading = reading.request_started();
        let stalled = Activity::new();
        let _stalled = stalled.request_started();
        let stalling = Activity::new();
        let _stalling = stalling.request_started();
        // 30 s in: one client has read nothing for 29 s, another for 5 s.
        stalled.write_blocked.store(1_001, Ordering::Relaxed);
        stalling.write_blocked.store(25_001, Ordering::Relaxed);
        let now = reading.epoch + Duration::from_secs(30);

        let mut open = HashMap::from([
            (1, connection(&reading)),
            (2, connection(&stalled)),
            (3, connection(&stalling)),
        ]);
        assert_eq!(eviction_victim(&open, now), Some(2));
        open.remove(&2);
        assert_eq!(
            eviction_victim(&open, now),
            None,
            "5 s without reading is not enough, and a reading client is never cut off"
        );
        assert_eq!(eviction_victim(&open, now + NOT_READING_EVICTABLE), Some(3));
        // An idle connection still goes first.
        let idle = Activity::new();
        open.insert(4, connection(&idle));
        assert_eq!(eviction_victim(&open, now + NOT_READING_EVICTABLE), Some(4));
    }

    #[test]
    fn limit_warnings_are_rate_limited_with_counts() {
        let peer: SocketAddr = "127.0.0.1:1234".parse().unwrap();
        let other: SocketAddr = "127.0.0.1:5678".parse().unwrap();
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut events = LimitEvents::default();
        assert_eq!(events.due_at(), None);
        assert_eq!(
            events.record(peer, at(0)),
            Some((1, peer)),
            "the first at once"
        );
        assert_eq!(events.record(peer, at(1)), None);
        assert_eq!(events.record(other, at(2)), None);
        assert_eq!(events.due_at(), Some(at(10)));
        assert_eq!(events.take_due(at(9)), None);
        assert_eq!(events.take_due(at(10)), Some((2, other)));
        assert_eq!(events.due_at(), None);
        assert_eq!(events.take_due(at(30)), None, "nothing new to report");
        assert_eq!(events.record(peer, at(15)), None);
        assert_eq!(events.record(peer, at(20)), Some((2, peer)));

        let mut log = LimitLog::default();
        log.refused(2, peer, at(0));
        log.closed(2, peer, at(3));
        log.refused(2, peer, at(4));
        log.closed(2, peer, at(5));
        assert_eq!(log.due_at(), Some(at(10)));
        log.flush(2, at(10));
        assert_eq!(log.due_at(), Some(at(13)));
        log.flush(2, at(13));
        assert_eq!(log.due_at(), None);
    }

    /// A client that reads nothing fills the socket buffers; the next write
    /// finds the socket full and starts the not-reading clock, and a write
    /// that goes through once it reads again stops it.
    #[tokio::test]
    async fn a_full_socket_marks_the_client_as_not_reading() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let activity = Activity::new();
        let mut io = TrackedIo {
            inner: server,
            activity: activity.clone(),
        };
        let chunk = vec![0u8; 64 * 1024];
        let mut sent = 0;
        while let Ok(written) =
            tokio::time::timeout(Duration::from_millis(200), io.write(&chunk)).await
        {
            sent += written.unwrap();
            assert!(sent < 1 << 30, "the socket never filled up");
            assert!(activity.not_reading_since().is_none(), "writes go through");
        }
        assert!(activity.not_reading_since().is_some());

        let reader = tokio::spawn(async move {
            let mut buffer = vec![0u8; 1 << 20];
            let mut received = 0;
            while let Ok(read) = client.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                received += read;
            }
            received
        });
        io.write_all(&chunk).await.unwrap();
        assert!(activity.not_reading_since().is_none());
        io.shutdown().await.unwrap();
        drop(io);
        assert!(reader.await.unwrap() >= sent + chunk.len());
    }

    /// At the connection limit, silent sockets can't lock clients out: a new
    /// client replaces the one idle the longest and gets its answer.
    #[tokio::test]
    async fn a_full_server_makes_room_for_new_clients() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let root = std::env::temp_dir().join(format!("yt-http-full-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("songs")).unwrap();
        let mut config = Config::for_tests(&root, &root.join("songs"));
        config.max_connections = 2;
        let pool = crate::db::create_pool(&root.join("db.sqlite3")).unwrap();
        crate::db::run_migrations(&pool, &config).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let shutdown = CancellationToken::new();
        let server = tokio::spawn(serve(
            listener,
            AppState::new(config, pool),
            shutdown.clone(),
        ));
        let pause = || tokio::time::sleep(Duration::from_millis(50));

        let mut oldest = TcpStream::connect(address).await.unwrap();
        pause().await;
        let mut newer = TcpStream::connect(address).await.unwrap();
        pause().await;
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(b"GET /hello HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut response))
            .await
            .expect("the new client is answered")
            .unwrap();
        let response = String::from_utf8_lossy(&response);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains(r#"{"hello":"world"}"#), "{response}");

        let mut byte = [0u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), oldest.read(&mut byte)).await;
        assert!(
            matches!(closed, Ok(Ok(0)) | Ok(Err(_))),
            "the longest-idle socket was closed: {closed:?}"
        );
        let still_open =
            tokio::time::timeout(Duration::from_millis(200), newer.read(&mut byte)).await;
        assert!(still_open.is_err(), "only one connection made room");

        shutdown.cancel();
        server.await.unwrap().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn the_middleware_stack_sets_headers_in_the_right_order() {
        let root = std::env::temp_dir().join(format!("yt-http-stack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("songs")).unwrap();
        let config = Config::for_tests(&root, &root.join("songs"));
        let pool = crate::db::create_pool(&root.join("db.sqlite3")).unwrap();
        crate::db::run_migrations(&pool, &config).unwrap();
        {
            let conn = pool.get().unwrap();
            for id in 1..=30 {
                conn.execute(
                    "INSERT INTO eras (id, key, name, notes, description, dominant_color, is_main, position) \
                     VALUES (?1, ?2, ?3, 'Some notes about this era', 'A description', '666666', 1, ?1)",
                    rusqlite::params![id, format!("era {id}"), format!("Era number {id}")],
                )
                .unwrap();
            }
        }
        let app = app(AppState::new(config, pool));
        let get = |uri: &str, headers: &[(&str, &str)]| {
            let mut builder = axum::http::Request::builder().uri(uri);
            for (name, value) in headers {
                builder = builder.header(*name, *value);
            }
            builder.body(Body::empty()).unwrap()
        };

        // Large JSON: compressed, ETag, Vary on both, CORS, no HSTS.
        let response = app
            .clone()
            .oneshot(get(
                "/eras",
                &[
                    ("accept-encoding", "gzip"),
                    ("origin", "http://localhost:4321"),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        let gzip_etag = response.headers()[header::ETAG].clone();
        assert!(
            gzip_etag.to_str().unwrap().starts_with("W/\""),
            "{gzip_etag:?}"
        );
        let vary = response.headers()[header::VARY]
            .to_str()
            .unwrap()
            .to_ascii_lowercase();
        assert!(
            vary.contains("origin") && vary.contains("accept-encoding"),
            "{vary}"
        );
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "http://localhost:4321"
        );
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_EXPOSE_HEADERS],
            EXPOSED_HEADERS
        );
        assert!(
            !response
                .headers()
                .contains_key(header::STRICT_TRANSPORT_SECURITY)
        );

        // Refusing identity (and everything else) still gets identity, not 406.
        for refusal in ["identity;q=0", "*;q=0", "br;q=0, identity;q=0"] {
            let response = app
                .clone()
                .oneshot(get("/eras", &[("accept-encoding", refusal)]))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{refusal}");
            assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
        }

        // The identity body carries the same weak ETag, and either form of it
        // revalidates either representation.
        let identity = app
            .clone()
            .oneshot(get("/eras", &[("accept-encoding", "identity")]))
            .await
            .unwrap();
        assert!(!identity.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(identity.headers()[header::ETAG], gzip_etag);
        let strong_form = gzip_etag
            .to_str()
            .unwrap()
            .trim_start_matches("W/")
            .to_string();
        for (encoding, tag) in [
            ("gzip", gzip_etag.to_str().unwrap()),
            ("identity", gzip_etag.to_str().unwrap()),
            ("gzip", strong_form.as_str()),
        ] {
            let response = app
                .clone()
                .oneshot(get(
                    "/eras",
                    &[("accept-encoding", encoding), ("if-none-match", tag)],
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::NOT_MODIFIED,
                "{encoding} {tag}"
            );
            assert_eq!(response.headers()[header::ETAG], gzip_etag);
        }

        // Small JSON is not compressed, but still varies on the encoding.
        let response = app
            .clone()
            .oneshot(get("/hello", &[("accept-encoding", "gzip")]))
            .await
            .unwrap();
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(response.headers()[header::VARY], "Origin, Accept-Encoding");

        // Errors: never cached, no ETag, security headers present.
        let response = app.clone().oneshot(get("/nope", &[])).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(!response.headers().contains_key(header::ETAG));
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");

        // The body limit sits inside the header middleware.
        let oversized = axum::http::Request::builder()
            .uri("/eras")
            .header(header::CONTENT_LENGTH, (BODY_LIMIT + 1).to_string())
            .body(Body::from(vec![0u8; BODY_LIMIT + 1]))
            .unwrap();
        let response = app.clone().oneshot(oversized).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert!(
            response
                .headers()
                .contains_key(header::ACCESS_CONTROL_EXPOSE_HEADERS)
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn json_responses_get_etags_and_304s() {
        use axum::routing::get;
        let app = axum::Router::new()
            .route(
                "/data",
                get(|| async {
                    let mut response = Response::new(Body::from(r#"{"a":1}"#));
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    response.headers_mut().insert(
                        header::CACHE_CONTROL,
                        HeaderValue::from_static("public, max-age=60"),
                    );
                    response
                }),
            )
            .route(
                "/private",
                get(|| async {
                    let mut response = Response::new(Body::from(r#"{"status":"ok"}"#));
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    response
                        .headers_mut()
                        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                    response
                }),
            )
            .layer(middleware::from_fn(json_etag));
        let request = |uri: &str, etag: Option<&str>| {
            let mut builder = axum::http::Request::builder().uri(uri);
            if let Some(etag) = etag {
                builder = builder.header(header::IF_NONE_MATCH, etag);
            }
            builder.body(Body::empty()).unwrap()
        };

        let response = app.clone().oneshot(request("/data", None)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let etag = response.headers()[header::ETAG]
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(etag, body_etag(br#"{"a":1}"#));
        assert!(etag.starts_with("W/\"") && etag.ends_with('"'), "{etag}");
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], br#"{"a":1}"#);

        // Weak comparison: the tag with or without its `W/` matches.
        let strong = etag.trim_start_matches("W/").to_string();
        for tag in [&etag, &strong] {
            let response = app
                .clone()
                .oneshot(request("/data", Some(tag)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "{tag}");
            assert_eq!(response.headers()[header::ETAG], etag.as_str());
        }
        let response = app
            .clone()
            .oneshot(request("/data", Some(&etag)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=60"
        );
        assert_eq!(response.headers()[header::VARY], "Accept-Encoding");
        assert!(response.headers().get(header::CONTENT_TYPE).is_none());

        let response = app
            .clone()
            .oneshot(request("/data", Some("\"stale\"")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(request("/private", Some("*")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::ETAG).is_none());
    }
}
