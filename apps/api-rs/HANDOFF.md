# API handoff notes

> **Historical document.** This file began as the handoff of the TypeScript → Rust port of the API (September 2026).
> The port is finished, the TypeScript API is gone, and the API has since moved to contract v2. For running,
> configuring and testing it see [`README.md`](../../README.md), [`AGENTS.md`](../../AGENTS.md) and
> [`API.md`](../../API.md). The module map and schema notes below are kept current; the port notes at the end are
> history.

## Module map

| File | Role |
|---|---|
| `src/main.rs` | Binary entry: loads `.env`, starts `tracing` logging (stdout, `RUST_LOG`), raises the soft open-file limit to the hard limit, then config → pool → migrations → state → stale temp-file cleanup → tool detection → playable scan → **bind** → spawn the sync → serve. Startup errors exit 1. |
| `src/config.rs` | Every environment variable: strict parsing, ranges, defaults (`MAX_CONNECTIONS` from the open-file limit); `.env` loading; workspace-root resolution; storage directories. |
| `src/http.rs` | Middleware (request span, `Accept-Encoding` normalisation, 30 s non-media timeout, CORS, security headers, `Vary`, 64 KiB body limit, compression, weak JSON ETag/304) and the hyper HTTP/1 server loop (TCP_NODELAY, `MAX_CONNECTIONS` with eviction of an idle or non-reading connection (`eviction_victim`), rate-limited limit warnings, header/idle/not-reading/stall watchdog, graceful drain ≤ 10 s). |
| `src/sync.rs` | The background sync: schedule (`SYNC_ON_START`, `SYNC_INTERVAL_MINUTES`), one run at a time, panic supervision, phases. |
| `src/routes/mod.rs` | Route table, 404/405 fallbacks, `EraId`/`SongId`/`Params` extractors, JSON helpers. |
| `src/routes/eras.rs` | `/eras`, `/eras/:id`, `/eras/:id/songs`. |
| `src/routes/songs.rs` | Song payload (`DownloadPolicy`: `unsupported` for YouTube while `YOUTUBE_DOWNLOAD=false`), filters, sorts, plain list, search mode (`SearchInput`: tokens + category markers, blank `q`), era-scoped search over `song_search_text`, search slots, `/songs/:id`. |
| `src/routes/status.rs` | `/health`, `/status`. |
| `src/routes/media.rs` | `/songs/:id/stream`, `/download`, `/duration`, `/eras/:id/cover`; transcode jobs and cache. A missing stored file is marked unplayable at once and queued for re-verification. |
| `src/request.rs` | `Query` (form decoding; last occurrence wins, values trimmed, blank = absent — for every route) and parameter validation. |
| `src/error.rs` | `ApiError`: text/plain + `no-store`; busy (`Retry-After`); unexpected → logged 500 JSON with an `ErrorDetail` extension. |
| `src/rank.rs` | Relevance ranking; `CatalogIndex`/`RankCache` (folded catalog, rebuilt per catalog fingerprint). |
| `src/search_text.rs` | `fold()` (TS twin: `apps/web/src/utils/search.ts`), `search_text` / `song_search_text` builders, `sort_title`, `category_rank`, `SONG_CATEGORIES`. |
| `src/playable.rs` | Playable-file set (rebuilt every sync), size/mtime cache, 15 s negative cache, `mark_missing`, safe file names. |
| `src/serve.rs` | Validators (ETag, `If-None-Match`, `If-Modified-Since` — future dates ignored, `If-Range`), single byte ranges (ignored by `HEAD`), file and growing-file bodies (256 KiB chunks). |
| `src/importer.rs` | Sheet fetch and parse (era rows by shape), sanity guards (songs, links, missing era rows; `IMPORT_FORCE`), stable ids (era renames, cross-era moves, tombstones), fingerprint, transactional apply. |
| `src/db.rs` | r2d2 pool (WAL, PRAGMAs), inline DDL, additive migrations and backfills, v1 → v2 migration, `meta` helpers. |
| `src/downloader.rs` | HTTP clients (proxies ignored); song downloads (pillows.su, imgur.gg, yt-dlp), per-sync cap and time budget, backoff for attempts vs. network trouble, per-service pause, 30-day retry of failed rows, reuse of files on disk; covers (fresh Google render, host allowlist, AVIF + JPEG encoding). |
| `src/backfill.rs` | Duration backfill (verifies files), re-verification queue, cleanup (unseen rows, 10% cap, kept downloaded rows, quarantine), startup temp-file cleanup. |
| `src/media.rs` | Tool detection (re-checked at most once a minute while one is missing), ffprobe verdicts (definitive vs. unknown), quarantine (`.invalid` 7 days, `.removed` 14 days), re-verification queue, `run_grouped` (yt-dlp in its own process group, killed as a group). |
| `src/public_net.rs` | SSRF guard for the media client: resolver dropping non-public addresses, redirect check. |
| `src/state.rs` | `AppState`: config, pool, caches, tools, probes, queues, HTTP clients, semaphores. |
| `src/catalogs.rs` | The `unreleased` catalog: sheet gid, Google doc id, sheet URLs. |
| `src/cover_version.rs` | Cover version = 12 hex digits of the SHA-256 of the cover bytes. |
| `src/dominant_color.rs` | Average colour of a cover's right-hand 20% strip (ffmpeg). |
| `src/repair.rs` | Merges duplicate era rows (run by the migration before the unique era key exists). |
| `src/text.rs` | JavaScript-compatible whitespace helpers, sheet-cell cleanup. |
| `tests/*.test.mjs` | Black-box contract suite, split into `eras`, `songs`, `media` + `helpers.mjs`. |
| `tests/fixtures/sheet.html` | Trimmed real sheet (4 eras, 30 songs: sub-eras, month dates, `~` lengths, multi-link cells, notes links, a duplicate pair, a zero-width character, an unknown era cell). |
| `tests/covers_live.rs`, `tests/downloader_live.rs` | Ignored network smoke tests (below). |

## Schema v2

New databases get the full shape from `CREATE TABLE IF NOT EXISTS`; existing ones get missing columns through
idempotent `ALTER TABLE … ADD COLUMN` and a one-time v1 → v2 backfill (`db::migrate`), so the API also works on an old
database before its first v2 import.

- **`eras`**: `id`, `key` (normalized name, unique index), `position` (sheet order), `name` (first line of the cell),
  `subtitle`, `notes`, `description` (`''` when empty), `image_url` (artwork URL of the last import),
  `dominant_color` (6 hex, `666666` by default), `cover_version` (hash of the on-disk cover; NULL = no cover),
  `cover_source` (`sha256:<hex>` of the downloaded source image), `cover_attempts`, `cover_next_attempt_at`,
  `cover_last_error` (per-era backoff), `is_main` (always 1 after a v2 import).
- **`songs`**: `id`, `era`, `catalog_id`, `name` (multi-line), `notes` (NULL when empty), `file_date`, `leak_date`
  (NULL when missing), `available_length`, `track_length`, `quality`, `url` (= `links[0]`), plus `position` (global
  sheet order), `era_position` (1-based within the era), `title`, `sub_era`, `links` and `notes_links` (JSON arrays),
  `file_date_precision`/`leak_date_precision` (`day`/`month`/`year`/NULL), `track_length_approx` (0/1),
  `search_text` (folded searchable fields incl. the era's name and subtitle: global search), `song_search_text` (the
  same without the era's name and subtitle: era-scoped search; backfilled at boot for rows that lack it),
  `sort_title` (natural-sort key), `category_rank` (0 best-of … 4 unmarked, 5 worst-of, 6 ai), `song_key` (content
  key for id matching).
- **`files`** (one row per downloadable primary link): `url` (PK; rows with a NULL url are deleted at boot),
  `status` (`pending`/`downloaded`/`failed`), `filename` (NULL until downloaded), `duration`, `attempts` (counted
  failures), `transient_failures` (network errors in a row; they don't count as attempts), `next_attempt_at` (backoff;
  for `failed` rows the 30-day retry, also set at boot for failed rows that have none), `last_error`, `last_seen_at`
  (last import whose Link(s) cells still contain the link, primary or not), and the legacy `downloaded` flag, kept in
  sync for old binaries.
- **`song_tombstones`** (`id`, `song_key`, `era_key`, `name`, `url`, `deleted_at`) and **`era_tombstones`** (`id`,
  `key`, `deleted_at`): ids of songs and eras an import removed, kept 90 days (pruned by imports that change the
  catalog) so rows restored upstream get their ids back.
- **`meta (key, value)`**: `schema_version` (`2`); `next_song_id`, `next_era_id` (id high-water marks: ids are never
  reused); `last_import_at` (Unix seconds of the last *successful* import, "unchanged" ones included);
  `last_import_ok` (`true`/`false`, most recent attempt); `last_import_error` (NULL after a success; `/status` only
  says "Catalog update failed"); `last_sheet_sha256` (fingerprint of the parsed catalog plus the parser version — the
  change detection, and the version of the ranking index); `last_cover_refresh_at` (last daily cover refresh);
  `cleanup_kept_downloaded` (unseen rows the last cleanup kept for their media; an increase is logged as an error).
- **Indexes**: `songs (catalog_id, position)`, `songs (catalog_id, era, position)`, `songs (catalog_id, era, id)`,
  `songs (catalog_id, id)`, `songs_search_scan (catalog_id, search_text, url)` (covers the global search scan),
  `eras (is_main)`, unique `eras (key)`. v1's single-column song indexes are dropped.
- **v1 → v2 backfill** (one transaction, only while `schema_version < 2`): merges duplicate eras; sets era keys,
  positions and `cover_version` (from `covers/<id>.avif`); songs get `position = id`, per-era `era_position`, `title`,
  the derived search/sort columns, `links = [url]`, dates `0 → NULL` (precision `day` for the rest); `files.status`
  becomes `downloaded` only when `downloaded = 1` and the file is on disk (else `pending` with `filename` NULL); the
  high-water marks start at max id + 1. The first v2 import then matches the existing rows, so ids survive.

## Live checks (network, ignored by default)

```bash
cd apps/api-rs
# Parse a saved htmlview/sheet page and print the catalog stats (no network).
IMPORT_SHEET=/path/to/sheet.html cargo test --lib live_sheet_snapshot -- --ignored --nocapture
# Downloads one real pillows.su file into /tmp/yt-dl-live and records it.
cargo test --test downloader_live -- --ignored --nocapture
# Renders the sheet on Google Sheets, downloads one artwork image, encodes it (ffmpeg if present) into /tmp/yt-cover-live.
cargo test --test covers_live -- --ignored --nocapture
# A real yt-dlp fetching from a slow local server: its whole process tree must be gone after the timeout.
YT_DLP_SLOW_URL=http://127.0.0.1:<port>/song.wav cargo test --lib yt_dlp_process_tree -- --ignored --nocapture
```

## Logs worth knowing

One `info` line per phase summarises a sync. Field names that tooling may grep for:

- import (`catalog imported`): `eras_added`, `eras_removed`, `eras_renamed`, `eras_restored`, `songs_added`,
  `songs_removed`, `kept_by_content`, `kept_by_link`, `kept_by_move`, `kept_by_name`, `restored`, `download_links`,
  `files_added`, `files`; an unchanged catalog logs `catalog unchanged since the last import; kept as is`; a refused
  import is an error (`sync phase failed`, `phase=import`) naming the guard and `IMPORT_FORCE`;
- downloads (`song downloads finished`): `reused`, `attempted`, `downloaded`, `failed`, `gave_up`, `transient`,
  `disk_full`, `paused_hosts`, `disabled_by_config`, `missing_tool`, `deferred`, `out_of_time`;
- covers (`covers synced`): `restored` (set-aside covers put back for restored eras), `needed`, `fetched`, `written`,
  `unchanged`, `failed`;
- cleanup (`cleanup finished`): `rows_removed`, `rows_kept`, `sweep_refused`, `media_quarantined` (was
  `media_removed`), `quarantine_removed`, `covers_set_aside`, `covers_removed`, `transcodes_removed`;
- connection limit (warnings, at most one per kind every 10 s): `refused` or `closed` (count since the last line),
  `max_connections`, `last_peer`;
- startup: `raised the open-file limit` (`from`, `to`) and `connection limit` (`open_file_limit`, `max_connections`).

## History: the port (September 2026)

The port reimplemented the Hono/Node API (`apps/api`, since deleted) with byte-for-byte parity, gated by the
black-box suite then called `tests/api.test.mjs` (45/45 against the Rust binary, headers and bodies diffed against
the Node server). Plan: [`RUST_PORT_PLAN.md`](../../RUST_PORT_PLAN.md). Decisions from that time that still explain
today's code:

- CORS is hand-rolled rather than `tower-http`'s `CorsLayer` (which lower-cased the exposed header value); the security
  headers reproduce Hono's `secureHeaders` defaults with `Cross-Origin-Resource-Policy: cross-origin` (HSTS has been
  dropped since).
- `routes::js_float` serializes whole-number floats without serde_json's `.0`, like `JSON.stringify`.
- `.env` is loaded from the repo root without overriding real environment variables, mirroring Node's `--env-file`.
- The importer upgrades Google's thumbnail artwork URLs (`=w102-h104`) to `=s512`. It first recognized era rows only
  by the artwork `<img>` in their penultimate cell (era rows gained a leading stats column); that rule still counts,
  but era rows are now recognized by their shape, so artwork is optional.
- `text.rs` reproduces JavaScript's `\s` and `trim` semantics for sheet text.

Superseded since (contract v2 and the review and fix-up rounds), so don't rely on older notes about them: the single
`api.test.mjs` file; the extra category catalogs, `/categories` and `/album-copies` (only `unreleased` is imported);
SHA-1 cover versions of the image URL (now a hash of the cover bytes); the ported `rankSongSearch` ranking, `LIKE`
search and the 1000-row search window (now `fold()` tokens over all matches); `416` for malformed ranges and immutable
media caching (now ignored ranges and `public, no-cache` + ETag); `song-<id>` download names; request handlers that
deleted invalid files (now only the background sync quarantines, on a definitive ffprobe verdict); the blocking import
at boot (now a background sync); the v1 fixture-seeding SQL; `eprintln!` logging (now `tracing`); strong JSON ETags
(now weak, one per body for every encoding); `/duration` cached as immutable (now `no-cache`); `era=%20` as a `400`
and first-occurrence parameters on the media routes (now one rule: last wins, trimmed, blank = absent); a `q` without
tokens matching every song (now blank, or a category filter when it has markers); era-scoped search matching the era
name; unlimited downloads per sync by default (now 200 within 20 minutes); a single import guard (now songs, links and
missing era rows); cleanup deleting the media of unseen rows (now quarantine, and downloaded rows are kept);
`failed` rows never retried (now after 30 days); `MAX_CONNECTIONS` refusing newcomers (now the longest-idle
connection is evicted).
