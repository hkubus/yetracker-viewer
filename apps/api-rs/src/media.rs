//! External media tools and file verification: detection of `ffmpeg`,
//! `ffprobe` and `yt-dlp`, ffprobe verdicts (shared and cached per file
//! version), quarantining of media files (rejected by ffprobe, or retired by
//! the cleanup), and the queue of files a request found missing or broken,
//! which the background sync re-verifies.
//!
//! Deletion safety: a media file is only moved aside when ffprobe ran to
//! completion and reported invalid data or no audio stream, or when no row
//! refers to it any more. Timeouts, spawn errors and signals leave the
//! verdict unknown and the file untouched. Quarantined files are deleted
//! after a grace period.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use lru::LruCache;
use serde_json::Value;
use tokio::process::Command;
use tracing::{info, warn};

const TOOL_CHECK_TIMEOUT: Duration = Duration::from_secs(15);
/// A tool found missing is looked for again at most this often.
const TOOL_RECHECK_INTERVAL: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_CACHED_PROBES: usize = 1024;
const MAX_QUEUED_REVERIFICATIONS: usize = 10_000;

/// An external program the API shells out to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Ffmpeg,
    Ffprobe,
    YtDlp,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::Ffmpeg, Tool::Ffprobe, Tool::YtDlp];

    /// Program name, resolved through `PATH`.
    pub fn binary(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "ffmpeg",
            Tool::Ffprobe => "ffprobe",
            Tool::YtDlp => "yt-dlp",
        }
    }

    fn version_flag(self) -> &'static str {
        match self {
            Tool::Ffmpeg | Tool::Ffprobe => "-version",
            Tool::YtDlp => "--version",
        }
    }
}

impl fmt::Display for Tool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.binary())
    }
}

/// Availability of every tool at one point in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ToolSet {
    pub ffmpeg: bool,
    pub ffprobe: bool,
    pub yt_dlp: bool,
}

impl ToolSet {
    pub fn has(&self, tool: Tool) -> bool {
        match tool {
            Tool::Ffmpeg => self.ffmpeg,
            Tool::Ffprobe => self.ffprobe,
            Tool::YtDlp => self.yt_dlp,
        }
    }

    pub fn missing(&self) -> Vec<Tool> {
        Tool::ALL
            .into_iter()
            .filter(|tool| !self.has(*tool))
            .collect()
    }
}

/// Cached tool availability. Detected at boot and at the start of every sync;
/// a spawn that fails with "not found" in between marks the tool missing.
/// Asking for a missing tool starts a new detection in the background (at
/// most once a minute), so a tool installed later is picked up without
/// waiting for the next sync.
#[derive(Default)]
pub struct Tools {
    inner: Arc<ToolFlags>,
}

#[derive(Default)]
struct ToolFlags {
    ffmpeg: AtomicBool,
    ffprobe: AtomicBool,
    yt_dlp: AtomicBool,
    detected: AtomicBool,
    /// A background detection is running.
    rechecking: AtomicBool,
    last_detection: Mutex<Option<Instant>>,
}

impl ToolFlags {
    fn flag(&self, tool: Tool) -> &AtomicBool {
        match tool {
            Tool::Ffmpeg => &self.ffmpeg,
            Tool::Ffprobe => &self.ffprobe,
            Tool::YtDlp => &self.yt_dlp,
        }
    }

    fn snapshot(&self) -> ToolSet {
        ToolSet {
            ffmpeg: self.flag(Tool::Ffmpeg).load(Ordering::Relaxed),
            ffprobe: self.flag(Tool::Ffprobe).load(Ordering::Relaxed),
            yt_dlp: self.flag(Tool::YtDlp).load(Ordering::Relaxed),
        }
    }

    /// Runs `<tool> -version` for the given tools (concurrently) and caches
    /// the answers. The first detection logs the full picture, later ones
    /// only changes.
    async fn detect(&self, tools: &[Tool]) -> ToolSet {
        *self
            .last_detection
            .lock()
            .expect("tool detection time poisoned") = Some(Instant::now());
        let results = futures_util::future::join_all(
            tools
                .iter()
                .map(|tool| async move { (*tool, tool_works(*tool).await) }),
        )
        .await;
        let first = !self.detected.swap(true, Ordering::Relaxed);
        for (tool, works) in results {
            let before = self.flag(tool).swap(works, Ordering::Relaxed);
            if !first && before != works {
                if works {
                    info!(tool = tool.binary(), "media tool is now available");
                } else {
                    warn!(tool = tool.binary(), "media tool is no longer available");
                }
            }
        }
        let snapshot = self.snapshot();
        if first {
            info!(
                ffmpeg = snapshot.ffmpeg,
                ffprobe = snapshot.ffprobe,
                yt_dlp = snapshot.yt_dlp,
                "media tools detected"
            );
        }
        snapshot
    }

    /// Whether the last detection is older than [`TOOL_RECHECK_INTERVAL`].
    fn detection_is_stale(&self) -> bool {
        self.last_detection
            .lock()
            .expect("tool detection time poisoned")
            .is_none_or(|at| at.elapsed() >= TOOL_RECHECK_INTERVAL)
    }
}

impl Tools {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `tool` is installed, as of the last detection. When it is
    /// not, a new detection may start in the background (see the type docs);
    /// this call still answers from the cache.
    pub fn available(&self, tool: Tool) -> bool {
        let available = self.inner.flag(tool).load(Ordering::Relaxed);
        if !available {
            self.recheck_in_background();
        }
        available
    }

    fn recheck_in_background(&self) {
        if !self.inner.detection_is_stale() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self.inner.rechecking.swap(true, Ordering::AcqRel) {
            return;
        }
        // Counts as a detection from now on, so that concurrent callers do
        // not queue up more.
        *self
            .inner
            .last_detection
            .lock()
            .expect("tool detection time poisoned") = Some(Instant::now());
        let flags = self.inner.clone();
        runtime.spawn(async move {
            flags.detect(&Tool::ALL).await;
            flags.rechecking.store(false, Ordering::Release);
        });
    }

    /// Availability of every tool as of the last detection.
    pub fn snapshot(&self) -> ToolSet {
        self.inner.snapshot()
    }

    /// Records that spawning `tool` failed because the binary is gone.
    pub fn mark_missing(&self, tool: Tool) {
        if self.inner.flag(tool).swap(false, Ordering::Relaxed) {
            warn!(tool = tool.binary(), "media tool is no longer available");
        }
    }

    /// Detects the given tools now (see [`ToolFlags::detect`]).
    pub async fn detect(&self, tools: &[Tool]) -> ToolSet {
        self.inner.detect(tools).await
    }
}

async fn tool_works(tool: Tool) -> bool {
    let mut command = Command::new(tool.binary());
    command
        .arg(tool.version_flag())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    matches!(
        tokio::time::timeout(TOOL_CHECK_TIMEOUT, command.status()).await,
        Ok(Ok(status)) if status.success()
    )
}

/// A 16-hex-digit suffix that makes temporary file names unique per attempt.
pub fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mixed = (u64::from(std::process::id()) << 32)
        ^ (u64::from(nanos) << 8)
        ^ count.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    format!("{mixed:016x}")
}

/// The last `limit` non-empty lines a child process wrote to stderr (read
/// to the end, keeping only a bounded tail in memory), trimmed and joined
/// with ` | ` so that a log entry or stored error stays on one line.
pub async fn stderr_tail(mut stderr: impl tokio::io::AsyncRead + Unpin, limit: usize) -> String {
    use tokio::io::AsyncReadExt;
    let mut collected = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                collected.extend_from_slice(&buffer[..read]);
                if collected.len() > 64 * 1024 {
                    collected.drain(..collected.len() - 32 * 1024);
                }
            }
        }
    }
    let text = String::from_utf8_lossy(&collected);
    // Progress output rewrites its line with `\r`; every piece is a line.
    let lines: Vec<&str> = text
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines[lines.len().saturating_sub(limit)..].join(" | ")
}

/// Why [`run_grouped`] has no exit status to report.
#[derive(Debug)]
pub enum RunError {
    /// The program is not installed.
    NotFound,
    Io(std::io::Error),
    TimedOut,
}

/// Kills a process group (SIGKILL) when dropped or asked to.
struct GroupKiller {
    pgid: Option<libc::pid_t>,
}

impl GroupKiller {
    fn kill(&mut self) {
        if let Some(pgid) = self.pgid.take().filter(|pgid| *pgid > 1) {
            // SAFETY: killpg only sends a signal to the group we created.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}

impl Drop for GroupKiller {
    fn drop(&mut self) {
        self.kill();
    }
}

/// How long the stderr reader may take to finish after the process group
/// is gone (a helper that escaped the group could hold the pipe open).
const STDERR_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs `command` as the leader of a new process group, with a time limit,
/// and returns its exit status and the tail of its stderr (see
/// [`stderr_tail`]). The whole group is killed with SIGKILL when the limit
/// passes, when the returned future is dropped (a cancelled sync, shutdown)
/// and after the command exits: programs like yt-dlp start helpers (a
/// bundled interpreter, ffmpeg) that would otherwise keep running, and
/// writing, after the command itself was killed.
pub async fn run_grouped(
    mut command: Command,
    limit: Duration,
    tail_lines: usize,
) -> Result<(ExitStatus, String), RunError> {
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            RunError::NotFound
        } else {
            RunError::Io(error)
        }
    })?;
    let mut group = GroupKiller {
        pgid: child.id().and_then(|id| libc::pid_t::try_from(id).ok()),
    };
    let stderr = child.stderr.take().expect("stderr is piped");
    let tail = tokio::spawn(stderr_tail(stderr, tail_lines));
    let status = match tokio::time::timeout(limit, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => return Err(RunError::Io(error)),
        Err(_) => {
            group.kill();
            let _ = child.wait().await;
            return Err(RunError::TimedOut);
        }
    };
    // Whatever the command left running goes too.
    group.kill();
    let tail = tokio::time::timeout(STDERR_DRAIN_TIMEOUT, tail)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    Ok((status, tail))
}

/// `file:<path>`: keeps ffmpeg/ffprobe from reading a file name as a protocol.
pub fn file_input(path: &Path) -> OsString {
    let mut input = OsString::from("file:");
    input.push(path.as_os_str());
    input
}

/// Why ffprobe rejected a file (a definitive answer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidReason {
    Empty,
    InvalidData,
    NoAudioStream,
}

impl InvalidReason {
    pub fn as_str(self) -> &'static str {
        match self {
            InvalidReason::Empty => "empty file",
            InvalidReason::InvalidData => "invalid data",
            InvalidReason::NoAudioStream => "no audio stream",
        }
    }
}

/// Why a probe gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uncertain {
    ToolMissing,
    Timeout,
    /// ffprobe was killed by a signal.
    Interrupted,
    Failed(String),
}

impl fmt::Display for Uncertain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Uncertain::ToolMissing => formatter.write_str("ffprobe is not available"),
            Uncertain::Timeout => formatter.write_str("ffprobe timed out"),
            Uncertain::Interrupted => formatter.write_str("ffprobe was interrupted"),
            Uncertain::Failed(detail) => formatter.write_str(detail),
        }
    }
}

/// ffprobe's opinion of a media file.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Readable, with an audio stream. `duration` is `None` when the
    /// container does not state one.
    Valid { duration: Option<f64> },
    /// ffprobe completed and rejected the file.
    Invalid(InvalidReason),
    /// No definitive answer; try again later and never delete on this.
    Unknown(Uncertain),
}

async fn run_ffprobe(path: &Path) -> Verdict {
    let mut command = Command::new(Tool::Ffprobe.binary());
    command
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file",
            "-show_entries",
            "format=duration:stream=codec_type",
            "-select_streams",
            "a",
            "-of",
            "json",
        ])
        .arg(file_input(path))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    match tokio::time::timeout(PROBE_TIMEOUT, command.output()).await {
        Err(_) => Verdict::Unknown(Uncertain::Timeout),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Verdict::Unknown(Uncertain::ToolMissing)
        }
        Ok(Err(error)) => {
            Verdict::Unknown(Uncertain::Failed(format!("could not run ffprobe: {error}")))
        }
        Ok(Ok(output)) => classify_probe(output.status.code(), &output.stdout, &output.stderr),
    }
}

/// Turns ffprobe's exit code (`None` = killed by a signal) and output into a
/// verdict. Only a completed run is ever definitive.
pub fn classify_probe(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Verdict {
    let stderr = String::from_utf8_lossy(stderr);
    match code {
        None => Verdict::Unknown(Uncertain::Interrupted),
        Some(0) => {
            let Ok(report) = serde_json::from_slice::<Value>(stdout) else {
                return Verdict::Unknown(Uncertain::Failed(
                    "ffprobe printed an unreadable report".to_string(),
                ));
            };
            let has_audio = report["streams"].as_array().is_some_and(|streams| {
                streams
                    .iter()
                    .any(|stream| stream["codec_type"].as_str() == Some("audio"))
            });
            if !has_audio {
                return Verdict::Invalid(InvalidReason::NoAudioStream);
            }
            let duration = report["format"]["duration"]
                .as_str()
                .and_then(|value| value.trim().parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value > 0.0);
            Verdict::Valid { duration }
        }
        Some(_) if stderr.contains("Invalid data found when processing input") => {
            Verdict::Invalid(InvalidReason::InvalidData)
        }
        Some(code) => {
            let detail = stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("no error output")
                .trim();
            Verdict::Unknown(Uncertain::Failed(format!(
                "ffprobe exited with {code}: {detail}"
            )))
        }
    }
}

type VerdictFuture = Shared<BoxFuture<'static, Verdict>>;

struct ProbeEntry {
    size: u64,
    mtime_ms: i64,
    id: u64,
    future: VerdictFuture,
}

/// Probe results per file version (path + size + mtime). Each probe runs in
/// its own task, so callers that go away (a cancelled request) never abandon
/// one half-way, and concurrent callers share it. Unknown verdicts are
/// dropped from the cache so the next caller probes again.
pub struct Probes {
    cache: Arc<Mutex<LruCache<PathBuf, ProbeEntry>>>,
    next_id: AtomicU64,
}

impl Default for Probes {
    fn default() -> Self {
        Self::new()
    }
}

impl Probes {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(MAX_CACHED_PROBES).expect("non-zero capacity"),
            ))),
            next_id: AtomicU64::new(1),
        }
    }

    /// ffprobe's verdict on the file at `path`, whose current size and mtime
    /// the caller has just read.
    pub async fn verdict(&self, path: &Path, size: u64, mtime_ms: i64) -> Verdict {
        if size == 0 {
            return Verdict::Invalid(InvalidReason::Empty);
        }
        let future = {
            let mut cache = self.cache.lock().expect("probe cache poisoned");
            match cache.get(path) {
                Some(entry) if entry.size == size && entry.mtime_ms == mtime_ms => {
                    entry.future.clone()
                }
                _ => {
                    let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                    let cache_handle = self.cache.clone();
                    let key = path.to_path_buf();
                    let task = tokio::spawn(async move {
                        let verdict = run_ffprobe(&key).await;
                        if matches!(verdict, Verdict::Unknown(_)) {
                            let mut cache = cache_handle.lock().expect("probe cache poisoned");
                            if cache.peek(&key).is_some_and(|entry| entry.id == id) {
                                cache.pop(&key);
                            }
                        }
                        verdict
                    });
                    let future = task
                        .map(|joined| {
                            joined.unwrap_or_else(|error| {
                                Verdict::Unknown(Uncertain::Failed(format!(
                                    "probe task failed: {error}"
                                )))
                            })
                        })
                        .boxed()
                        .shared();
                    cache.put(
                        path.to_path_buf(),
                        ProbeEntry {
                            size,
                            mtime_ms,
                            id,
                            future: future.clone(),
                        },
                    );
                    future
                }
            }
        };
        future.await
    }

    /// Drops any cached verdict for `path` (after the file was moved away).
    pub fn forget(&self, path: &Path) {
        self.cache.lock().expect("probe cache poisoned").pop(path);
    }
}

/// Why a media file was moved aside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quarantine {
    /// ffprobe rejected it.
    Rejected,
    /// The cleanup retired it: no `files` row refers to it any more.
    Retired,
}

impl Quarantine {
    const ALL: [Quarantine; 2] = [Quarantine::Rejected, Quarantine::Retired];

    fn suffix(self) -> &'static str {
        match self {
            Quarantine::Rejected => ".invalid",
            Quarantine::Retired => ".removed",
        }
    }

    /// How long the cleanup keeps a quarantined file before deleting it.
    /// Retired media is irreplaceable when its host dropped the file, so it
    /// gets a longer grace period for a restore by hand.
    pub fn retention_secs(self) -> i64 {
        match self {
            Quarantine::Rejected => 7 * 24 * 60 * 60,
            Quarantine::Retired => 14 * 24 * 60 * 60,
        }
    }
}

/// Moves a media file aside as `<filename>.<unix seconds>.invalid` (rejected
/// by ffprobe) or `….removed` (retired) inside the same directory; the
/// cleanup job deletes quarantined files after their retention. Returns the
/// new name.
pub async fn quarantine(
    dir: &Path,
    filename: &str,
    now: i64,
    kind: Quarantine,
) -> std::io::Result<String> {
    // Keep the new name within NAME_MAX even for (unusually) long names.
    let base = if filename.len() > 200 {
        let digest = <sha2::Sha256 as sha2::Digest>::digest(filename.as_bytes());
        format!("quarantined-{}", hex::encode(&digest[..8]))
    } else {
        filename.to_string()
    };
    // rename() replaces an existing target: never overwrite an earlier
    // quarantined copy of the same name (moved aside within the same second).
    let mut seconds = now;
    let quarantined = loop {
        let candidate = format!("{base}.{seconds}{}", kind.suffix());
        if !tokio::fs::try_exists(dir.join(&candidate)).await? {
            break candidate;
        }
        seconds += 1;
    };
    tokio::fs::rename(dir.join(filename), dir.join(&quarantined)).await?;
    Ok(quarantined)
}

/// When and why a quarantined file (named by [`quarantine`]) was moved aside.
pub fn quarantined_at(filename: &str) -> Option<(i64, Quarantine)> {
    Quarantine::ALL.into_iter().find_map(|kind| {
        let stem = filename.strip_suffix(kind.suffix())?;
        let (_, seconds) = stem.rsplit_once('.')?;
        if seconds.is_empty() || !seconds.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        Some((seconds.parse().ok()?, kind))
    })
}

/// File URLs whose media a request found missing, empty or unreadable. The
/// background sync re-checks them (request handlers never change download
/// state themselves).
#[derive(Default)]
pub struct ReverifyQueue {
    urls: Mutex<HashSet<String>>,
}

impl ReverifyQueue {
    pub fn enqueue(&self, url: &str) {
        let mut urls = self.urls.lock().expect("reverify queue poisoned");
        if urls.len() < MAX_QUEUED_REVERIFICATIONS && !urls.contains(url) {
            urls.insert(url.to_string());
        }
    }

    pub fn take_all(&self) -> Vec<String> {
        let mut urls = self.urls.lock().expect("reverify queue poisoned");
        std::mem::take(&mut *urls).into_iter().collect()
    }

    pub fn len(&self) -> usize {
        self.urls.lock().expect("reverify queue poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_completed_probes_are_definitive() {
        let audio = br#"{"streams":[{"codec_type":"audio"}],"format":{"duration":"196.903854"}}"#;
        assert_eq!(
            classify_probe(Some(0), audio, b""),
            Verdict::Valid {
                duration: Some(196.903854)
            }
        );
        let no_duration = br#"{"streams":[{"codec_type":"audio"}],"format":{}}"#;
        assert_eq!(
            classify_probe(Some(0), no_duration, b""),
            Verdict::Valid { duration: None }
        );
        let image = br#"{"streams":[],"format":{}}"#;
        assert_eq!(
            classify_probe(Some(0), image, b""),
            Verdict::Invalid(InvalidReason::NoAudioStream)
        );
        assert_eq!(
            classify_probe(
                Some(1),
                b"{}",
                b"file:x.mp3: Invalid data found when processing input\n"
            ),
            Verdict::Invalid(InvalidReason::InvalidData)
        );
        // Anything else is not a verdict on the file itself.
        assert!(matches!(
            classify_probe(Some(1), b"{}", b"file:x.mp3: Permission denied\n"),
            Verdict::Unknown(Uncertain::Failed(detail)) if detail.contains("Permission denied")
        ));
        assert_eq!(
            classify_probe(None, b"", b""),
            Verdict::Unknown(Uncertain::Interrupted)
        );
        assert!(matches!(
            classify_probe(Some(0), b"not json", b""),
            Verdict::Unknown(_)
        ));
    }

    #[test]
    fn quarantine_names_carry_their_timestamp_and_kind() {
        assert_eq!(
            quarantined_at("abc.mp3.1790000000.invalid"),
            Some((1_790_000_000, Quarantine::Rejected))
        );
        assert_eq!(
            quarantined_at("abc.mp3.1790000000.removed"),
            Some((1_790_000_000, Quarantine::Retired))
        );
        assert_eq!(quarantined_at("abc.mp3.invalid"), None);
        assert_eq!(quarantined_at("abc.mp3"), None);
        assert_eq!(quarantined_at("abc.x1.invalid"), None);
        assert!(Quarantine::Retired.retention_secs() > Quarantine::Rejected.retention_secs());
    }

    #[tokio::test]
    async fn quarantine_moves_the_file_aside() {
        let dir = std::env::temp_dir().join(format!("yt-quarantine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bad.mp3"), b"junk").unwrap();
        std::fs::write(dir.join("old.mp3"), b"audio").unwrap();
        let moved = quarantine(&dir, "bad.mp3", 1_790_000_000, Quarantine::Rejected)
            .await
            .unwrap();
        assert_eq!(moved, "bad.mp3.1790000000.invalid");
        assert!(!dir.join("bad.mp3").exists());
        assert!(dir.join(&moved).exists());
        let retired = quarantine(&dir, "old.mp3", 1_790_000_001, Quarantine::Retired)
            .await
            .unwrap();
        assert_eq!(retired, "old.mp3.1790000001.removed");
        // A second rejection of the same name in the same second keeps both.
        std::fs::write(dir.join("bad.mp3"), b"junk again").unwrap();
        let again = quarantine(&dir, "bad.mp3", 1_790_000_000, Quarantine::Rejected)
            .await
            .unwrap();
        assert_eq!(again, "bad.mp3.1790000001.invalid");
        assert_eq!(std::fs::read(dir.join(&moved)).unwrap(), b"junk");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reverify_queue_deduplicates() {
        let queue = ReverifyQueue::default();
        queue.enqueue("https://a");
        queue.enqueue("https://a");
        queue.enqueue("https://b");
        assert_eq!(queue.len(), 2);
        let mut taken = queue.take_all();
        taken.sort();
        assert_eq!(taken, ["https://a", "https://b"]);
        assert!(queue.is_empty());
    }

    #[test]
    fn unique_suffixes_differ() {
        let first = unique_suffix();
        let second = unique_suffix();
        assert_eq!(first.len(), 16);
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn stderr_tail_keeps_the_last_lines_on_one_line() {
        let input: &[u8] = b"one\n\ntwo\nthree  \n  four\n";
        assert_eq!(stderr_tail(input, 2).await, "three | four");
        let progress: &[u8] = b"ERROR: first\r\n[download]  10%\r[download] 100%\nlast\n";
        assert_eq!(
            stderr_tail(progress, 3).await,
            "[download]  10% | [download] 100% | last"
        );
        assert_eq!(stderr_tail(&b""[..], 2).await, "");
    }

    #[tokio::test]
    async fn a_missing_tool_is_looked_for_again_at_most_once_a_minute() {
        let tools = Tools::new();
        // Nothing detected yet: asking starts a detection in the background.
        assert!(!tools.available(Tool::Ffmpeg));
        assert!(!tools.inner.detection_is_stale());
        for _ in 0..200 {
            if !tools.inner.rechecking.load(Ordering::Acquire)
                && tools.inner.detected.load(Ordering::Relaxed)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(tools.inner.detected.load(Ordering::Relaxed));
        // Right after a detection nothing new starts.
        tools.mark_missing(Tool::Ffmpeg);
        assert!(!tools.available(Tool::Ffmpeg));
        assert!(!tools.inner.rechecking.load(Ordering::Acquire));
        // Once the last detection is old enough, the next ask starts one.
        *tools.inner.last_detection.lock().unwrap() =
            Instant::now().checked_sub(TOOL_RECHECK_INTERVAL);
        tools.available(Tool::Ffmpeg);
        assert!(!tools.inner.detection_is_stale());
    }

    /// Whether `pid` is gone (or a zombie nobody reaped yet).
    fn process_is_gone(pid: i32) -> bool {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Err(_) => true,
            Ok(stat) => stat
                .rsplit_once(')')
                .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')),
        }
    }

    async fn wait_until_gone(pid: i32) -> bool {
        for _ in 0..100 {
            if process_is_gone(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    /// `sh` that starts a helper (`sleep`), records its pid and then runs
    /// `rest`.
    fn forking_shell(pid_file: &Path, rest: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(format!(
            "sleep 30 & echo $! > '{}'; {rest}",
            pid_file.display()
        ));
        command
    }

    async fn helper_pid(pid_file: &Path) -> i32 {
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(pid_file)
                && let Ok(pid) = text.trim().parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the helper pid was not written");
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yt-group-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_timed_out_command_takes_its_helpers_along() {
        let dir = scratch_dir("timeout");
        let pid_file = dir.join("helper.pid");
        let command = forking_shell(&pid_file, "echo busy >&2; sleep 30");
        let result = run_grouped(command, Duration::from_millis(500), 5).await;
        assert!(matches!(result, Err(RunError::TimedOut)), "{result:?}");
        let helper = helper_pid(&pid_file).await;
        assert!(wait_until_gone(helper).await, "helper {helper} survived");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn dropping_the_run_takes_the_helpers_along() {
        let dir = scratch_dir("drop");
        let pid_file = dir.join("helper.pid");
        let command = forking_shell(&pid_file, "sleep 30");
        let run = run_grouped(command, Duration::from_secs(60), 5);
        assert!(
            tokio::time::timeout(Duration::from_millis(400), run)
                .await
                .is_err()
        );
        let helper = helper_pid(&pid_file).await;
        assert!(wait_until_gone(helper).await, "helper {helper} survived");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn helpers_left_running_after_exit_are_stopped() {
        let dir = scratch_dir("exit");
        let pid_file = dir.join("helper.pid");
        let command = forking_shell(&pid_file, "echo 'ERROR: one' >&2; echo two >&2; exit 3");
        let (status, tail) = run_grouped(command, Duration::from_secs(10), 5)
            .await
            .unwrap();
        assert_eq!(status.code(), Some(3));
        assert_eq!(tail, "ERROR: one | two");
        let helper = helper_pid(&pid_file).await;
        assert!(wait_until_gone(helper).await, "helper {helper} survived");
        let missing = run_grouped(
            Command::new("/nonexistent/yt-dlp"),
            Duration::from_secs(1),
            5,
        )
        .await;
        assert!(matches!(missing, Err(RunError::NotFound)), "{missing:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Processes (not zombies) whose command line mentions `needle`.
    fn processes_mentioning(needle: &str) -> Vec<i32> {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
            .filter(|pid| {
                std::fs::read(format!("/proc/{pid}/cmdline"))
                    .is_ok_and(|cmdline| String::from_utf8_lossy(&cmdline).contains(needle))
                    && !process_is_gone(*pid)
            })
            .collect()
    }

    /// Manual check with a real yt-dlp (the static build is a PyInstaller
    /// bundle that forks, and `--download-sections` makes it run ffmpeg)
    /// fetching from a slow local server, e.g.:
    /// `PATH=/path/to/tools:$PATH YT_DLP_SLOW_URL=http://127.0.0.1:3505/song.wav
    ///  cargo test --lib yt_dlp_process_tree -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn yt_dlp_process_tree_goes_away_on_timeout() {
        let url = std::env::var("YT_DLP_SLOW_URL").expect("set YT_DLP_SLOW_URL");
        let dir = scratch_dir("yt-dlp");
        let mut command = Command::new(Tool::YtDlp.binary());
        command
            .args([
                "--no-playlist",
                "--ignore-config",
                "--no-progress",
                "--download-sections",
                "*10-inf",
                "-o",
            ])
            .arg(dir.join("audio.%(ext)s"))
            .arg("--")
            .arg(&url);
        let needle = dir.to_string_lossy().into_owned();
        let watcher = tokio::spawn({
            let needle = needle.clone();
            async move {
                // The most processes seen at once while yt-dlp ran.
                let mut most = 0;
                for _ in 0..40 {
                    most = most.max(processes_mentioning(&needle).len());
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                most
            }
        });
        let result = run_grouped(command, Duration::from_secs(5), 5).await;
        println!("result: {result:?}");
        let most = watcher.await.unwrap();
        println!("processes seen while running: {most}");
        assert!(matches!(result, Err(RunError::TimedOut)), "{result:?}");
        assert!(most >= 2, "expected yt-dlp to run helpers, saw {most}");
        tokio::time::sleep(Duration::from_millis(500)).await;
        let survivors = processes_mentioning(&needle);
        assert!(survivors.is_empty(), "still running: {survivors:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tool_sets_list_what_is_missing() {
        let set = ToolSet {
            ffmpeg: true,
            ffprobe: false,
            yt_dlp: false,
        };
        assert_eq!(set.missing(), [Tool::Ffprobe, Tool::YtDlp]);
        assert!(set.has(Tool::Ffmpeg));
    }
}
