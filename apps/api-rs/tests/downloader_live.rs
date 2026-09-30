//! Live downloader smoke test. Ignored by default because it hits the network
//! and downloads a real `pillows.su` file:
//!
//! `cargo test --test downloader_live -- --ignored --nocapture`

use tokio_util::sync::CancellationToken;
use yetracker_api::config::Config;
use yetracker_api::db;
use yetracker_api::downloader;
use yetracker_api::media::Tool;
use yetracker_api::state::AppState;

#[tokio::test]
#[ignore = "hits pillows.su and downloads a real file"]
async fn downloads_a_pillows_file_and_records_it() {
    let dir = std::path::PathBuf::from("/tmp/yt-dl-live");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    unsafe {
        std::env::set_var("STORAGE_DIR", &dir);
        std::env::set_var("SONGS_DIR", dir.join("songs"));
        std::env::set_var("YOUTUBE_DOWNLOAD", "false");
        std::env::set_var("MAX_DOWNLOADS_PER_CYCLE", "1");
    }

    let config = Config::load().expect("config loads");
    let pool = db::create_pool(&config.storage_path.join("db.sqlite3")).expect("pool");
    db::run_migrations(&pool, &config).expect("migrations");
    let state = AppState::new(config, pool);
    state.tools.detect(&Tool::ALL).await;

    let url = "https://pillows.su/f/0004a715d6fce3b6ae9a59f9b44b766f";
    let url_owned = url.to_string();
    db::call(&state.pool, move |conn| {
        conn.execute(
            "INSERT INTO songs (id, era, name, url, position, quality) \
             VALUES (1, NULL, 'Live test', ?1, 1, 'CD Quality')",
            [&url_owned],
        )?;
        conn.execute(
            "INSERT INTO files (url, status, attempts) VALUES (?1, 'pending', 0)",
            [&url_owned],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let summary = downloader::download_songs(&state, &CancellationToken::new())
        .await
        .expect("download completes");
    assert_eq!(summary.downloaded, 1, "{summary:?}");

    let url_owned = url.to_string();
    let (status, filename): (String, Option<String>) = db::call(&state.pool, move |conn| {
        Ok(conn.query_row(
            "SELECT status, filename FROM files WHERE url = ?1",
            [url_owned],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    })
    .await
    .unwrap();

    assert_eq!(status, "downloaded");
    let filename = filename.expect("filename recorded");
    assert!(
        state.config.songs_path.join(&filename).exists(),
        "file on disk: {filename}"
    );
    assert!(
        state.playable.is_playable(Some(&filename)),
        "marked playable"
    );
    println!("downloaded {filename}");
}
