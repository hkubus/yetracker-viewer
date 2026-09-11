# Rust port — handoff notes

Branch: `rust-port` (created off `main`). Plan: `RUST_PORT_PLAN.md` (repo root).
Acceptance gate: `apps/api/tests/api.test.mjs` (black-box, adaptive).

Work stopped mid-Phase 1. The crate **compiles and its unit tests pass**, but the
HTTP layer is not wired up yet, so the acceptance suite has **not** been run
against Rust.

## Verified right now

```bash
# unit tests (18/18)
cd apps/api-rs && cargo test --lib

# Node baseline / acceptance gate (45/45, ~1.1s)
cp -r storage /tmp/yt-test
cd apps/api && SYNC_ON_START=false STORAGE_DIR=/tmp/yt-test API_PORT=3100 node src/index.ts &
API_BASE_URL=http://127.0.0.1:3100 node --test tests/
```

`storage/songs/` is **empty**, so every media *success* test self-skips
(`t.skip('no playable songs on this server')`) and passes vacuously. Phase 2
(stream/download/duration/transcode) is therefore **not exercised at all** by
the current run. Before trusting Phase 2, seed the copy with a playable fixture:

```bash
ffmpeg -f lavfi -i "sine=frequency=440:duration=2" -b:a 32k /tmp/yt-test/songs/yt-port-fixture.mp3
sqlite3 /tmp/yt-test/db.sqlite3 <<'SQL'
INSERT INTO files (url, downloaded, filename, duration)
  VALUES ('https://example.invalid/yt-port-fixture', 1, 'yt-port-fixture.mp3', NULL);
INSERT INTO songs (id, era, catalog_id, name, notes, file_date, leak_date,
                   available_length, track_length, quality, url)
  SELECT (SELECT max(id)+1 FROM songs), 1, 'unreleased', 'ZZ Fixture Tone', '',
         0, 0, 'Full', 2, 'CD Quality', 'https://example.invalid/yt-port-fixture';
SQL
```

`duration` is deliberately NULL so `/songs/:id/duration` exercises the real
ffprobe path. Use **separate copies** for the Node and Rust runs
(`/tmp/yt-test-node`, `/tmp/yt-test-rs`) — media 404 paths mutate `files` rows.

## Done (written, compiles, unit-tested)

| File | Mirrors | Notes |
|---|---|---|
| `src/config.rs` | `config.ts` | same env names/defaults/messages, workspace-root walk, `mkdir -p` |
| `src/catalogs.rs` | `catalogs.ts` | table copied verbatim |
| `src/db.rs` | `db/client.ts` + DDL in `index.ts` | r2d2 pool, PRAGMAs per connection, verbatim DDL + `table_info` migrations |
| `src/error.rs` | Hono error semantics | `Http` → `text/plain;charset=UTF-8` body = message; `Unexpected` → `500 {"error":"Internal server error"}` + log |
| `src/request.rs` | `util/request.ts` | `positiveInteger`, `paginationValue`, `escapeLikePattern` |
| `src/text.rs` | JS string semantics | `\s` set incl. U+FEFF, `trim`, collapse, `toLowerCase`, UTF-16 length |
| `src/playable.rs` | `util/playableFiles.ts` | atomic set swap, 32-way scan, `storedSongPath`, `isSafeFilename` |
| `src/cover_version.rs` | `util/coverVersion.ts` | sha1[..12], LRU 1000 |
| `src/rank.rs` | `util/rankSongSearch.ts` | full port incl. max-heap and NaN comparator semantics |
| `src/serve.rs` | `util/serveFile.ts` + range blocks | range parsing, abort-safe streaming body |
| `src/media.rs` | `util/getDuration.ts`, `util/invalidFiles.ts` | shared duration futures, probe, `deleteInvalidFile` |
| `src/repair.rs` | `util/repairEras.ts` | runs at startup |
| `src/state.rs` | — | `AppState`: pool, caches, transcode semaphore |
| `src/routes/mod.rs` | — | full route table + JSON/cache helpers |

## Not written yet

1. `src/routes/eras.rs`, `songs.rs`, `categories.rs`, `album_copies.rs` —
   declared in `routes/mod.rs`, so the crate will not build until they exist.
2. `src/main.rs` — still the `cargo new` hello world. Needs: config load → pool →
   `run_migrations` → `repair_era_duplicates` → `playable.refresh()` → router →
   bind → `println!("API listening on http://{host}:{port}")` (that exact line is
   the hub readiness pattern), SIGINT/SIGTERM graceful shutdown.
3. `src/lib.rs` — add `pub mod routes;`.
4. Middleware stack in `main.rs` (see below).
5. Transcode path in `stream?quality=` (semaphore + `ffmpeg … pipe:1` stream).
6. Seed the media fixture (above) and run the suite against Rust.

## Middleware: do NOT use `tower-http`'s `CorsLayer`

Probed against the running Node original:

| Case | Observed |
|---|---|
| no `Origin` | `access-control-expose-headers: X-Total-Count`, `vary: Origin` |
| allowed `Origin` | + `access-control-allow-origin: <echo>` |
| disallowed `Origin` | **no** `access-control-allow-origin` |
| `OPTIONS` (any path, even unknown) | `204` + `access-control-allow-methods: GET,HEAD,OPTIONS` + expose + ACAO + `vary: Origin` |
| `POST /health` | `404` JSON `{"error":"Not found"}` (not 405) |
| unknown path | `404` JSON `{"error":"Not found"}` |
| 400 from a route | `text/plain;charset=UTF-8`, body = message, no newline |
| `/eras/1/cover` with `Range` | `200` full body (covers ignore ranges) |

`CorsLayer` only emits CORS headers when `Origin` is present, and lowercases the
exposed header value to `x-total-count` — the test asserts `/X-Total-Count/`
case-sensitively. Hand-roll a `middleware::from_fn` instead (OPTIONS short-circuit
→ 204, else run inner and then attach headers).

Secure headers to reproduce (hono `secureHeaders` defaults, observed):
`cross-origin-opener-policy: same-origin`,
`cross-origin-resource-policy: cross-origin`,
`origin-agent-cluster: ?1`, `referrer-policy: no-referrer`,
`strict-transport-security: max-age=15552000; includeSubDomains`,
`x-content-type-options: nosniff`, `x-dns-prefetch-control: off`,
`x-download-options: noopen`, `x-frame-options: SAMEORIGIN`,
`x-permitted-cross-domain-policies: none`, `x-xss-protection: 0`.

Compression: hono compresses only when the content-type is in
`COMPRESSIBLE_CONTENT_TYPE_REGEX` (JSON/text yes, `image/avif` no) and either
`Content-Length` is absent or `>= 1024`; it drops `Content-Length` and prefixes
`W/` to `ETag`. `tower_http::compression::CompressionLayer` with a custom
`Predicate` covering our two served types (`application/json`, `text/*`) is
enough; note hyper sets `Content-Length` on full JSON bodies, so small JSON
stays uncompressed (accepted deviation, no test covers it).

## Gotchas already handled

* `escape '\'` in SQL must be written `"escape '\\'"` in Rust — the JS template
  literal `sql`'\\'`` collapses to a single backslash.
* `rank.rs` reproduces JS comparator `NaN` semantics: `sort` coerces NaN → `+0`
  (stable), but the max-heap's raw `>= 0` / `> 0` checks treat NaN as *false*.
  `compare_ranked` returns `f64` for exactly this reason.
* `Shared<BoxFuture<…>>` needs `duration_future(…).await.await` in `get_duration`.
* `isSafeFilename` accepts `.`/`..`; only `storedSongPath` rejects them.
* axum 0.8 uses `{id}` path syntax (the plan mentions axum 0.7 / `:id`); the crate
  is on axum 0.8.9 with `http-body` + `httpdate` added for the streaming/cover
  paths.
* `Query<HashMap<String, String>>` is the planned query extractor; note
  `?quality=` must be treated as falsy (JS `if (quality)`).

## Extra verification worth doing

Run both servers against fresh copies and diff the eras table after startup
(`SYNC_ON_START=false`), which isolates `repairEraDuplicates`:

```bash
sqlite3 /tmp/yt-test-node/db.sqlite3 "select * from eras order by id" > /tmp/eras-node.txt
sqlite3 /tmp/yt-test-rs/db.sqlite3   "select * from eras order by id" > /tmp/eras-rs.txt
diff /tmp/eras-node.txt /tmp/eras-rs.txt
```
