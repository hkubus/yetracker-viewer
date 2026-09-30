# YeTracker Viewer API — contract v2

Read-only HTTP API of the Rust/axum service in `apps/api-rs` (routes in `src/routes/*.rs`, middleware in
`src/http.rs`). TypeScript types for every payload: `packages/types/src/index.d.ts`.

**Source of truth:** the black-box contract suite `apps/api-rs/tests/{eras,songs,media}.test.mjs` (shared helpers in
`tests/helpers.mjs`). This document describes the suite and the code; where they disagree, the tests win. How to run
the suite: see `AGENTS.md`.

| Route | Returns | `Cache-Control` of a 200 |
|---|---|---|
| `GET /health` | `{"status":"ok"}` (503 `{"status":"error"}`) | `no-store` |
| `GET /hello` | `{"hello":"world"}` (smoke test) | — |
| `GET /status` | catalog freshness and counts | JSON default¹ |
| `GET /eras` | `Era[]` (main eras, catalog order) | JSON default |
| `GET /eras/:id` | `Era` | JSON default |
| `GET /eras/:id/songs` | `Song[]` page + `X-Total-Count` | JSON default |
| `GET /eras/:id/cover` | cover image | `public, max-age=31536000, immutable` for the current `?v=`, else `public, no-cache` |
| `GET /songs` | plain mode: `Song[]` page + `X-Total-Count`; search/filter mode: `{songs, total, offset, limit}` | JSON default |
| `GET /songs/:id` | `Song` | JSON default |
| `GET /songs/:id/stream` | the stored audio file, or an Ogg Opus / AAC transcode (`?quality=`, `?format=`, `?start=`) | `public, no-cache` (live transcode: `no-store`) |
| `GET /songs/:id/download` | the stored file as an attachment | `public, no-cache` |
| `GET /songs/:id/duration` | `{"duration": seconds}` | `public, no-cache` (revalidate with the ETag) |

¹ JSON default = `public, max-age=60, s-maxage=300, stale-while-revalidate=600`.

Every route reads the primary catalog only (`catalog_id = 'unreleased'`, see `src/catalogs.rs`).

---

## Conventions

### Base URL

`http://<API_HOST>:<API_PORT>` (default `http://127.0.0.1:3000`). Behind a reverse proxy the web app may reach it under
a prefix such as `/api` (`PUBLIC_API_URL`); the proxy strips the prefix. The API has no path prefix of its own.

### Methods

- Every route answers `GET` and `HEAD` (same status and headers, no body — except that `HEAD` ignores `Range`, see
  [stored files](#stored-files-validators-and-ranges)). A `HEAD` on `/songs/:id/stream` or `/songs/:id/download`
  never reads the file and never starts ffmpeg.
- `OPTIONS` is the CORS preflight: `204` with `Access-Control-Allow-Methods: GET,HEAD,OPTIONS`, the requested headers
  echoed in `Access-Control-Allow-Headers`, `Vary: Origin, Access-Control-Request-Headers`.
- Any other method on a known path → `405` JSON `{"error":"Method not allowed"}` with `Allow: GET, HEAD, OPTIONS`.
- Unknown path (including `/eras/`) → `404` JSON `{"error":"Not found"}`.
- The API is public and read-only: there is no authentication and no write route. Don't add private or admin routes
  without an auth layer.

### Errors

- Route errors: the status plus a `text/plain;charset=UTF-8` body that is exactly the message, e.g. `400` +
  `Invalid era id`. Messages per route are listed below and collected in the [error index](#error-index).
- Unknown paths (`404`) and wrong methods (`405`) answer JSON `{"error": "…"}`; unexpected failures answer `500` JSON
  `{"error":"Internal server error"}` (the detail is logged, never sent).
- Every error — including `413` from the body limit and `416` — carries `Cache-Control: no-store` and never an `ETag`.
- `503` responses that are worth retrying soon carry `Retry-After` (seconds): busy search/transcode slots, request
  timeouts, duration probe timeouts. A `503` without `Retry-After` means "not available on this server" (a missing
  media tool); retrying won't help.

### Caching and validators

- Successful JSON responses (`200` to `GET`/`HEAD`) that aren't `no-store` — every JSON route but `/health` — get a
  weak `ETag`: `W/"<32 hex digits of the SHA-256 of the uncompressed body>"`. It is the same for the gzip, deflate and
  identity bodies (they are the same data, not the same bytes, hence weak). `If-None-Match` uses weak comparison
  (comma lists, `*`, the tag with or without `W/`) → `304` with `ETag`, `Cache-Control`, `Expires`, `Vary` and
  `X-Total-Count`, whatever the encoding.
- Media files have their own validators (`ETag`, `Last-Modified`, `If-Range`); see [media](#media-routes).
- `Cache-Control` per route: see the table above. Errors: `no-store`.

### CORS, Vary and security headers

- `Access-Control-Allow-Origin` echoes the request `Origin` when it is listed in `CORS_ORIGINS`, or is `*` when
  `CORS_ORIGINS` contains `*`; otherwise it is absent. `Access-Control-Expose-Headers: X-Total-Count, Retry-After` is
  always sent.
- `Vary` always includes `Origin`, plus `Accept-Encoding` on JSON and text responses (values are appended, never
  replaced).
- Every response: `Cross-Origin-Opener-Policy: same-origin`, `Cross-Origin-Resource-Policy: cross-origin`,
  `Origin-Agent-Cluster: ?1`, `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`,
  `X-DNS-Prefetch-Control: off`, `X-Download-Options: noopen`, `X-Frame-Options: SAMEORIGIN`,
  `X-Permitted-Cross-Domain-Policies: none`, `X-XSS-Protection: 0`. The API sends no HSTS header (set it at the TLS
  proxy if you want it).

### Compression

JSON and `text/*` responses of at least 1 KiB are gzip- or deflate-compressed when the client accepts it. Media is
never compressed. A client that refuses identity (`identity;q=0`, `*;q=0`) still gets an uncompressed `200` — the API
never answers `406`.

### Limits and timeouts

| Limit | Value |
|---|---|
| Request body | 64 KiB (`413`) |
| Non-media request (everything but `/songs/:id/stream`, `/songs/:id/download`, `/eras/:id/cover`) | 30 s → `503 Request timed out` + `Retry-After: 5` |
| Request headers | 30 s from connect (or from the first byte of the next request) |
| Idle keep-alive connection | closed after 75 s |
| Response that moves no bytes | closed after 10 min |
| Client that reads nothing of its response (its socket stays full) | connection closed after 60 s, which also frees the file it streams |
| Open connections | `MAX_CONNECTIONS` (default `min(1024, (open-file limit − 128) / 3)`, at least 1: a streamed file holds two descriptors, and 128 stay for the process; the API raises its soft limit to the hard limit at startup). At the limit a new connection replaces the one idle the longest (no request in flight) or one whose client has read nothing for 10 s or more (that one goes before connections opened less than a second ago); it is closed on accept only when every connection is busy. Replacements and refusals are logged at most every 10 s, with counts |
| Concurrent searches | `SEARCH_CONCURRENCY` (default 4) slots, taken by text searches (a `q` with [tokens](#folding-and-tokens), on `/songs` and `/eras/:id/songs`) and by `/songs` or `/eras/:id/songs` pages with `limit` > 100; such a request waits ≤ 10 s for a slot, else `503 Too many searches right now, try again` + `Retry-After: 2`. Filter-only requests and pages ≤ 100 never wait |
| Concurrent transcodes | `MAX_CONCURRENT_TRANSCODES` (default: number of CPUs, at least 2) |
| Graceful shutdown | stops accepting on SIGINT/SIGTERM, drains open connections for ≤ 10 s |

HTTP/1.1 only. TLS is expected to terminate at a reverse proxy.

### Ids, pagination and totals

- Path ids and id filters must match `^[1-9][0-9]*$` and be ≤ 2^53−1 (`Number.MAX_SAFE_INTEGER`): no sign, no
  leading zeros, no spaces inside (id filters are trimmed first, like every query value; path ids are not). Anything
  else — also a segment that isn't valid UTF-8 once decoded, like `%FF` — is `400 Invalid era id` / `Invalid song
  id` / `Invalid <filter>`.
- `limit`: digits, ≥ 1; missing or blank → the route's default; above the maximum it is **clamped**, not rejected.
  Else `400 Invalid limit`.
- `offset`: digits, 0–10000; missing or blank → 0. Anything else, including values above 10000, is `400 Invalid
  offset` (no clamping).
- List routes send `X-Total-Count`: the number of items matching the request on every page, also past the end (an
  `offset` beyond the last item returns `[]` with the real total).

### Query strings

Every route, media included, reads its parameters the same way (`request::Query`):

- Form-decoded (`+` is a space, `%XY` a byte). Parameters a route doesn't read are ignored, whatever their value
  (`/eras/:id/songs?era=%FF` is fine: only `/songs` reads `era`).
- The **last occurrence** of a repeated parameter wins (`sort=bogus&sort=name` sorts by name, `sort=name&sort=` is
  the default order).
- Values are **trimmed** (JavaScript whitespace): `era=+31+` is era 31, `category=%20best-of%20` is `best-of`.
- A **blank** value — empty or whitespace only — counts as **absent**, for every parameter: `era=%20`, `playable=`,
  `limit=%20`, `quality=` (media) are no filter / the default.
- A value that isn't valid UTF-8 once decoded only matters if it is the winning occurrence (`q=%FF&q=love` is fine,
  `q=love&q=%FF` is not). On `/songs` such a parameter (`q`, `era`, `eraFrom`, `eraTo`, `quality`, `availability`,
  `playable`, `category`, `sort`, `limit`, `offset`) is `400 Invalid <label>` (`q` → `Invalid search query`), on
  `/eras/:id/songs` likewise for the ones it reads (`q`, `category`, `sort`, `limit`, `offset`); on
  `/songs/:id/stream` `quality` is `400 Invalid quality for file`; on `/eras/:id/cover` `format` is `400 Invalid cover
  format`, and an undecodable `v` is simply not the current version.

---

## Objects

### Era

Example (abridged):

```json
{
  "id": 31,
  "position": 30,
  "name": "DONDA 2 [V1]",
  "subtitle": "(Donda 2: 4 Da Kidz, For The Children, War)",
  "notes": "(08/29/2021) (Donda officially releases)\n(11/15/2021) (Donda (Deluxe) officially releases)",
  "description": "In February 2022, Ye announced a sequel to Donda…",
  "dominantColor": "878798",
  "hasCover": true,
  "coverVersion": "feb197403c4a",
  "songsCount": 956
}
```

| Field | Type | Meaning |
|---|---|---|
| `id` | number | Stable era id (never renumbered, never reused). |
| `position` | number | 1-based order of the era in the sheet. Lists and era ranges use it, not `id`. |
| `name` | string | First line of the sheet's era-name cell. |
| `subtitle` | string \| null | The remaining lines of that cell joined with a space. |
| `notes`, `description` | string | May contain `\n`; `""` when empty. |
| `dominantColor` | string | Six hex digits without `#`; `666666` when unknown. |
| `hasCover` | boolean | A cover file exists on disk. |
| `coverVersion` | string \| null | 12 hex digits of the SHA-256 of the cover file's bytes; `null` iff `hasCover` is false. Changes exactly when the image changes. |
| `songsCount` | number | Songs of the era (also on `/eras/:id`). |

### Song

Example (abridged; `notesLinks` and the month precision are illustrative):

```json
{
  "id": 6765,
  "eraId": 31,
  "eraPosition": 441,
  "catalogId": "unreleased",
  "name": "NEBRASKA [V4]\n(prod. Bryant Troy & Digital Nas)",
  "title": "NEBRASKA [V4]",
  "subEra": "2.22.22 Sessions",
  "notes": "OG Filename: nebraska digital nas v1 128 bpm\nFirst version with Digital Nas additions.",
  "notesLinks": [{ "text": "the Common vs. Kanye freestyle battle", "url": "https://imgur.gg/f/nhOhAwL" }],
  "fileDate": 1644192000,
  "fileDatePrecision": "day",
  "leakDate": 1769904000,
  "leakDatePrecision": "month",
  "availableLength": "OG File",
  "trackLength": 225,
  "trackLengthApprox": false,
  "quality": "Lossless",
  "url": "https://pillows.su/f/f69d23c3ffe5fa5cc81f28f24cf5b43b",
  "links": ["https://pillows.su/f/f69d23c3ffe5fa5cc81f28f24cf5b43b"],
  "downloadState": "downloaded",
  "playable": true,
  "duration": 225.493163
}
```

Every key is always present, in this order.

| Field | Type | Meaning |
|---|---|---|
| `id` | number | Stable song id (see [data sync](README.md#data-sync)). |
| `eraId` | number \| null | The song's era (`null` only for a row without an era, which the importer never writes). |
| `eraPosition` | number | 1-based position of the song in its era, in catalog order, over **all** songs of the era. The era listing's page is `ceil(eraPosition / limit)` (the web uses 100 per page). |
| `catalogId` | string | Always `"unreleased"`. |
| `name` | string | The whole name cell; may contain `\n` (title line, credit line(s), alternate-titles line). |
| `title` | string | First line of `name`, category emoji markers kept. |
| `subEra` | string \| null | The sheet section (sub-era header row) the song sits under. |
| `notes` | string | Keeps line breaks; `""` when empty. |
| `notesLinks` | `{text, url}[]` | Links found inside the notes cell (`[]` when none). |
| `fileDate`, `leakDate` | number \| null | Unix seconds at UTC midnight of the first day of the period; `null` when missing or unparseable. |
| `fileDatePrecision`, `leakDatePrecision` | `"day"` \| `"month"` \| `"year"` \| null | How much of the date is known (`Nov 2017` → month, `2015` → year); `null` exactly when the date is `null`. |
| `availableLength` | string \| null | `Full`, `Snippet`, `Confirmed`, `Beat Only`, `Partial`, `Tagged`, `OG File`, `Stem Bounce`, `Rumored`, `Conflicting Sources`. |
| `trackLength` | number \| null | Seconds, as stated by the sheet. |
| `trackLengthApprox` | boolean | The sheet value was approximate (`~2:00`). |
| `quality` | string \| null | `Low Quality`, `High Quality`, `CD Quality`, `Lossless`, `Not Available`, `Recording`. |
| `url` | string \| null | The primary link, `links[0]`. |
| `links` | string[] | Every http(s) link of the Link(s) cell, deduplicated, primary first. The primary is the first pillows.su link, else an imgur.gg file page, YouTube, Instagram, X/Twitter, else the first link in cell order. |
| `downloadState` | string | See below. |
| `playable` | boolean | `/songs/:id/stream` can serve the song: a downloaded file exists on disk. Same as `downloadState === "downloaded"`. A media request that finds the file missing or empty makes the song unplayable at once (here, in the `playable` filter and in `/status`), before the next sync looks at it. |
| `duration` | number \| null | Probed audio duration in seconds (fractional) when `playable` and known, else `null`. |

`downloadState`:

| Value | Meaning |
|---|---|
| `downloaded` | The file is on disk (`playable` is true). |
| `none` | The song has no link. |
| `unsupported` | It has a link, but this server doesn't download it: quality `Not Available`, a host other than pillows.su, imgur.gg (`/f/<id>` pages), YouTube, Instagram, X/Twitter, or a YouTube link while `YOUTUBE_DOWNLOAD=false`. |
| `failed` | The downloader gave up (HTTP 404/410, not an audio file, or 8 failed attempts; network errors — also those yt-dlp reports — and a full disk don't count as attempts). It tries again 30 days later. |
| `pending` | Everything else: queued, retrying with backoff, downloads switched off (`DOWNLOADS_ENABLED=false`), a media tool missing, or the file vanished and waits to be downloaded again. |

### SearchSong

A `Song` plus the era's display data, returned by `/songs` in search/filter mode:

| Field | Type | Meaning |
|---|---|---|
| `eraName` | string | The era's `name`. |
| `dominantColor` | string | The era's `dominantColor`. |
| `eraHasCover` | boolean | The era's `hasCover`. |
| `eraCoverVersion` | string \| null | The era's `coverVersion`. |

---

## Search, filters and order

### Folding and tokens

Every text match folds both sides with the same function — Rust `fold()` in `apps/api-rs/src/search_text.rs`, TS twin
`fold()` in `apps/web/src/utils/search.ts`; they must stay identical:

1. remove apostrophes (`'` `’` `‘` `ʼ` `` ` `` `´`);
2. Unicode NFKD, drop combining marks (`Beyoncé` → `Beyonce`, `JAŸ-Z` → `JAY-Z`); spell out letters without a
   decomposition: Ø/ø→o, Æ/æ→ae, Œ/œ→oe, ß→ss, Ł/ł→l, Đ/đ→d, Þ/þ→th, ı→i;
3. lowercase (locale-independent), remove apostrophes again;
4. remove zero-width characters (U+200B–U+200D, U+2060, U+FEFF) and variation selectors (U+FE0E, U+FE0F);
5. replace every run of characters that are not letters or digits with one space, trim.

`fold("⭐️ NEBRASKA [V4] (feat. JAŸ-Z)") == "nebraska v4 feat jay z"`, `fold("can’t") == "cant"`.

The tokens of `q` are the space-separated words of `fold(q)`. A song matches when **every** token is a substring of
its search text. So matching is case-, accent- and punctuation-insensitive, token order doesn't matter, and prefixes
match while typing. The search text depends on the route:

- `/songs?q=` (global): the folded name, notes, era name, era subtitle, sub-era, quality and availability — so
  `donda` finds every song of a DONDA era.
- `/eras/:id/songs?q=` (one era): the song's own text only — name, notes, sub-era, quality and availability, without
  the era's name and subtitle (which every song of the era would match).

Besides text, `q` may contain [category markers](#categories) (⭐ ✨ 🏆 🏅 🗑️ 🤖, anywhere, with or without the
variation selectors U+FE0E/U+FE0F). They filter exactly like `category` does, combined with each other, with the
text and with `category=` by AND: `q=⭐ glory` = `q=glory&category=best-of`, `q=🗑️🤖` = songs carrying both markers.
A `q` with neither tokens nor markers (`???`, `★ …`, a lone zero-width space) counts as **blank**: it is ignored like
a missing `q`, so `/songs?q=???` is the plain list.

`q` itself is trimmed with inner whitespace collapsed; more than 100 characters (Unicode scalar values) → `400 Search
query is too long`.

### Categories

A category is an emoji marker in front of the title: `best-of` ⭐, `special` ✨, `grails` 🏆, `wanted` 🏅,
`worst-of` 🗑️, `ai` 🤖. The `category` filter keeps songs whose title contains the marker (a `🗑️🤖` song is in both
`worst-of` and `ai`); an unknown value → `400 Invalid category filter`. The same markers typed into `q` filter the
same way (see [folding and tokens](#folding-and-tokens)).

### Sorts (`sort`)

| Value | Order |
|---|---|
| `catalog` (default; `id` is an alias kept for old links) | Sheet order. |
| `category` | best-of, special, grails, wanted, unmarked, worst-of, AI — by the best marker in the title. |
| `leak-newest` / `leak-oldest` | By `leakDate`; songs without one last. |
| `file-newest` | By `fileDate`, newest first; songs without one last. |
| `name` | Natural title order: markers stripped, folded, numbers compared numerically (`[V3] < [V9] < [V39]`); titles without letters or digits (`???`) last. |

Every order ends in catalog order, so ties are stable. Values are case-sensitive; anything else is `400 Invalid sort`.

### Relevance (search mode with tokens)

`/songs?q=` ranks **all** matches, then applies `offset`/`limit`; `sort` is validated but ignored. Order of the
criteria (`apps/api-rs/src/rank.rs`):

1. where the folded query phrase occurs: the title line outside parentheses, then the rest of the name
   (parenthetical parts, credits, alternate titles), a phrase spanning the title line or the name, the era (name,
   subtitle, sub-era), the notes, quality/availability; songs that only contain the tokens scattered across fields
   come last;
2. how it matches: the whole field (for titles also ignoring a trailing `[V2]`-style tag and a leading `Artist - `),
   a prefix ending at a word boundary, a whole word, a word start, anywhere;
3. earlier matches first;
4. category (best-of … unmarked, worst-of, AI) — only between otherwise equal matches;
5. shorter fields first (less text beyond the phrase);
6. playable songs first;
7. catalog order.

---

## Routes

### `GET /health`

Liveness plus a trivial database query. `200 {"status":"ok"}`, or `503 {"status":"error"}` when no pooled connection
answers `SELECT 1` within 3 s. Always `Cache-Control: no-store`, no ETag. Meant for load balancers and monitoring.

### `GET /hello`

Smoke test: `200 {"hello":"world"}`.

### `GET /status`

```json
{
  "status": "ok",
  "lastImportAt": 1790760584,
  "lastImportOk": true,
  "lastImportError": null,
  "eras": 43,
  "songs": 9666,
  "playableSongs": 19
}
```

| Field | Type | Meaning |
|---|---|---|
| `lastImportAt` | number \| null | Unix seconds of the last **successful** import (an import that found the catalog unchanged counts); `null` before the first one. A later failed attempt does not change it. |
| `lastImportOk` | boolean \| null | Whether the most recent import attempt succeeded; `null` before the first attempt. |
| `lastImportError` | string \| null | `"Catalog update failed"` when the most recent attempt failed, else `null`. The real error stays in the logs and in `meta.last_import_error`. |
| `eras` | number | Main eras (= length of `/eras`). |
| `songs` | number | Songs in the catalog (= `X-Total-Count` of plain `/songs`). |
| `playableSongs` | number | Songs whose file is on disk (= `total` of `/songs?playable=true`). |

### `GET /eras`

Main eras in catalog order (`ORDER BY position`): `Era[]`. No parameters.

### `GET /eras/:id`

One `Era`, same shape as in the list (`songsCount` included).

Errors: `400 Invalid era id`, `404 Era does not exist`.

### `GET /eras/:id/songs`

One page of an era's songs: `Song[]`, with `X-Total-Count` = songs matching `q` and `category`.

| Param | Default | Rules |
|---|---|---|
| `limit` | 100 | 1–500 (clamped) |
| `offset` | 0 | 0–10000 |
| `sort` | `catalog` | see [sorts](#sorts-sort) |
| `q` | — | token match against the song's own text (not the era's name or subtitle, see [folding](#folding-and-tokens)) plus category markers, ≤ 100 chars; filters only, no ranking |
| `category` | — | category id |

A request with query tokens or `limit` > 100 takes a [search slot](#limits-and-timeouts).

Errors: `400 Invalid era id | Invalid limit | Invalid offset | Invalid sort | Search query is too long | Invalid
search query | Invalid category filter` (and `Invalid <label>` for an undecodable `q`, `category`, `sort`, `limit` or
`offset`; the `/songs` filters are ignored here, see [query strings](#query-strings)), `404 Era does not exist`
(checked after the parameters), `503 Too many searches right now, try again` + `Retry-After: 2`.

### `GET /songs` — plain list mode

When `q` is blank (missing, whitespace, or without tokens and category markers, like `???`) and no filter parameter
has a value: the whole catalog as a bare `Song[]` page, with `X-Total-Count` = number of songs in the catalog.

| Param | Default | Rules |
|---|---|---|
| `limit` | 100 | 1–500 (clamped) |
| `offset` | 0 | 0–10000 |
| `sort` | `catalog` | see [sorts](#sorts-sort) |

A page with `limit` > 100 takes a [search slot](#limits-and-timeouts).

Errors: `400 Invalid limit | Invalid offset | Invalid sort` (and `Invalid <label>` for an undecodable known
parameter), `503 Too many searches right now, try again` + `Retry-After: 2`.

### `GET /songs` — search/filter mode

Active when `q` has tokens or category markers, or when any of `era`, `eraFrom`, `eraTo`, `quality`, `availability`,
`playable`, `category` has a non-blank value. Filters combine with AND.

| Param | Rules |
|---|---|
| `q` | token match plus category markers (see [folding](#folding-and-tokens)), ≤ 100 chars; with tokens the results are [ranked](#relevance-search-mode-with-tokens) |
| `era` | era id; songs of that era (an unknown id gives an empty result) |
| `eraFrom`, `eraTo` | era ids; songs of the eras whose **position** lies between the two (inclusive; either bound may be omitted). Unknown id → `400 Invalid starting era filter` / `Invalid ending era filter`; from after to → `400 Starting era must not be after ending era` |
| `quality` | one of the six quality values (exact, case-sensitive) |
| `availability` | one of the ten availability values |
| `playable` | `true` or `false` (lowercase) |
| `category` | category id |
| `sort` | used when there are no query tokens (default `catalog`); validated but ignored with tokens |
| `limit` | default 50, clamped to 50 |
| `offset` | 0–10000 |

Response (with `X-Total-Count` = `total`):

```json
{ "songs": [/* SearchSong */], "total": 2609, "offset": 0, "limit": 50 }
```

`total` counts every match after all filters (no cap); `offset` and `limit` are the values applied. A request with
query tokens takes a [search slot](#limits-and-timeouts): when all `SEARCH_CONCURRENCY` slots stay busy for 10 s →
`503 Too many searches right now, try again` + `Retry-After: 2`. Filter-only requests (including a `q` with only
category markers) never wait.

Errors: `400 Search query is too long | Invalid search query | Invalid era filter | Invalid starting era filter |
Invalid ending era filter | Starting era must not be after ending era | Invalid quality filter | Invalid availability
filter | Invalid playable filter | Invalid category filter | Invalid sort | Invalid limit | Invalid offset`.

### `GET /songs/:id`

One `Song` (the same object as in the era listing). Errors: `400 Invalid song id`, `404 Song not found`.

---

## Media routes

Handlers never delete media or change download state. When the database says a song has a file but the file is
missing, empty or rejected by ffprobe, the request gets `404 Song file not found` and the file is queued for the
background sync to re-verify (and download again if needed). A missing or empty file also stops counting as playable
right away (song payloads, the `playable` filter, `/status`); the download row itself is left for the sync.

The external tools `ffmpeg`, `ffprobe` and `yt-dlp` are looked up on `PATH` at boot and at the start of every sync; a
request that needs a missing tool also triggers a background re-check, at most once a minute. Without ffmpeg
transcodes answer `503 Transcoding unavailable`; without ffprobe unknown durations answer `503 Duration probing
unavailable`. Stored files are served either way.

### Stored files: validators and ranges

Shared by `/stream` (original) and `/download`, and by cached transcodes:

- `ETag: "<size hex>-<mtime ms hex>"` (strong), `Last-Modified`, `Accept-Ranges: bytes`, `Content-Length`,
  `Cache-Control: public, no-cache` (revalidate: the ETag changes when a song's file is replaced).
- `If-None-Match` (weak comparison, comma lists, `*`) takes precedence over `If-Modified-Since`; a match → `304` with
  `ETag`, `Cache-Control`, `Last-Modified`. An `If-Modified-Since` date later than the server's clock is invalid and
  ignored (`200`).
- `HEAD` ignores `Range` (ranges are defined for `GET`): it answers like a `HEAD` without one — `200` with the whole
  file's `Content-Length` (or `304`), never `206`/`416`, no `Content-Range`.
- `Range`: one `bytes` range (unit case-insensitive): `bytes=N-M`, `bytes=N-`, `bytes=-N`; an end past the file is
  clamped → `206` + `Content-Range: bytes <start>-<end>/<size>`. A range the server doesn't understand — unparseable,
  another unit, several ranges, `5-2` — is ignored → `200` with the whole file. A range that starts at or after the end
  (or `bytes=-0`) → `416` `Range Not Satisfiable` (text/plain) with `Content-Range: bytes */<size>`,
  `Cache-Control: no-store` and no ETag.
- `If-Range`: the range applies only if the value equals the current strong ETag or exactly the `Last-Modified`
  date; otherwise → `200` with the whole file.
- Bodies stream in 256 KiB chunks.

### `GET /songs/:id/stream`

The stored audio file, or with `?quality=<kbps>` an Ogg Opus (or AAC) transcode of it.

- `quality`: absent or blank → the original; otherwise one of `64`, `128`, `192`, `256`, `320` (the player's
  bitrates), written like an id (no sign, no leading zero: `064` is refused like `0128`) and read like every [query
  parameter](#query-strings) (last occurrence wins, trimmed). Anything else → `400 Invalid quality for file`, checked
  before the song is looked up.
- `format` (only read with `quality`): absent, blank or `opus` → Ogg Opus; `aac` → ADTS AAC-LC (`audio/aac`, for
  AVPlayer on iOS, which cannot play Ogg). Anything else → `400 Invalid format`.
- `start` (only read with `quality`): seconds into the song at which the transcode begins — plain decimal, at most 3
  decimals, `0`–`86400` (`0`, absent or blank = the beginning), else `400 Invalid start`. A client that cannot seek a
  live transcode restarts it with `start`. Such transcodes are never cached (every seek would be its own entry).
- `format` and `start` are checked right after `quality`, also before the song is looked up.
- Original: `Content-Type` by file extension — `mp3` `audio/mpeg`, `opus` `audio/ogg; codecs=opus`, `ogg`/`oga`
  `audio/ogg`, `flac` `audio/flac`, `wav` `audio/wav`, `aif`/`aiff`/`aifc` `audio/aiff`, `m4a`/`mp4`/`alac` `audio/mp4`,
  `aac` `audio/aac`, `webm`/`weba` `audio/webm`, `wma` `audio/x-ms-wma`, otherwise `application/octet-stream`.
- Transcode (`Content-Type: audio/ogg; codecs=opus`, or `audio/aac` with `format=aac`; first audio stream, metadata
  kept):
  - **Cached** (`<STORAGE_DIR>/transcodes/`): served exactly like a stored file (ranges, ETag, `304`,
    `public, no-cache`, `Content-Length`).
  - **Not cached**: the request waits up to 15 s for a transcode slot (`MAX_CONCURRENT_TRANSCODES`), else `503
    Transcoding capacity reached; try again shortly` + `Retry-After: 5`. ffmpeg then writes at full speed to a temp
    file while the response streams it as it grows: `200`, no `Content-Length`, `Accept-Ranges: none`,
    `Cache-Control: no-store`. Concurrent requests for the same transcode share it; once no request has been reading it for 10 s,
    ffmpeg is stopped; the slot is released when ffmpeg exits. The finished file moves into the cache (unless
    `TRANSCODE_CACHE_MAX_MB=0`, the transcode is larger than that budget, or it has a `start`), which is trimmed to
    its budget, least recently used first. The cache key covers the source file's name, size and mtime plus the
    bitrate, format and start, so a replaced file never gets a stale transcode.
  - No ffmpeg → `503 Transcoding unavailable` (no `Retry-After`). ffmpeg failing before its first output, or no
    output within 30 s → `500 Could not transcode song`. A failure after the headers aborts the body.
  - `HEAD` never starts ffmpeg: cached → the file headers (with `Content-Length`); otherwise the live headers (no
    `Content-Length`) when a slot is free or the transcode is already running, `503` + `Retry-After: 5` when no slot
    is free, `503 Transcoding unavailable` without ffmpeg. The web player polls `HEAD` until `Content-Length` appears
    to know when a transcode is cached (seekable).

Errors: `400 Invalid song id | Invalid quality for file | Invalid format | Invalid start`, `404 Song not found | Song file not found`, `500 Could not
read song file | Could not stream song | Could not transcode song`, `503` as above.

### `GET /songs/:id/download`

The stored file as an attachment, with the [stored-file semantics](#stored-files-validators-and-ranges) (ranges,
`416`, `If-Range`, `304`):

- `Content-Type: application/octet-stream`.
- `Content-Disposition: attachment; filename="<ascii name>"; filename*=UTF-8''<percent-encoded name>`. The name is
  the first line of the song's title with characters unsafe in file names (`<>:"/\|?*`, control and invisible
  formatting characters) replaced by spaces, whitespace collapsed, at most 120 characters (cut at a word boundary),
  Windows device names (`CON`, `COM1`, …) suffixed with `_`, falling back to `song-<id>`; plus the stored file's
  extension. The ASCII `filename` strips accents and replaces other non-ASCII characters (and `"`, `\`, `%`) with `_`.

Errors: `400 Invalid song id`, `404 Song not found | Song file not found`, `500 Could not read song file | Could not
stream song`.

### `GET /songs/:id/duration`

`200 {"duration": 176.117551}` (seconds, fractional), `Cache-Control: public, no-cache` with the weak JSON `ETag`
(`If-None-Match` → `304`): a song's file can be replaced under the same id, so the value is revalidated rather than
cached as immutable. The stored duration is returned while the file exists; otherwise the file is probed with
ffprobe and the result is stored (so song payloads report it too).

Errors: `400 Invalid song id`; `404 Song not found`; `404 Song file not found` (no file, or ffprobe rejected it as
invalid — it is queued for re-verification); `422 Could not determine file duration` (the file states no duration);
`503 Duration probing unavailable` (no ffprobe); `503 Could not determine file duration` + `Retry-After: 30` (the
probe timed out or failed for an unknown reason).

### `GET /eras/:id/cover`

The era's cover image: `?v=<coverVersion>` (optional), `?format=jpeg` (optional; `jpg` is accepted too). Both are
read like every [query parameter](#query-strings) (last occurrence wins, trimmed, blank = absent); any other `format`
is `400 Invalid cover format`, checked before the cover is looked up.

- Without `format`: the primary cover — `covers/<id>.avif` (512×512), or the original image
  (`.jpg`/`.png`/`.webp`/`.gif`) when it was stored while ffmpeg was unavailable. `Content-Type` accordingly.
- `format=jpeg`: the JPEG variant (`covers/<id>.jpg`, written next to the AVIF; also the primary itself when that is
  a JPEG), else `404 Cover not found`. Useful where AVIF isn't supported (Open Graph images).
- `ETag: "<12 hex digits of the SHA-256 of the served bytes>"` (for the AVIF this equals `coverVersion`),
  `Last-Modified`, `Content-Length`. `If-None-Match` / `If-Modified-Since` → `304` (a future `If-Modified-Since` is
  ignored).
- `Cache-Control: public, max-age=31536000, immutable` only when `v` equals the current `coverVersion`; otherwise
  `public, no-cache`. Clients should request `/eras/:id/cover?v=<coverVersion>` so the URL changes with the image.
- No range support: `Range` is ignored and there is no `Accept-Ranges` header.

Errors: `400 Invalid era id | Invalid cover format`, `404 Cover not found` (no cover on disk, also for unknown eras),
`500 Could not load cover`.

---

## Error index

| Status | Message | Where |
|---|---|---|
| 400 | `Invalid era id` / `Invalid song id` | any `/eras/:id…` / `/songs/:id…` path with a non-canonical id |
| 400 | `Invalid limit`, `Invalid offset`, `Invalid sort` | list routes |
| 400 | `Search query is too long`, `Invalid search query` | `q` over 100 chars / not valid UTF-8 |
| 400 | `Invalid category filter` | `category` |
| 400 | `Invalid era filter`, `Invalid starting era filter`, `Invalid ending era filter`, `Starting era must not be after ending era` | `/songs` era filters |
| 400 | `Invalid quality filter`, `Invalid availability filter`, `Invalid playable filter` | `/songs` filters |
| 400 | `Invalid quality for file` | `/songs/:id/stream?quality=` |
| 400 | `Invalid format`, `Invalid start` | `/songs/:id/stream?format=`, `?start=` (with `quality`) |
| 400 | `Invalid cover format` | `/eras/:id/cover?format=` |
| 404 | `Era does not exist` | `/eras/:id`, `/eras/:id/songs` |
| 404 | `Song not found` | `/songs/:id` and its media routes |
| 404 | `Song file not found` | media routes: no usable file on disk |
| 404 | `Cover not found` | `/eras/:id/cover` |
| 404 | JSON `{"error":"Not found"}` | unknown path |
| 405 | JSON `{"error":"Method not allowed"}` + `Allow` | wrong method |
| 413 | (body limit) | request body over 64 KiB |
| 416 | `Range Not Satisfiable` + `Content-Range: bytes */<size>` | stored files |
| 422 | `Could not determine file duration` | `/songs/:id/duration` |
| 500 | `Could not read song file`, `Could not stream song`, `Could not transcode song`, `Could not load cover` | media I/O failures |
| 500 | JSON `{"error":"Internal server error"}` | unexpected failures (logged) |
| 503 | JSON `{"status":"error"}` | `/health`: database unavailable |
| 503 | `Request timed out` + `Retry-After: 5` | non-media request over 30 s |
| 503 | `Too many searches right now, try again` + `Retry-After: 2` | `/songs` and `/eras/:id/songs`: text searches and pages with `limit` > 100 |
| 503 | `Transcoding capacity reached; try again shortly` + `Retry-After: 5` | transcodes |
| 503 | `Transcoding unavailable` | transcodes without ffmpeg |
| 503 | `Duration probing unavailable` | `/duration` without ffprobe |
| 503 | `Could not determine file duration` + `Retry-After: 30` | `/duration` probe timeout |
