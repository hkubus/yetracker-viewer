//! Maintenance jobs of the background sync: the duration backfill (which
//! also verifies downloaded files), re-verification of files that requests
//! found missing or broken, the cleanup of stale rows, orphaned media, covers
//! of removed eras and the transcode cache, and the removal of temporary
//! files left behind by a previous run.
//!
//! Nothing here deletes media outright: files are moved into quarantine
//! (deleted after a grace period) when ffprobe definitively rejects them or
//! when no `files` row refers to them any more, and only under names the
//! downloader produces. Rows that hold downloaded media are never removed
//! automatically, and a cleanup that would remove an unusual share of the
//! rows at once removes nothing. The covers of a removed era are set aside
//! while its tombstone can still bring the era back (with its id, and then
//! its cover), and deleted once the tombstone expires.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use futures_util::stream::{self, StreamExt};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::db::{self, meta_keys};
use crate::downloader::{
    self, COVER_EXTENSIONS, FileCheck, check_file, covers_dir, natural_stem, record_failure,
    record_missing, set_aside_cover, split_media_name,
};
use crate::error::ApiError;
use crate::importer::TOMBSTONE_RETENTION_SECS;
use crate::media::{Quarantine, quarantine, quarantined_at};
use crate::routes::media::{enforce_transcode_budget, transcodes_dir};
use crate::state::AppState;

/// `files` rows not seen by an import for this long are removed.
const UNSEEN_ROW_SECS: i64 = 7 * 24 * 60 * 60;
/// One cleanup removes nothing when more than this share of the `files`
/// rows would go.
const MAX_SWEEP_PERCENT: usize = 10;
/// Media no row refers to is kept at least this long.
const ORPHAN_MEDIA_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// Leftover temporary cover files older than this are removed.
const STALE_TEMP_AGE: Duration = Duration::from_secs(60 * 60);

/// What happened to one stored file.
enum Verified {
    Kept,
    Missing,
    Rejected,
    Unverified,
}

/// Checks a downloaded file: stores its duration, resets the row when the
/// file is gone, moves it aside and counts a failed attempt when ffprobe
/// rejects it.
async fn verify_stored_file(
    state: &AppState,
    url: &str,
    filename: &str,
) -> Result<Verified, ApiError> {
    match check_file(state, filename).await {
        FileCheck::Good(duration) => {
            if let Some(duration) = duration {
                let (url, filename) = (url.to_string(), filename.to_string());
                db::call(&state.pool, move |conn| {
                    conn.execute(
                        "UPDATE files SET duration = ?1 WHERE url = ?2 AND filename = ?3",
                        rusqlite::params![duration, url, filename],
                    )?;
                    Ok(())
                })
                .await?;
            }
            Ok(Verified::Kept)
        }
        FileCheck::Unverified => Ok(Verified::Unverified),
        FileCheck::Missing => {
            info!(
                url,
                filename, "stored file is missing; it will be downloaded again"
            );
            state.playable.set_playable(filename, false);
            record_missing(state, url).await?;
            Ok(Verified::Missing)
        }
        FileCheck::Rejected(reason) => {
            let gave_up = record_failure(state, url, &reason, false).await?;
            warn!(url, filename, %reason, gave_up, "stored file rejected");
            Ok(Verified::Rejected)
        }
    }
}

/// Re-checks the files requests reported (see `media::ReverifyQueue`).
pub async fn reverify_queued(state: &AppState) -> Result<usize, ApiError> {
    let urls = state.reverify.take_all();
    if urls.is_empty() {
        return Ok(0);
    }
    let mut checked = 0;
    for url in urls {
        let lookup = url.clone();
        let row: Option<(String, Option<String>)> = db::call(&state.pool, move |conn| {
            let mut statement =
                conn.prepare("SELECT status, filename FROM files WHERE url = ?1")?;
            let mut rows = statement.query_map([lookup], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.next().transpose().map_err(ApiError::from)
        })
        .await?;
        let Some((status, Some(filename))) = row else {
            continue;
        };
        if status != "downloaded" {
            continue;
        }
        checked += 1;
        if let Verified::Unverified = verify_stored_file(state, &url, &filename).await? {
            debug!(url, filename, "could not verify a reported file yet");
        }
    }
    info!(checked, "re-verified files reported by requests");
    Ok(checked)
}

/// What one duration backfill did.
#[derive(Debug, Default, Clone, Copy)]
pub struct BackfillSummary {
    pub checked: usize,
    pub missing: usize,
    pub rejected: usize,
    pub unverified: usize,
}

/// Probes downloaded files without a known duration, storing the duration
/// and handling files that vanished or turn out to be broken.
pub async fn backfill_durations(
    state: &AppState,
    shutdown: &CancellationToken,
) -> Result<BackfillSummary, ApiError> {
    let files: Vec<(String, String)> = db::call(&state.pool, |conn| {
        let mut statement = conn.prepare(
            "SELECT url, filename FROM files WHERE url IS NOT NULL AND status = 'downloaded' \
             AND filename IS NOT NULL AND (duration IS NULL OR duration <= 0)",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await?;
    let mut summary = BackfillSummary::default();
    if files.is_empty() {
        return Ok(summary);
    }
    let concurrency = state.config.backfill_concurrency.clamp(1, files.len());
    let outcomes: Vec<Result<Verified, ApiError>> = stream::iter(files)
        .take_while(|_| std::future::ready(!shutdown.is_cancelled()))
        .map(|(url, filename)| async move { verify_stored_file(state, &url, &filename).await })
        .buffer_unordered(concurrency)
        .collect()
        .await;
    for outcome in outcomes {
        summary.checked += 1;
        match outcome {
            Ok(Verified::Kept) => {}
            Ok(Verified::Missing) => summary.missing += 1,
            Ok(Verified::Rejected) => summary.rejected += 1,
            Ok(Verified::Unverified) => summary.unverified += 1,
            Err(error) => warn!(%error, "backfill could not update a file row"),
        }
    }
    info!(
        checked = summary.checked,
        missing = summary.missing,
        rejected = summary.rejected,
        unverified = summary.unverified,
        "duration backfill finished"
    );
    Ok(summary)
}

/// What one cleanup did.
#[derive(Debug, Default, Clone, Copy)]
pub struct CleanupSummary {
    /// `files` rows removed (unseen, without downloaded media).
    pub rows_removed: usize,
    /// Unseen rows kept because they hold downloaded media.
    pub rows_kept: usize,
    /// The unseen-row sweep was skipped: it would have removed too much.
    pub sweep_refused: bool,
    /// Media files moved to quarantine (no row refers to them any more).
    pub media_quarantined: usize,
    /// Quarantined files deleted after their retention.
    pub quarantine_removed: usize,
    /// Cover files of removed eras set aside while a tombstone can restore
    /// the era.
    pub covers_set_aside: usize,
    /// Cover files deleted: of removed eras without a (live) tombstone, and
    /// set-aside ones that expired or were superseded.
    pub covers_removed: usize,
    pub transcodes_removed: usize,
}

/// Whether a stored name is one the downloader produces (`<hash>.<ext>`), so
/// that files someone else put into `SONGS_DIR` are never touched.
fn is_downloader_name(filename: &str) -> bool {
    split_media_name(filename).is_some_and(|(stem, _)| {
        (16..=128).contains(&stem.len()) && stem.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn older_than(metadata: &std::fs::Metadata, age: Duration) -> bool {
    metadata
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|elapsed| elapsed >= age)
}

/// Names referenced by `files` rows: stored file names and the natural stems
/// of every link (a file under a row's natural name is that row's media even
/// before the row points at it).
struct References {
    filenames: HashSet<String>,
    stems: HashSet<String>,
}

impl References {
    fn covers(&self, filename: &str) -> bool {
        self.filenames.contains(filename)
            || split_media_name(filename).is_some_and(|(stem, _)| self.stems.contains(stem))
    }
}

async fn references(state: &AppState) -> Result<References, ApiError> {
    let rows: Vec<(String, Option<String>)> = db::call(&state.pool, |conn| {
        let mut statement =
            conn.prepare("SELECT url, filename FROM files WHERE url IS NOT NULL")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(ApiError::from)
    })
    .await?;
    let mut references = References {
        filenames: HashSet::new(),
        stems: HashSet::new(),
    };
    for (url, filename) in rows {
        references.stems.insert(natural_stem(&url));
        if let Some(filename) = filename {
            references.filenames.insert(filename);
        }
    }
    Ok(references)
}

/// What [`sweep_unseen_rows`] decided.
#[derive(Debug, Default)]
struct Sweep {
    /// Removed rows (url, stored file name).
    removed: Vec<(String, Option<String>)>,
    /// Links of unseen rows kept because they hold downloaded media.
    kept: Vec<String>,
    /// Refused: (rows that would have gone, rows in the table).
    refused: Option<(usize, usize)>,
}

/// Deletes the `files` rows no import has seen for 7 days (measured against
/// the last successful import, so a broken sheet never empties the table),
/// with two limits, because an upstream change in how the sheet writes its
/// links looks exactly like every link disappearing:
/// - when more than [`MAX_SWEEP_PERCENT`] of all rows would go in one run,
///   nothing is deleted (the caller reports it);
/// - rows holding downloaded media are never deleted here: the media may
///   no longer exist anywhere else. They are kept (and reported) until
///   someone removes them by hand or the link returns.
fn sweep_unseen_rows(conn: &Connection, now: i64) -> Result<Sweep, ApiError> {
    conn.execute(
        "UPDATE files SET last_seen_at = ?1 WHERE last_seen_at IS NULL",
        [now],
    )?;
    let mut sweep = Sweep::default();
    let Some(last_import) = db::meta_get_i64(conn, meta_keys::LAST_IMPORT_AT)? else {
        return Ok(sweep);
    };
    let cutoff = last_import - UNSEEN_ROW_SECS;
    // Read and delete under the write lock.
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let total: i64 = transaction.query_row("SELECT count(*) FROM files", [], |row| row.get(0))?;
    let unseen: Vec<(String, Option<String>, String)> = {
        let mut statement = transaction.prepare(
            "SELECT url, filename, status FROM files WHERE url IS NOT NULL AND last_seen_at < ?1",
        )?;
        let rows =
            statement.query_map([cutoff], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    if unseen.is_empty() {
        return Ok(sweep);
    }
    let total = total.max(0) as usize;
    if unseen.len() * 100 > total * MAX_SWEEP_PERCENT {
        sweep.refused = Some((unseen.len(), total));
        return Ok(sweep);
    }
    {
        let mut delete = transaction.prepare("DELETE FROM files WHERE url = ?1")?;
        for (url, filename, status) in unseen {
            if status == "downloaded" || filename.is_some() {
                sweep.kept.push(url);
            } else {
                delete.execute([&url])?;
                sweep.removed.push((url, filename));
            }
        }
    }
    transaction.commit()?;
    Ok(sweep)
}

/// Logs what the sweep refused or kept. Kept rows are an error the first
/// time they show up (the count went up since the last cleanup) and a debug
/// line afterwards, so a known leftover does not flood the log.
async fn report_sweep(state: &AppState, sweep: &Sweep) -> Result<(), ApiError> {
    if let Some((unseen, total)) = sweep.refused {
        error!(
            unseen,
            rows = total,
            limit_percent = MAX_SWEEP_PERCENT,
            "refusing to remove this many download rows in one cleanup; the sheet's links may \
             have changed shape. Rows and media stay; remove them by hand if the change is real"
        );
        return Ok(());
    }
    let kept = sweep.kept.len() as i64;
    let reported = db::call(&state.pool, move |conn| {
        let reported = db::meta_get_i64(conn, meta_keys::CLEANUP_KEPT_DOWNLOADED)?.unwrap_or(0);
        db::meta_set(
            conn,
            meta_keys::CLEANUP_KEPT_DOWNLOADED,
            Some(&kept.to_string()),
        )?;
        Ok(reported)
    })
    .await?;
    if kept > reported {
        let examples: Vec<&str> = sweep.kept.iter().take(5).map(String::as_str).collect();
        error!(
            rows = kept,
            new = kept - reported,
            examples = %examples.join(", "),
            "download rows with downloaded media have not been seen by an import for 7 days; \
             they are kept, since the media may exist nowhere else. Delete the rows by hand to \
             free the space (their media then goes to quarantine)"
        );
    } else if kept > 0 {
        debug!(
            rows = kept,
            "unseen download rows with downloaded media are still kept"
        );
    }
    Ok(())
}

async fn remove_file_logged(path: &Path, what: &str) -> bool {
    match tokio::fs::remove_file(path).await {
        Ok(()) => {
            debug!(path = %path.display(), what, "removed");
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            warn!(path = %path.display(), %error, what, "could not remove");
            false
        }
    }
}

/// Moves media no row refers to any more into quarantine (see
/// [`Quarantine::Retired`]); the file can be restored by hand until the
/// quarantine is purged.
async fn retire_media(state: &AppState, name: &str, why: &str, now: i64) -> bool {
    match quarantine(&state.config.songs_path, name, now, Quarantine::Retired).await {
        Ok(moved) => {
            state.playable.set_playable(name, false);
            info!(filename = name, quarantined = %moved, why, "media moved to quarantine");
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            warn!(filename = name, %error, why, "could not move media to quarantine");
            false
        }
    }
}

struct DirEntry {
    name: String,
    path: PathBuf,
    metadata: std::fs::Metadata,
}

async fn list_dir(dir: &Path) -> io::Result<Vec<DirEntry>> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut entries = Vec::new();
        let listing = match std::fs::read_dir(&dir) {
            Ok(listing) => listing,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(entries),
            Err(error) => return Err(error),
        };
        for entry in listing {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            entries.push(DirEntry {
                name,
                path: entry.path(),
                metadata,
            });
        }
        Ok(entries)
    })
    .await
    .map_err(io::Error::other)?
}

/// Runs the cleanup (see the module docs).
pub async fn cleanup(state: &AppState) -> Result<CleanupSummary, ApiError> {
    let mut summary = CleanupSummary::default();
    let songs_dir = state.config.songs_path.clone();
    let now = db::unix_now();

    let sweep = db::call(&state.pool, move |conn| sweep_unseen_rows(conn, now)).await?;
    report_sweep(state, &sweep).await?;
    summary.rows_removed = sweep.removed.len();
    summary.rows_kept = sweep.kept.len();
    summary.sweep_refused = sweep.refused.is_some();
    let references = references(state).await?;

    let entries = match list_dir(&songs_dir).await {
        Ok(entries) => entries,
        Err(error) => {
            warn!(path = %songs_dir.display(), %error, "could not list the songs directory");
            Vec::new()
        }
    };

    // Media of the removed rows (unless another row uses it) is retired.
    let removed_names: HashSet<String> = sweep
        .removed
        .iter()
        .flat_map(|(url, filename)| {
            let stem = natural_stem(url);
            filename.iter().cloned().chain(
                entries
                    .iter()
                    .filter(move |entry| {
                        split_media_name(&entry.name).is_some_and(|(found, _)| found == stem)
                    })
                    .map(|entry| entry.name.clone()),
            )
        })
        .collect();
    for name in &removed_names {
        if !references.covers(name)
            && crate::playable::is_safe_filename(name)
            && retire_media(state, name, "its download row was removed", now).await
        {
            summary.media_quarantined += 1;
        }
    }

    // With no rows at all (a fresh database) nothing counts as orphaned: the
    // files may simply not be linked yet.
    let has_rows = !references.filenames.is_empty() || !references.stems.is_empty();
    for entry in &entries {
        if !entry.metadata.is_file() || removed_names.contains(&entry.name) {
            continue;
        }
        if let Some((at, kind)) = quarantined_at(&entry.name) {
            if now - at >= kind.retention_secs()
                && remove_file_logged(&entry.path, "quarantined file").await
            {
                summary.quarantine_removed += 1;
            }
            continue;
        }
        if has_rows
            && is_downloader_name(&entry.name)
            && !references.covers(&entry.name)
            && older_than(&entry.metadata, ORPHAN_MEDIA_AGE)
            && retire_media(state, &entry.name, "no download row refers to it", now).await
        {
            summary.media_quarantined += 1;
        }
    }

    (summary.covers_set_aside, summary.covers_removed) = remove_orphan_covers(state, now).await?;

    match enforce_transcode_budget(
        transcodes_dir(&state.config),
        state.config.transcode_cache_max_bytes,
    )
    .await
    {
        Ok((removed, _)) => summary.transcodes_removed = removed,
        Err(error) => warn!(%error, "could not trim the transcode cache"),
    }

    info!(
        rows_removed = summary.rows_removed,
        rows_kept = summary.rows_kept,
        sweep_refused = summary.sweep_refused,
        media_quarantined = summary.media_quarantined,
        quarantine_removed = summary.quarantine_removed,
        covers_set_aside = summary.covers_set_aside,
        covers_removed = summary.covers_removed,
        transcodes_removed = summary.transcodes_removed,
        "cleanup finished"
    );
    Ok(summary)
}

/// Tidies the covers directory: covers of eras that no longer exist are set
/// aside while the era's tombstone can still restore it (the cover phase puts
/// them back then) and deleted otherwise; set-aside covers go once the
/// tombstone expired or the era has a cover again; stale temporary files go.
/// Covers are left alone unless the last import succeeded and eras exist:
/// after a failed first import (e.g. right after a database reset) every
/// cover would look orphaned. Returns (set aside, deleted).
async fn remove_orphan_covers(state: &AppState, now: i64) -> Result<(usize, usize), ApiError> {
    let (era_ids, tombstoned, last_import_ok) = db::call(&state.pool, move |conn| {
        let era_ids: HashSet<i64> = {
            let mut statement = conn.prepare("SELECT id FROM eras")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        // Tombstones are pruned by imports that change the catalog; one past
        // its retention counts as gone already.
        let tombstoned: HashSet<i64> = {
            let mut statement =
                conn.prepare("SELECT id FROM era_tombstones WHERE deleted_at >= ?1")?;
            let rows = statement.query_map([now - TOMBSTONE_RETENTION_SECS], |row| row.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        let ok = db::meta_get(conn, meta_keys::LAST_IMPORT_OK)?.as_deref() == Some("true");
        Ok((era_ids, tombstoned, ok))
    })
    .await?;
    let covers = covers_dir(&state.config);
    let entries = match list_dir(&covers).await {
        Ok(entries) => entries,
        Err(error) => {
            warn!(path = %covers.display(), %error, "could not list the covers directory");
            return Ok((0, 0));
        }
    };
    let prune = !era_ids.is_empty() && last_import_ok;
    // Eras with a cover in place (a set-aside one is superseded then).
    let covered: HashSet<i64> = entries
        .iter()
        .filter(|entry| entry.metadata.is_file() && entry.metadata.len() > 0)
        .filter_map(|entry| cover_era(&entry.name))
        .collect();
    let (mut set_aside, mut removed) = (0, 0);
    for entry in entries {
        if !entry.metadata.is_file() {
            continue;
        }
        if entry.name.ends_with(".tmp") {
            if older_than(&entry.metadata, STALE_TEMP_AGE) {
                remove_file_logged(&entry.path, "temporary cover file").await;
            }
            continue;
        }
        if !prune {
            continue;
        }
        if let Some((id, _, _)) = set_aside_cover(&entry.name) {
            let keep = if era_ids.contains(&id) {
                !covered.contains(&id)
            } else {
                tombstoned.contains(&id)
            };
            if !keep && remove_file_logged(&entry.path, "set-aside cover").await {
                removed += 1;
            }
            continue;
        }
        let Some(id) = cover_era(&entry.name).filter(|id| !era_ids.contains(id)) else {
            continue;
        };
        if !tombstoned.contains(&id) {
            if remove_file_logged(&entry.path, "cover of a removed era").await {
                removed += 1;
            }
            continue;
        }
        match quarantine(&covers, &entry.name, now, Quarantine::Retired).await {
            Ok(moved) => {
                debug!(file = %entry.name, set_aside_as = %moved, "set aside the cover of a removed era");
                set_aside += 1;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                warn!(file = %entry.name, %error, "could not set aside the cover of a removed era");
            }
        }
    }
    Ok((set_aside, removed))
}

/// The era of a cover file `<era id>.<ext>`.
fn cover_era(name: &str) -> Option<i64> {
    let (id, extension) = name.split_once('.')?;
    if !COVER_EXTENSIONS.contains(&extension) || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    id.parse().ok()
}

/// A temporary name the downloader uses in the songs directory:
/// `<name>.<16 hex digits>.tmp`, `.ytdl-<16 hex digits>`, or the original
/// downloader's `<hash>.<ext>.tmp`.
fn is_temporary_song_name(name: &str) -> bool {
    let hex16 = |text: &str| text.len() == 16 && text.bytes().all(|byte| byte.is_ascii_hexdigit());
    if let Some(suffix) = name.strip_prefix(".ytdl-") {
        return hex16(suffix);
    }
    let Some(stem) = name.strip_suffix(".tmp") else {
        return false;
    };
    stem.rsplit_once('.')
        .is_some_and(|(_, random)| hex16(random))
        || is_downloader_name(stem)
}

/// Removes temporary files and directories an interrupted previous run left
/// behind. Runs at startup, before any download or transcode can start. The
/// covers and transcode directories belong to the API, so every `*.tmp` there
/// goes; in the songs directory only the downloader's own names do.
pub async fn remove_stale_temp_files(config: &Config) -> usize {
    let mut removed = 0;
    let dirs = [
        (config.songs_path.clone(), true),
        (downloader::covers_dir(config), false),
        (transcodes_dir(config), false),
    ];
    for (dir, shared) in dirs {
        let Ok(entries) = list_dir(&dir).await else {
            continue;
        };
        for entry in entries {
            let temporary = if shared {
                is_temporary_song_name(&entry.name)
            } else {
                entry.name.ends_with(".tmp") && entry.metadata.is_file()
            };
            if !temporary {
                continue;
            }
            let result = if entry.metadata.is_dir() {
                tokio::fs::remove_dir_all(&entry.path).await
            } else {
                tokio::fs::remove_file(&entry.path).await
            };
            match result {
                Ok(()) => removed += 1,
                Err(error) => {
                    warn!(path = %entry.path.display(), %error, "could not remove a stale temporary file");
                }
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SharedState;

    const DAY: i64 = 24 * 60 * 60;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("yt-cleanup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("songs")).unwrap();
        std::fs::create_dir_all(root.join("covers")).unwrap();
        root
    }

    fn state_at(root: &Path) -> SharedState {
        let config = Config::for_tests(root, &root.join("songs"));
        let pool = db::create_pool(&root.join("db.sqlite3")).unwrap();
        db::run_migrations(&pool, &config).unwrap();
        crate::state::AppState::new(config, pool)
    }

    /// `files` rows: `seen` rows seen by the last import, plus the given
    /// (url, filename, status) rows unseen for 8 days.
    fn rows(state: &AppState, now: i64, seen: usize, unseen: &[(&str, Option<&str>, &str)]) {
        let conn = state.pool.get().unwrap();
        db::meta_set(&conn, meta_keys::LAST_IMPORT_AT, Some(&now.to_string())).unwrap();
        db::meta_set(&conn, meta_keys::LAST_IMPORT_OK, Some("true")).unwrap();
        for index in 0..seen {
            conn.execute(
                "INSERT INTO files (url, status, last_seen_at) VALUES (?1, 'pending', ?2)",
                rusqlite::params![format!("https://imgur.gg/f/seen{index}"), now],
            )
            .unwrap();
        }
        for (url, filename, status) in unseen {
            conn.execute(
                "INSERT INTO files (url, filename, status, last_seen_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![url, filename, status, now - 8 * DAY],
            )
            .unwrap();
        }
    }

    fn file_rows(state: &AppState) -> i64 {
        let conn = state.pool.get().unwrap();
        conn.query_row("SELECT count(*) FROM files", [], |row| row.get(0))
            .unwrap()
    }

    fn age(path: &Path, seconds: u64) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(seconds))
            .unwrap();
    }

    #[tokio::test]
    async fn cleanup_keeps_downloaded_rows_and_quarantines_instead_of_deleting() {
        let root = scratch("retire");
        let state = state_at(&root);
        let songs = root.join("songs");
        let now = db::unix_now();
        let gone = "https://imgur.gg/f/gone";
        let kept = "https://imgur.gg/f/kept";
        let kept_name = format!("{}.mp3", natural_stem(kept));
        rows(
            &state,
            now,
            30,
            &[
                (gone, None, "pending"),
                (kept, Some(&kept_name), "downloaded"),
            ],
        );
        // Media under the removed row's natural name, the kept row's media,
        // an old unreferenced download and a young one, and quarantined files
        // of various ages.
        let gone_name = format!("{}.mp3", natural_stem(gone));
        let orphan = format!("{}.ogg", "a".repeat(64));
        let young = format!("{}.ogg", "b".repeat(64));
        for name in [&gone_name, &kept_name, &orphan, &young] {
            std::fs::write(songs.join(name), b"audio").unwrap();
        }
        age(&songs.join(&orphan), 2 * DAY as u64);
        let expired_retired = format!("x.mp3.{}.removed", now - 15 * DAY);
        let fresh_retired = format!("y.mp3.{}.removed", now - 8 * DAY);
        let expired_rejected = format!("z.mp3.{}.invalid", now - 8 * DAY);
        for name in [&expired_retired, &fresh_retired, &expired_rejected] {
            std::fs::write(songs.join(name), b"old").unwrap();
        }

        let summary = cleanup(&state).await.unwrap();
        assert_eq!((summary.rows_removed, summary.rows_kept), (1, 1));
        assert!(!summary.sweep_refused);
        assert_eq!(summary.media_quarantined, 2);
        assert_eq!(summary.quarantine_removed, 2);
        assert_eq!(file_rows(&state), 31);
        assert!(songs.join(&kept_name).exists());
        assert!(songs.join(&young).exists());
        assert!(songs.join(&fresh_retired).exists());
        assert!(!songs.join(&expired_retired).exists());
        assert!(!songs.join(&expired_rejected).exists());
        for name in [&gone_name, &orphan] {
            assert!(!songs.join(name).exists(), "{name} still in place");
            let retired = std::fs::read_dir(&songs)
                .unwrap()
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .find(|entry| entry.starts_with(name.as_str()) && entry.ends_with(".removed"));
            assert!(retired.is_some(), "{name} was not quarantined");
        }
        {
            let conn = state.pool.get().unwrap();
            assert_eq!(
                db::meta_get_i64(&conn, meta_keys::CLEANUP_KEPT_DOWNLOADED).unwrap(),
                Some(1)
            );
        }
        // The next run keeps the row again and deletes nothing more.
        let again = cleanup(&state).await.unwrap();
        assert_eq!((again.rows_removed, again.rows_kept), (0, 1));
        assert!(songs.join(&kept_name).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_cleanup_that_would_remove_too_many_rows_removes_none() {
        let root = scratch("refuse");
        let state = state_at(&root);
        let now = db::unix_now();
        // 3 of 20 rows unseen: 15% > 10%.
        rows(
            &state,
            now,
            17,
            &[
                ("https://imgur.gg/f/a", None, "pending"),
                ("https://imgur.gg/f/b", None, "failed"),
                ("https://imgur.gg/f/c", None, "pending"),
            ],
        );
        let summary = cleanup(&state).await.unwrap();
        assert!(summary.sweep_refused);
        assert_eq!((summary.rows_removed, summary.rows_kept), (0, 0));
        assert_eq!(file_rows(&state), 20);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn nothing_is_unseen_before_the_first_import() {
        let root = scratch("fresh");
        let state = state_at(&root);
        {
            let conn = state.pool.get().unwrap();
            conn.execute(
                "INSERT INTO files (url, status, last_seen_at) VALUES ('https://imgur.gg/f/a', 'pending', 1)",
                [],
            )
            .unwrap();
        }
        let summary = cleanup(&state).await.unwrap();
        assert_eq!(summary.rows_removed, 0);
        assert_eq!(file_rows(&state), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn covers_are_only_pruned_after_a_successful_import() {
        let root = scratch("covers");
        let state = state_at(&root);
        let covers = root.join("covers");
        let now = db::unix_now();
        std::fs::write(covers.join("99.avif"), b"cover").unwrap();
        // No eras yet (e.g. the first import after a reset failed).
        assert_eq!(remove_orphan_covers(&state, now).await.unwrap(), (0, 0));
        {
            let conn = state.pool.get().unwrap();
            conn.execute("INSERT INTO eras (id, key, name) VALUES (1, 'a', 'A')", [])
                .unwrap();
            db::meta_set(&conn, meta_keys::LAST_IMPORT_OK, Some("false")).unwrap();
        }
        assert_eq!(remove_orphan_covers(&state, now).await.unwrap(), (0, 0));
        assert!(covers.join("99.avif").exists());
        {
            let conn = state.pool.get().unwrap();
            db::meta_set(&conn, meta_keys::LAST_IMPORT_OK, Some("true")).unwrap();
        }
        std::fs::write(covers.join("1.avif"), b"cover").unwrap();
        assert_eq!(remove_orphan_covers(&state, now).await.unwrap(), (0, 1));
        assert!(!covers.join("99.avif").exists());
        assert!(covers.join("1.avif").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// A removed era's covers wait (out of the cover route's reach) while
    /// its tombstone can bring the era back, and go when it expires.
    #[tokio::test]
    async fn covers_of_tombstoned_eras_are_set_aside_until_the_tombstone_expires() {
        let root = scratch("covers-tombstone");
        let state = state_at(&root);
        let covers = root.join("covers");
        let now = db::unix_now();
        {
            let conn = state.pool.get().unwrap();
            conn.execute_batch(
                "INSERT INTO eras (id, key, name) VALUES (1, 'a', 'A');
                 INSERT INTO meta (key, value) VALUES ('last_import_ok', 'true')
                   ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
            )
            .unwrap();
            // Era 7 was removed a day ago; era 8 long ago (its tombstone is
            // past the retention but not pruned yet); era 9 had no key.
            conn.execute(
                "INSERT INTO era_tombstones (id, key, deleted_at) VALUES (7, 'g', ?1), (8, 'h', ?2)",
                [now - DAY, now - TOMBSTONE_RETENTION_SECS - DAY],
            )
            .unwrap();
        }
        for name in ["1.avif", "7.avif", "7.jpg", "8.avif", "9.png"] {
            std::fs::write(covers.join(name), name).unwrap();
        }
        assert_eq!(remove_orphan_covers(&state, now).await.unwrap(), (2, 2));
        let set_aside = [
            format!("7.avif.{now}.removed"),
            format!("7.jpg.{now}.removed"),
        ];
        let mut expected = vec!["1.avif".to_string()];
        expected.extend(set_aside.iter().cloned());
        assert_eq!(names(&covers), expected);
        assert_eq!(set_aside_cover(&set_aside[0]), Some((7, "avif", now)));
        // Nothing changes while the tombstone lives.
        assert_eq!(
            remove_orphan_covers(&state, now + DAY).await.unwrap(),
            (0, 0)
        );
        assert_eq!(names(&covers), expected);
        // Once it expires, the set-aside covers go too.
        let later = now + TOMBSTONE_RETENTION_SECS;
        assert_eq!(remove_orphan_covers(&state, later).await.unwrap(), (0, 2));
        assert_eq!(names(&covers), ["1.avif"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A restored era gets its set-aside cover back in the cover phase, before
    /// anything would be downloaded; a set-aside cover superseded by a new one
    /// is deleted by the cleanup.
    #[tokio::test]
    async fn restored_eras_get_their_set_aside_covers_back() {
        let root = scratch("covers-restore");
        // Offline: the cover phase must not render the sheet.
        let mut config = Config::for_tests(&root, &root.join("songs"));
        config.downloads_enabled = false;
        let pool = db::create_pool(&root.join("db.sqlite3")).unwrap();
        db::run_migrations(&pool, &config).unwrap();
        let state = crate::state::AppState::new(config, pool);
        let covers = root.join("covers");
        let now = db::unix_now();
        {
            let conn = state.pool.get().unwrap();
            conn.execute_batch(
                "INSERT INTO eras (id, key, name, is_main, dominant_color) VALUES \
                   (7, 'g', 'G', 1, '666666'), (8, 'h', 'H', 1, '666666');
                 INSERT INTO meta (key, value) VALUES ('last_import_ok', 'true')
                   ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
            )
            .unwrap();
        }
        // Era 7 came back: an older and a newer set-aside cover. Era 8 came
        // back too, but already has a new cover.
        for name in [
            format!("7.avif.{}.removed", now - 10),
            format!("7.avif.{now}.removed"),
            format!("7.jpg.{now}.removed"),
            format!("8.avif.{now}.removed"),
            "8.avif".to_string(),
        ] {
            std::fs::write(covers.join(&name), &name).unwrap();
        }
        let summary = downloader::sync_covers(
            &state,
            &CancellationToken::new(),
            crate::media::ToolSet::default(),
        )
        .await
        .unwrap();
        assert_eq!(summary.restored, 1);
        assert_eq!(
            std::fs::read_to_string(covers.join("7.avif")).unwrap(),
            format!("7.avif.{now}.removed"),
            "the latest set-aside cover"
        );
        assert!(covers.join("7.jpg").exists());
        let version: Option<String> = state
            .pool
            .get()
            .unwrap()
            .query_row("SELECT cover_version FROM eras WHERE id = 7", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            version,
            Some(crate::cover_version::cover_version_of(
                format!("7.avif.{now}.removed").as_bytes()
            ))
        );
        // The leftovers are superseded by covers in place.
        assert_eq!(remove_orphan_covers(&state, now).await.unwrap(), (0, 2));
        assert_eq!(names(&covers), ["7.avif", "7.jpg", "8.avif"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recognises_downloader_names() {
        assert!(is_downloader_name("0598cc0e31be5d31d4a1c2501c659989.wav"));
        assert!(is_downloader_name(&format!("{}.ogg", "a".repeat(64))));
        assert!(!is_downloader_name("my song.mp3"));
        assert!(!is_downloader_name("cafe.mp3"));
        assert!(!is_downloader_name(
            "0598cc0e31be5d31d4a1c2501c659989.wav.1790000000.invalid"
        ));
    }

    #[test]
    fn recognises_temporary_song_names() {
        assert!(is_temporary_song_name("abc.mp3.0123456789abcdef.tmp"));
        assert!(is_temporary_song_name(
            "0598cc0e31be5d31d4a1c2501c659989.wav.tmp"
        ));
        assert!(is_temporary_song_name(".ytdl-0123456789abcdef"));
        assert!(!is_temporary_song_name(".ytdl-foo"));
        assert!(!is_temporary_song_name("notes.tmp"));
        assert!(!is_temporary_song_name("song.mp3"));
    }

    #[tokio::test]
    async fn stale_temporary_files_are_removed_at_startup() {
        let root = std::env::temp_dir().join(format!("yt-stale-temp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let songs = root.join("songs");
        let covers = root.join("covers");
        let transcodes = root.join("transcodes");
        for dir in [&songs, &covers, &transcodes] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::create_dir_all(songs.join(".ytdl-0123456789abcdef")).unwrap();
        for (dir, name) in [
            (&songs, "abc.mp3.0123456789abcdef.tmp"),
            (&songs, "keep.tmp"),
            (&songs, "0598cc0e31be5d31d4a1c2501c659989.wav"),
            (&covers, "12.0123456789abcdef.avif.tmp"),
            (&covers, "12.avif"),
            (&transcodes, "k-1-2-128.0123456789abcdef.tmp"),
            (&transcodes, "k-1-2-128.ogg"),
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let config = Config::for_tests(&root, &songs);
        assert_eq!(remove_stale_temp_files(&config).await, 4);
        assert!(songs.join("keep.tmp").exists());
        assert!(songs.join("0598cc0e31be5d31d4a1c2501c659989.wav").exists());
        assert!(covers.join("12.avif").exists());
        assert!(transcodes.join("k-1-2-128.ogg").exists());
        assert!(!songs.join(".ytdl-0123456789abcdef").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
