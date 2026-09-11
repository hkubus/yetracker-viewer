# YeTracker Viewer API

Hono API (`apps/api/src`). File-based routing via `util/loadRoutes.ts`:

- `apps/api/src/routes/hello.ts` → `GET /hello`
- `apps/api/src/routes/eras/index.ts` → `GET /eras`
- `apps/api/src/routes/eras/[id]/index.ts` → `GET /eras/:id`
- `apps/api/src/routes/eras/[id]/songs.ts` → `GET /eras/:id/songs`
- `apps/api/src/routes/eras/[id]/cover.ts` → `GET /eras/:id/cover`
- `apps/api/src/routes/songs/index.ts` → `GET /songs`
- `apps/api/src/routes/songs/[id]/index.ts` → `GET /songs/:id`
- `apps/api/src/routes/songs/[id]/stream.ts` → `GET /songs/:id/stream`
- `apps/api/src/routes/songs/[id]/download.ts` → `GET /songs/:id/download`
- `apps/api/src/routes/songs/[id]/duration.ts` → `GET /songs/:id/duration`
- `apps/api/src/routes/categories/index.ts` → `GET /categories`
- `apps/api/src/routes/categories/[id]/index.ts` → `GET /categories/:id`
- `apps/api/src/routes/categories/[id]/songs.ts` → `GET /categories/:id/songs`
- `apps/api/src/routes/album-copies/index.ts` → `GET /album-copies`
- Defined directly in `apps/api/src/index.ts`: `GET /health`

All documented routes are `GET`-only. `HEAD`/`OPTIONS` are handled by CORS/compression middleware.

## Base URL and global behavior

- Local dev default: `http://127.0.0.1:3000` (`API_HOST`/`API_PORT` in `.env`).
- Behind the web app it may be exposed as `PUBLIC_API_URL` (e.g. `/api` with prefix stripped before forwarding).
- CORS: `origin` from `CORS_ORIGINS` (or `*`), `allowMethods: GET, HEAD, OPTIONS`, `exposeHeaders: X-Total-Count`.
- Middleware: `secureHeaders({ crossOriginResourcePolicy: 'cross-origin' })`, `compress({ threshold: 1024 })`, `bodyLimit({ maxSize: 64KB })`.
- Success JSON uses `c.json()`.
- Unknown path: `404 { "error": "Not found" }`.
- Thrown `HTTPException(status, message)` produces that status with `message` (e.g. `Invalid era id`, `Song not found`). Unexpected errors: `500 { "error": "Internal server error" }`.

## Common conventions

### IDs and pagination helpers (`util/request.ts`)

- `positiveInteger(value, label)`: path/era filter. Must match `/^\d+$/`, safe integer, `>= 1`. Else `400 Invalid <label>`.
- `paginationValue(value, fallback, maximum, label)`:
  - Missing/empty → `fallback`.
  - Must match `/^\d+$/`, safe integer, `>= 1` for `limit`, `>= 0` for `offset`. Else `400 Invalid limit|offset`.
  - Clamped with `Math.min(parsed, maximum)` (over-maximum is clamped, not rejected).

### Catalogs (`catalogs.ts`)

- `PRIMARY_CATALOG_ID = "unreleased"`.
- `/eras*` and `/songs` (both plain and search modes) only read `catalog_id = "unreleased"`.
- Known `catalog.id` values: `unreleased`, `released`, `recent`, `best-of`, `worst-of`, `special`, `grails-wanted`, `stems`, `album-copies`, `ssc`, `fakes`.
- `GET /categories` lists all except `unreleased` and `album-copies` (`mainPageSection: true`).
- `GET /categories/:id` rejects `unreleased` with 404, but `album-copies` is addressable directly even though it is omitted from the list.

### Shared enums (`packages/types/src/index.d.ts`, `routes/songs/index.ts`)

- `Quality`: `Low Quality | High Quality | CD Quality | Lossless | Not Available | Recording`
- `AvailableLength` / `availability`: `Full | Snippet | Confirmed | Beat Only | Partial | Tagged | OG File | Stem Bounce | Rumored | Conflicting Sources`

### Derived fields

- `playable: boolean` (`util/playableFiles.ts`): true iff `files.filename` is non-null, is a bare basename, and exists on disk under songs dir with `size > 0`.
- `duration: number | null`: `files.duration` (seconds, float) when `playable`, else `null`.
- `coverVersion: string`: `sha1(imageUrl ?? '').hexdigest[0:12]` (`util/coverVersion.ts`). Used by web for cache-busting cover URLs. Never null; empty source still hashes to a value.
- `fileDate`, `leakDate`: Unix seconds (`Math.floor(Date.parse(cell)/1000)`, `0` when missing/unparsable).
- `downloaded`: `files.downloaded` (`1` stored, else `0`); `null` when no `files` row (left join miss).
- Dates, counts, search `q` handling is ASCII `lower()`-based; `q` is trimmed, inner whitespace collapsed to single spaces, max 100 chars else `400 Search query is too long`.

---

## `GET /health`

Readiness check (`index.ts`).

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
- Output `200`: single object with all `songs` columns, header `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600`:
  ```json
  {
    "id": 10,
    "era": 1,
    "catalog_id": "unreleased",
    "name": "...",
    "notes": "...",
    "file_date": 1700000000,
    "leak_date": 1700000000,
    "available_length": "Snippet",
    "track_length": 65,
    "quality": "High Quality",
    "url": "https://pillows.su/..."
  }
  ```
  Note: keys are raw DB column names (`era`, `catalog_id`, `file_date`, …) via `db.select().from(songsTable)`, unlike the camelCase list views.
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

## `GET /categories`

List secondary catalogs (“sheets”).

- Input: none.
- Output `200`: `Category[]`, header `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600`:
  ```json
  [
    {
      "id": "released",
      "name": "Released",
      "description": "Released songs, features, and production credits.",
      "songsCount": 123,
      "sourceUrl": "https://yetracker.net/#gid=762588265"
    }
  ]
  ```
- Fields: `id: string` (slug), `name: string`, `description: string`, `songsCount: number` (count of `songs` with that `catalog_id`), `sourceUrl: string` (`https://yetracker.net/#gid=<gid>`).
- Excludes `unreleased` and `album-copies`.

## `GET /categories/:id`

Single category.

- Path param: `id` — catalog slug string (case-sensitive, e.g. `released`, `recent`, `best-of`, `worst-of`, `special`, `grails-wanted`, `stems`, `ssc`, `fakes`; also `album-copies` technically resolves). `unreleased` or unknown → `404 Category does not exist`.
- Output `200`: single `Category` object (same shape as list items), same `Cache-Control` header.

## `GET /categories/:id/songs`

Paginated, optionally searched songs for one category (any `catalog_id` except primary).

- Path param: `id` — slug, same 404 rules as above.
- Query params: same as `GET /eras/:id/songs` — `limit` default `100` max `500`, `offset` default `0` max `10000`, `q` optional max 100 searched against `name/notes/quality/availableLength` (no era-name search here).
- Response headers: `X-Total-Count`, `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600`.
- Output `200`: array ordered by `songs.id ASC`, same item shape as era songs (`id, eraId, catalogId, name, notes, fileDate, leakDate, availableLength, trackLength, quality, url, downloaded, playable, duration`).

## `GET /album-copies`

Album/demo copies grouped by normalized title.

- Input: none (no pagination; full `catalog_id = "album-copies"` ordered by `songs.id ASC`).
- Output `200`: `AlbumCopyGroup[]`, header `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600`:
  ```json
  [
    {
      "name": "Donda (Demo Tape)",
      "copies": [
        {
          "id": 99,
          "eraId": 5,
          "catalogId": "album-copies",
          "name": "Donda (Demo Tape)",
          "notes": "...",
          "fileDate": 1700000000,
          "leakDate": 1700000000,
          "availableLength": "Full",
          "trackLength": 3600,
          "quality": "Lossless",
          "url": "https://pillows.su/...",
          "eraName": "Donda Era",
          "coverVersion": "a1b2c3d4e5f6",
          "playable": true,
          "duration": 3600.5
        }
      ]
    }
  ]
  ```
- Grouping key: `name.trim().replace(/\s+/g,' ').toLowerCase()`; display `name` is first-seen trimmed form (fallback `Untitled album copy`). `eraName: string|null` (left-joined era), `coverVersion: string|null` (from `eras.image_url`), `playable`/`duration` derived as elsewhere.
