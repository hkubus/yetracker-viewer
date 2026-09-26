# yetracker-viewer

An Astro SSR frontend and Rust/axum API for browsing and playing the Ye Tracker catalog.

## Requirements

- Node.js 22.12 or newer
- pnpm 11
- `ffmpeg`, `ffprobe`, and `yt-dlp` available on `PATH`
- Persistent storage for the SQLite database, covers, and downloaded media

## Local development

```sh
cp .env.example .env
pnpm install --frozen-lockfile
pnpm dev
```

The defaults bind the API to `127.0.0.1:3000` and the web app to
`127.0.0.1:4321`.

On startup, the importer refreshes the main Unreleased catalog and the
song-oriented Yetracker sheets listed in `apps/api-rs/src/catalogs.rs`. The web
home page exposes those additional sheets under the main era list, and each
category has its own paginated song view.

The API also re-imports the catalogs and retries missing covers/media in the
background every 30 minutes, independent of `SYNC_ON_START`, so new entries and
downloads appear without a restart. Setting `SYNC_ON_START=false` only skips the
blocking import at boot.

## Production

Build the web app once:

```sh
pnpm install --frozen-lockfile
pnpm --filter web build
```

Run the API and web app as separate supervised processes:

```sh
pnpm start:api
pnpm start:web
```

Copy `.env.example` to `.env` and set at least:

```dotenv
API_HOST=0.0.0.0
API_PORT=3000
WEB_HOST=0.0.0.0
WEB_PORT=4321
PUBLIC_API_URL=https://example.com/api
API_INTERNAL_URL=http://127.0.0.1:3000
CORS_ORIGINS=https://example.com
STORAGE_DIR=/var/lib/yetracker
SONGS_DIR=/srv/yetracker/songs
```

`PUBLIC_API_URL` is used by visitors' browsers and may be a relative path such
as `/api`. `API_INTERNAL_URL` must be an absolute URL reachable by the SSR
process. If the API is hosted on a separate domain, use that public API origin
for both `PUBLIC_API_URL` and `CORS_ORIGINS`. `SONGS_DIR` can place downloaded
song media on a separate volume; relative paths are resolved from the workspace
root. If it is unset, song media remains under `STORAGE_DIR/songs`.

Put both processes behind a TLS-terminating reverse proxy. Route the site to
port 4321 and, for the example above, strip the `/api` prefix before forwarding
API requests to port 3000. Forward range requests and do not buffer media
responses. Add proxy-level request and bandwidth limits, especially for
`/songs/*/stream` and `/songs/*/download`.

The API exposes `GET /health` for readiness checks. Keep `STORAGE_DIR` and
`SONGS_DIR` persistent and never serve them directly. Set `SYNC_ON_START=false`
after initial catalog setup if startup must not depend on upstream services. The
application is a public, read-only catalog and has no user authentication; do
not add private or administrative routes without an authentication layer.
