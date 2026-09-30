//! HTTP plumbing for media files: validators (ETag, Last-Modified),
//! conditional requests (If-None-Match, If-Modified-Since, If-Range), single
//! byte ranges, and streaming in 256 KiB chunks — including files that are
//! still being written (live transcodes).

use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use futures_util::stream;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::watch;
use tokio_util::io::ReaderStream;

/// Read size for streamed files.
pub const CHUNK_SIZE: usize = 256 * 1024;

const TEXT_PLAIN: &str = "text/plain;charset=UTF-8";

/// An inclusive byte range within a representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    pub fn length(&self) -> u64 {
        self.end - self.start + 1
    }
}

/// How to answer a request given its `Range` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeRequest {
    /// No usable range (absent, unparseable, another unit, several ranges):
    /// send the whole representation.
    Full,
    /// One satisfiable range.
    Partial(ByteRange),
    /// One well-formed range that lies entirely past the end: 416.
    Unsatisfiable,
}

/// Interprets a `Range` header for a representation of `size` bytes
/// (RFC 9110 §14). Only single `bytes` ranges are served; everything the
/// server does not understand is ignored rather than rejected.
pub fn parse_range(value: &str, size: u64) -> RangeRequest {
    let Some((unit, set)) = value.split_once('=') else {
        return RangeRequest::Full;
    };
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return RangeRequest::Full;
    }
    let mut specs = set
        .split(',')
        .map(str::trim)
        .filter(|spec| !spec.is_empty());
    let (Some(spec), None) = (specs.next(), specs.next()) else {
        return RangeRequest::Full;
    };
    let Some((first, last)) = spec.split_once('-') else {
        return RangeRequest::Full;
    };
    let (first, last) = (first.trim(), last.trim());

    if first.is_empty() {
        let Some(suffix) = parse_position(last) else {
            return RangeRequest::Full;
        };
        if suffix == 0 || size == 0 {
            return RangeRequest::Unsatisfiable;
        }
        return RangeRequest::Partial(ByteRange {
            start: size.saturating_sub(suffix),
            end: size - 1,
        });
    }

    let Some(start) = parse_position(first) else {
        return RangeRequest::Full;
    };
    let end = if last.is_empty() {
        None
    } else {
        match parse_position(last) {
            Some(end) if end >= start => Some(end),
            _ => return RangeRequest::Full,
        }
    };
    if start >= size {
        return RangeRequest::Unsatisfiable;
    }
    RangeRequest::Partial(ByteRange {
        start,
        end: end.map_or(size - 1, |end| end.min(size - 1)),
    })
}

/// A byte position: ASCII digits only. Values beyond `u64` saturate, which is
/// past the end of any file anyway.
fn parse_position(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(text.parse::<u64>().unwrap_or(u64::MAX))
}

/// The entity tags of an `If-None-Match`-style list, as `(weak, "opaque")`
/// with the quotes kept. Malformed members are skipped.
fn entity_tags(value: &str) -> Vec<(bool, &str)> {
    let mut tags = Vec::new();
    let mut rest = value;
    loop {
        rest = rest.trim_start_matches(|character: char| {
            character == ',' || character.is_ascii_whitespace()
        });
        if rest.is_empty() {
            break;
        }
        let weak = rest.starts_with("W/");
        if weak {
            rest = &rest[2..];
        }
        if !rest.starts_with('"') {
            match rest.find(',') {
                Some(comma) => {
                    rest = &rest[comma..];
                    continue;
                }
                None => break,
            }
        }
        match rest[1..].find('"') {
            Some(close) => {
                tags.push((weak, &rest[..close + 2]));
                rest = &rest[close + 2..];
            }
            None => break,
        }
    }
    tags
}

fn opaque_tag(etag: &str) -> &str {
    etag.strip_prefix("W/").unwrap_or(etag)
}

/// `If-None-Match` evaluated with the weak comparison function
/// (RFC 9110 §13.1.2): `*` or any listed tag with the same opaque value.
pub fn if_none_match_hits(value: &str, etag: &str) -> bool {
    if value.trim() == "*" {
        return true;
    }
    let current = opaque_tag(etag);
    entity_tags(value)
        .into_iter()
        .any(|(_, tag)| tag == current)
}

fn header_values(headers: &HeaderMap, name: &HeaderName) -> Option<String> {
    let mut values = headers.get_all(name).iter().peekable();
    values.peek()?;
    let mut joined = String::new();
    for value in values {
        let Ok(value) = value.to_str() else {
            continue;
        };
        if !joined.is_empty() {
            joined.push_str(", ");
        }
        joined.push_str(value);
    }
    Some(joined)
}

fn whole_seconds(time: SystemTime) -> SystemTime {
    match time.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => UNIX_EPOCH + Duration::from_secs(elapsed.as_secs()),
        Err(_) => time,
    }
}

/// Whether a GET/HEAD with these headers can be answered with 304 Not
/// Modified. `If-None-Match` takes precedence over `If-Modified-Since`, and
/// an `If-Modified-Since` date later than the server's clock is invalid, so
/// it is ignored (a client can't pin a representation with a future date).
pub fn is_not_modified(headers: &HeaderMap, etag: &str, last_modified: Option<SystemTime>) -> bool {
    is_not_modified_at(headers, etag, last_modified, SystemTime::now())
}

fn is_not_modified_at(
    headers: &HeaderMap,
    etag: &str,
    last_modified: Option<SystemTime>,
    now: SystemTime,
) -> bool {
    if let Some(value) = header_values(headers, &header::IF_NONE_MATCH) {
        return if_none_match_hits(&value, etag);
    }
    let since = headers
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| httpdate::parse_http_date(value.trim()).ok())
        .filter(|since| *since <= now);
    match (since, last_modified) {
        (Some(since), Some(modified)) => whole_seconds(modified) <= since,
        _ => false,
    }
}

/// `If-Range`: the range applies only while the validator still matches —
/// a strong entity tag equal to the current one, or exactly the current
/// Last-Modified date.
pub fn if_range_matches(value: &str, etag: &str, last_modified: Option<SystemTime>) -> bool {
    let value = value.trim();
    if value.starts_with('"') || value.starts_with("W/") {
        return !value.starts_with("W/") && !etag.starts_with("W/") && value == etag;
    }
    match (httpdate::parse_http_date(value), last_modified) {
        (Ok(date), Some(modified)) => whole_seconds(modified) == date,
        _ => false,
    }
}

/// The range to serve, honouring `If-Range`.
pub fn requested_range(
    headers: &HeaderMap,
    size: u64,
    etag: &str,
    last_modified: Option<SystemTime>,
) -> RangeRequest {
    let Some(range) = headers.get(header::RANGE) else {
        return RangeRequest::Full;
    };
    let Ok(range) = range.to_str() else {
        return RangeRequest::Full;
    };
    let range_applies = match headers.get(header::IF_RANGE) {
        None => true,
        Some(value) => value
            .to_str()
            .is_ok_and(|value| if_range_matches(value, etag, last_modified)),
    };
    if range_applies {
        parse_range(range, size)
    } else {
        RangeRequest::Full
    }
}

/// Milliseconds since the epoch (0 for times before it).
pub fn unix_millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Strong ETag of a stored file: `"<size hex>-<mtime ms hex>"`.
pub fn file_etag(size: u64, mtime_ms: i64) -> String {
    format!("\"{size:x}-{:x}\"", mtime_ms.max(0))
}

pub fn set_header(response: &mut Response, name: HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}

fn empty(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response
}

/// A `text/plain` response that must not be cached.
pub fn plain_response(status: StatusCode, message: &'static str) -> Response {
    let mut response = Response::new(Body::from(message));
    *response.status_mut() = status;
    set_header(&mut response, header::CONTENT_TYPE, TEXT_PLAIN);
    set_header(&mut response, header::CACHE_CONTROL, "no-store");
    response
}

/// Headers of a file response besides the range/length ones.
pub struct FileHeaders<'a> {
    pub content_type: &'a str,
    pub cache_control: &'a str,
    pub content_disposition: Option<&'a str>,
}

/// Answers a GET/HEAD for an already opened file: 304 when the conditional
/// headers match, 206 or 416 for a single byte range, 200 otherwise. Only
/// the headers are produced for HEAD, which ignores `Range` (ranges are
/// defined for GET only), so it always describes the whole file.
pub async fn file_response(
    request: &HeaderMap,
    head_only: bool,
    file: File,
    size: u64,
    modified: Option<SystemTime>,
    headers: &FileHeaders<'_>,
) -> io::Result<Response> {
    let etag = file_etag(size, modified.map(unix_millis).unwrap_or(0));
    let last_modified = modified.map(httpdate::fmt_http_date);
    let validators = |response: &mut Response| {
        set_header(response, header::ETAG, &etag);
        set_header(response, header::CACHE_CONTROL, headers.cache_control);
        if let Some(last_modified) = &last_modified {
            set_header(response, header::LAST_MODIFIED, last_modified);
        }
    };

    if is_not_modified(request, &etag, modified) {
        let mut response = empty(StatusCode::NOT_MODIFIED);
        validators(&mut response);
        return Ok(response);
    }

    let wanted = if head_only {
        RangeRequest::Full
    } else {
        requested_range(request, size, &etag, modified)
    };
    let (status, range) = match wanted {
        RangeRequest::Unsatisfiable => {
            let mut response =
                plain_response(StatusCode::RANGE_NOT_SATISFIABLE, "Range Not Satisfiable");
            set_header(
                &mut response,
                header::CONTENT_RANGE,
                &format!("bytes */{size}"),
            );
            set_header(&mut response, header::ACCEPT_RANGES, "bytes");
            return Ok(response);
        }
        RangeRequest::Partial(range) => (StatusCode::PARTIAL_CONTENT, Some(range)),
        RangeRequest::Full => (StatusCode::OK, None),
    };

    let mut response = empty(status);
    validators(&mut response);
    set_header(&mut response, header::CONTENT_TYPE, headers.content_type);
    set_header(&mut response, header::ACCEPT_RANGES, "bytes");
    if let Some(disposition) = headers.content_disposition {
        set_header(&mut response, header::CONTENT_DISPOSITION, disposition);
    }
    let length = match range {
        Some(range) => {
            set_header(
                &mut response,
                header::CONTENT_RANGE,
                &format!("bytes {}-{}/{size}", range.start, range.end),
            );
            range.length()
        }
        None => size,
    };
    set_header(&mut response, header::CONTENT_LENGTH, &length.to_string());
    if !head_only {
        *response.body_mut() = file_body(file, range).await?;
    }
    Ok(response)
}

/// Streams `file` (or one range of it) in [`CHUNK_SIZE`] reads. Dropping the
/// body closes the file.
pub async fn file_body(mut file: File, range: Option<ByteRange>) -> io::Result<Body> {
    Ok(match range {
        None => Body::from_stream(ReaderStream::with_capacity(file, CHUNK_SIZE)),
        Some(range) => {
            file.seek(io::SeekFrom::Start(range.start)).await?;
            Body::from_stream(ReaderStream::with_capacity(
                file.take(range.length()),
                CHUNK_SIZE,
            ))
        }
    })
}

/// Progress of a file that is still being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Growth {
    /// `written` bytes are on disk and readable.
    Writing { written: u64 },
    /// The file is complete at `size` bytes.
    Complete { size: u64 },
    /// The writer gave up; the file is incomplete.
    Failed,
}

struct GrowingReader<T> {
    file: File,
    offset: u64,
    progress: watch::Receiver<T>,
    growth: fn(&T) -> Growth,
    buffer: Vec<u8>,
    finished: bool,
}

impl<T> GrowingReader<T> {
    async fn next_chunk(&mut self) -> io::Result<Option<Bytes>> {
        loop {
            let growth = (self.growth)(&self.progress.borrow_and_update());
            let (available, complete) = match growth {
                Growth::Writing { written } => (written.saturating_sub(self.offset), false),
                Growth::Complete { size } => (size.saturating_sub(self.offset), true),
                Growth::Failed => return Err(io::Error::other("the transcode failed")),
            };
            if available > 0 {
                let wanted = available.min(CHUNK_SIZE as u64) as usize;
                let read = self.file.read(&mut self.buffer[..wanted]).await?;
                if read == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the transcode output ended early",
                    ));
                }
                self.offset += read as u64;
                return Ok(Some(Bytes::copy_from_slice(&self.buffer[..read])));
            }
            if complete {
                return Ok(None);
            }
            if self.progress.changed().await.is_err() {
                // The writer is gone: finish only if it completed.
                let growth = (self.growth)(&self.progress.borrow());
                if !matches!(growth, Growth::Complete { .. }) {
                    return Err(io::Error::other("the transcode stopped"));
                }
            }
        }
    }
}

/// Streams a file that another task is still writing, following `progress`
/// until the writer reports completion (or failure, which aborts the body).
/// The reader keeps its own file handle, so it survives the writer renaming
/// the file when it is done.
pub fn growing_file_body<T>(
    file: File,
    progress: watch::Receiver<T>,
    growth: fn(&T) -> Growth,
) -> Body
where
    T: Send + Sync + 'static,
{
    let reader = GrowingReader {
        file,
        offset: 0,
        progress,
        growth,
        buffer: vec![0; CHUNK_SIZE],
        finished: false,
    };
    Body::from_stream(stream::unfold(reader, |mut reader| async move {
        if reader.finished {
            return None;
        }
        match reader.next_chunk().await {
            Ok(Some(chunk)) => Some((Ok(chunk), reader)),
            Ok(None) => None,
            Err(error) => {
                reader.finished = true;
                Some((Err(error), reader))
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partial(start: u64, end: u64) -> RangeRequest {
        RangeRequest::Partial(ByteRange { start, end })
    }

    #[test]
    fn serves_single_byte_ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), partial(0, 99));
        assert_eq!(parse_range("bytes=5-", 1000), partial(5, 999));
        assert_eq!(parse_range("bytes=-10", 1000), partial(990, 999));
        assert_eq!(parse_range("bytes=-2000", 1000), partial(0, 999));
        assert_eq!(parse_range("bytes=0-100000", 1000), partial(0, 999));
        assert_eq!(parse_range("bytes=999-999", 1000), partial(999, 999));
        // The unit is case-insensitive; whitespace around the parts is tolerated.
        assert_eq!(parse_range("Bytes=0-0", 1000), partial(0, 0));
        assert_eq!(parse_range("BYTES = 1 - 2", 1000), partial(1, 2));
        // A trailing empty list member is not a second range.
        assert_eq!(parse_range("bytes=0-9,", 1000), partial(0, 9));
        // Positions beyond u64 saturate: a huge end or suffix covers the rest.
        assert_eq!(
            parse_range("bytes=0-99999999999999999999", 1000),
            partial(0, 999)
        );
        assert_eq!(
            parse_range("bytes=-99999999999999999999", 1000),
            partial(0, 999)
        );
    }

    #[test]
    fn ignores_ranges_it_does_not_understand() {
        for value in [
            "bytes=nonsense",
            "bytes=-",
            "bytes=",
            "bytes",
            "items=0-9",
            "bytes=0-99,200-",
            "bytes=5-2",
            "bytes=a-b",
            "bytes=1.5-2",
            "bytes=0x10-",
            "=0-1",
        ] {
            assert_eq!(parse_range(value, 1000), RangeRequest::Full, "{value}");
        }
    }

    #[test]
    fn rejects_ranges_past_the_end() {
        assert_eq!(
            parse_range("bytes=1000-", 1000),
            RangeRequest::Unsatisfiable
        );
        assert_eq!(
            parse_range("bytes=1000-2000", 1000),
            RangeRequest::Unsatisfiable
        );
        assert_eq!(parse_range("bytes=-0", 1000), RangeRequest::Unsatisfiable);
        assert_eq!(
            parse_range("bytes=99999999999999999999-", 1000),
            RangeRequest::Unsatisfiable
        );
        assert_eq!(parse_range("bytes=0-", 0), RangeRequest::Unsatisfiable);
        assert_eq!(parse_range("bytes=-5", 0), RangeRequest::Unsatisfiable);
    }

    #[test]
    fn if_none_match_uses_weak_comparison() {
        let etag = "\"1f-2a\"";
        assert!(if_none_match_hits("\"1f-2a\"", etag));
        assert!(if_none_match_hits("W/\"1f-2a\"", etag));
        assert!(if_none_match_hits("\"x\", W/\"1f-2a\"", etag));
        assert!(if_none_match_hits("\"x\",\"1f-2a\"", etag));
        assert!(if_none_match_hits(" * ", etag));
        assert!(if_none_match_hits("\"1f-2a\"", "W/\"1f-2a\""));
        assert!(!if_none_match_hits("\"1f-2b\"", etag));
        assert!(!if_none_match_hits("1f-2a", etag));
        assert!(!if_none_match_hits("", etag));
        // Commas inside a quoted tag do not split it.
        assert!(if_none_match_hits("\"a,b\"", "\"a,b\""));
        assert!(!if_none_match_hits("\"a,b\"", "\"a\""));
        assert!(if_none_match_hits("junk, \"1f-2a\"", etag));
    }

    #[test]
    fn if_modified_since_applies_without_if_none_match() {
        let modified = UNIX_EPOCH + Duration::from_millis(1_700_000_000_500);
        let at = httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        let earlier = httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(1_699_999_999));
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_MODIFIED_SINCE, at.parse().unwrap());
        assert!(is_not_modified(&headers, "\"a\"", Some(modified)));
        headers.insert(header::IF_MODIFIED_SINCE, earlier.parse().unwrap());
        assert!(!is_not_modified(&headers, "\"a\"", Some(modified)));
        // If-None-Match wins over If-Modified-Since.
        headers.insert(header::IF_MODIFIED_SINCE, at.parse().unwrap());
        headers.insert(header::IF_NONE_MATCH, "\"b\"".parse().unwrap());
        assert!(!is_not_modified(&headers, "\"a\"", Some(modified)));
        headers.append(header::IF_NONE_MATCH, "\"a\"".parse().unwrap());
        assert!(is_not_modified(&headers, "\"a\"", Some(modified)));
    }

    /// RFC 9110 §13.1.3: an `If-Modified-Since` later than the server's
    /// clock is invalid and must not produce a 304.
    #[test]
    fn if_modified_since_in_the_future_is_ignored() {
        let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let now = modified + Duration::from_secs(3600);
        let mut headers = HeaderMap::new();
        let future = httpdate::fmt_http_date(now + Duration::from_secs(86_400));
        headers.insert(header::IF_MODIFIED_SINCE, future.parse().unwrap());
        assert!(!is_not_modified_at(&headers, "\"a\"", Some(modified), now));
        let present = httpdate::fmt_http_date(now);
        headers.insert(header::IF_MODIFIED_SINCE, present.parse().unwrap());
        assert!(is_not_modified_at(&headers, "\"a\"", Some(modified), now));
        // The real clock: a date far in the future never matches.
        headers.insert(
            header::IF_MODIFIED_SINCE,
            "Fri, 31 Dec 9999 23:59:59 GMT".parse().unwrap(),
        );
        assert!(!is_not_modified(&headers, "\"a\"", Some(modified)));
    }

    /// HEAD describes the whole file: `Range` (and so 206/416) is for GET.
    #[tokio::test]
    async fn head_ignores_ranges() {
        let dir = std::env::temp_dir().join(format!("yt-serve-head-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.mp3");
        std::fs::write(&path, vec![7u8; 100]).unwrap();
        let file_headers = FileHeaders {
            content_type: "audio/mpeg",
            cache_control: "public, no-cache",
            content_disposition: None,
        };
        let respond = |range: &'static str, head_only: bool| {
            let path = path.clone();
            let file_headers = &file_headers;
            async move {
                let mut request = HeaderMap::new();
                request.insert(header::RANGE, range.parse().unwrap());
                let file = File::open(&path).await.unwrap();
                file_response(&request, head_only, file, 100, None, file_headers)
                    .await
                    .unwrap()
            }
        };

        for range in ["bytes=0-9", "bytes=100-", "bytes=-0"] {
            let head = respond(range, true).await;
            assert_eq!(head.status(), StatusCode::OK, "HEAD {range}");
            assert_eq!(head.headers()[header::CONTENT_LENGTH], "100");
            assert!(head.headers().get(header::CONTENT_RANGE).is_none());
            assert_eq!(head.headers()[header::ACCEPT_RANGES], "bytes");
            assert!(head.headers().contains_key(header::ETAG));
        }
        let get = respond("bytes=0-9", false).await;
        assert_eq!(get.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(get.headers()[header::CONTENT_RANGE], "bytes 0-9/100");
        let past = respond("bytes=100-", false).await;
        assert_eq!(past.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn if_range_needs_a_strong_match() {
        let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let date = httpdate::fmt_http_date(modified);
        assert!(if_range_matches("\"1f-2a\"", "\"1f-2a\"", None));
        assert!(!if_range_matches("W/\"1f-2a\"", "\"1f-2a\"", None));
        assert!(!if_range_matches("\"other\"", "\"1f-2a\"", None));
        assert!(if_range_matches(&date, "\"1f-2a\"", Some(modified)));
        assert!(!if_range_matches(
            &date,
            "\"1f-2a\"",
            Some(modified + Duration::from_secs(1))
        ));
        assert!(!if_range_matches("yesterday", "\"1f-2a\"", Some(modified)));

        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, "bytes=0-9".parse().unwrap());
        assert_eq!(requested_range(&headers, 100, "\"e\"", None), partial(0, 9));
        headers.insert(header::IF_RANGE, "\"stale\"".parse().unwrap());
        assert_eq!(
            requested_range(&headers, 100, "\"e\"", None),
            RangeRequest::Full
        );
        headers.insert(header::IF_RANGE, "\"e\"".parse().unwrap());
        assert_eq!(requested_range(&headers, 100, "\"e\"", None), partial(0, 9));
    }

    #[test]
    fn file_etags_are_size_and_mtime() {
        assert_eq!(file_etag(255, 4096), "\"ff-1000\"");
        assert_eq!(file_etag(1, -5), "\"1-0\"");
    }

    #[tokio::test]
    async fn growing_bodies_follow_the_writer() {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;

        let dir = std::env::temp_dir().join(format!("yt-growing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.tmp");
        let mut writer = tokio::fs::File::create(&path).await.unwrap();
        let (sender, receiver) = watch::channel(Growth::Writing { written: 0 });
        let reader = File::open(&path).await.unwrap();
        let mut body =
            growing_file_body(reader, receiver, |growth: &Growth| *growth).into_data_stream();

        let feeder = tokio::spawn(async move {
            let mut written = 0u64;
            for chunk in [b"hello ".as_slice(), b"growing ", b"world"] {
                writer.write_all(chunk).await.unwrap();
                writer.flush().await.unwrap();
                written += chunk.len() as u64;
                sender.send_replace(Growth::Writing { written });
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            sender.send_replace(Growth::Complete { size: written });
        });
        let mut collected = Vec::new();
        while let Some(chunk) = body.next().await {
            collected.extend_from_slice(&chunk.unwrap());
        }
        feeder.await.unwrap();
        assert_eq!(collected, b"hello growing world");

        // A failing writer aborts the body with an error.
        let (sender, receiver) = watch::channel(Growth::Writing { written: 0 });
        let reader = File::open(&path).await.unwrap();
        let mut body =
            growing_file_body(reader, receiver, |growth: &Growth| *growth).into_data_stream();
        sender.send_replace(Growth::Failed);
        assert!(body.next().await.unwrap().is_err());
        assert!(body.next().await.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
