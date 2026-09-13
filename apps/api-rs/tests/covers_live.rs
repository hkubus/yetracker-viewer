//! Live cover smoke test. Ignored by default because it fetches a real image
//! and runs ffmpeg:
//!
//! `cargo test --test covers_live -- --ignored --nocapture`

use yetracker_api::config::Config;
use yetracker_api::db;
use yetracker_api::downloader;
use yetracker_api::state::AppState;

const IMAGE_URL: &str = "https://picsum.photos/512";

#[tokio::test]
#[ignore = "fetches a real image and runs ffmpeg"]
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
    db::run_migrations(&pool).expect("migrations");
    let state = AppState::new(config, pool);

    db::call(&state.pool, |conn| {
        conn.execute(
            "INSERT INTO eras (id, name, notes, image_url, description, dominant_color, is_main) \
             VALUES (1, 'Test Era', '', ?1, '', NULL, 1)",
            [IMAGE_URL],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    downloader::download_covers(&state)
        .await
        .expect("covers complete");

    let cover_path = state.config.storage_path.join("covers").join("1.avif");
    let metadata = tokio::fs::metadata(&cover_path)
        .await
        .expect("cover written");
    assert!(metadata.len() > 0, "cover non-empty");

    let color: Option<String> = db::call(&state.pool, |conn| {
        let mut statement = conn.prepare("SELECT dominant_color FROM eras WHERE id = 1")?;
        let color = statement.query_row([], |row| row.get(0))?;
        Ok(color)
    })
    .await
    .unwrap();
    let color = color.expect("dominant color set");
    assert_ne!(color, "666666", "extracted color replaces the default");
    println!("cover {} bytes, dominant color {color}", metadata.len());
}
