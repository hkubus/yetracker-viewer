# AGENTS.md

## Setup
- Node >= 22.12, pnpm 11, Rust toolchain (for `apps/api-rs`), and `ffmpeg`, `ffprobe`, `yt-dlp` on `PATH`.
- Run `cp .env.example .env` before `pnpm dev` (web uses `--env-file-if-exists`). The Rust API also loads the repo-root `.env` without overriding real environment variables.
- `storage/` holds the SQLite DB, covers, and downloaded media and is gitignored/persistent.

## Commands (run from repo root)
- `pnpm dev` runs the web app on `:4321` (web only). `pnpm start:api` runs the Rust API on `:3000` (`cargo run --release --manifest-path apps/api-rs/Cargo.toml`); `pnpm start:web` runs the built web app.
- `pnpm build`; `pnpm typecheck`; `pnpm lint` = `biome check .`; `pnpm format`.
- `pnpm test:contract` runs the black-box HTTP contract suite (`node --test apps/api-rs/tests/`) against a running server.
- The Rust API creates/migrates its SQLite DB itself on boot (inline DDL in `apps/api-rs/src/db.rs`); there is no `resetDb`/drizzle step. To reset, stop the server and delete `storage/db.sqlite3*`.

## API tests need a running server
`apps/api-rs/tests/api.test.mjs` is a black-box HTTP contract suite. It reads `API_BASE_URL` (default `http://127.0.0.1:3000`) and discovers IDs adaptively.

```sh
cp -r storage /tmp/yt-test
SYNC_ON_START=false STORAGE_DIR=/tmp/yt-test API_PORT=3100 ./apps/api-rs/target/release/yetracker-api &
API_BASE_URL=http://127.0.0.1:3100 pnpm test:contract
```

Always test against a **copy** of `storage/`: media 404 paths reset `files` rows so the downloader retries. `SYNC_ON_START=false` keeps boot offline. Media success tests self-skip when no playable file exists on disk.

## Layout and entrypoints
- `apps/api-rs` — Rust/axum API. Routes live in `src/routes/**`; `src/main.rs` owns boot, middleware, background sync, and shutdown; `src/db.rs` owns connection setup plus inline DDL/`ALTER TABLE` migrations. The catalog importer, cover/song downloader, and duration backfill are implemented (`src/importer.rs`, `src/downloader.rs`, `src/backfill.rs`). Read `RUST_PORT_PLAN.md` and `apps/api-rs/HANDOFF.md` for history; `cargo test --lib` covers pure logic.
- `apps/web` — Astro SSR (`@astrojs/node`, standalone). Runtime entry is `server.mjs`. `src/config.ts` centralizes fetching: browser uses `PUBLIC_API_URL`, SSR uses `API_INTERNAL_URL` (must be absolute in production). `src/middleware.ts` sets CSP/security headers and edge cache-control.
- `packages/types` — type-only (`exports.import: null`), consumed via `workspace:*`.

## Quirks
- `API.md` documents the contract — trust `apps/api-rs/tests/api.test.mjs` as the source of truth.
- Route errors are `text/plain` bodies (e.g. `400 Invalid era id`); only unknown routes return `404 {"error":"Not found"}`.
- API is GET-only, public, and read-only. Do not add private/admin routes without an auth layer.
- `/eras*` and both `/songs` modes read `catalog_id = 'unreleased'` only; see `src/catalogs.rs`.
- Biome: single quotes, 2-space indent, 120 width. `apps/web/biome.json` is just `"extends": "//"`.
