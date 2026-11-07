//! Shared process state: configuration, the DB pool, caches, tool
//! availability, HTTP clients and the concurrency limits.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, Semaphore};

use crate::config::Config;
use crate::db::Pool;
use crate::downloader::HttpClients;
use crate::media::{Probes, ReverifyQueue, Tools};
use crate::playable::PlayableFiles;
use crate::rank::RankCache;
use crate::routes::media::Transcodes;
use crate::routes::status::StatusSnapshot;

pub struct AppState {
    pub config: Config,
    pub pool: Pool,
    pub playable: PlayableFiles,
    pub rank_cache: RankCache,
    /// Which of ffmpeg, ffprobe and yt-dlp are installed (refreshed by every
    /// sync).
    pub tools: Tools,
    /// ffprobe verdicts per file version.
    pub probes: Probes,
    /// Files requests found missing or broken, for the sync to re-check.
    pub reverify: ReverifyQueue,
    /// Shared HTTP clients for downloads and covers.
    pub http: HttpClients,
    /// `MAX_CONCURRENT_TRANSCODES` slots; a slot is held while ffmpeg runs.
    pub transcode_slots: Arc<Semaphore>,
    /// Half of those (rounded up), also held by transcodes that start at an
    /// offset: nothing else can reuse their output, so they never take every
    /// slot from whole-song transcodes (which are shared and cached).
    pub seek_slots: Arc<Semaphore>,
    /// Transcodes in progress, joined by concurrent requests.
    pub transcodes: Transcodes,
    /// Bounds concurrent search queries (`SEARCH_CONCURRENCY`) so a burst of
    /// searches cannot occupy every pooled SQLite connection.
    pub search_slots: Arc<Semaphore>,
    /// The database's side of `/status` and when it was read (it scans the
    /// whole catalog, so it is shared for a few seconds).
    pub status_cache: Mutex<Option<(Instant, Arc<StatusSnapshot>)>>,
}

impl AppState {
    pub fn new(config: Config, pool: Pool) -> Arc<Self> {
        let transcode_slots = Arc::new(Semaphore::new(config.max_concurrent_transcodes));
        let seek_slots = Arc::new(Semaphore::new(config.max_concurrent_transcodes.div_ceil(2)));
        let search_slots = Arc::new(Semaphore::new(config.search_concurrency));
        let http = HttpClients::new().expect("the HTTP clients can be built");
        Arc::new(AppState {
            config,
            pool,
            playable: PlayableFiles::new(),
            rank_cache: RankCache::new(),
            tools: Tools::new(),
            probes: Probes::new(),
            reverify: ReverifyQueue::default(),
            http,
            transcode_slots,
            seek_slots,
            transcodes: Transcodes::default(),
            search_slots,
            status_cache: Mutex::new(None),
        })
    }
}

pub type SharedState = Arc<AppState>;
