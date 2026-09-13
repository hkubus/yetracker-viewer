//! Cover and song downloader, ported from `scraper/downloader.ts`.
//!
//! Covers are fetched, encoded to 512x512 AVIF with ffmpeg and colour-sampled.
//! Songs come from `pillows.su` (direct HTTP) or `yt-dlp` (YouTube/Instagram/
//! Twitter), gated by `YOUTUBE_DOWNLOAD`.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use url::Url;

use crate::db;
use crate::error::ApiError;
use crate::media::{InvalidReason, ProbeOutcome, delete_invalid_file, probe_audio_file};
use crate::state::AppState;

const COVER_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const COVER_MAX_BYTES: u64 = 20 * 1024 * 1024;
const PILLOW_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const YTDLP_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const FETCH_USER_AGENT: &str = "yetracker-viewer/1.0 (+https://yetracker.net)";
const COVER_CONCURRENCY: usize = 4;
const SONG_CONCURRENCY: usize = 5;

const LATE_REGISTRATION_DOMINANT_COLOR: &str = "5a240a";
const LATE_REGISTRATION_ERA_NAME: &str = "Late Registration";

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

async fn stream_body_to_file(response: reqwest::Response, path: &Path) -> Result<(), String> {
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|error| error.to_string())?;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        file.write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
    }
    file.flush().await.map_err(|error| error.to_string())?;
    Ok(())
}

struct EraCoverJob {
    id: i64,
    name: Option<String>,
    image_url: Option<String>,
    dominant_color: Option<String>,
    cover_source: Option<String>,
}

pub async fn download_covers(state: &AppState) -> Result<(), ApiError> {
    let eras = db::call(&state.pool, |conn| {
        let mut statement = conn
            .prepare("SELECT id, name, image_url, dominant_color, cover_source FROM eras WHERE is_main = 1")?;
        let rows = statement.query_map([], |row| {
            Ok(EraCoverJob {
                id: row.get(0)?,
                name: row.get(1)?,
                image_url: row.get(2)?,
                dominant_color: row.get(3)?,
                cover_source: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await?;

    let client = reqwest::Client::builder()
        .timeout(COVER_FETCH_TIMEOUT)
        .build()
        .map_err(ApiError::unexpected)?;

    stream::iter(eras)
        .for_each_concurrent(COVER_CONCURRENCY, |era| {
            let client = &client;
            async move {
                if let Err(error) = process_cover(state, client, &era).await {
                    eprintln!(
                        "cover processing failed for era {} ({:?}) {error:?}",
                        era.id, era.name
                    );
                }
            }
        })
        .await;
    Ok(())
}

async fn process_cover(
    state: &AppState,
    client: &reqwest::Client,
    era: &EraCoverJob,
) -> Result<(), ApiError> {
    let Some(image_url) = era.image_url.as_deref().filter(|url| !url.is_empty()) else {
        eprintln!("no image url for era id {}", era.id);
        return Ok(());
    };
    let covers_dir = state.config.storage_path.join("covers");
    let cover_path = covers_dir.join(format!("{}.avif", era.id));

    // Skip only when the cover already reflects this artwork URL. The colour
    // alone is not enough: it is preserved across imports even when the
    // artwork changes, so the cover may still need re-encoding.
    if era.dominant_color.is_some() && era.cover_source.as_deref() == Some(image_url) {
        if let Ok(metadata) = tokio::fs::metadata(&cover_path).await {
            if metadata.is_file() && metadata.len() > 0 {
                return Ok(());
            }
        }
    }

    let response = client
        .get(image_url)
        .header(reqwest::header::USER_AGENT, FETCH_USER_AGENT)
        .send()
        .await
        .map_err(ApiError::unexpected)?;
    if !response.status().is_success() {
        return Err(ApiError::unexpected(format!(
            "Failed to download cover {}: HTTP {}",
            era.id,
            response.status().as_u16()
        )));
    }
    if response.content_length().unwrap_or(0) > COVER_MAX_BYTES {
        return Err(ApiError::unexpected(format!(
            "Cover {} exceeds the 20 MiB size limit",
            era.id
        )));
    }

    let temp_path = covers_dir.join(format!("{}.source.tmp", era.id));
    let encoded_path = covers_dir.join(format!("{}.tmp.avif", era.id));

    let result = async {
        stream_body_to_file(response, &temp_path).await?;
        run_ffmpeg(
            &[
                "-y",
                "-i",
                &temp_path.to_string_lossy(),
                "-vf",
                "scale=512:512:force_original_aspect_ratio=increase,crop=512:512,setsar=1",
                "-c:v",
                "libsvtav1",
                "-crf",
                "18",
                "-preset",
                "3",
                "-still-picture",
                "1",
                &encoded_path.to_string_lossy(),
            ],
            Duration::from_secs(120),
        )
        .await?;
        tokio::fs::rename(&encoded_path, &cover_path)
            .await
            .map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    }
    .await;

    let _ = tokio::fs::remove_file(&temp_path).await;
    let _ = tokio::fs::remove_file(&encoded_path).await;
    result.map_err(ApiError::unexpected)?;

    let mut dominant_color_hex = "666666".to_string();
    match state.dominant_colors.get(&cover_path).await {
        Ok(color) => {
            let extracted: String = color.iter().map(|byte| format!("{byte:02x}")).collect();
            if extracted.len() == 6 && extracted.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                dominant_color_hex = extracted;
            }
        }
        Err(error) => eprintln!(
            "dominant color extraction failed for era {} ({:?}) {error}",
            era.id, era.name
        ),
    }

    if era.name.as_deref() == Some(LATE_REGISTRATION_ERA_NAME) {
        dominant_color_hex = LATE_REGISTRATION_DOMINANT_COLOR.to_string();
    }

    let era_id = era.id;
    let cover_source = image_url.to_string();
    db::call(&state.pool, move |conn| {
        conn.execute(
            "UPDATE eras SET dominant_color = ?1, cover_source = ?2 WHERE id = ?3",
            rusqlite::params![dominant_color_hex, cover_source, era_id],
        )?;
        Ok(())
    })
    .await?;
    Ok(())
}

async fn run_ffmpeg(args: &[&str], timeout: Duration) -> Result<(), String> {
    let mut command = tokio::process::Command::new("ffmpeg");
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(timeout, command.status()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(format!("ffmpeg exited with {status}")),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("ffmpeg timed out".to_string()),
    }
}

fn split_filename(filename: &str) -> Option<(&str, &str)> {
    let dot = filename.rfind('.')?;
    if dot == 0 || dot == filename.len() - 1 {
        return None;
    }
    Some((&filename[..dot], &filename[dot + 1..]))
}

fn parse_content_disposition_extension(header: Option<&str>) -> String {
    let Some(header) = header else {
        return "bin".to_string();
    };
    let Ok(regex) = regex::Regex::new(r#"filename\*?=(?:UTF-8'' )?"?([^";]+)"?"#) else {
        return "bin".to_string();
    };
    let Some(captures) = regex.captures(header) else {
        return "bin".to_string();
    };
    let mut candidate = captures
        .get(1)
        .map(|value| value.as_str().trim().to_string())
        .unwrap_or_default();
    if header.to_ascii_lowercase().starts_with("utf-8''") || candidate.contains('%') {
        if let Ok(decoded) = percent_decode(&candidate) {
            candidate = decoded;
        }
    }
    let extension = match candidate.rfind('.') {
        Some(dot) => candidate[dot + 1..].to_ascii_lowercase(),
        None => candidate.to_ascii_lowercase(),
    };
    if !extension.is_empty()
        && extension.len() <= 8
        && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        extension
    } else {
        "bin".to_string()
    }
}

fn percent_decode(value: &str) -> Result<String, ()> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(());
            }
            let high = (bytes[index + 1] as char).to_digit(16).ok_or(())?;
            let low = (bytes[index + 2] as char).to_digit(16).ok_or(())?;
            decoded.push((high * 16 + low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| ())
}

async fn update_file_row(
    state: &AppState,
    url: &str,
    downloaded: i64,
    filename: &str,
) -> Result<(), ApiError> {
    let url = url.to_string();
    let filename = filename.to_string();
    db::call(&state.pool, move |conn| {
        conn.execute(
            "UPDATE files SET downloaded = ?1, filename = ?2 WHERE url = ?3",
            rusqlite::params![downloaded, filename, url],
        )?;
        Ok(())
    })
    .await
}

pub async fn download_songs(state: &AppState) -> Result<(), ApiError> {
    let songs_path = state.config.songs_path.clone();
    if tokio::fs::metadata(&songs_path).await.is_err() {
        tokio::fs::create_dir_all(&songs_path)
            .await
            .map_err(|error| ApiError::unexpected(error.to_string()))?;
    }

    let mut hash_to_extension: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut entries = tokio::fs::read_dir(&songs_path)
        .await
        .map_err(|error| ApiError::unexpected(error.to_string()))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| ApiError::unexpected(error.to_string()))?
    {
        let filename = entry.file_name().to_string_lossy().into_owned();
        if filename.ends_with(".tmp") || filename.ends_with(".part") {
            continue;
        }
        if let Some((hash, extension)) = split_filename(&filename) {
            hash_to_extension.insert(hash.to_string(), extension.to_string());
        }
    }

    let hash_to_extension = std::sync::Arc::new(hash_to_extension);

    let files = db::call(&state.pool, |conn| {
        let mut statement =
            conn.prepare("SELECT url FROM files WHERE downloaded IS NULL OR downloaded = 0")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await?;

    println!("starting download of {} files", files.len());

    stream::iter(files)
        .for_each_concurrent(SONG_CONCURRENCY, |url| {
            let hash_to_extension = hash_to_extension.clone();
            async move {
                let file_url = url;
                let mut filename = sha256_hex(&file_url);
                if let Some(extension) = hash_to_extension.get(&filename) {
                    let reused = format!("{filename}.{extension}");
                    let probe = probe_audio_file(state, &reused).await;
                    if probe.valid {
                        let _ = update_file_row(state, &file_url, 1, &reused).await;
                        state
                            .playable
                            .refresh_one(&state.config.songs_path, &reused)
                            .await;
                        return;
                    }
                    // A file with this hash exists but is corrupt/not audio: remove
                    // it and fall through to download a fresh copy.
                    delete_invalid_file(
                        state,
                        &reused,
                        Some(&file_url),
                        probe.reason.unwrap_or(InvalidReason::Unreadable),
                    )
                    .await;
                }

                let outcome = download_one(state, &file_url, &mut filename).await;
                if let Err(error) = outcome {
                    eprintln!("download failed for {file_url} {error}");
                    let _ = update_file_row(state, &file_url, 0, &filename).await;
                    if !filename.is_empty() {
                        state.playable.set_playable(&filename, false);
                    }
                    filename.clear();
                }

                if filename.is_empty() {
                    return;
                }
                let probe: ProbeOutcome = probe_audio_file(state, &filename).await;
                if !probe.valid {
                    delete_invalid_file(
                        state,
                        &filename,
                        Some(&file_url),
                        probe.reason.unwrap_or(InvalidReason::Unreadable),
                    )
                    .await;
                    return;
                }
                let _ = update_file_row(state, &file_url, 1, &filename).await;
                state
                    .playable
                    .refresh_one(&state.config.songs_path, &filename)
                    .await;
            }
        })
        .await;
    Ok(())
}

async fn download_one(
    state: &AppState,
    url_string: &str,
    filename: &mut String,
) -> Result<(), String> {
    let url = Url::parse(url_string).map_err(|error| error.to_string())?;
    let host = url.host_str().unwrap_or_default().to_string();
    match host.as_str() {
        "pillows.su" => {
            let hash = url
                .path()
                .rsplit('/')
                .find(|segment| !segment.is_empty())
                .unwrap_or_default();
            let response = reqwest::Client::new()
                .get(format!("https://api.pillows.su/api/download/{hash}"))
                .header(reqwest::header::USER_AGENT, FETCH_USER_AGENT)
                .timeout(PILLOW_DOWNLOAD_TIMEOUT)
                .send()
                .await
                .map_err(|error| error.to_string())?;
            if !response.status().is_success() {
                return Err(format!(
                    "Failed to download {url_string}: HTTP {}",
                    response.status().as_u16()
                ));
            }
            let extension = parse_content_disposition_extension(
                response
                    .headers()
                    .get(reqwest::header::CONTENT_DISPOSITION)
                    .and_then(|value| value.to_str().ok()),
            );
            *filename = format!("{hash}.{extension}");
            let temp_filename = format!("{filename}.tmp");
            let temp_path = state.config.songs_path.join(&temp_filename);
            let final_path = state.config.songs_path.join(&*filename);
            if let Err(error) = stream_body_to_file(response, &temp_path).await {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(error);
            }
            if let Err(error) = tokio::fs::rename(&temp_path, &final_path).await {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(error.to_string());
            }
            Ok(())
        }
        "youtu.be" | "www.youtube.com" => {
            if !state.config.youtube_download {
                println!("skipping YouTube download (YOUTUBE_DOWNLOAD=false): {url_string}");
                filename.clear();
                return Ok(());
            }
            download_ytdlp(state, &url, filename.as_str()).await?;
            *filename = format!("{filename}.ogg");
            Ok(())
        }
        "www.instagram.com" | "twitter.com" => {
            download_ytdlp(state, &url, filename.as_str()).await?;
            *filename = format!("{filename}.ogg");
            Ok(())
        }
        _ => Err(format!("unknown host {host} for {url_string}")),
    }
}

async fn download_ytdlp(state: &AppState, url: &Url, filename: &str) -> Result<(), String> {
    let output_path = state.config.songs_path.join(filename);
    let timestamp = url
        .query_pairs()
        .find(|(key, _)| key == "t")
        .map(|(_, value)| value.into_owned());

    let mut args: Vec<String> = vec![
        "-x".to_string(),
        "--audio-quality".to_string(),
        "0".to_string(),
        "--audio-format".to_string(),
        "opus".to_string(),
        "-o".to_string(),
        output_path.to_string_lossy().into_owned(),
    ];
    if let Some(timestamp) = timestamp {
        if !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!(
                "Refusing to pass non-numeric timestamp to yt-dlp: {timestamp}"
            ));
        }
        args.push("--download-sections".to_string());
        args.push(format!("*{timestamp}-inf"));
    }
    args.push(url.to_string());

    let mut delay_ms: u64 = 1000;
    for attempt in 0..3 {
        let mut command = tokio::process::Command::new("yt-dlp");
        command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let result = tokio::time::timeout(YTDLP_TIMEOUT, command.status()).await;
        if let Ok(Ok(status)) = &result {
            if status.success() {
                break;
            }
        }
        if attempt == 2 {
            return Err(format!("yt-dlp failed for {url}"));
        }
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        delay_ms *= 2;
    }

    let opus_path = format!("{}.opus", output_path.to_string_lossy());
    let ogg_path = format!("{}.ogg", output_path.to_string_lossy());
    if tokio::fs::metadata(&opus_path).await.is_ok() {
        if opus_path != ogg_path {
            tokio::fs::rename(&opus_path, &ogg_path)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    } else if tokio::fs::metadata(&ogg_path).await.is_ok() {
        Ok(())
    } else {
        Err(format!(
            "yt-dlp produced neither {opus_path} nor {ogg_path}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_disposition_extensions() {
        assert_eq!(
            parse_content_disposition_extension(Some("attachment; filename=\"x.mp3\"")),
            "mp3"
        );
        assert_eq!(
            parse_content_disposition_extension(Some("attachment; filename*=UTF-8''a%20b.flac")),
            "flac"
        );
        assert_eq!(parse_content_disposition_extension(None), "bin");
        assert_eq!(
            parse_content_disposition_extension(Some("attachment; filename=\"noext\"")),
            "noext"
        );
    }

    #[test]
    fn splits_hash_and_extension() {
        assert_eq!(split_filename("abcd.mp3"), Some(("abcd", "mp3")));
        assert_eq!(split_filename(".tmp"), None);
        assert_eq!(split_filename("noext"), None);
        assert_eq!(split_filename("x."), None);
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b%2Fc"), Ok("a b/c".to_string()));
        assert!(percent_decode("%ZZ").is_err());
    }
}
