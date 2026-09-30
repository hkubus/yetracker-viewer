//! In-memory playable-file set plus a size/mtime cache, ported from
//! `util/playableFiles.ts`.
//!
//! The set is rebuilt from disk by every sync; lookups that miss it re-stat
//! the file, and a miss confirmed on disk is remembered for a short while
//! ([`NEGATIVE_TTL`]) so listings with many missing files don't stat them on
//! every request. A media request that finds a listed file gone drops it
//! from the set at once ([`PlayableFiles::mark_missing`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use futures_util::stream::{self, StreamExt};

use crate::error::ApiError;
use crate::text::utf16_len;

const SCAN_CONCURRENCY: usize = 32;

/// How long a file confirmed missing (or empty) is reported unplayable
/// without looking at the disk again.
pub const NEGATIVE_TTL: Duration = Duration::from_secs(15);

/// Remembered misses before expired ones are swept.
const MAX_NEGATIVE_ENTRIES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FileMeta {
    pub size: u64,
    pub mtime_ms: i64,
}

#[derive(Default)]
struct Inner {
    files: HashSet<String>,
    meta: HashMap<String, FileMeta>,
}

pub struct PlayableFiles {
    inner: RwLock<Inner>,
    /// Files seen missing or empty, with when that was checked.
    misses: Mutex<HashMap<String, Instant>>,
}

impl PlayableFiles {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
            misses: Mutex::new(HashMap::new()),
        }
    }

    /// Rebuilds the set from disk, then publishes it atomically.
    pub async fn refresh(&self, songs_path: &Path) -> Result<(), ApiError> {
        let mut entries = tokio::fs::read_dir(songs_path)
            .await
            .map_err(ApiError::unexpected)?;
        let mut filenames = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(ApiError::unexpected)? {
            let file_type = entry.file_type().await.map_err(ApiError::unexpected)?;
            if file_type.is_file() {
                filenames.push(entry.file_name().to_string_lossy().into_owned());
            }
        }

        let scanned: Vec<(String, Option<FileMeta>)> =
            stream::iter(filenames.into_iter().map(|filename| {
                let dir = songs_path.to_path_buf();
                async move {
                    let meta = stat_meta(&dir.join(&filename)).await;
                    (filename, meta)
                }
            }))
            .buffer_unordered(SCAN_CONCURRENCY)
            .collect()
            .await;

        let mut next = Inner::default();
        for (filename, meta) in scanned {
            if let Some(meta) = meta
                && meta.size > 0
            {
                next.meta.insert(filename.clone(), meta);
                next.files.insert(filename);
            }
        }

        let mut inner = self.inner.write().expect("playable files lock poisoned");
        *inner = next;
        drop(inner);
        self.misses
            .lock()
            .expect("playable misses lock poisoned")
            .clear();
        Ok(())
    }

    pub fn get_meta(&self, filename: &str) -> Option<FileMeta> {
        self.inner
            .read()
            .expect("playable files lock poisoned")
            .meta
            .get(filename)
            .copied()
    }

    pub fn is_playable(&self, filename: Option<&str>) -> bool {
        let Some(filename) = filename else {
            return false;
        };
        if !is_bare_filename(filename) {
            return false;
        }
        self.inner
            .read()
            .expect("playable files lock poisoned")
            .files
            .contains(filename)
    }

    /// [`Self::is_playable`], but re-stats the file when the set does not know
    /// it yet, memoising the answer.
    ///
    /// The set is a snapshot taken by the last sync, so media copied in or
    /// restored out of band stays invisible to it until the next one — even
    /// though `/songs/{id}/stream` serves that very file, because the stream
    /// path falls back to a direct `stat` when the meta cache misses.
    /// Resolving a miss the same way keeps listings and streaming in
    /// agreement. Misses are remembered for [`NEGATIVE_TTL`].
    pub async fn resolve_playable(&self, songs_path: &Path, filename: Option<&str>) -> bool {
        let Some(filename) = filename else {
            return false;
        };
        if !is_bare_filename(filename) {
            return false;
        }
        if self.is_playable(Some(filename)) {
            return true;
        }
        if self.recently_missing(filename) {
            return false;
        }
        self.refresh_one(songs_path, filename).await
    }

    /// [`Self::resolve_playable`] for callers already on a blocking thread
    /// (e.g. inside a database closure).
    pub fn resolve_playable_blocking(&self, songs_path: &Path, filename: Option<&str>) -> bool {
        let Some(filename) = filename else {
            return false;
        };
        if !is_bare_filename(filename) {
            return false;
        }
        if self.is_playable(Some(filename)) {
            return true;
        }
        if self.recently_missing(filename) {
            return false;
        }
        self.record_stat(filename, std::fs::metadata(songs_path.join(filename)).ok())
    }

    fn recently_missing(&self, filename: &str) -> bool {
        let misses = self.misses.lock().expect("playable misses lock poisoned");
        misses
            .get(filename)
            .is_some_and(|checked| checked.elapsed() < NEGATIVE_TTL)
    }

    fn remember_miss(&self, filename: &str) {
        let mut misses = self.misses.lock().expect("playable misses lock poisoned");
        if misses.len() >= MAX_NEGATIVE_ENTRIES {
            misses.retain(|_, checked| checked.elapsed() < NEGATIVE_TTL);
            if misses.len() >= MAX_NEGATIVE_ENTRIES {
                misses.clear();
            }
        }
        misses.insert(filename.to_string(), Instant::now());
    }

    /// A request found `filename` missing or empty: stop reporting it as
    /// playable right away (listings, `playable=true`, `/status`) instead of
    /// trusting the set until the next sync rescans the disk. Memory only;
    /// the stored download state is the sync's business.
    pub fn mark_missing(&self, filename: &str) {
        if !is_bare_filename(filename) {
            return;
        }
        self.set_playable(filename, false);
        self.remember_miss(filename);
    }

    pub fn set_playable(&self, filename: &str, playable: bool) {
        if !is_bare_filename(filename) {
            return;
        }
        let mut inner = self.inner.write().expect("playable files lock poisoned");
        if playable {
            inner.files.insert(filename.to_string());
        } else {
            inner.files.remove(filename);
            inner.meta.remove(filename);
        }
        drop(inner);
        if playable {
            self.misses
                .lock()
                .expect("playable misses lock poisoned")
                .remove(filename);
        }
    }

    /// Re-stats a single file and updates the caches; returns whether it is playable.
    pub async fn refresh_one(&self, songs_path: &Path, filename: &str) -> bool {
        if !is_bare_filename(filename) {
            return false;
        }
        let metadata = tokio::fs::metadata(songs_path.join(filename)).await.ok();
        self.record_stat(filename, metadata)
    }

    fn record_stat(&self, filename: &str, metadata: Option<std::fs::Metadata>) -> bool {
        match metadata.filter(|meta| meta.is_file() && meta.len() > 0) {
            Some(meta) => {
                self.set_playable(filename, true);
                self.inner
                    .write()
                    .expect("playable files lock poisoned")
                    .meta
                    .insert(filename.to_string(), meta_of(&meta));
                true
            }
            None => {
                self.set_playable(filename, false);
                self.remember_miss(filename);
                false
            }
        }
    }
}

impl Default for PlayableFiles {
    fn default() -> Self {
        Self::new()
    }
}

/// `basename(filename) === filename` on POSIX.
pub fn is_bare_filename(filename: &str) -> bool {
    !filename.contains('/')
}

/// Filename validation shared by `storedSongPath` and `isSafeFilename`.
pub fn is_safe_filename(filename: &str) -> bool {
    let length = utf16_len(filename);
    length > 0
        && length <= 255
        && is_bare_filename(filename)
        && !filename.contains('\\')
        && !filename.contains('\0')
}

/// `storedSongPath`: rejects unsafe names with `404 Song file not found`.
pub fn stored_song_path(songs_path: &Path, filename: &str) -> Result<PathBuf, ApiError> {
    let length = utf16_len(filename);
    if length == 0
        || length > 255
        || filename == "."
        || filename == ".."
        || !is_safe_filename(filename)
    {
        return Err(ApiError::not_found("Song file not found"));
    }
    Ok(songs_path.join(filename))
}

pub fn meta_of(metadata: &std::fs::Metadata) -> FileMeta {
    FileMeta {
        size: metadata.len(),
        mtime_ms: mtime_ms_of(metadata),
    }
}

pub fn mtime_ms_of(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

async fn stat_meta(path: &Path) -> Option<FileMeta> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    Some(meta_of(&metadata))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_filenames() {
        assert!(is_safe_filename("song.mp3"));
        assert!(!is_safe_filename(""));
        // `isSafeFilename` only guards the path shape; `.`/`..` are rejected
        // by `storedSongPath` (see below), not here.
        assert!(is_safe_filename("."));
        assert!(is_safe_filename(".."));
        assert!(!is_safe_filename("a/b"));
        assert!(!is_safe_filename("a\\b"));
        assert!(!is_safe_filename("a\0b"));
        assert!(!is_safe_filename(&"x".repeat(256)));
        assert!(stored_song_path(Path::new("/tmp"), "..").is_err());
        assert!(stored_song_path(Path::new("/tmp"), ".").is_err());
        assert!(stored_song_path(Path::new("/tmp"), "ok.mp3").is_ok());
    }

    #[test]
    fn set_playable_ignores_nested_names() {
        let files = PlayableFiles::new();
        files.set_playable("nested/name.mp3", true);
        assert!(!files.is_playable(Some("nested/name.mp3")));
        files.set_playable("name.mp3", true);
        assert!(files.is_playable(Some("name.mp3")));
        assert!(!files.is_playable(None));
        files.set_playable("name.mp3", false);
        assert!(!files.is_playable(Some("name.mp3")));
    }

    /// A file restored out of band is invisible to the boot-time set, but the
    /// stream endpoint still serves it (it stats on a meta-cache miss). The
    /// listing endpoints must agree, or songs read as unplayable while
    /// streaming fine.
    #[tokio::test]
    async fn resolve_playable_picks_up_files_added_after_the_boot_scan() {
        let dir = std::env::temp_dir().join(format!("yt-playable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp songs dir");

        let files = PlayableFiles::new();
        files.refresh(&dir).await.expect("initial scan");

        // Not on disk yet, and absent from the DB's point of view.
        assert!(!files.is_playable(Some("restored.mp3")));
        assert!(!files.resolve_playable(&dir, Some("restored.mp3")).await);

        std::fs::write(dir.join("restored.mp3"), b"audio").expect("write media file");
        // The miss above is remembered for a while; a rescan forgets it.
        assert!(!files.resolve_playable(&dir, Some("restored.mp3")).await);
        std::fs::remove_file(dir.join("restored.mp3")).expect("hide media file");
        files.refresh(&dir).await.expect("rescan");
        std::fs::write(dir.join("restored.mp3"), b"audio").expect("write media file");

        // Still unknown to the set built at boot...
        assert!(!files.is_playable(Some("restored.mp3")));
        // ...but resolving it stats the directory, so the listing agrees with
        // the stream endpoint, and the answer is memoised from then on.
        assert!(files.resolve_playable(&dir, Some("restored.mp3")).await);
        assert!(files.is_playable(Some("restored.mp3")));

        // Empty files are not playable, and a `NULL` filename short-circuits.
        std::fs::write(dir.join("empty.mp3"), b"").expect("write empty file");
        assert!(!files.resolve_playable(&dir, Some("empty.mp3")).await);
        assert!(!files.resolve_playable(&dir, None).await);
        assert!(!files.resolve_playable(&dir, Some("nested/name.mp3")).await);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn long_ago() -> Instant {
        Instant::now()
            .checked_sub(NEGATIVE_TTL + Duration::from_secs(1))
            .expect("the monotonic clock runs longer than the TTL")
    }

    /// A confirmed miss is not re-checked on disk until it expires, a
    /// rescan, or the downloader marking the file playable.
    #[test]
    fn misses_are_remembered_briefly() {
        let dir = std::env::temp_dir().join(format!("yt-playable-miss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp songs dir");

        let files = PlayableFiles::new();
        assert!(!files.resolve_playable_blocking(&dir, Some("late.mp3")));
        std::fs::write(dir.join("late.mp3"), b"audio").expect("write media file");
        // Still inside the negative TTL: no new stat.
        assert!(!files.resolve_playable_blocking(&dir, Some("late.mp3")));

        // Expire the miss by hand instead of sleeping for the TTL.
        files
            .misses
            .lock()
            .unwrap()
            .insert("late.mp3".to_string(), long_ago());
        assert!(files.resolve_playable_blocking(&dir, Some("late.mp3")));

        // The downloader's `set_playable` clears a remembered miss at once.
        assert!(!files.resolve_playable_blocking(&dir, Some("next.mp3")));
        std::fs::write(dir.join("next.mp3"), b"audio").expect("write media file");
        files.set_playable("next.mp3", true);
        assert!(files.resolve_playable_blocking(&dir, Some("next.mp3")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file a request found gone stops counting as playable at once, even
    /// though the set built by the last sync still lists it.
    #[tokio::test]
    async fn files_found_missing_stop_being_playable() {
        let dir = std::env::temp_dir().join(format!("yt-playable-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp songs dir");
        std::fs::write(dir.join("gone.mp3"), b"audio").expect("write media file");
        let files = PlayableFiles::new();
        files.refresh(&dir).await.expect("scan");
        assert!(files.resolve_playable_blocking(&dir, Some("gone.mp3")));
        assert!(files.get_meta("gone.mp3").is_some());

        std::fs::remove_file(dir.join("gone.mp3")).expect("delete media file");
        // The snapshot still believes in it until someone notices.
        assert!(files.resolve_playable_blocking(&dir, Some("gone.mp3")));
        files.mark_missing("gone.mp3");
        assert!(!files.is_playable(Some("gone.mp3")));
        assert!(files.get_meta("gone.mp3").is_none());
        assert!(!files.resolve_playable_blocking(&dir, Some("gone.mp3")));
        assert!(!files.resolve_playable(&dir, Some("gone.mp3")).await);

        // Once the file is back (and the miss expired, or a download marked
        // it playable), it counts again.
        std::fs::write(dir.join("gone.mp3"), b"audio").expect("restore media file");
        files.set_playable("gone.mp3", true);
        assert!(files.resolve_playable_blocking(&dir, Some("gone.mp3")));
        files.mark_missing("nested/name.mp3");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
