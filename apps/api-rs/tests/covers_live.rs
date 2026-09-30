//! Live cover smoke test. Ignored by default because it renders the real
//! sheet on Google Sheets, downloads an artwork image and runs ffmpeg:
//!
//! `cargo test --test covers_live -- --ignored --nocapture`

use tokio_util::sync::CancellationToken;
use yetracker_api::config::Config;
use yetracker_api::db;
use yetracker_api::downloader;
use yetracker_api::importer;
use yetracker_api::media::Tool;
use yetracker_api::state::AppState;

#[tokio::test]
#[ignore = "renders the live sheet, downloads artwork and runs ffmpeg"]
async fn downloads_and_encodes_a_cover() {
    let dir = std::path::PathBuf::from("/tmp/yt-cover-live");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    unsafe {
        std::env::set_var("STORAGE_DIR", &dir);
        std::env::set_var("SONGS_DIR", dir.join("songs"));
    }

    let config = Config::load().expect("config loads");
    let pool = db::create_pool(&config.storage_path.join("db.sqlite3")).expect("pool");
    db::run_migrations(&pool, &config).expect("migrations");
    let state = AppState::new(config, pool);
    let tools = state.tools.detect(&Tool::ALL).await;

    let key = importer::era_key("Late Registration");
    db::call(&state.pool, move |conn| {
        conn.execute(
            "INSERT INTO eras (id, key, name, notes, description, dominant_color, is_main, position) \
             VALUES (1, ?1, 'Late Registration', '', '', '666666', 1, 1)",
            [key],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let summary = downloader::sync_covers(&state, &CancellationToken::new(), tools)
        .await
        .expect("covers complete");
    assert_eq!(summary.written, 1, "{summary:?}");

    let covers = downloader::covers_dir(&state.config);
    let primary = if tools.ffmpeg { "1.avif" } else { "1.jpg" };
    let metadata = std::fs::metadata(covers.join(primary)).expect("cover written");
    assert!(metadata.len() > 0, "cover non-empty");

    let (version, color): (Option<String>, String) = db::call(&state.pool, |conn| {
        Ok(conn.query_row(
            "SELECT cover_version, dominant_color FROM eras WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    })
    .await
    .unwrap();
    assert_eq!(version.as_deref().map(str::len), Some(12));
    if tools.ffmpeg {
        assert_eq!(color, "5a240a", "Late Registration keeps its fixed colour");
    }
    println!(
        "cover {} bytes, version {version:?}, colour {color}",
        metadata.len()
    );
}
