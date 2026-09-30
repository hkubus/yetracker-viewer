//! Environment parsing and storage layout. Mirrors `apps/api/src/config.ts`
//! (same variable names, defaults and failure messages for the original
//! settings) plus the sync, download, transcode-cache and search knobs.
//!
//! Every variable is optional: unset and blank values mean "use the default"
//! (except `CORS_ORIGINS`, where an explicit blank value allows no origin).
//! Anything else must parse strictly or startup fails with a message naming
//! the variable. Booleans accept `true/false`, `1/0`, `yes/no` and `on/off`
//! (case-insensitive); integers are plain ASCII digits within the documented
//! range. Relative paths resolve from the workspace root. `*_MB` settings are
//! binary megabytes (1 MB = 1,048,576 bytes).

use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use tracing::warn;

const BYTES_PER_MB: u64 = 1024 * 1024;
/// Upper bound for the `*_MB` settings (1 TiB), far above any sane value but
/// low enough that the byte counts never overflow.
const MAX_MB: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Config {
    /// `API_HOST`, falling back to `HOST`; default `127.0.0.1`.
    pub api_host: String,
    /// `API_PORT`, falling back to `PORT`; default 3000.
    pub api_port: u16,
    /// `CORS_ORIGINS`: comma-separated exact http(s) origins, or `*` for any;
    /// default `http://localhost:4321,http://127.0.0.1:4321`.
    pub cors_origins: Vec<String>,
    /// `SYNC_ON_START` (default true): run a catalog sync when the server boots.
    pub sync_on_start: bool,
    /// `SYNC_INTERVAL_MINUTES` (default 30, 0–525600): pause between periodic
    /// syncs. `None` when set to 0, which disables the periodic loop.
    pub sync_interval: Option<Duration>,
    /// `IMPORT_FORCE` (default false): accept an import even when the fetched
    /// catalog is suspiciously smaller than the current one.
    pub import_force: bool,
    /// `CATALOG_SHEET_FILE` (optional): import this saved sheet HTML file
    /// instead of fetching the live sheet. Must name an existing file.
    pub catalog_sheet_file: Option<PathBuf>,
    /// `DOWNLOADS_ENABLED` (default true): master switch for background
    /// downloads, i.e. song media and era covers. Media already on disk is
    /// still linked to its songs while it is off.
    pub downloads_enabled: bool,
    /// `YOUTUBE_DOWNLOAD` (default true): download YouTube links with yt-dlp.
    pub youtube_download: bool,
    /// `DOWNLOAD_CONCURRENCY` (default 3, 1–32): parallel song downloads.
    pub download_concurrency: usize,
    /// `MAX_DOWNLOADS_PER_CYCLE` (default 200, 0–1000000): cap on download
    /// attempts per sync, so that a large backlog (e.g. every link of a
    /// freshly imported catalog) cannot hold up the next catalog import for
    /// hours. `None` (the value 0) means unlimited.
    pub max_downloads_per_cycle: Option<usize>,
    /// `DOWNLOAD_TIME_BUDGET_MINUTES` (default 20, 0–1440): how long one
    /// sync's download phase keeps starting downloads. Downloads still
    /// running when it ends get five more minutes, then they are stopped
    /// and retried by a later sync. `None` (the value 0) means no limit.
    pub download_time_budget: Option<Duration>,
    /// `MAX_DOWNLOAD_MB` (default 1024, 1–1048576): largest accepted media
    /// download, in bytes (enforced while the file streams in).
    pub max_download_bytes: u64,
    /// `MIN_FREE_DISK_MB` (default 1024, 0–1048576): free space the songs
    /// filesystem must keep; checked before each download, in bytes. 0
    /// disables the check.
    pub min_free_disk_bytes: u64,
    /// `BACKFILL_CONCURRENCY` (default 8, 1–64): parallel duration probes in
    /// the backfill.
    pub backfill_concurrency: usize,
    /// `MAX_CONCURRENT_TRANSCODES` (default: the number of CPUs, at least 2;
    /// 1–100): simultaneous ffmpeg transcodes.
    pub max_concurrent_transcodes: usize,
    /// `TRANSCODE_CACHE_MAX_MB` (default 2048, 0–1048576): size budget of the
    /// on-disk transcode cache (`<STORAGE_DIR>/transcodes`), in bytes; the
    /// least recently used transcodes are evicted. 0 keeps no transcodes.
    pub transcode_cache_max_bytes: u64,
    /// `SEARCH_CONCURRENCY` (default 4, 1–64): simultaneous search queries.
    pub search_concurrency: usize,
    /// `MAX_CONNECTIONS` (1–65536; default `min(1024, (open-file limit −
    /// 128) / 3)`, at least 1): open client connections. A connection
    /// streaming a file holds two descriptors (its socket and the file); the
    /// default leaves a third per connection spare, plus 128 for the
    /// process itself (database, listener, downloads, ffprobe/ffmpeg pipes),
    /// so a flood of connections cannot exhaust file descriptors. At the
    /// limit a new connection replaces an idle or stalled one (see
    /// `http.rs`), and is refused only when every connection is busy.
    pub max_connections: usize,
    /// `STORAGE_DIR` (default `storage`): SQLite database, `covers/`,
    /// `transcodes/` and, unless `SONGS_DIR` says otherwise, `songs/`.
    pub storage_path: PathBuf,
    /// `SONGS_DIR` (default `<STORAGE_DIR>/songs`): downloaded song media.
    pub songs_path: PathBuf,
    /// Raw `STORAGE_DIR` value, used in the directory-creation error message.
    storage_dir_raw: String,
}

/// Returns the raw value of a variable, `None` when it is unset.
type Lookup<'a> = &'a dyn Fn(&str) -> Result<Option<String>, String>;

impl Config {
    pub fn load() -> Result<Config, String> {
        load_env_file();
        let config = Config::from_lookup(&env_lookup, &resolve_workspace_root())?;
        config.create_directories()?;
        Ok(config)
    }

    fn from_lookup(lookup: Lookup<'_>, workspace_root: &Path) -> Result<Config, String> {
        let text = |name: &str| setting(lookup, name);
        let flag = |name: &str, fallback: bool| read_bool(name, text(name)?.as_deref(), fallback);
        let integer = |name: &str, fallback: u64, min: u64, max: u64| {
            read_integer(name, text(name)?.as_deref(), fallback, min, max)
        };

        let storage_dir_raw = text("STORAGE_DIR")?.unwrap_or_else(|| "storage".to_string());
        let storage_path = resolve_path(workspace_root, &storage_dir_raw);
        let songs_path = match text("SONGS_DIR")? {
            Some(value) => resolve_path(workspace_root, &value),
            None => storage_path.join("songs"),
        };

        // A blank `API_HOST=`/`API_PORT=` (as in a copied `.env`) means "not
        // set", so it must not shadow `HOST`/`PORT`.
        let api_host = match text("API_HOST")? {
            Some(value) => value,
            None => text("HOST")?.unwrap_or_else(|| "127.0.0.1".to_string()),
        };
        let port = match text("API_PORT")? {
            Some(value) => Some(value),
            None => text("PORT")?,
        };
        let api_port = read_port(port.as_deref(), 3000)?;
        let cors_origins = read_origins(lookup("CORS_ORIGINS")?.as_deref())?;

        let catalog_sheet_file = match text("CATALOG_SHEET_FILE")? {
            Some(value) => {
                let path = resolve_path(workspace_root, &value);
                if !path.is_file() {
                    return Err(format!(
                        "CATALOG_SHEET_FILE must name an existing file, received: {value} (resolved to {})",
                        path.display()
                    ));
                }
                Some(path)
            }
            None => None,
        };

        let sync_interval_minutes = integer("SYNC_INTERVAL_MINUTES", 30, 0, 525_600)?;
        let max_downloads_per_cycle = integer("MAX_DOWNLOADS_PER_CYCLE", 200, 0, 1_000_000)?;
        let download_budget_minutes = integer("DOWNLOAD_TIME_BUDGET_MINUTES", 20, 0, 1440)?;

        Ok(Config {
            api_host,
            api_port,
            cors_origins,
            sync_on_start: flag("SYNC_ON_START", true)?,
            sync_interval: (sync_interval_minutes > 0)
                .then(|| Duration::from_secs(sync_interval_minutes * 60)),
            import_force: flag("IMPORT_FORCE", false)?,
            catalog_sheet_file,
            downloads_enabled: flag("DOWNLOADS_ENABLED", true)?,
            youtube_download: flag("YOUTUBE_DOWNLOAD", true)?,
            download_concurrency: integer("DOWNLOAD_CONCURRENCY", 3, 1, 32)? as usize,
            max_downloads_per_cycle: (max_downloads_per_cycle > 0)
                .then_some(max_downloads_per_cycle as usize),
            download_time_budget: (download_budget_minutes > 0)
                .then(|| Duration::from_secs(download_budget_minutes * 60)),
            max_download_bytes: integer("MAX_DOWNLOAD_MB", 1024, 1, MAX_MB)? * BYTES_PER_MB,
            min_free_disk_bytes: integer("MIN_FREE_DISK_MB", 1024, 0, MAX_MB)? * BYTES_PER_MB,
            backfill_concurrency: integer("BACKFILL_CONCURRENCY", 8, 1, 64)? as usize,
            max_concurrent_transcodes: integer(
                "MAX_CONCURRENT_TRANSCODES",
                default_transcode_slots(),
                1,
                100,
            )? as usize,
            transcode_cache_max_bytes: integer("TRANSCODE_CACHE_MAX_MB", 2048, 0, MAX_MB)?
                * BYTES_PER_MB,
            search_concurrency: integer("SEARCH_CONCURRENCY", 4, 1, 64)? as usize,
            max_connections: integer(
                "MAX_CONNECTIONS",
                max_connections_for(open_file_limit()),
                1,
                65536,
            )? as usize,
            storage_path,
            songs_path,
            storage_dir_raw,
        })
    }

    /// Whether the songs directory lives inside `STORAGE_DIR`. Only then is it
    /// created on demand: a `SONGS_DIR` elsewhere is usually a mount point, and
    /// creating it while the mount is missing would quietly fill the root
    /// filesystem with downloads.
    pub fn songs_dir_is_managed(&self) -> bool {
        normalize_lexically(&self.songs_path).starts_with(normalize_lexically(&self.storage_path))
    }

    fn create_directories(&self) -> Result<(), String> {
        let songs_managed = self.songs_dir_is_managed();
        let mut dirs = vec![
            self.storage_path.clone(),
            self.storage_path.join("covers"),
            self.storage_path.join("transcodes"),
        ];
        if songs_managed {
            dirs.push(self.songs_path.clone());
        }
        for dir in &dirs {
            fs::create_dir_all(dir).map_err(|error| {
                format!(
                    "Failed to create storage directories (STORAGE_DIR={} resolved to {}): {error}",
                    self.storage_dir_raw,
                    self.storage_path.display()
                )
            })?;
        }
        if !songs_managed && !self.songs_path.is_dir() {
            warn!(
                path = %self.songs_path.display(),
                "SONGS_DIR does not exist and is outside STORAGE_DIR, so it is not created; \
                 songs stay unplayable and downloads are skipped until it exists"
            );
        }
        Ok(())
    }
}

/// Loads the repo-root `.env` without overriding variables already present in
/// the environment, mirroring the Node scripts' `--env-file ../../.env`. A
/// missing file is not an error, and calling this again is harmless.
pub fn load_env_file() {
    let _ = dotenvy::from_path(resolve_workspace_root().join(".env"));
}

fn env_lookup(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(format!("{name} must be valid UTF-8")),
    }
}

/// The trimmed value of a variable; unset and blank both mean `None`.
fn setting(lookup: Lookup<'_>, name: &str) -> Result<Option<String>, String> {
    Ok(lookup(name)?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

fn resolve_path(workspace_root: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

/// Resolves `.` and `..` components without touching the filesystem.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn find_workspace_root(start: &Path) -> PathBuf {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join("package.json").exists() {
            return dir;
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => return start.to_path_buf(),
        }
    }
}

/// The Node implementation resolves relative to the module directory
/// (`apps/api/src` → three levels up). Use the executable's directory so a
/// `cargo run` build lands on the same repository root, and fall back to the
/// current directory when the binary lives outside the tree.
fn resolve_workspace_root() -> PathBuf {
    if let Ok(exe) = env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let root = find_workspace_root(dir);
        if root.join("package.json").exists() {
            return root;
        }
    }
    match env::current_dir() {
        Ok(cwd) => find_workspace_root(&cwd),
        Err(_) => PathBuf::from("."),
    }
}

/// The soft limit on open file descriptors (`RLIMIT_NOFILE`); `None` when
/// unlimited or unknown.
pub fn open_file_limit() -> Option<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit only writes into the struct we pass.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return None;
    }
    (limit.rlim_cur != libc::RLIM_INFINITY).then_some(limit.rlim_cur)
}

/// Raises the soft limit on open file descriptors to the hard limit (the
/// common default soft limit of 1024 is easily reached by idle client
/// sockets). Returns the soft limit before and after; best effort.
pub fn raise_open_file_limit() -> Result<(u64, u64), String> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit only writes into the struct we pass.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let before = limit.rlim_cur;
    if limit.rlim_cur == limit.rlim_max || limit.rlim_cur == libc::RLIM_INFINITY {
        return Ok((before, before));
    }
    // An unlimited hard limit is not a valid soft limit; 2^20 is the usual
    // kernel maximum (`fs.nr_open`).
    let targets = if limit.rlim_max == libc::RLIM_INFINITY {
        vec![1 << 20]
    } else {
        vec![limit.rlim_max, limit.rlim_max.min(1 << 20)]
    };
    let mut error = String::new();
    for target in targets {
        let raised = libc::rlimit {
            rlim_cur: target,
            rlim_max: limit.rlim_max,
        };
        // SAFETY: setrlimit only reads the struct we pass.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
            return Ok((before, target));
        }
        error = std::io::Error::last_os_error().to_string();
    }
    Err(error)
}

/// Descriptors kept for the process itself when deriving the default
/// `MAX_CONNECTIONS`: stdio, the listener and the runtime, the database pool
/// (three files per connection), and the background jobs (downloads, cover
/// fetches, ffprobe/ffmpeg pipes). An idle server holds about 30.
const RESERVED_FILES: u64 = 128;
/// Descriptors budgeted per connection: a streamed file holds two (the
/// socket and the file), plus one spare (e.g. a live transcode's pipes).
const FILES_PER_CONNECTION: u64 = 3;

/// Default `MAX_CONNECTIONS` for an open-file limit:
/// `(limit − RESERVED_FILES) / FILES_PER_CONNECTION`, between 1 and 1024, so
/// that the connection cap engages before descriptors run out even when
/// every connection streams a file.
fn max_connections_for(open_files: Option<u64>) -> u64 {
    open_files.map_or(1024, |limit| {
        (limit.saturating_sub(RESERVED_FILES) / FILES_PER_CONNECTION).clamp(1, 1024)
    })
}

/// One transcode per CPU, but at least two.
fn default_transcode_slots() -> u64 {
    let cpus = std::thread::available_parallelism()
        .map(|count| count.get() as u64)
        .unwrap_or(1);
    cpus.clamp(2, 100)
}

fn is_ascii_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn read_bool(name: &str, value: Option<&str>, fallback: bool) -> Result<bool, String> {
    let Some(value) = value else {
        return Ok(fallback);
    };
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!(
            "{name} must be one of true/false, 1/0, yes/no, on/off, received: {value}"
        )),
    }
}

fn read_port(value: Option<&str>, fallback: u16) -> Result<u16, String> {
    let Some(value) = value else {
        return Ok(fallback);
    };
    if value.is_empty() {
        return Ok(fallback);
    }
    if !is_ascii_digits(value) {
        return Err(format!("Invalid API port: {value}"));
    }
    let port: u64 = value
        .parse()
        .map_err(|_| format!("API port must be between 1 and 65535, received: {value}"))?;
    if !(1..=65_535).contains(&port) {
        return Err(format!(
            "API port must be between 1 and 65535, received: {value}"
        ));
    }
    Ok(port as u16)
}

/// Plain ASCII digits within `min..=max`; `None` means the fallback.
fn read_integer(
    name: &str,
    value: Option<&str>,
    fallback: u64,
    min: u64,
    max: u64,
) -> Result<u64, String> {
    let Some(value) = value else {
        return Ok(fallback);
    };
    let kind = if min == 0 {
        "a non-negative integer"
    } else {
        "a positive integer"
    };
    if !is_ascii_digits(value) {
        return Err(format!("{name} must be {kind}, received: {value}"));
    }
    match value.parse::<u64>() {
        Ok(parsed) if (min..=max).contains(&parsed) => Ok(parsed),
        _ => Err(format!(
            "{name} must be {kind} between {min} and {max}, received: {value}"
        )),
    }
}

fn read_origins(value: Option<&str>) -> Result<Vec<String>, String> {
    let raw = value.unwrap_or("http://localhost:4321,http://127.0.0.1:4321");
    let origins: Vec<String> = raw
        .split(',')
        .map(|origin| origin.trim())
        .filter(|origin| !origin.is_empty())
        .map(|origin| origin.to_string())
        .collect();

    for origin in &origins {
        if origin == "*" {
            continue;
        }
        if !is_exact_http_origin(origin) {
            return Err(format!(
                "CORS_ORIGINS must contain exact HTTP(S) origins, received: {origin}"
            ));
        }
    }
    Ok(origins)
}

/// True when `new URL(origin).origin === origin` for an http(s) URL: no path,
/// query or fragment, lower-cased ASCII host, optional valid port.
fn is_exact_http_origin(origin: &str) -> bool {
    let rest = match origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    {
        Some(rest) => rest,
        None => return false,
    };
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', '\\']) {
        return false;
    }

    let (host, port) = if rest.starts_with('[') {
        let Some(close) = rest.find(']') else {
            return false;
        };
        let host = &rest[..=close];
        let tail = &rest[close + 1..];
        if tail.is_empty() {
            (host, None)
        } else if let Some(port) = tail.strip_prefix(':') {
            (host, Some(port))
        } else {
            return false;
        }
    } else {
        match rest.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        }
    };

    if host.is_empty() || !host.is_ascii() || host.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return false;
    }
    if let Some(port) = port {
        if !is_ascii_digits(port) {
            return false;
        }
        match port.parse::<u32>() {
            Ok(value) if (1..=65_535).contains(&value) => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
impl Config {
    /// Defaults, with the given storage and songs directories.
    pub(crate) fn for_tests(storage: &Path, songs: &Path) -> Config {
        let storage = storage.to_string_lossy().into_owned();
        let songs = songs.to_string_lossy().into_owned();
        let lookup = move |name: &str| {
            Ok(match name {
                "STORAGE_DIR" => Some(storage.clone()),
                "SONGS_DIR" => Some(songs.clone()),
                _ => None,
            })
        };
        Config::from_lookup(&lookup, Path::new("/")).expect("test configuration")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const ROOT: &str = "/srv/workspace";

    fn config_from(vars: &[(&str, &str)]) -> Result<Config, String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        let lookup = move |name: &str| Ok(vars.get(name).cloned());
        Config::from_lookup(&lookup, Path::new(ROOT))
    }

    #[test]
    fn accepts_exact_origins_only() {
        assert!(is_exact_http_origin("http://localhost:4321"));
        assert!(is_exact_http_origin("https://example.com"));
        assert!(is_exact_http_origin("http://[::1]:3000"));
        assert!(!is_exact_http_origin("http://localhost:4321/"));
        assert!(!is_exact_http_origin("http://LOCALHOST:4321"));
        assert!(!is_exact_http_origin("ftp://example.com"));
        assert!(!is_exact_http_origin("http://example.com:0"));
        assert!(!is_exact_http_origin("http://example.com:99999"));
        assert!(!is_exact_http_origin("http://example.com/path"));
    }

    #[test]
    fn parses_ports_and_integers() {
        assert_eq!(read_port(None, 3000).unwrap(), 3000);
        assert_eq!(read_port(Some(""), 3000).unwrap(), 3000);
        assert_eq!(read_port(Some("65535"), 3000).unwrap(), 65535);
        assert!(read_port(Some("0"), 3000).is_err());
        assert!(read_port(Some("abc"), 3000).is_err());
        assert_eq!(read_integer("MAX", Some("100"), 2, 1, 100).unwrap(), 100);
        assert_eq!(read_integer("MAX", None, 2, 1, 100).unwrap(), 2);
        assert_eq!(
            read_integer("MAX", Some("101"), 2, 1, 100).unwrap_err(),
            "MAX must be a positive integer between 1 and 100, received: 101"
        );
        assert_eq!(
            read_integer("MAX", Some("-1"), 2, 1, 100).unwrap_err(),
            "MAX must be a positive integer, received: -1"
        );
        assert!(read_integer("MAX", Some("0"), 2, 1, 100).is_err());
        assert!(read_integer("MAX", Some("1.5"), 2, 1, 100).is_err());
        assert!(read_integer("MAX", Some("99999999999999999999999"), 2, 1, 100).is_err());
        assert_eq!(read_integer("MIN", Some("0"), 5, 0, 10).unwrap(), 0);
    }

    #[test]
    fn booleans_are_strict() {
        for truthy in ["true", "TRUE", "1", "yes", "Yes", "on", "ON"] {
            assert!(read_bool("FLAG", Some(truthy), false).unwrap(), "{truthy}");
        }
        for falsy in ["false", "False", "0", "no", "NO", "off", "Off"] {
            assert!(!read_bool("FLAG", Some(falsy), true).unwrap(), "{falsy}");
        }
        assert!(read_bool("FLAG", None, true).unwrap());
        assert!(!read_bool("FLAG", None, false).unwrap());
        for invalid in ["maybe", "2", "truthy", "y", "-1"] {
            assert!(read_bool("FLAG", Some(invalid), true).is_err(), "{invalid}");
        }
    }

    #[test]
    fn defaults_apply_when_nothing_is_set() {
        let config = config_from(&[]).unwrap();
        assert_eq!(config.api_host, "127.0.0.1");
        assert_eq!(config.api_port, 3000);
        assert_eq!(
            config.cors_origins,
            ["http://localhost:4321", "http://127.0.0.1:4321"]
        );
        assert!(config.sync_on_start);
        assert_eq!(config.sync_interval, Some(Duration::from_secs(30 * 60)));
        assert!(!config.import_force);
        assert_eq!(config.catalog_sheet_file, None);
        assert!(config.downloads_enabled);
        assert!(config.youtube_download);
        assert_eq!(config.download_concurrency, 3);
        assert_eq!(config.max_downloads_per_cycle, Some(200));
        assert_eq!(
            config.download_time_budget,
            Some(Duration::from_secs(20 * 60))
        );
        assert_eq!(config.max_download_bytes, 1024 * BYTES_PER_MB);
        assert_eq!(config.min_free_disk_bytes, 1024 * BYTES_PER_MB);
        assert_eq!(config.backfill_concurrency, 8);
        assert_eq!(
            config.max_concurrent_transcodes as u64,
            default_transcode_slots()
        );
        assert!(config.max_concurrent_transcodes >= 2);
        assert_eq!(config.transcode_cache_max_bytes, 2048 * BYTES_PER_MB);
        assert_eq!(config.search_concurrency, 4);
        assert_eq!(
            config.max_connections as u64,
            max_connections_for(open_file_limit())
        );
        assert_eq!(config.storage_path, Path::new(ROOT).join("storage"));
        assert_eq!(config.songs_path, Path::new(ROOT).join("storage/songs"));
        assert!(config.songs_dir_is_managed());
    }

    #[test]
    fn blank_values_fall_back_instead_of_shadowing() {
        let config = config_from(&[
            ("API_PORT", ""),
            ("PORT", "4100"),
            ("API_HOST", "  "),
            ("HOST", "0.0.0.0"),
            ("SYNC_ON_START", ""),
            ("STORAGE_DIR", ""),
        ])
        .unwrap();
        assert_eq!(config.api_port, 4100);
        assert_eq!(config.api_host, "0.0.0.0");
        assert!(config.sync_on_start);
        assert_eq!(config.storage_path, Path::new(ROOT).join("storage"));

        let config = config_from(&[("API_PORT", "3100"), ("PORT", "4100")]).unwrap();
        assert_eq!(config.api_port, 3100);
        let config = config_from(&[("API_HOST", " 10.0.0.1 "), ("HOST", "0.0.0.0")]).unwrap();
        assert_eq!(config.api_host, "10.0.0.1");
    }

    #[test]
    fn explicitly_blank_cors_origins_allow_none() {
        assert!(
            config_from(&[("CORS_ORIGINS", "")])
                .unwrap()
                .cors_origins
                .is_empty()
        );
        assert!(config_from(&[("CORS_ORIGINS", "http://example.com/")]).is_err());
    }

    #[test]
    fn parses_the_sync_download_and_search_knobs() {
        let config = config_from(&[
            ("SYNC_INTERVAL_MINUTES", "15"),
            ("IMPORT_FORCE", "yes"),
            ("DOWNLOADS_ENABLED", "off"),
            ("YOUTUBE_DOWNLOAD", "0"),
            ("DOWNLOAD_CONCURRENCY", "5"),
            ("MAX_DOWNLOADS_PER_CYCLE", "5"),
            ("DOWNLOAD_TIME_BUDGET_MINUTES", "0"),
            ("MAX_DOWNLOAD_MB", "10"),
            ("MIN_FREE_DISK_MB", "0"),
            ("BACKFILL_CONCURRENCY", "16"),
            ("TRANSCODE_CACHE_MAX_MB", "0"),
            ("SEARCH_CONCURRENCY", "2"),
            ("MAX_CONNECTIONS", "64"),
        ])
        .unwrap();
        assert_eq!(config.sync_interval, Some(Duration::from_secs(15 * 60)));
        assert!(config.import_force);
        assert!(!config.downloads_enabled);
        assert!(!config.youtube_download);
        assert_eq!(config.download_concurrency, 5);
        assert_eq!(config.max_downloads_per_cycle, Some(5));
        assert_eq!(config.download_time_budget, None);
        assert_eq!(config.max_download_bytes, 10 * BYTES_PER_MB);
        assert_eq!(config.min_free_disk_bytes, 0);
        assert_eq!(config.backfill_concurrency, 16);
        assert_eq!(config.transcode_cache_max_bytes, 0);
        assert_eq!(config.search_concurrency, 2);
        assert_eq!(config.max_connections, 64);

        let disabled = config_from(&[
            ("SYNC_INTERVAL_MINUTES", "0"),
            ("MAX_DOWNLOADS_PER_CYCLE", "0"),
            ("DOWNLOAD_TIME_BUDGET_MINUTES", "45"),
        ])
        .unwrap();
        assert_eq!(disabled.sync_interval, None);
        assert_eq!(disabled.max_downloads_per_cycle, None);
        assert_eq!(
            disabled.download_time_budget,
            Some(Duration::from_secs(45 * 60))
        );
    }

    #[test]
    fn the_connection_cap_leaves_room_for_other_descriptors() {
        assert_eq!(max_connections_for(Some(256)), 42);
        assert_eq!(max_connections_for(Some(1024)), 298);
        assert_eq!(max_connections_for(Some(3200)), 1024);
        assert_eq!(max_connections_for(Some(524_288)), 1024);
        assert_eq!(max_connections_for(Some(128)), 1);
        assert_eq!(max_connections_for(Some(1)), 1);
        assert_eq!(max_connections_for(None), 1024);
        // Every connection streaming a file (socket + file) still leaves the
        // reserve free.
        for limit in [256, 300, 1024, 2048, 3200, 4096, 65_536] {
            let connections = max_connections_for(Some(limit));
            assert!(
                2 * connections + RESERVED_FILES <= limit,
                "{limit}: {connections} connections"
            );
        }
        let limit = open_file_limit();
        assert!(limit.is_none_or(|limit| limit > 0));
    }

    #[test]
    fn invalid_values_are_startup_errors() {
        for (name, value) in [
            ("SYNC_INTERVAL_MINUTES", "-1"),
            ("SYNC_INTERVAL_MINUTES", "abc"),
            ("SYNC_INTERVAL_MINUTES", "525601"),
            ("SYNC_ON_START", "maybe"),
            ("DOWNLOADS_ENABLED", "2"),
            ("IMPORT_FORCE", "sure"),
            ("DOWNLOAD_CONCURRENCY", "0"),
            ("DOWNLOAD_CONCURRENCY", "33"),
            ("MAX_DOWNLOADS_PER_CYCLE", "-5"),
            ("DOWNLOAD_TIME_BUDGET_MINUTES", "1441"),
            ("MAX_DOWNLOAD_MB", "0"),
            ("MIN_FREE_DISK_MB", "1.5"),
            ("BACKFILL_CONCURRENCY", "-3"),
            ("BACKFILL_CONCURRENCY", "0"),
            ("BACKFILL_CONCURRENCY", "abc"),
            ("BACKFILL_CONCURRENCY", "3.9"),
            ("MAX_CONCURRENT_TRANSCODES", "101"),
            ("TRANSCODE_CACHE_MAX_MB", "1048577"),
            ("SEARCH_CONCURRENCY", "0"),
            ("MAX_CONNECTIONS", "0"),
            ("API_PORT", "70000"),
        ] {
            let error = config_from(&[(name, value)]).expect_err(&format!("{name}={value}"));
            assert!(
                error.contains(name) || name == "API_PORT",
                "error for {name}={value} should name the variable: {error}"
            );
        }
    }

    #[test]
    fn catalog_sheet_file_must_exist() {
        let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        let config = config_from(&[("CATALOG_SHEET_FILE", manifest)]).unwrap();
        assert_eq!(
            config.catalog_sheet_file.as_deref(),
            Some(Path::new(manifest))
        );
        let missing = config_from(&[("CATALOG_SHEET_FILE", "no/such/sheet.html")]).unwrap_err();
        assert!(missing.contains("CATALOG_SHEET_FILE"), "{missing}");
        assert!(
            missing.contains("/srv/workspace/no/such/sheet.html"),
            "{missing}"
        );
    }

    #[test]
    fn only_songs_dirs_inside_storage_are_managed() {
        let inside = config_from(&[("SONGS_DIR", "storage/media")]).unwrap();
        assert!(inside.songs_dir_is_managed());
        let mount = config_from(&[("SONGS_DIR", "/mnt/music")]).unwrap();
        assert!(!mount.songs_dir_is_managed());
        let escaping = config_from(&[("SONGS_DIR", "storage/../songs")]).unwrap();
        assert!(!escaping.songs_dir_is_managed());
        let sibling = config_from(&[
            ("STORAGE_DIR", "/data/store"),
            ("SONGS_DIR", "/data/storefront"),
        ])
        .unwrap();
        assert!(!sibling.songs_dir_is_managed());
    }
}
