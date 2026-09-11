//! Live downloader smoke test. Ignored by default because it hits the network
//! and downloads a real `pillows.su` file:
//!
//! `cargo test --test downloader_live -- --ignored --nocapture`

use yetracker_api::config::Config;
use yetracker_api::db;
use yetracker_api::downloader;
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
    }

    let config = Config::load().expect("config loads");
    let pool = db::create_pool(&config.storage_path.join("db.sqlite3")).expect("pool");
    db::run_migrations(&pool).expect("migrations");
    let state = AppState::new(config, pool);

    let url = "https://pillows.su/f/0004a715d6fce3b6ae9a59f9b44b766f";
    let url_owned = url.to_string();
    db::call(&state.pool, move |conn| {
        conn.execute(
            "INSERT INTO files (url, downloaded, filename, duration) VALUES (?1, 0, NULL, NULL)",
            [url_owned],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    downloader::download_songs(&state).await.expect("download completes");

    let url_owned = url.to_string();
    let row: (i64, Option<String>) = db::call(&state.pool, move |conn| {
        let mut statement = conn.prepare("SELECT downloaded, filename FROM files WHERE url = ?1")?;
        let row = statement.query_row([url_owned], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(row)
    })
    .await
    .unwrap();

    assert_eq!(row.0, 1, "downloaded flag");
    let filename = row.1.expect("filename recorded");
    assert!(state.config.songs_path.join(&filename).exists(), "file on disk: {filename}");
    let probe = yetracker_api::media::probe_audio_file(&state, &filename).await;
    assert!(probe.valid, "probe valid: {probe:?}");
    assert!(state.playable.is_playable(Some(&filename)), "marked playable");
    println!("downloaded {filename}");
}
