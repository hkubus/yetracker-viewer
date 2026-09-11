//! ffmpeg dominant-colour sampling with an mtime-validated LRU cache, ported
//! from `util/getDominantColor.ts`.

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::{BoxFuture, Shared};
use futures_util::FutureExt;
use lru::LruCache;
use tokio::process::Command;

const SAMPLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CACHED_COLORS: usize = 500;

pub type ColorFuture = Shared<BoxFuture<'static, Result<[u8; 3], String>>>;

struct ColorEntry {
    mtime_ms: i64,
    future: ColorFuture,
}

/// Cache of dominant-colour probes keyed by absolute path.
pub struct DominantColors {
    cache: Arc<Mutex<LruCache<String, ColorEntry>>>,
}

impl DominantColors {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(LruCache::new(
                std::num::NonZeroUsize::new(MAX_CACHED_COLORS).expect("non-zero capacity"),
            ))),
        }
    }

    /// Average colour of the right-hand 20% strip of `path`, deduplicated while
    /// in flight and re-probed when the file's mtime changes.
    pub fn get(&self, path: &Path) -> ColorFuture {
        let mtime_ms = std::fs::metadata(path)
            .ok()
            .and_then(|metadata| {
                metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_millis() as i64)
            })
            .unwrap_or(-1);
        let key = path.to_string_lossy().into_owned();

        {
            let mut cache = self.cache.lock().expect("dominant color cache poisoned");
            if let Some(entry) = cache.get(&key) {
                if entry.mtime_ms == mtime_ms {
                    return entry.future.clone();
                }
            }
        }

        let cache = self.cache.clone();
        let path_owned = path.to_path_buf();
        let key_for_task = key.clone();
        let future: ColorFuture = async move {
            let result = sample_color(&path_owned).await;
            if result.is_err() {
                // Failed probes are evicted so a later call retries them.
                cache.lock().expect("dominant color cache poisoned").pop(&key_for_task);
            }
            result
        }
        .boxed()
        .shared();

        self.cache
            .lock()
            .expect("dominant color cache poisoned")
            .put(key, ColorEntry { mtime_ms, future: future.clone() });
        future
    }
}

impl Default for DominantColors {
    fn default() -> Self {
        Self::new()
    }
}

async fn sample_color(path: &Path) -> Result<[u8; 3], String> {
    let mut command = Command::new("ffmpeg");
    command
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-vf",
            "crop=iw*0.2:ih:iw*0.8:0,scale=1:1:flags=area",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let output = match tokio::time::timeout(SAMPLE_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Err(error.to_string()),
        Err(_) => return Err("dominant color sampling timed out".to_string()),
    };
    if !output.status.success() {
        return Err(format!("ffmpeg exited with {}", output.status));
    }
    if output.stdout.len() < 3 {
        return Err(format!("Could not sample color from {}", path.display()));
    }
    Ok([output.stdout[0], output.stdout[1], output.stdout[2]])
}
