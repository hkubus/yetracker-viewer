# Rust port — handoff notes

Branch: `rust-port` (created off `main`). Plan: `RUST_PORT_PLAN.md` (repo root).
Acceptance gate: `apps/api/tests/api.test.mjs` (black-box, adaptive).

**Status: Phases 1–3 are implemented.** The crate compiles, `cargo test` passes,
the full acceptance suite passes 45/45 against the Rust binary (including media),
and header/body parity was diffed byte-for-byte against the running Node original.
The catalog importer, cover/song downloader and duration backfill are ported and
validated against live services.

## Verified right now

```bash
cd apps/api-rs && cargo test              # 23 lib + 2 ignored live smoke tests
cd apps/api-rs && cargo build

# acceptance (Rust) — 45/45
cp -r storage /tmp/yt-test-rs        # seed a playable fixture, see below
# Pin SONGS_DIR too: the binary now loads .env, whose SONGS_DIR is absolute.
SYNC_ON_START=false STORAGE_DIR=/tmp/yt-test-rs SONGS_DIR=/tmp/yt-test-rs/songs \
  API_PORT=3200 ./target/release/yetracker-api &
API_BASE_URL=http://127.0.0.1:3200 node --test ../api/tests/
```

Seed a playable fixture (`storage/songs/` is empty otherwise, so media success
tests skip). Use `cp -p` so Node and Rust copies share an mtime/ETag:

```bash
ffmpeg -loglevel error -y -f lavfi -i "sine=frequency=440:duration=2" -b:a 32k /tmp/fixture.mp3
cp -p /tmp/fixture.mp3 /tmp/yt-test-rs/songs/yt-port-fixture.mp3
sqlite3 /tmp/yt-test-rs/db.sqlite3 <<'SQL'
INSERT INTO files (url, downloaded, filename, duration)
  VALUES ('https://example.invalid/yt-port-fixture', 1, 'yt-port-fixture.mp3', NULL);
INSERT INTO songs (id, era, catalog_id, name, notes, file_date, leak_date,
                   available_length, track_length, quality, url)
  SELECT (SELECT max(id)+1 FROM songs), 1, 'unreleased', 'ZZ Fixture Tone', '',
         0, 0, 'Full', 2, 'CD Quality', 'https://example.invalid/yt-port-fixture';
SQL
```

## Phase 3 live checks (network, ignored by default)

```bash
# Importer parity: same saved sheet through Rust and importer.ts -> 9480 songs,
# 46 eras, and a byte-identical 6147-URL set.
IMPORT_SHEET=/tmp/sheet.html cargo test --lib live_sheet_snapshot -- --ignored --nocapture

# Downloader: fetches/marks/probes a real pillows.su file.
cargo test --test downloader_live -- --ignored --nocapture

# Covers: fetches an image, encodes AVIF with ffmpeg, samples the colour.
cargo test --test covers_live -- --ignored --nocapture
```

A full `SYNC_ON_START=true` boot was also run against a throwaway dir: all 11
catalogs imported (e.g. Unreleased 9480 songs, files table denormalised).

## Layout

| File | Mirrors | Notes |
|---|---|---|
| `src/config.rs` | `config.ts` | same env names/defaults/messages, workspace-root walk, `mkdir -p` |
| `src/catalogs.rs` | `catalogs.ts` | table copied verbatim |
| `src/db.rs` | `db/client.ts` + DDL in `index.ts` | r2d2 pool, PRAGMAs per connection, verbatim DDL + `table_info` migrations |
| `src/error.rs` | Hono error semantics | `Http` → `text/plain;charset=UTF-8`; `Unexpected` → `500 {"error":"Internal server error"}` + log |
| `src/request.rs` | `util/request.ts` | `positiveInteger`, `paginationValue`, `escapeLikePattern` |
| `src/text.rs` | JS string semantics | `\s` set incl. U+FEFF, `trim`, collapse, lower-case, UTF-16 length |
| `src/playable.rs` | `util/playableFiles.ts` | atomic set swap, 32-way scan, `storedSongPath`, `isSafeFilename` |
| `src/cover_version.rs` | `util/coverVersion.ts` | sha1[..12], LRU 1000 |
| `src/rank.rs` | `util/rankSongSearch.ts` | full port incl. max-heap and NaN comparator semantics |
| `src/serve.rs` | `util/serveFile.ts` + range blocks | range parsing (safe-integer bounds), abort-safe streaming |
| `src/media.rs` | `util/getDuration.ts`, `util/invalidFiles.ts` | shared duration futures, probe, `deleteInvalidFile` |
| `src/repair.rs` | `util/repairEras.ts` | runs at startup |
| `src/dominant_color.rs` | `util/getDominantColor.ts` | ffmpeg strip sample, mtime LRU + in-flight dedup |
| `src/backfill.rs` | `util/backfillDurations.ts` | concurrency from `BACKFILL_CONCURRENCY` |
| `src/importer.rs` | `scraper/importer.ts` | reqwest + scraper, transactional replace |
| `src/downloader.rs` | `scraper/downloader.ts` | reqwest covers (ffmpeg AVIF) + pillows/yt-dlp songs |
| `src/state.rs` | — | `AppState`: pool, caches, transcode semaphore, `DominantColors` |
| `src/routes/**` | `routes/**` | full route table (eras/songs/categories/album-copies) |
| `src/main.rs` | `index.ts` | boot, CORS + secure-headers + compression, background sync, shutdown |

## Config / .env

`Config::load` mirrors the Node scripts' `--env-file ../../.env`: it loads the
repo-root `.env` (found by walking up from the executable to `package.json`) with
`dotenvy`, **without** overriding variables already present in the environment —
the same precedence as Node's `--env-file`. A `.env` containing
`STORAGE_DIR=./storage` and an absolute `SONGS_DIR` therefore resolves to the
exact same `storage/db.sqlite3` and media directory the Node server uses. Running
`SYNC_ON_START=true` (the default) performs the import + background chain too, so
the binary is a drop-in for `node src/index.ts`.

## Middleware parity notes

* Hand-rolled CORS: always `Access-Control-Expose-Headers: X-Total-Count` and
  `Vary: Origin`; echoes an allowed `Origin` (or `*` if configured); `OPTIONS`
  short-circuits to `204` with allow-methods and echoes
  `Access-Control-Request-Headers`. Do **not** use `tower-http`'s `CorsLayer`
  (it lower-cases the exposed header value, which the test matches exactly).
* Secure headers reproduce hono `secureHeaders` defaults; CORP is `cross-origin`.
* Compression: `tower-http::CompressionLayer` with a predicate limiting it to
  `application/json` / `text/*`. Hono has no effective size threshold because its
  responses carry no `Content-Length` at middleware time, so neither do we.
* 416 responses use an unknown-length empty body so hyper emits
  `Transfer-Encoding: chunked` (no `Content-Length`), matching Hono.
* `js_float` serialises whole-float durations without serde_json's `.0` suffix,
  matching `JSON.stringify`.

## Known deviations (no test coverage)

* `importer.rs` date parsing approximates `Date.parse` with a set of common
  formats (date-only forms treated as UTC).
* Like the original (and because the current sheet gained a stats column), the
  primary catalog's 5-cell era-image branch no longer matches the 6-cell era
  rows, so fresh imports leave `eras.image_url` empty. This is parity, not a
  regression — do not "fix" only the Rust side without changing `importer.ts`.
* `rank.rs` uses an approximated collator (case-insensitive + numeric, no accent
  folding) for the final title tie-break.
* `download_name` truncates by Unicode scalar values rather than UTF-16 units.
* Background sync runs the same phases in order; log lines are close but not
  identical to Node's (no per-50 download progress line).
