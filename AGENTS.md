# AGENTS.md

## Setup
- Node >= 22.12, pnpm 11, and `ffmpeg`, `ffprobe`, `yt-dlp` on `PATH`.
- Run `cp .env.example .env` before `pnpm dev`: `apps/api` scripts pass `--env-file ../../.env` and crash if it is missing (web uses `--env-file-if-exists`).
- TypeScript runs directly under `node` (type stripping); there is no dev build step. `storage/` holds the SQLite DB, covers, and downloaded media and is gitignored/persistent.

## Commands (run from repo root)
- `pnpm dev` runs both apps (`-r --recursive`): web on `:4321`, API on `:3000`.
- `pnpm --filter @yetracker/api dev` / `pnpm --filter web dev`; `pnpm build`; `pnpm start:api` / `pnpm start:web`.
- `pnpm typecheck`; `pnpm lint` = `biome check .`; `pnpm format`.
- `pnpm --filter @yetracker/api resetDb` deletes `storage/db.sqlite3*` then runs `drizzle-kit push`.

## API tests need a running server
`apps/api/tests/api.test.mjs` is a black-box HTTP contract suite (no root `test` script). It reads `API_BASE_URL` (default `http://127.0.0.1:3000`) and discovers IDs adaptively.

```sh
cp -r storage /tmp/yt-test
cd apps/api && SYNC_ON_START=false STORAGE_DIR=/tmp/yt-test API_PORT=3100 node src/index.ts &
API_BASE_URL=http://127.0.0.1:3100 pnpm --filter @yetracker/api test
```

Always test against a **copy** of `storage/`: media 404 paths reset `files` rows so the downloader retries. `SYNC_ON_START=false` keeps boot offline. Media success tests self-skip when no playable file exists on disk.

## Layout and entrypoints
- `apps/api` — Hono API. Routes are file-based (`src/util/loadRoutes.ts`): add `src/routes/**/<name>.ts` exporting `{ routes: { get: { handler } } }`. `src/index.ts` owns boot, inline DDL/`ALTER TABLE` migrations (drizzle-kit is not used for migrations), middleware, and shutdown.
- `apps/api` `build` is `tsc`, but the root `tsconfig.json` sets `noEmit`, so it only typechecks; production `start` runs `src/index.ts` directly.
- `apps/web` — Astro SSR (`@astrojs/node`, standalone). Runtime entry is `server.mjs`. `src/config.ts` centralizes fetching: browser uses `PUBLIC_API_URL`, SSR uses `API_INTERNAL_URL` (must be absolute in production). `src/middleware.ts` sets CSP/security headers and edge cache-control.
- `packages/types` — type-only (`exports.import: null`), consumed via `workspace:*`.
- `apps/api-rs` — in-progress Rust/axum port of `apps/api` on branch `rust-port`, targeting byte-for-byte parity. Read `RUST_PORT_PLAN.md` and `apps/api-rs/HANDOFF.md`; HANDOFF lags the working tree, so check `git status`. Same `api.test.mjs` is the acceptance gate; `cargo test --lib` covers pure logic. Importer/downloader are stubbed (logs and skips).

## Quirks
- `API.md` documents the contract but is stale on error bodies and `GET /songs/:id` key casing — trust `apps/api/tests/api.test.mjs`.
- Route errors are `text/plain` bodies (e.g. `400 Invalid era id`); only unknown routes return `404 {"error":"Not found"}`.
- API is GET-only, public, and read-only. Do not add private/admin routes without an auth layer.
- `/eras*` and both `/songs` modes read `catalog_id = 'unreleased'` only; see `src/catalogs.ts`.
- Biome: single quotes, 2-space indent, 120 width. Per-app `biome.json` files are just `"extends": "//"`.
