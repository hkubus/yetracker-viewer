//! Environment parsing and storage layout. Mirrors `apps/api/src/config.ts`
//! (same variable names, defaults, validation and failure messages).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    pub api_host: String,
    pub api_port: u16,
    pub cors_origins: Vec<String>,
    pub sync_on_start: bool,
    #[allow(dead_code)]
    pub youtube_download: bool,
    pub max_concurrent_transcodes: usize,
    pub storage_path: PathBuf,
    pub songs_path: PathBuf,
    /// Raw `STORAGE_DIR` value, used in the directory-creation error message.
    storage_dir_raw: String,
}

impl Config {
    pub fn load() -> Result<Config, String> {
        let workspace_root = resolve_workspace_root();
        let storage_dir_raw = env::var("STORAGE_DIR").unwrap_or_else(|_| "storage".to_string());
        let storage_path = resolve_path(&workspace_root, &storage_dir_raw);

        let songs_dir = match env::var("SONGS_DIR") {
            Ok(value) if !value.is_empty() => value,
            _ => storage_path.join("songs").to_string_lossy().into_owned(),
        };
        let songs_path = resolve_path(&workspace_root, &songs_dir);

        let raw_api_host = env::var("API_HOST")
            .or_else(|_| env::var("HOST"))
            .unwrap_or_else(|_| "127.0.0.1".to_string());
        let api_host = raw_api_host.trim().to_string();
        if api_host.is_empty() {
            return Err("API_HOST must be a non-empty hostname or IP address".to_string());
        }

        let api_port = read_port(env::var("API_PORT").ok().as_deref().or(env::var("PORT").ok().as_deref()), 3000)?;
        let cors_origins = read_origins(env::var("CORS_ORIGINS").ok().as_deref())?;
        let sync_on_start = read_bool(env::var("SYNC_ON_START").ok().as_deref(), true);
        let youtube_download = read_bool(env::var("YOUTUBE_DOWNLOAD").ok().as_deref(), true);
        let max_concurrent_transcodes = read_positive_integer(
            env::var("MAX_CONCURRENT_TRANSCODES").ok().as_deref(),
            2,
            "MAX_CONCURRENT_TRANSCODES",
            100,
        )?;

        let config = Config {
            api_host,
            api_port,
            cors_origins,
            sync_on_start,
            youtube_download,
            max_concurrent_transcodes: max_concurrent_transcodes as usize,
            storage_path,
            songs_path,
            storage_dir_raw,
        };
        config.create_directories()?;
        Ok(config)
    }

    fn create_directories(&self) -> Result<(), String> {
        let covers = self.storage_path.join("covers");
        for dir in [&self.storage_path, &self.songs_path, &covers] {
            fs::create_dir_all(dir).map_err(|error| {
                format!(
                    "Failed to create storage directories (STORAGE_DIR={} resolved to {}): {error}",
                    self.storage_dir_raw,
                    self.storage_path.display()
                )
            })?;
        }
        Ok(())
    }
}

fn resolve_path(workspace_root: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() { path.to_path_buf() } else { workspace_root.join(path) }
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
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let root = find_workspace_root(dir);
            if root.join("package.json").exists() {
                return root;
            }
        }
    }
    match env::current_dir() {
        Ok(cwd) => find_workspace_root(&cwd),
        Err(_) => PathBuf::from("."),
    }
}

fn is_ascii_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn read_bool(value: Option<&str>, fallback: bool) -> bool {
    let raw = value.unwrap_or(if fallback { "true" } else { "false" });
    !matches!(raw.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no")
}

fn read_port(value: Option<&str>, fallback: u16) -> Result<u16, String> {
    let Some(value) = value else { return Ok(fallback) };
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
        return Err(format!("API port must be between 1 and 65535, received: {value}"));
    }
    Ok(port as u16)
}

fn read_positive_integer(value: Option<&str>, fallback: u32, name: &str, max: u32) -> Result<u32, String> {
    let Some(value) = value else { return Ok(fallback) };
    if value.is_empty() {
        return Ok(fallback);
    }
    if !is_ascii_digits(value) {
        return Err(format!("{name} must be a positive integer, received: {value}"));
    }
    let parsed: u64 = value
        .parse()
        .map_err(|_| format!("{name} must be a positive integer, received: {value}"))?;
    if parsed < 1 || parsed > u64::from(max) {
        return Err(format!(
            "{name} must be a positive integer between 1 and {max}, received: {value}"
        ));
    }
    Ok(parsed as u32)
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
            return Err(format!("CORS_ORIGINS must contain exact HTTP(S) origins, received: {origin}"));
        }
    }
    Ok(origins)
}

/// True when `new URL(origin).origin === origin` for an http(s) URL: no path,
/// query or fragment, lower-cased ASCII host, optional valid port.
fn is_exact_http_origin(origin: &str) -> bool {
    let rest = match origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://")) {
        Some(rest) => rest,
        None => return false,
    };
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', '\\']) {
        return false;
    }

    let (host, port) = if rest.starts_with('[') {
        let Some(close) = rest.find(']') else { return false };
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
mod tests {
    use super::*;

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
        assert_eq!(read_positive_integer(Some("100"), 2, "MAX", 100).unwrap(), 100);
        assert!(read_positive_integer(Some("101"), 2, "MAX", 100).is_err());
    }
}
