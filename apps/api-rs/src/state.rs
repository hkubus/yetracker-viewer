//! Shared process state: DB pool, caches and the transcode semaphore.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use lru::LruCache;
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

use crate::config::Config;
use crate::cover_version::CoverVersions;
use crate::db::Pool;
use crate::media::DurationFuture;
use crate::playable::PlayableFiles;
use crate::rank::RankCache;

pub struct DurationEntry {
    pub mtime_ms: i64,
    pub future: DurationFuture,
}

pub struct AppState {
    pub config: Config,
    pub pool: Pool,
    pub playable: PlayableFiles,
    pub cover_versions: CoverVersions,
    pub rank_cache: RankCache,
    pub duration_cache: Arc<Mutex<LruCache<String, DurationEntry>>>,
    pub duration_in_flight: AsyncMutex<HashMap<String, DurationFuture>>,
    pub transcode_slots: Semaphore,
}

impl AppState {
    pub fn new(config: Config, pool: Pool) -> Arc<Self> {
        let transcode_slots = Semaphore::new(config.max_concurrent_transcodes);
        Arc::new(AppState {
            config,
            pool,
            playable: PlayableFiles::new(),
            cover_versions: CoverVersions::new(),
            rank_cache: RankCache::new(),
            duration_cache: Arc::new(Mutex::new(LruCache::new(
                std::num::NonZeroUsize::new(500).expect("non-zero capacity"),
            ))),
            duration_in_flight: AsyncMutex::new(HashMap::new()),
            transcode_slots,
        })
    }
}

pub type SharedState = Arc<AppState>;
