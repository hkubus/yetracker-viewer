//! Range parsing and abort-safe file streaming, mirroring `util/serveFile.ts`
//! and the range blocks in the stream/download routes.

use std::path::Path;

use axum::body::Body;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileRange {
    pub start: u64,
    pub end: u64,
}

/// Parses `Range: bytes=N-M | N- | -N`.
///
/// Returns `None` for malformed *and* unsatisfiable ranges: both routes treat
/// those identically (stream answers 416, download falls back to a 200 body).
pub fn parse_range(header: &str, size: u64) -> Option<FileRange> {
    let rest = header.strip_prefix("bytes=")?;
    let (start_text, end_text) = rest.split_once('-')?;
    if end_text.contains('-') {
        return None;
    }
    let digits_or_empty =
        |value: &str| value.bytes().all(|byte| byte.is_ascii_digit());
    if !digits_or_empty(start_text) || !digits_or_empty(end_text) {
        return None;
    }

    let size = size as i128;
    let mut start: i128 = if start_text.is_empty() { 0 } else { start_text.parse().ok()? };
    let mut end: i128 = if end_text.is_empty() { size - 1 } else { end_text.parse().ok()? };

    if start_text.is_empty() && !end_text.is_empty() {
        let suffix_length: i128 = end_text.parse().ok()?;
        start = (size - suffix_length).max(0);
        end = size - 1;
    }

    if start < 0 || start >= size || end < start {
        return None;
    }
    end = end.min(size - 1);
    if end < 0 {
        return None;
    }
    Some(FileRange {
        start: start as u64,
        end: end as u64,
    })
}

/// Streams `path` (or the requested byte range) as a response body. Dropping
/// the body aborts the read and closes the file handle.
pub async fn file_body(path: &Path, range: Option<FileRange>) -> std::io::Result<Body> {
    let mut file = tokio::fs::File::open(path).await?;
    match range {
        None => Ok(Body::from_stream(ReaderStream::new(file))),
        Some(range) => {
            file.seek(std::io::SeekFrom::Start(range.start)).await?;
            let length = range.end - range.start + 1;
            Ok(Body::from_stream(ReaderStream::new(file.take(length))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_closed_open_and_suffix_ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some(FileRange { start: 0, end: 99 }));
        assert_eq!(parse_range("bytes=5-", 1000), Some(FileRange { start: 5, end: 999 }));
        assert_eq!(parse_range("bytes=-10", 1000), Some(FileRange { start: 990, end: 999 }));
        assert_eq!(parse_range("bytes=-2000", 1000), Some(FileRange { start: 0, end: 999 }));
        assert_eq!(parse_range("bytes=0-100000", 1000), Some(FileRange { start: 0, end: 999 }));
        assert_eq!(parse_range("bytes=-", 1000), Some(FileRange { start: 0, end: 999 }));
    }

    #[test]
    fn rejects_malformed_and_unsatisfiable_ranges() {
        assert_eq!(parse_range("bytes=nonsense", 1000), None);
        assert_eq!(parse_range("bytes=5-2", 1000), None);
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=0-99,200-", 1000), None);
        assert_eq!(parse_range("items=0-9", 1000), None);
        assert_eq!(parse_range("bytes=-0", 1000), None);
        assert_eq!(parse_range("bytes=99999999999999999999-", 1000), None);
    }
}
