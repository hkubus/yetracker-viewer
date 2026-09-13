# YeTracker Viewer API

Rust/axum service (`apps/api-rs`). Routes live in `apps/api-rs/src/routes/**`
(`eras.rs`, `songs.rs`, wired via `mod.rs`;
`GET /health` and `GET /hello` are defined in `apps/api-rs/src/routes/mod.rs`):

- `GET /hello`
- `GET /eras`, `GET /eras/:id`
- `GET /eras/:id/songs`, `GET /eras/:id/cover`
- `GET /songs`, `GET /songs/:id`
- `GET /songs/:id/stream`, `GET /songs/:id/download`, `GET /songs/:id/duration`

The contract source of truth is the black-box suite
`apps/api-rs/tests/api.test.mjs`; this document is descriptive and may lag it.

All documented routes are `GET`-only. `HEAD`/`OPTIONS` are handled by CORS/compression middleware.

## Base URL and global behavior

- Local dev default: `http://127.0.0.1:3000` (`API_HOST`/`API_PORT` in `.env`).
- Behind the web app it may be exposed as `PUBLIC_API_URL` (e.g. `/api` with prefix stripped before forwarding).
- CORS: `origin` from `CORS_ORIGINS` (or `*`), allowed methods `GET, HEAD, OPTIONS`, exposes `X-Total-Count`.
- Middleware: security headers (including `Cross-Origin-Resource-Policy: cross-origin`), response compression, 64KB body limit.
- Success JSON is served as `application/json`.
- Route errors: that status code with a `text/plain` body = the message (e.g. `400 Invalid era id`, `404 Song not found`). Only unknown paths return `404 { "error": "Not found" }` as JSON. Unexpected failures: `500 { "error": "Internal server error" }`.

## Common conventions

### IDs and pagination helpers (`apps/api-rs/src/request.rs`)

- `positiveInteger(value, label)`: path/era filter. Must match `/^\d+$/`, safe integer, `>= 1`. Else `400 Invalid <label>`.
- `paginationValue(value, fallback, maximum, label)`:
  - Missing/empty → `fallback`.
  - Must match `/^\d+$/`, safe integer, `>= 1` for `limit`, `>= 0` for `offset`. Else `400 Invalid limit|offset`.
  - Clamped with `Math.min(parsed, maximum)` (over-maximum is clamped, not rejected).

### Catalogs (`apps/api-rs/src/catalogs.rs`)

- `PRIMARY_CATALOG_ID = "unreleased"`.
- `/eras*` and `/songs` (both plain and search modes) only read `catalog_id = "unreleased"`.

### Shared enums (`packages/types/src/index.d.ts`)

- `Quality`: `Low Quality | High Quality | CD Quality | Lossless | Not Available | Recording`
- `AvailableLength` / `availability`: `Full | Snippet | Confirmed | Beat Only | Partial | Tagged | OG File | Stem Bounce | Rumored | Conflicting Sources`

### Derived fields

- `playable: boolean`: true iff `files.filename` is non-null, is a bare basename, and exists on disk under songs dir with `size > 0`.
- `duration: number | null`: `files.duration` (seconds, float) when `playable`, else `null`.
- `coverVersion: string`: `sha1(imageUrl ?? '')[0:12]` (hex). Used by web for cache-busting cover URLs. Never null; empty source still hashes to a value.
- `fileDate`, `leakDate`: Unix seconds (`0` when missing/unparsable).
- `downloaded`: `files.downloaded` (`1` stored, else `0`); `null` when no `files` row (left join miss).
- Dates, counts, search `q` handling is ASCII `lower()`-based; `q` is trimmed, inner whitespace collapsed to single spaces, max 100 chars else `400 Search query is too long`.

---

## `GET /health`

Readiness check.

- Input: none.
- Output `200`:
  ```json
  { "status": "ok" }
  ```

## `GET /hello`

Smoke-test route.

- Input: none.
- Output `200`:
  ```json
  { "hello": "world" }
  ```

## `GET /eras`

List main eras with song counts (primary catalog only).

- Input: none (no query params).
- Output `200`: `Era[]`, header `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600`.
  ```json
  [
    {
      "id": 1,
      "name": "808s & Heartbreak Era",
      "notes": "...",
      "description": "...",
      "dominantColor": "666666",
      "coverVersion": "a1b2c3d4e5f6",
      "songsCount": 42
    }
  ]
  ```
- Fields: `id: number`, `name: string|null`, `notes: string|null`, `description: string|null`, `dominantColor: string|null`, `coverVersion: string` (derived, `image_url` itself is not returned), `songsCount: number` (count of `songs` where `era = eras.id` and `catalog_id = "unreleased"`).

## `GET /eras/:id`

Single era by numeric id.

- Path param: `id` — positive integer (`400 Invalid era id`).
- Output `200`: single `Era` without `songsCount`, header `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600`.
  ```json
  {
    "id": 1,
    "name": "...",
    "notes": "...",
    "description": "...",
    "dominantColor": "666666",
    "coverVersion": "a1b2c3d4e5f6"
  }
  ```
- Errors: `400` invalid id, `404 Era does not exist`.

## `GET /eras/:id/songs`

Paginated, optionally searched songs for one era (primary catalog only).

- Path param: `id` — positive integer (`400 Invalid era id`; `404 Era does not exist` if no era row, checked before song query).
- Query params:
  | Name | Default | Max | Notes |
  |------|---------|-----|-------|
  | `limit` | `100` | `500` | min 1, `400 Invalid limit` |
  | `offset` | `0` | `10000` | min 0, `400 Invalid offset` |
  | `q` | — | 100 chars | optional; trimmed/collapsed; case-insensitive `LIKE %q%` (escaped `\ % _`) against `songs.name`, `songs.notes`, `songs.quality`, `songs.available_length` |
  | `category` | — | — | optional emoji-category filter (`best-of`, `special`, `grails`, `wanted`, `worst-of`, `ai`); keeps songs whose `songs.name` contains the category's emoji (`instr`, base codepoint so variation selectors also match); unknown id → `400 Invalid category filter`; empty = no filter |
- Response headers: `X-Total-Count: <total matching, before limit/offset>` (via `count(*) over()`), `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600`.
- Output `200`: array ordered by `songs.id ASC`:
  ```json
  [
    {
      "id": 10,
      "eraId": 1,
      "catalogId": "unreleased",
      "name": "Song title",
      "notes": "...",
      "fileDate": 1700000000,
      "leakDate": 1700000000,
      "availableLength": "Snippet",
      "trackLength": 65,
      "quality": "High Quality",
      "url": "https://pillows.su/...",
      "downloaded": 1,
      "playable": true,
      "duration": 65.12
    }
  ]
  ```
- `filename`, `fileDuration`, window `total` are stripped server-side; `playable`/`duration` are derived as above. `downloaded: number|null`.

## `GET /eras/:id/cover`

Era cover image file.

- Path param: `id` — positive integer (`400 Invalid era id`).
- Request header: `If-None-Match` (optional, for `304`).
- Output `200`: binary `image/avif` from `<STORAGE_DIR>/covers/<id>.avif`, streamed with Range support (`Accept-Ranges: bytes`):
  - `Cache-Control: public, max-age=86400, immutable`
  - `ETag: "<coverVersion>-<sizeHex>-<mtimeMsHex>"` (coverVersion derived from `String(id)`, not DB `image_url`)
  - `Last-Modified`, `Content-Length`, `Content-Type: image/avif`
- Output `304`: when `If-None-Match` equals current `ETag` (empty body).
- Errors: `404 Cover not found` (missing/not-a-file), `500 Could not load cover`.

## `GET /songs`

Two modes sharing one path.

### A. Search / filter mode (if `q`, `era`, `eraFrom`, `eraTo`, `quality`, `availability`, or `playable` is present)

- Query params:
  | Name | Notes |
  |------|-------|
  | `q` | optional, trimmed/collapsed/lowercased, max 100 else `400 Search query is too long`; substring (`instr > 0`) over `songs.name + notes + era.name + quality + availableLength` (CR/LF → space) |
  | `era` | optional positive integer (`400 Invalid era filter`); exact `songs.era = era` |
  | `eraFrom` | optional positive integer (`400 Invalid starting era filter`); `songs.era >= eraFrom` |
  | `eraTo` | optional positive integer (`400 Invalid ending era filter`); `songs.era <= eraTo`; `400 Starting era must not be after ending era` if `eraFrom > eraTo` |
  | `quality` | optional, must be one of 6 `Quality` values else `400 Invalid quality filter` |
  | `availability` | optional, must be one of 10 availability values else `400 Invalid availability filter` |
  | `playable` | optional `true`/`false` string else `400 Invalid playable filter`; applied in JS after DB fetch |
  | `limit` | default `50`, max `50`, min 1 |
  | `offset` | accepted by parser but **ignored** in this mode (ranking slices from 0) |
- Behavior: DB fetches up to 1000 candidates (`catalog_id = "unreleased"` + filters), then `rankSongSearch(matches, q, limit)` in JS (title > parenthetical > era > notes > quality > availability; ⭐✨🏅🗑️🤖 markers, playable-first, word-boundary/length tie-breaks). Without `q`, first `limit` matches in DB order.
- Output `200`: envelope (no `X-Total-Count` header here), header `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600`:
  ```json
  {
    "songs": [
      {
        "id": 10,
        "eraId": 1,
        "name": "...",
        "notes": "...",
        "quality": "High Quality",
        "availableLength": "Snippet",
        "eraName": "Era name",
        "dominantColor": "666666",
        "playable": true,
        "eraPosition": 3
      }
    ],
    "total": 27
  }
  ```
  - `eraPosition: number` = `row_number() over (partition by songs.era order by songs.id)` (fallback `1`).
  - `total: number` = pre-`limit` match count (post-`playable` filter, pre-rank slice).

### B. Plain list mode (no search/filter params)

- Query params: `limit` default `100` max `500`; `offset` default `0` max `10000`.
- Output `200`: bare array (no envelope, no `X-Total-Count`), ordered implicitly by rowid:
  ```json
  [
    {
      "id": 10,
      "eraId": 1,
      "catalogId": "unreleased",
      "name": "...",
      "notes": "...",
      "fileDate": 1700000000,
      "leakDate": 1700000000,
      "availableLength": "Snippet",
      "trackLength": 65,
      "quality": "High Quality",
      "url": "https://pillows.su/..."
    }
  ]
  ```
  - No `playable`/`duration` enrichment in this mode.

## `GET /songs/:id`

Single song DB row (no file enrichment).

- Path param: `id` — positive integer (`400 Invalid song id`).
- Output `200`: single object with camelCase keys, header `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600`:
  ```json
  {
    "id": 10,
    "eraId": 1,
    "catalogId": "unreleased",
    "name": "...",
    "notes": "...",
    "fileDate": 1700000000,
    "leakDate": 1700000000,
    "availableLength": "Snippet",
    "trackLength": 65,
    "quality": "High Quality",
    "url": "https://pillows.su/..."
  }
  ```
- Errors: `404 Song not found`.

## `GET /songs/:id/stream`

Stream (or live-transcode) the stored audio file. Range-capable.

- Path param: `id` — positive integer (`400 Invalid song id`).
- Query param: `quality` — optional bitrate kbps as digits-only string, integer `8–320` else `400 Invalid quality for file`. When present, live-transcodes source to Opus via ffmpeg.
- Request headers: `Range: bytes=<start>-<end>` (optional, `bytes=N-M`, `bytes=N-`, `bytes=-N`), `If-None-Match` (only honored for non-transcoded responses), `If-Range` (optional).
- Success without `?quality` (`200` or `206`):
  - `Content-Type` by stored extension: `mp3→audio/mpeg`, `opus→audio/opus`, `ogg→audio/ogg`, `flac→audio/flac`, `wav→audio/wav`, `aif|aiff→audio/aiff`, `m4a→audio/mp4`, `aac→audio/aac`, `mp4→video/mp4`, `webm→video/webm`, else `application/octet-stream`.
  - `ETag: "<sizeHex>-<mtimeMsHex>"`, `Cache-Control: public, max-age=31536000, immutable`, `Accept-Ranges: bytes`, `Content-Length`, and on ranges `Content-Range: bytes <s>-<e>/<size>` with `206`.
  - `304` when `If-None-Match` matches and no `?quality`.
  - `416` with `Content-Range: bytes */<size>` for malformed/unsatisfiable ranges.
- Success with `?quality=N`:
  - `Content-Type: audio/opus`, `Accept-Ranges: none`, `Cache-Control: no-store`, chunked live stream (no `Content-Length`/`ETag`).
  - `503 Transcoding capacity reached; try again shortly` (+ `Retry-After: 5`) when concurrent transcodes ≥ `MAX_CONCURRENT_TRANSCODES`.
- Errors: `404 Song not found` (no song row), `500 Could not find file for song` (row exists but no `files.filename`), `404 Song file not found` (missing/empty/probe-failed file; also triggers DB cleanup for re-download), `500 Could not stream song`.

## `GET /songs/:id/download`

Download stored file as attachment. Range-capable.

- Path param: `id` — positive integer (`400 Invalid song id`).
- Request headers: `Range`, `If-None-Match`, `If-Range` (range only honored when `If-Range` missing or equal to `ETag`; malformed ranges fall through to `200` instead of `416`, unlike stream).
- Output `200` (or `206` / `304`):
  - `Content-Type: application/octet-stream`
  - `Content-Disposition: attachment; filename="song-<id><ext>"; filename*=UTF-8''<encoded sanitized song name><ext>` (control/`<>:"/\|?*` → space, collapsed, max 120 chars, fallback `song-<id>`)
  - `ETag`, `Accept-Ranges: bytes`, `Cache-Control: public, max-age=31536000, immutable`, `Content-Length`, and on ranges `Content-Range` + `206`.
- Errors: `404 Song not found`, `404 Song file not found` (same cleanup semantics as stream; `duration === 0` legacy marker also triggers cleanup).

## `GET /songs/:id/duration`

Probed audio duration in seconds.

- Path param: `id` — positive integer (`400 Invalid song id`).
- Output `200`: header `Cache-Control: public, max-age=86400, immutable`:
  ```json
  { "duration": 65.12 }
  ```
  - If `files.duration` already set (and `!== 0`), returned directly.
  - Else `ffprobe` result is persisted to `files.duration` and returned. Concurrent probes for same path are de-duplicated in memory.
- Errors: `404 Song not found`, `404 Song file not found` / `Could not find file for song`, `422 Could not determine file duration` (probe null / legacy `0` marker / corrupt file; triggers cleanup for re-download), `500 Could not determine file duration`.
