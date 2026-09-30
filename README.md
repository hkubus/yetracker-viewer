# yetracker-viewer

A web viewer and audio player for the unreleased-music catalog of [Ye Tracker](https://yetracker.net). A Rust API
imports the tracker's sheet into SQLite, keeps it in sync, downloads era covers and the audio the catalog links to, and
serves JSON plus the media (range requests, Opus transcodes). An Astro site renders the eras, searches the catalog and
plays songs in a player that keeps going from page to page.

| Path | What |
|---|---|
| `apps/api-rs` | Rust/axum API (crate `yetracker-api`). HTTP contract: [`API.md`](API.md). |
| `apps/web` | Astro 7 SSR site on Node ([`apps/web/README.md`](apps/web/README.md)). |
| `packages/types` | TypeScript types of the API contract (type-only). |
| [`AGENTS.md`](AGENTS.md) | Working notes for contributors and coding agents: commands, test recipes, layout, quirks. |

## Requirements

- **Node.js ≥ 22.18** (`engines` in both `package.json` files; the web unit tests run TypeScript through Node's type
  stripping).
- **pnpm 11** (`packageManager` pins the exact version; `corepack enable` picks it up).
- **Rust stable ≥ 1.88** — `rust-toolchain.toml` selects the stable channel with rustfmt and clippy.
- **`ffmpeg`, `ffprobe`, `yt-dlp` on `PATH`** for the API. All three are optional; the API detects them at boot and at
  every sync (a request that needs a missing tool also triggers a re-check, at most once a minute), logs one warning
  per sync naming what is missing, and degrades:
  - without **ffmpeg**: no transcodes (`/songs/:id/stream?quality=` answers 503; the player falls back to the original
    file), covers are stored as the original image (no AVIF/JPEG variants, no dominant color), and no
    YouTube/Instagram/X downloads;
  - without **ffprobe**: no durations for new files (`/songs/:id/duration` answers 503 unless one is stored) and
    downloaded files are not verified;
  - without **yt-dlp**: YouTube/Instagram/X links are not downloaded (they stay pending).
- Disk for `STORAGE_DIR`: the database is small, a full media mirror takes many GB.

## Quick start (development)

```sh
cp .env.example .env              # every setting is documented in this file
pnpm install --frozen-lockfile
pnpm dev:api                      # terminal 1: the API (debug build) on http://127.0.0.1:3000
pnpm dev                          # terminal 2: the web app on http://localhost:4321
```

The first `pnpm dev:api` compiles the crate (a few minutes), creates `storage/` and starts listening right away. The
first sync then runs in the background: it imports the catalog from yetracker.net, fetches the era covers from Google
Sheets and starts downloading audio — the example `.env` caps that at 10 files per sync (`MAX_DOWNLOADS_PER_CYCLE`;
the default is 200 files within 20 minutes per sync). To work offline, set `DOWNLOADS_ENABLED=false` and
`CATALOG_SHEET_FILE=apps/api-rs/tests/fixtures/sheet.html` (a small real extract: 4 eras, 30 songs).

`pnpm start:api` runs an optimized build instead (`cargo run --release`). Other commands:

| Command | What it does |
|---|---|
| `pnpm build` | Builds the web app into `apps/web/dist`. |
| `pnpm start:web` | Serves the built web app (`apps/web/server.mjs`, loads the repo-root `.env`). |
| `pnpm test` | `cargo test` for the API, then the web unit tests. |
| `pnpm test:contract` | Black-box HTTP contract suite against a running API (`API_BASE_URL`; see `AGENTS.md`). |
| `pnpm test:contract:local` | Starts a throwaway API on the offline fixture sheet and runs the contract suite against it (`--build`, `--release`). |
| `pnpm lint` / `pnpm format` | Biome check / format. |
| `pnpm typecheck` | `astro check` for the web app, `tsc` for `packages/types`. |

CI (`.github/workflows/ci.yml`, on pushes and pull requests to `main`) has three jobs: **web** (lint, typecheck, web
unit tests, build), **rust** (`cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
`cargo test --locked`) and **contract** (debug build, then `pnpm test:contract:local`).

## iOS app

`apps/ios` is a SwiftUI client with the same features as the web app. Open
`apps/ios/YeTracker.xcodeproj` in Xcode 16+ and run it against the local API;
see [`apps/ios/README.md`](apps/ios/README.md).

Sideloading: add the SideStore/AltStore source
`https://hkubus.github.io/yetracker-viewer/sidestore.json`, install **Ye Tracker**
and set your API server in **Settings → Server**. The source lists the releases
of the GitHub mirror ([hkubus/yetracker-viewer](https://github.com/hkubus/yetracker-viewer)).

To release: push a tag `ios-v<version>` (e.g. `ios-v1.0.1`) to the origin. It is
mirrored to GitHub, where `.github/workflows/ios.yml` builds the unsigned IPA,
publishes it as a release and redeploys the source. Every push that touches
`apps/ios` builds the IPA as a workflow artifact.

## Production

### Build

```sh
pnpm install --frozen-lockfile
pnpm build                                                     # -> apps/web/dist
cargo build --release --manifest-path apps/api-rs/Cargo.toml   # -> apps/api-rs/target/release/yetracker-api
```

### Run

Two long-running processes:

- **API**: `apps/api-rs/target/release/yetracker-api`. A single self-contained binary (SQLite and TLS are built in);
  it needs a writable `STORAGE_DIR` and, for everything to work, the media tools on `PATH`. It creates and migrates
  its database on boot. It reads `.env` from the repo root — the first directory with a `package.json`, searching
  upwards from the binary (failing that, from the working directory; failing that, the working directory itself) —
  and resolves relative paths from there; real environment variables win. Logs go to stdout (`RUST_LOG`). At startup
  it raises its soft open-file limit to the hard limit; `MAX_CONNECTIONS` defaults to `(limit − 128) / 3`, at most
  1024 (a streamed file holds a socket and the file).
- **Web**: `node server.mjs` in `apps/web` (or `pnpm start:web`, which also loads the repo-root `.env`; plain `node`
  does not). The build is self-contained: at runtime the web server needs only `dist/`, `server.mjs` and `env.mjs`
  from `apps/web` (plus its `package.json` if you start it with `pnpm start`) — no `src/`, no `node_modules`. Copying
  those to a directory of their own is a complete deployment. It validates its environment first and exits listing
  every problem.

Settings to review (all documented in [`.env.example`](.env.example)):

```dotenv
# API
API_HOST=127.0.0.1
API_PORT=3000
STORAGE_DIR=/var/lib/yetracker
# Optional separate volume for the audio:
# SONGS_DIR=/srv/yetracker/songs
# Empty: the site reaches the API same-origin under /api, no CORS needed.
CORS_ORIGINS=
# The defaults (the example file caps downloads at 10 per sync for development): each sync downloads at most 200
# files and starts downloads for at most 20 minutes, so the mirror fills up over successive syncs.
MAX_DOWNLOADS_PER_CYCLE=200
DOWNLOAD_TIME_BUDGET_MINUTES=20
# Web
WEB_HOST=127.0.0.1
WEB_PORT=4321
SITE_URL=https://example.com
PUBLIC_API_URL=/api
API_INTERNAL_URL=http://127.0.0.1:3000
# The reverse proxy terminates TLS and sets X-Forwarded-Proto (see below).
TRUST_PROXY=true
```

Keep comments on their own lines: systemd's `EnvironmentFile=` (used below) has no inline comments.

The web server requires `PUBLIC_API_URL` and, unless that is absolute, `API_INTERNAL_URL`. If the API lives on its
own origin instead, use it for `PUBLIC_API_URL` (e.g. `https://api.example.com`) and list the site's origin in
`CORS_ORIGINS`. `SITE_URL` is optional but recommended: without it pages have no canonical or `og:url` links,
`robots.txt`, `sitemap.xml` and Open Graph image URLs are built from the request's origin (`Host`, or with
`TRUST_PROXY` the `X-Forwarded-Proto`/`X-Forwarded-Host` headers) and those responses are sent `Cache-Control:
private`, and the server logs a warning at startup (also when `SITE_URL` points at localhost or 127.x).

### Reverse proxy

Put both processes behind one TLS-terminating reverse proxy and keep them bound to `127.0.0.1`:

- Route the site to the web server and `/api/` to the API **with the prefix stripped** (the API has no prefix).
- HTTPS headers: the web server sends `Strict-Transport-Security: max-age=63072000` and `upgrade-insecure-requests`
  only for requests that arrived over HTTPS — over its own TLS (`SERVER_CERT_PATH`/`SERVER_KEY_PATH`), or with
  `TRUST_PROXY=true` when the proxy sends `X-Forwarded-Proto: https`. Behind a TLS-terminating proxy, set
  `TRUST_PROXY=true` and have the proxy set (overwrite) `X-Forwarded-Proto`; never enable it when clients can reach
  the web server directly. `SITE_URL`'s scheme plays no part. The API sends no HSTS; add it at the proxy if the API
  has its own host.
- Compression is built in (web: brotli/gzip, API: gzip/deflate for JSON and text ≥ 1 KiB); don't compress again at the
  proxy, and never compress audio.
- For `/songs/*/stream` and `/songs/*/download`: pass `Range`, `If-Range`, `If-None-Match` and `If-Modified-Since`
  through, turn response buffering off (live transcodes stream while ffmpeg writes them) and allow long reads.
- Add per-IP request, connection and bandwidth limits at the proxy, especially for the media routes. The API itself
  only caps total connections (`MAX_CONNECTIONS`: at the cap a new connection replaces the longest-idle one or one
  whose client has read nothing for 10 s; a client that reads nothing for 60 s is dropped), concurrent transcodes and
  concurrent searches.
- Caching: HTML is `public, max-age=60, s-maxage=300, stale-while-revalidate=600` (errors, redirects and pages that
  rendered without their data are `no-store`), hashed assets under `/_astro/` are immutable, API JSON uses the same
  short TTLs plus weak ETags, media files and `/songs/:id/duration` are `public, no-cache` with validators, and
  covers are immutable per `?v=` version — safe to put a CDN in front. Without `SITE_URL`, responses that embed the
  request's origin are private: `robots.txt` and `sitemap.xml` (`private, max-age=3600`) and, with a same-origin
  `PUBLIC_API_URL`, era pages with a cover (`private, max-age=60`, for their Open Graph image URL).

Example (nginx):

```nginx
server {
    listen 443 ssl;
    server_name example.com;
    # ssl_certificate / ssl_certificate_key ...

    location /api/ {
        proxy_pass http://127.0.0.1:3000/;   # the trailing slash strips /api
        proxy_buffering off;
        proxy_read_timeout 15m;
    }

    location / {
        proxy_pass http://127.0.0.1:4321;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;   # read with TRUST_PROXY=true
    }
}
```

### systemd

```ini
# /etc/systemd/system/yetracker-api.service
[Unit]
Description=yetracker-viewer API
Wants=network-online.target
After=network-online.target

[Service]
User=yetracker
WorkingDirectory=/opt/yetracker-viewer
# The binary loads /opt/yetracker-viewer/.env itself.
ExecStart=/opt/yetracker-viewer/apps/api-rs/target/release/yetracker-api
Restart=on-failure
# SIGTERM: stops accepting, drains connections (≤ 10 s) and the background sync.
TimeoutStopSec=30

[Install]
WantedBy=multi-user.target
```

```ini
# /etc/systemd/system/yetracker-web.service
[Unit]
Description=yetracker-viewer web
After=network-online.target yetracker-api.service

[Service]
User=yetracker
WorkingDirectory=/opt/yetracker-viewer/apps/web
# `node server.mjs` does not read .env on its own.
EnvironmentFile=/opt/yetracker-viewer/.env
ExecStart=/usr/bin/node server.mjs
Restart=on-failure
TimeoutStopSec=20

[Install]
WantedBy=multi-user.target
```

`GET /health` on the API is a liveness check that also queries the database. The application is a public, read-only
catalog without user accounts; don't add private or administrative routes without an authentication layer.

## Storage

Keep `STORAGE_DIR` (and `SONGS_DIR`) persistent and never serve them directly.

```text
$STORAGE_DIR/
  db.sqlite3, db.sqlite3-wal, db.sqlite3-shm   SQLite in WAL mode: catalog, download state, sync metadata
  covers/<era id>.avif                         512x512 cover, plus <era id>.jpg (JPEG variant); without ffmpeg
                                               the original image (.jpg/.png/.webp/.gif)
  transcodes/<file key>-<kbps>.ogg             Opus transcode cache (LRU, TRANSCODE_CACHE_MAX_MB)
  songs/                                       downloaded audio, unless SONGS_DIR points elsewhere:
    <pillows.su hash>.<ext> / <sha256 of the link>.<ext>
    <name>.<unix time>.invalid                 rejected by ffprobe; deleted after 7 days
    <name>.<unix time>.removed                 retired: no download row refers to it any more; deleted after
                                               14 days (until then it can be recovered by hand)
    *.<16 hex>.tmp, .ytdl-<16 hex>/            downloads in progress; leftovers are removed at startup
```

Back up the three database files together while the API is stopped (or use `sqlite3 db.sqlite3 ".backup …"`). To
reset the catalog, stop the API and delete `db.sqlite3*` (and `covers/`); downloaded audio in `songs/` is linked again
by the next sync, but song and era ids start over. Databases of older versions are upgraded in place on boot.

## Data sync

The API runs one background sync at a time: right after boot when `SYNC_ON_START=true` (the listener is up first, so
the stored catalog is served meanwhile), then `SYNC_INTERVAL_MINUTES` after the previous run ended (`0` disables
periodic runs). Each run detects the media tools, then goes through these phases; a failing phase is logged and the
next one still runs, and shutdown interrupts a run.

1. **Import.** Fetches the sheet from `https://yetracker.net/htmlview/sheet?headers=true&gid=34972268` (30 s timeout,
   retries only on network errors and 5xx, 50 MB cap; the usual proxy variables apply) or reads `CATALOG_SHEET_FILE`,
   and parses eras, sub-eras and songs (multi-line names, all links, notes links, month/year dates, `~` lengths). Era
   rows are recognized by their shape (fewer cells than the header, spanning the same columns, the name cell starting
   at the Name column), so their artwork is optional. The catalog is written in one transaction.
   - *Change detection*: a fingerprint of the parsed catalog is stored; an unchanged catalog is not rewritten.
   - *Sanity guards*: a sheet without eras, songs or a required column is rejected. A changed catalog is also refused
     when it would drop below 80% of the stored songs (with more than 100 stored), below 80% of the stored distinct
     downloadable links (with more than 100 stored; catches a change in how the sheet writes links), or when song rows
     name a stored era that no longer has an era row. `IMPORT_FORCE=true` skips these three guards. A failed or refused
     import leaves the catalog untouched and shows up in `GET /status` (`lastImportOk: false`); the reason is logged.
   - *Stable ids*: an era keeps its id by normalized name; a renamed era keeps it when more than half of its songs
     (same folded title and link) reappear under one new era name. Songs are matched by content (era, name, notes,
     primary link, length), then by era + name + link, then by name + link and title + link in any era (songs that
     moved to another era or had their credits edited), then by era + name. Songs and eras deleted upstream keep a
     tombstone for 90 days, so a row that comes back within that time gets its old id again. New ids come from a
     high-water mark, so ids are never reused and never pass to a different song, and inserting a row upstream moves
     no other song's id. Lists follow the sheet order, not ids.
2. **Covers.** When an era has no cover, and once a day, the API renders the sheet straight from Google Sheets (its
   artwork links expire within minutes), downloads the images (HTTPS from `docs.google.com` /
   `*.googleusercontent.com` only, 20 MB cap) and stores a 512×512 AVIF plus a JPEG, and the era's dominant color.
   Failures back off per era.
3. **Downloads.** Files that requests found missing or broken are re-checked, and files already on disk are linked
   to their rows. Then links that are due are downloaded, fewest failed attempts first, then in catalog order:
   pillows.su, imgur.gg file pages, and YouTube/Instagram/X through yt-dlp (songs of quality "Not Available" are
   skipped, and YouTube while `YOUTUBE_DOWNLOAD=false`). Per sync: at most `MAX_DOWNLOADS_PER_CYCLE` downloads
   (default 200), `DOWNLOAD_CONCURRENCY` at a time, started within `DOWNLOAD_TIME_BUDGET_MINUTES` (default 20; ones
   still running then get 5 more minutes, then they are stopped and retried by a later sync), only while
   `MIN_FREE_DISK_MB` stays free, none bigger than `MAX_DOWNLOAD_MB`. A big backlog, like the first sync's ~6,000
   links, is therefore worked off over several syncs without delaying the next import. Each download goes to a
   temporary file that is fsynced and renamed into place.
   - *Failures* count as attempts and are retried after 30 min × 2^(attempts − 1), at most 7 days; HTTP 404/410, a
     non-audio file or the 8th failed attempt gives up (`downloadState: "failed"`), and a failed link is tried again
     30 days later.
   - *Network trouble* (DNS and connection errors, timeouts, HTTP 5xx, 429 and 408, also as reported by yt-dlp and
     its curl backend) is not an attempt: the link waits 30 min × 2^(n − 1), at most 6 h, and after 5 such errors in
     a row the service (pillows.su, imgur.gg, YouTube, Instagram, X) is left alone for the rest of the sync.
   - A *full disk* (or quota) while storing a file is not an attempt either: the link waits like after network
     trouble, and no further download starts in that sync.
   - Direct downloads only connect to public IP addresses (redirects included) and ignore `HTTP_PROXY`/`HTTPS_PROXY`/
     `ALL_PROXY`, since a proxy would resolve the host names and bypass that check.
4. **Backfill** (needs ffprobe). ffprobe verifies downloaded files and stores their durations. A file is moved aside
   (`.invalid`) only when ffprobe definitely rejects it (invalid data, no audio stream) — never on a timeout.
5. **Cleanup.** Removes download rows that no import has seen for 7 days (counted from the last successful import),
   except rows holding downloaded media: those are kept, since the media may exist nowhere else, and logged as an
   error once (delete such a row by hand to let its media go). When more than 10% of all rows are that stale at once
   (the sheet's links may have changed shape), it removes none and logs an error. Media that no row refers to
   (unreferenced downloads older than a day, and
   leftovers of removed rows) is moved to quarantine as `.removed`; quarantined files are deleted after 14 days
   (`.removed`) or 7 days (`.invalid`). Finally (only after a successful import) it sets the covers of removed eras
   aside (`.removed`) while their tombstone can still bring the era back — the cover phase then puts them back, no
   download needed — and deletes them once it expired, and it trims the least recently used transcodes over the cache
   budget.

`DOWNLOADS_ENABLED=false` skips the network part of phases 2 and 3; everything else still runs.
