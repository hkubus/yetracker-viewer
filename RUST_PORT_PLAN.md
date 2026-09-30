# Rust Port Plan — `apps/api` (Hono → Rust)

> **Historical document.** This is the plan the TypeScript → Rust port followed
> in September 2026. The port is done and `apps/api` (the `apps/api/src` paths
> below) no longer exists. The API has since moved to contract v2, so most of
> the behavior described here — v1 payloads, the category catalogs, range and
> caching rules, query-parameter handling, SHA-1 cover versions, the ranking
> port, deleting files from request handlers, `eprintln!` logging, the settings
> and defaults of §2 — is out of date. Current docs: [`README.md`](README.md),
> [`AGENTS.md`](AGENTS.md), [`API.md`](API.md), every setting in
> [`.env.example`](.env.example), and the module map and schema in
> [`apps/api-rs/HANDOFF.md`](apps/api-rs/HANDOFF.md).
> The acceptance suite `api.test.mjs` named below has been split into
> `apps/api-rs/tests/{eras,songs,media}.test.mjs`.

Goal: reimplement `apps/api/src` in Rust with **byte-for-byte behavioral parity**, verified by the existing black-box contract tests (then `apps/api-rs/tests/api.test.mjs`). Web app (`apps/web`) is untouched.

## 0. Source inventory (read these first)

```
API.md                                  # human contract (slightly stale on error format, see §3)
apps/api/src/index.ts                   # global middleware, DDL, startup sequence, shutdown
apps/api/src/config.ts                  # env parsing, storage dirs
apps/api/src/catalogs.ts                # PRIMARY_CATALOG_ID + CATALOGS table
apps/api/src/db/schema.ts               # drizzle schema (source of truth for column names)
apps/api/src/db/client.ts               # PRAGMAs
apps/api/src/util/request.ts            # positiveInteger + paginationValue
apps/api/src/util/loadRoutes.ts         # file→path mapping
apps/api/src/util/playableFiles.ts      # in-memory playable set + fileMeta cache
apps/api/src/util/coverVersion.ts       # sha1(imageUrl)[:12]
apps/api/src/util/rankSongSearch.ts     # JS ranking for GET /songs?q= (port exactly)
apps/api/src/util/storedFile.ts         # filename validation + songsPath join
apps/api/src/util/serveFile.ts          # abort-safe streaming
apps/api/src/util/getDuration.ts        # ffprobe duration + LRU(path+mtime)
apps/api/src/util/invalidFiles.ts       # probeAudioFile + deleteInvalidFile
apps/api/src/util/transcode.ts          # ffmpeg-stream → opus
apps/api/src/util/getBitrate.ts         # currently unused by routes, port last
apps/api/src/util/getDominantColor.ts   # only used by downloader, Phase 3
apps/api/src/util/backfillDurations.ts  # Phase 3
apps/api/src/util/repairEras.ts         # startup repair, Phase 1 must port
apps/api/src/scraper/importer.ts        # Phase 3 only
apps/api/src/scraper/downloader.ts      # Phase 3 only
apps/api/src/routes/**/*.ts             # 14 route files (see §4)
packages/types/src/index.d.ts           # Quality / AvailableLength enums
```

Test harness: `apps/api-rs/tests/api.test.mjs` — adaptive (discovers IDs), runs against `API_BASE_URL`. **This is the acceptance gate.**

> Running the suite today: see "Contract tests need a running server" in `AGENTS.md` (the command that stood here targeted the old single test file).

## 1. Recommended Rust stack

* `axum 0.7 + tokio` (full), `tower-http`: `cors`, `compression-gzip/deflate/br`, `set-header`, `timeout`, `trace`.
* SQLite: `rusqlite` (bundled) + `r2d2_sqlite` or `deadpool-sqlite`. Reason: current code is synchronous `node-sqlite` with WAL + `busy_timeout=5000`. Use `spawn_blocking` for all DB + `stat` calls. Do **not** use `sqlx` async SQLite unless you re-tune locking — `rusqlite` is a closer match.
* `serde / serde_json`, `sha1`, `hex`, `mime_guess` (but hardcode audio map, see §4.8), `tokio-util::io::ReaderStream`, `tokio::process::Command` for `ffmpeg/ffprobe`, `tokio::sync::Semaphore` for transcodes, `once_cell`, `dashmap` or `std::sync::RwLock<HashSet>` for playable set.
* Config: hand-rolled `std::env` parsing mirroring `config.ts` (fail fast with same messages).

Do not introduce an ORM. Hand-write SQL mirroring the drizzle queries below.

## 2. Config / filesystem layout (port `config.ts` exactly)

Env (same names/defaults/validation):

| Var | Default | Rule |
|---|---|---|
| `STORAGE_DIR` | `storage` | absolute or resolved vs workspace root (walk up to `package.json`); `mkdir -p {storage,storage/covers,songsPath}` on boot |
| `SONGS_DIR` | `{storage}/songs` | same resolution |
| `API_HOST`/`HOST` | `127.0.0.1` | non-empty after trim |
| `API_PORT`/`PORT` | `3000` | `^\d+$`, 1–65535 |
| `CORS_ORIGINS` | `http://localhost:4321,http://127.0.0.1:4321` | comma-split, trim; each `*` or exact `http(s)://` origin (`parsed.origin == raw`) |
| `SYNC_ON_START` | `true` | `0/false/no` (case-insensitive, trimmed) disables |
| `YOUTUBE_DOWNLOAD` | `true` | same bool parse (Phase 3) |
| `MAX_CONCURRENT_TRANSCODES` | `2` | `^\d+$`, 1–100 |
| `BACKFILL_CONCURRENCY` | `8` | Phase 3 |

Storage: `{storage}/db.sqlite3`, `{storage}/covers/{id}.avif`, `{songsPath}/{filename}`.

## 3. Global HTTP behavior (port `index.ts`)

* Only `GET` routes exist. `POST /health` → `404` (Axum: register only `get()`, add `fallback` handler).
* Middleware order: `secure-headers` (at minimum `Cross-Origin-Resource-Policy: cross-origin`), `compress` (threshold 1024), body limit 64KB (even though GET-only, keep for parity), CORS.
* CORS: `allowMethods=[GET,HEAD,OPTIONS]`, `exposeHeaders=[X-Total-Count]`, `origin = *` if list contains `*` else echo request `Origin` iff in list else no header. `Access-Control-Expose-Headers` must contain `X-Total-Count` (test asserts this).
* `GET /health` → `200 {"status":"ok"}`.
* **Error format gotcha:** `API.md` says JSON errors, but Hono `HTTPException` bodies are `text/plain`. Tests assert `text()` matches `/Invalid era id/` etc. Port as: route errors → `status + text/plain` body = message (e.g. `400 Invalid era id`); unknown route → `404 application/json {"error":"Not found"}`; unexpected → `500 {"error":"Internal server error"}` + an error log line (`eprintln!` at the time; logging is `tracing` now).
* `keepAliveTimeout=61s`, `headersTimeout` equivalent; handle `SIGINT/SIGTERM`: stop accepting, abort background chain, close DB.
* Startup DDL **must run verbatim** (from `index.ts`): `CREATE TABLE IF NOT EXISTS eras/songs/files`, the 8 `CREATE INDEX IF NOT EXISTS`, `PRAGMA table_info` conditional `ALTER TABLE ADD COLUMN files.duration / eras.is_main / songs.catalog_id`, then `UPDATE eras SET dominant_color='666666' WHERE dominant_color IS NULL OR trim(dominant_color)=''`.
* PRAGMAs on open (from `db/client.ts`): `journal_mode=WAL, busy_timeout=5000, synchronous=NORMAL, foreign_keys=ON, cache_size=-64000, temp_store=MEMORY, mmap_size=67108864, journal_size_limit=67108864`.
* Startup sequence: DDL → `repairEraDuplicates()` → `refreshPlayableFiles()` → mount routes → if `SYNC_ON_START`: blocking `importData()` then abort-checked chain `downloadCovers → backfillDurations → downloadSongs → backfillDurations`. **Phase 1 may stub importer/downloader with `SYNC_ON_START=false`**, but `repair + refreshPlayableFiles` are required.

DB tables (column names matter for `GET /songs/:id` which returns raw row):

```sql
eras(id PK, name, notes, image_url, description, dominant_color, is_main DEFAULT 1)
songs(id PK, era, catalog_id DEFAULT 'unreleased', name, notes, file_date, leak_date,
      available_length, track_length, quality, url)
files(url PK, downloaded DEFAULT 0, filename, duration REAL)
```

## 4. Shared helpers

1. **`positiveInteger(v,label)`**: `None`/not `^\d+$`/not safe-int/` <1` → `400 Invalid {label}`. Labels: `era id`, `song id`, `era filter`, `starting era filter`, `ending era filter`.
2. **`paginationValue(v,fallback,max,label)`**: missing/`""` → fallback; not `^\d+$`/`<min` (1 for limit, 0 for offset) → `400 Invalid limit|offset`; else `min(parsed,max)` (clamp, don't reject).
3. **`q` normalization**: `trim().replace(/\s+/g,' ').toLowerCase()` (ASCII `lower`, not locale). `len>100` → `400 Search query is too long`.
4. **`playable`**: `filename != null && basename(filename)==filename && playableSet.contains(filename)` where set = files on disk with `size>0` (built at boot via `readdir` + `stat` with concurrency 32, atomic swap). Keep `fileMeta: filename → {size, mtimeMs}` cache to avoid per-request `stat`. `setSongPlayable/refreshSongPlayable` mutate on download/delete. `storedSongPath()`: reject if `len 0| >255`, `== "."|".."` , contains `/ \ 0`, or `basename != input` → `404 Song file not found`.
5. **`coverVersion`**: `sha1(imageUrl ?? "").hex()[0:12]`, LRU 1000. Note: era-cover ETag uses `sha1(String(id))`, not DB `image_url`.
6. **`rankSongSearch`** (exact port of `util/rankSongSearch.ts`): strip leading `⭐✨🏅🗑️🤖` (priority 0–4, repeat loop), `normalize=same as q`, `splitParentheticalText` (depth-count parens, strip parens), `fieldScore(value,query,base)`: `pos=value.indexOf(query)`; `wordChar=/[\p{L}\p{N}]/u`; `startsAtWord = pos==0 || !wordChar(before)`; scoring `exact→base, pos0→base+10+c, wordWord→base+20+c, wordStart→base+30+c, substr→base+40+c` where `c=min(pos,99)/100 + min(lenDiff,999)/100000`. `relevance`: title-exact=0, outside-parens base 0, inside +1000, full-title +1500, era +2000, notes +3000, quality +4000, availability +4100. Sort: `trunc(score)`, `categoryPriority`, `playable desc`, `score`, `Intl.Collator(base,numeric)` title, `id`. Top-`limit` via max-heap. For collator use `icu_collator` or `human_sort`-style fallback; tests don't assert exact search order, but keep tier logic identical.
7. **Range/ETag** (`util/serveFile.ts` + stream/download/cover routes): always `Accept-Ranges: bytes` (except transcode: `none`), `ETag="<sizeHex>-<mtimeMsHex>"`, honor `If-None-Match → 304` (empty body). Parse `Range: bytes=N-M | N- | -N`. Stream: malformed/unsatisfiable → `416 + Content-Range: bytes */size`. Download: malformed/invalid → fall through to `200` (no 416); only serve `206` when `If-Range` missing or `==ETag`. Use `tokio::fs::File` + `ReaderStream` + `StreamBody`; kill stream on client abort.
8. **`probeAudioFile` + `deleteInvalidFile`** (`invalidFiles.ts`): `stat` (missing→`missing`, size 0→`empty`), `ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1` 10s (`ENOENT` binary-missing → fail-open `valid`), `null/<=0` → `no-duration`, then `ffprobe -select_streams a:0 -show_entries stream=codec_name` (`ENOENT`→fail-open, empty→`no-audio-stream`). Do **not** use bitrate. `deleteInvalidFile`: `unlink`, `playable.remove`, `UPDATE files SET downloaded=0,duration=NULL WHERE url`, log. (Superseded: request handlers no longer delete files; the background sync quarantines them on a definitive ffprobe verdict only.)
9. **Duration** (`getDuration.ts`): `tokio::process ffprobe` same args, LRU 500 keyed `(path,mtimeMs)`, in-flight dedup `HashMap<path,SharedFuture>`.

## 5. Routes (all `GET`)

Cache headers: JSON lists/details → `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600` (`JSON_CACHE` in tests). Covers → `public, max-age=86400, immutable`. Stream/download → `public, max-age=31536000, immutable`. Duration → `public, max-age=86400, immutable`. Transcode → `no-store`.

* `GET /hello` → `{"hello":"world"}`.
* `GET /eras`: `SELECT eras.* + image_url AS coverSource + count(songs.id) AS songsCount FROM eras LEFT JOIN songs ON era AND catalog='unreleased' WHERE is_main=1 GROUP BY eras.id` (no ORDER). Map → `{id,name,notes,description,dominantColor,coverVersion,songsCount}` (strip `image_url/is_main`).
* `GET /eras/:id`: validate id; `SELECT ... WHERE id LIMIT 1` else `404 Era does not exist`. Same shape minus `songsCount`.
* `GET /eras/:id/songs?limit=100/500&offset=0/10000&q?`: validate id, check era exists else 404 (before song query). `WHERE era=? AND catalog='unreleased` + optional `LOWER(coalesce(col,'')) LIKE %esc% ESCAPE '\'` (escape `\ % _`, OR over `name,notes,quality,available_length`)`. `SELECT songs.{id,era,catalog,name,notes,file_date,leak_date,available_length,track_length,quality,url}, files.{downloaded,filename,duration AS fileDuration}, count(*) OVER() AS total ORDER BY songs.id ASC LIMIT ? OFFSET ?`. `X-Total-Count: total`. Map: drop `filename/fileDuration/total`, `playable=isSongPlayable`, `duration=playable? fileDuration: null`.
* `GET /eras/:id/cover`: validate id; `stat {storage}/covers/{id}.avif` → `404 Cover not found` (even if era missing) / `500 Could not load cover`. `ETag="<coverVersion(id)>-<sizeHex>-<mtimeHex>"`, `Last-Modified`, honor `If-None-Match→304`, else `image/avif` + range-less full stream.
* `GET /songs` — two modes: if `q|era|eraFrom|eraTo|quality|availability|playable` present → search mode else plain.
  - Search: validate enums (`Quality`: 6 values, `AvailableLength`: 10 values in `API.md`), `playable=true|false`, `era*` ids, `eraFrom>eraTo→400`. SQL: `SELECT songs.{id,era,name,notes,quality,available_length}, eras.{name AS eraName,dominant_color}, files.filename, row_number() OVER (PARTITION BY era ORDER BY id) AS eraPosition FROM songs LEFT JOIN eras LEFT JOIN files WHERE catalog='unreleased' [+ instr(lower(replace CR/LF…)),query)>0] [+era/range/quality/availability] LIMIT 1000` (offset ignored). JS: `playable` enrich + filter, then `rankSongSearch` if `q` else `slice(0,limit)` where `limit=paginationValue(limit,50,50)`. Return `{songs:[{…,playable,eraPosition??1}], total: pre-limit count}` (no `X-Total-Count`).
  - Plain: `limit=paginationValue(limit,100,500)`, `offset=(offset,0,10000)`, `SELECT {id,era,catalog,name,notes,file_date,leak_date,available_length,track_length,quality,url} WHERE catalog='unreleased' LIMIT/OFFSET`, bare array (no `playable`).
* `GET /songs/:id` → `SELECT * WHERE id LIMIT 1` else `404 Song not found`. Return row with drizzle camelCase keys (`eraId,catalogId,fileDate,leakDate,availableLength,trackLength`) — match test, not `API.md`'s snake_case.
* `GET /songs/:id/stream[?quality]`: validate id; `quality`: `^\d+$` + 8–320 else `400 Invalid quality for file`. Lookup `songs LEFT JOIN files`; no row→404; `!filename→500 Could not find file for song` (legacy); `duration==0→deleteInvalidFile+404`. `stat` → missing/empty→delete+`404 Song file not found`. `ETag` + `If-None-Match→304` (only when no `?quality`). With `?quality`: `Semaphore(MAX_CONCURRENT_TRANSCODES)` try-acquire else `503 Transcoding capacity reached; try again shortly + Retry-After: 5`; `audio/opus, Accept-Ranges: none, no-store`, spawn `ffmpeg -i input -map_metadata 0 -f ogg -c:a libopus -b:a {q}k pipe:1`, stream stdout, kill on abort; on failure probe + maybe delete, else `500 Could not stream song`. Without: MIME by ext (`mp3→audio/mpeg, opus→audio/opus, ogg→audio/ogg, flac→audio/flac, wav→audio/wav, aif/aiff→audio/aiff, m4a→audio/mp4, aac→audio/aac, mp4→video/mp4, webm→video/webm`, else `octet-stream`), full Range/If-Range/416 logic above.
* `GET /songs/:id/download`: same lookup but `!filename→404 Song file not found` (not 500), no transcode, `application/octet-stream`, `Content-Disposition: attachment; filename="song-{id}{ext}"; filename*=UTF-8''{encoded}` where display name = `name` stripped of `Cc<>:"/\|?*`→space, collapsed, trimmed, sliced 120 chars, fallback `song-{id}`; `filename*` = `encodeURIComponent` with `!'()*` also escaped. Range fallback-to-200 variant.
* `GET /songs/:id/duration`: same lookup; `!song→404`; `!filename→404 Could not find file for song`; if `duration != null`: `0→delete+422 Could not determine file duration` else `200 {duration}`; else `stat` (404 on miss), in-flight dedup + `getDuration`, `null→delete+422`, else `UPDATE files SET duration WHERE url` + `200 {duration}`. Probe spawn failure with numeric exit + `url` → delete+422 else 500.
* `GET /categories`: `SELECT catalog_id,count GROUP BY`; return `getCategoryCatalogs()` (= all `CATALOGS` except `unreleased` and `mainPageSection==true`, i.e. exclude `album-copies`) mapped to `{id,name,description,songsCount,sourceUrl=https://yetracker.net/#gid={gid}}`.
* `GET /categories/:id`: raw slug; `getCatalog==None || id==unreleased →404 Category does not exist`. `album-copies` resolves here though excluded from list. Same object shape + count query.
* `GET /categories/:id/songs`: same as era-songs but `WHERE catalog=?`, no era-existence check, same `limit/offset/q` rules, same LIKE over 4 cols, same `X-Total-Count` + mapping.
* `GET /album-copies`: no validation/pagination; `SELECT songs.* + eras.name AS eraName + eras.image_url + files.{filename,duration} WHERE catalog='album-copies' ORDER BY id ASC`; group in Rust by `trim/collapse/lower(name)` (display = first-seen trimmed or `Untitled album copy`) → `[{name, copies:[{…,eraName,coverVersion: sha1(eraImageUrl)[:12],playable,duration}]}]`.

`CATALOGS` table: copy `catalogs.ts` verbatim (`unreleased/released/recent/best-of/worst-of/special/grails-wanted/stems/album-copies/ssc/fakes` + gids/descriptions).

## 6. Phased implementation

1. **Phase 0 — harness**: `cargo new` workspace (e.g. `apps/api-rs`), copy `storage/db.sqlite3` + covers + a few songs to `/tmp/yt-test`; run original on `:3100`, confirm `node --test` green as baseline.
2. **Phase 1 — read-only + file serving**: config, DB open+PRAGMA+DDL, playable scan, coverVersion, request validators, `/health /hello /eras* /songs (both modes) /songs/:id /categories* /album-copies /eras/:id/cover`. Verify with tests (media tests will skip/fail until Phase 2 — set `SONGS_DIR` to copy with ≥1 playable file).
3. **Phase 2 — media**: `storedFile/range/serveFile`, ETags, `stream/download/duration` incl. `deleteInvalidFile` side effects, `getDuration` cache, transcode semaphore + `ffmpeg`. Must pass full `api.test.mjs` including `206/304/416`, `?quality=32 → audio/opus`, invalid-quality 400s.
4. **Phase 3 — background (optional for parity, required for prod)**: port `repairEras`, `backfillDurations` (concurrency 8), `importer` (fetch `https://yetracker.net/htmlview/sheet?headers=true&gid=`, 4-concurrency, 30s, 3× backoff, row parsing, transactional replace), `downloader` (covers: 4-conc, `ffmpeg→avif 512 + dominantColor`; songs: 5-conc, `pillows.su` vs `yt-dlp` opus paths, `probeAudioFile` gate). Gate all behind `SYNC_ON_START`/`YOUTUBE_DOWNLOAD`.
5. **Phase 4 — hardening**: `ffmpeg/ffprobe` absence fail-open, abort-safety, `busy_timeout` contention test, proxy range test, `CORS_ORIGINS=*` vs exact test, `limit` clamp test (`?limit=9999` clamps, doesn't 400).

## 7. Common pitfalls

* Error body must be `text/plain`, not JSON (except fallback 404). Copy exact messages: `Invalid era id|song id|limit|offset|era filter|starting era filter|ending era filter|quality for file`, `Invalid quality|availability|playable filter`, `Search query is too long`, `Starting era must not be after ending era`, `Era does not exist|Song not found|Cover not found|Song file not found|Could not find file for song|Could not load cover|Could not stream song|Could not determine file duration|Transcoding capacity reached; try again shortly|Category does not exist`.
* `GET /eras/` (trailing slash) → fallback 404 JSON, not 400.
* `GET /songs` search mode ignores `offset`; plain mode has no `total`/`X-Total-Count`.
* Stream `!filename` is 500 but download `!filename` is 404 — keep the asymmetry.
* `duration==0` is a legacy invalid marker → delete + 404/422, not success.
* Never serve `STORAGE_DIR` directly; validate filenames to block traversal.
* SQLite `lower()` is ASCII-only — replicate with Rust `to_lowercase` on ASCII path or `lower()` in SQL consistently on both sides.

Deliverable: Rust service listening on `API_HOST:API_PORT` passing `API_BASE_URL=<rust> node --test tests/` against the same DB copy as the Node original.
