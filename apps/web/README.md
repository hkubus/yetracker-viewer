# Web app (`apps/web`)

The Ye Tracker viewer site: Astro 7 in server mode (`@astrojs/node`, standalone). Pages are rendered on the server
from the API (`API_INTERNAL_URL`); in the browser, search, covers and audio go straight to the API
(`PUBLIC_API_URL`). The API contract is in [`../../API.md`](../../API.md), the payload types in `packages/types`.

## Scripts

Run in this directory (`pnpm <script>`) or from the repo root (`pnpm --filter web <script>`; the root also has
`pnpm dev`, `pnpm build`, `pnpm start:web`).

| Script | What it does |
|---|---|
| `dev` | Astro dev server with HMR on `WEB_HOST:WEB_PORT` (default `127.0.0.1:4321`). Loads the repo-root `.env`. |
| `build` | Production build into `dist/`. |
| `start` | `node server.mjs`: the production server over `dist/` (run `build` first). Loads the repo-root `.env`. |
| `preview` | `astro preview` — the adapter's own server, without `server.mjs`'s compression and caching (it keeps public pages in memory for 10 s and compressed hashed assets for good). |
| `typecheck` | `astro check` (strict). |
| `test` | Unit tests: `node --test 'src/**/*.test.ts'`. |
| `lint` | `biome check .` |

Node ≥ 22.18 (`engines`): the unit tests run TypeScript through Node's type stripping.

## Configuration

Read at runtime, never baked into the build, so one build runs anywhere. `env.mjs` (next to `server.mjs`) validates
the values for `server.mjs` (which exits listing every problem before loading the app), for `src/config.ts` and for
the listen address of `astro dev`/`astro preview` (`astro.config.mjs`). Blank values count as unset everywhere. All
of them are also described in the repo-root `.env.example`.

| Variable | Default | Notes |
|---|---|---|
| `WEB_HOST`, `WEB_PORT` | `127.0.0.1`, `4321` | Fall back to `HOST`/`PORT`. An invalid port stops `server.mjs` and every astro command with a clear message. |
| `PUBLIC_API_URL` | dev: `http://localhost:3000` | API base URL for browsers: absolute http(s) URL or a same-origin path such as `/api`. Required in production. |
| `API_INTERNAL_URL` | dev: `http://127.0.0.1:3000` | Absolute API URL for SSR. Defaults to `PUBLIC_API_URL` when that is absolute; otherwise required in production. |
| `SITE_URL` | — | Public origin (`https://example.com`, no path). Optional, recommended in production. Canonical and `og:url` tags need it (they are left out without it); `robots.txt`, `sitemap.xml` and Open Graph image URLs use it, else the request's origin (see `TRUST_PROXY`), and such responses are then sent `Cache-Control: private`. `server.mjs` logs a warning at startup when it is unset or points at this machine. Its scheme has no effect on HSTS. |
| `TRUST_PROXY` | `false` | Trust the reverse proxy's `X-Forwarded-Proto` and `X-Forwarded-Host`. `X-Forwarded-Proto: https` then counts as an HTTPS request (HSTS and `upgrade-insecure-requests`, which otherwise need a TLS connection to `server.mjs`); while `SITE_URL` is unset the forwarded scheme and host (a plain `host[:port]`) make up the request's origin. Only behind a proxy that sets or overwrites these headers. |
| `SERVER_CERT_PATH`, `SERVER_KEY_PATH` | — | Both set: `server.mjs` serves HTTPS itself. |

## Pages

- `/` — `pages/index.astro`: catalog counts and "Updated … ago" (from `GET /status`), the era grid with an instant
  era filter, recently leaked playable songs, and the song search. `/eras`, the recent leaks and `/status` are fetched
  in parallel; if one fails the page says so and is sent with `Cache-Control: no-store` (a `404` from `/status`, as
  from an older API, doesn't count as a failure). Without the era list the page is only an apology, so it answers
  `503` with `Retry-After` (the API's, else 30 s); the friendly page and its "Try again" link still render.
- `/eras/:id` — `pages/eras/[id].astro`: era header (cover, subtitle, collapsible description), the song list (100
  songs per page, `?page=&q=&category=&sort=`), pagination and previous/next era links. A malformed id or unknown era
  → 404, a bad parameter → 400 page, a page past the end → `302` to the last page, an unreachable or failing API →
  503 with `Retry-After`. With a same-origin `PUBLIC_API_URL` and no `SITE_URL`, a page with a cover builds its
  Open Graph image URL from the request and is sent `private, max-age=60`.
- `/robots.txt`, `/sitemap.xml` (every era page; 503 while the API is down; `public, max-age=3600` with `SITE_URL`,
  else `private, max-age=3600`), `404`, `500` (a 503 variant for API outages). Error pages share
  `components/ErrorPage.astro`. The middleware sends the standard reason phrase with every status ("404 Not Found").

The home page search keeps its state in the URL — `/?q=<text>&eraFrom=<era id>&eraTo=<era id>&playable=true` (defaults
omitted; the era range compares era positions like the API) — so links, reloads and back/forward show the same search.
On load the address is rewritten in place (`replaceState`, no history entry) to that canonical form: unknown era ids,
bogus values and stray parameters are dropped, `playable=1` becomes `true`, `q` is trimmed and loses control characters
(`%00`), the hash is kept (skipped while the era list is unavailable). Filters alone (playable only, an era range) list
songs without a query; "Show more" pages with `offset`. A query counts when it has words or numbers (folded tokens) or
category markers (⭐ ✨ 🏆 🏅 🗑️ 🤖, sent to the API, which filters by them); anything else (`???`) shows a hint and sends
nothing. Results are also kept in `sessionStorage` (`yt:global-search`, 30 minutes) so back/forward restores them,
including extra pages and the scroll position.

## Source layout

```text
server.mjs               production server (see below)
env.mjs                  runtime env validation, shared by server.mjs, src/config.ts and astro.config.mjs
src/
  config.ts              server-only: runtime env, SSR API client (fetchApi/fetchJson, typed errors, 5 s timeout),
                         requestOrigin / isSecureRequest / originDependentCacheControl, PAGE_CACHE_CONTROL
  middleware.ts          CSP, security headers, HSTS for HTTPS requests, Cache-Control for pages, status lines
  layouts/Layout.astro   <head> (meta, <meta name="yt-api-url">, font), skip link, <main tabindex="-1">, global
                         CSS, footer, the player
  pages/                 routes (above)
  components/            Era, SongList, Pagination, GlobalSongSearch, RecentLeaks, Player, CoverImage, BackButton,
                         SiteFooter, ErrorPage, icons/ (inline SVG components)
  scripts/               client modules (below)
  utils/                 pure helpers, each with a *.test.ts next to it
  songSorts.ts, songCategories.ts   sort and category options of the song list; categoryMarkersIn() (+ test)
public/                  favicons, apple-touch-icon.png
```

Fonts: self-hosted Be Vietnam Pro (latin + latin-ext) through Astro's `<Font />`, weights 400, 600 and 700 only,
`font-display: optional` (a face that isn't there for the first render is not swapped in, so fonts never shift the
layout) and no preload.

Shared helpers in `src/utils/`: `search.ts` (`fold`, `tokens`, `matchesAllTokens` — the TS twin of the API's folding,
keep them identical; `normalizeQuery`/`clampQuery` for every query read from a URL or a search box: control characters
dropped, whitespace collapsed, the 100-character limit), `dates.ts` (catalog dates with day/month/year precision, always
UTC; relative times), `duration.ts`, `color.ts` (`themeFor`: contrast-safe colors from an era's dominant color for any
input), `cover.ts` (cover URLs, initials), `songRow.ts` and `songDisplay.ts` (turning API songs into rows, links,
play-button attributes), `globalSearchState.ts` (home search URL/API parameters, search terms incl. category markers),
`era-page.ts` (era page URLs, page math), `shared-api.ts` + `cache.ts` (server-only cached `/eras`, `/status` and recent leaks), `api-base-url.ts` (client-side API
URL from the meta tag), `back-navigation.ts` (back links that act like the browser's Back button). `env.test.ts` tests
`../../env.mjs`.

## Client scripts

All client code is a processed module in `src/scripts/`, imported from a `<script>` tag: it is bundled, minified and
served from `/_astro/*.js`. The CSP (`script-src 'self'`) forbids inline scripts and `on*=` attributes, and
`astro.config.mjs` stops Vite from inlining small scripts. Client code never imports `config.ts`; it reads the API URL
from `<meta name="yt-api-url">` (`utils/api-base-url.ts`).

With the ClientRouter each module runs once per document. Page scripts (`song-list.ts`, `global-search.ts`,
`home.ts`) set a page up on `astro:page-load` (and on the first load) and release everything they registered —
listeners, observers, timers, fetches — on `astro:before-swap` through one `AbortController` per page.

- `song-list.ts` — the era song list: instant filtering of the rendered page, clamped notes with "More" toggles, the
  extra-links menus, clean search URLs, focus after a search, centering of a deep-linked `#song-<id>` row on fresh
  navigations and after a same-page `#song-<id>` link (e.g. the player's era link). The instant filter matches like
  the API's era search: every folded word must occur in the song's own text (title cell, notes, quality,
  availability, sub-era — not the era's name), and typed category markers must all be in the title. Typing the
  query the server already answered hides nothing.
- `global-search.ts` — the home page search: debounced requests with timeouts and aborts, URL state (normalized on
  load), "Show more" (a superseded request never leaves it stuck), keyboard navigation (Enter, arrows, Escape), the
  era-range slider, the result cache.
- `home.ts` — the era filter and the "Updated … ago" refresh.
- `skip-link.ts` — "Skip to main content" focuses `<main>` instead of navigating to `#main-content` (no fetch, no
  hash, no history entry); one capture-phase listener for the whole session.
- `player/` — the site-wide player. `components/Player.astro` holds the markup and styles and is persisted across
  navigations (`transition:persist`), so playback continues from page to page. `index.ts` (entry, runs once),
  `controller.ts` (playback, queue, failure recovery, resume), `view.ts` (DOM, theme, live announcements),
  `targets.ts` (the DOM contract below), `track.ts` (track model, attribute and payload parsing), `queue.ts` (queue
  and continuation paging), `probe.ts` (`HEAD` probes that classify failures, `startStream()` to start a transcode,
  `waitForStream()` to wait until a live transcode reaches a position or is cached), `storage.ts` (`localStorage`
  keys `yetracker:quality|volume|muted|player`), `media-session.ts`, `keyboard.ts`, `capabilities.ts`, `format.ts`;
  tests in `*.test.ts`.

The era song list (`components/SongList.astro`) groups rows by sub-era; when a page starts in the middle of a
sub-era, its first header reads "… (continued)" (decided by one extra SSR request for the song before the page,
`limit=1`, 1.5 s timeout, only in catalog order after the first page). Row icons are CSS masks on the buttons and
links (no inline SVG per row). Notes are clamped to three lines from the first layout where scripts run
(`@media (scripting: enabled)`); browsers without that media feature clamp once `song-list.ts` marks the list
`data-enhanced`.

### Player behavior

- Quality: 128 kbps Opus transcodes by default when the browser plays Ogg Opus, otherwise the original file (and the
  quality control is hidden). When a transcode fails the player retries once after `Retry-After` if the API was busy,
  and otherwise switches this song to the original file with a short notice (the stored preference stays).
- Changing the quality mid-song keeps the current stream playing: the new transcode is started and polled with `HEAD`
  for up to 60 s, and the player switches at the current position once it is cached. If it isn't ready or available
  in time, the song stays on its stream, the quality menu is reverted and a notice says why (the new preference is
  kept for the next songs).
- Queue: a click takes a snapshot of the playable, visible songs of that list; Next and auto-advance continue past
  the page by fetching the following pages of the era (not when the list is filtered on the client: then the queue
  ends with the visible songs); songs that fail to load are skipped with a notice. When the next page can't be
  fetched, the player stays paused on the current song with a notice, and Next tries again.
- Seeking in a live (still running) transcode pauses playback and waits up to 15 s (polling `HEAD`) until the stream
  has loaded up to the target, or the transcode is cached (then the cached file is loaded at the target). If neither
  happens it never starts over: it plays on from where it was, or from as far toward the target as the stream has
  loaded, with a notice.
- A transcode that ends more than max(10 s, 5 %) before the song's length only counts as cut off when the file's real
  duration (`/songs/:id/duration`, fetched once per song) says so; then the song falls back to the original file.
- Resume: the current song, position and queue are saved; the next visit shows the bar paused there (no autoplay).
  Close forgets it.
- Keyboard: Space plays/pauses and ←/→ seek 5 s (not while typing or with Alt/Ctrl/Meta/Shift); on the seek slider
  ←/→ 5 s, PageUp/PageDown 30 s, Home/End.
- Media Session metadata and position are kept up to date. The tab title reads `▶ <title> – Ye Tracker` only while a
  song plays in a hidden tab: a visible page keeps its own title, since the browser files the current title under the
  history entry being left (Back/Forward included) and the router announces pages by their title. A navigation (from
  `astro:before-preparation` until 250 ms after `astro:page-load`) and `pagehide` keep the page's title too.

### DOM contract between song lists and the player

Lists never call the player; everything goes through attributes and one event:

- Play buttons: `button[data-play-target]` with `data-id` (required), `data-title` (display title),
  `data-era-id`, `data-era-name`, `data-era-position`, `data-track-length` (whole seconds, cut off rather than rounded
  like every displayed length: the probed duration, else the sheet's length; empty when unknown),
  `data-dominant-color` (6 hex digits), `data-has-cover` (`true|false`), `data-cover-version`; rendered with
  `aria-pressed="false"`. CSS shows a play or a pause icon by `aria-pressed` (a mask on `.song-action--play` in the
  era list, two inline SVG icons in the home page lists).
- Rows: `[data-play-row]` around each song (not around sub-era headers or empty states); rows with `hidden` are
  skipped. The player marks the current row `data-playing="true"` + `aria-current="true"` and sets its button to
  `aria-pressed="true"` / "Pause <title>" while playing.
- Queue scope: the nearest `[data-play-scope]`. Optional continuation attributes on it — `data-queue-era-id`,
  `data-queue-offset` (offset of the first row), `data-queue-total` (the API's `X-Total-Count`), `data-queue-params`
  (the `q`/`category`/non-default `sort` of the list) — let the player fetch the following pages from
  `/eras/:id/songs`. They are ignored while any row of the scope is `hidden` (a list filtered on the client): the
  continuation describes the unfiltered list.
- Events: `document` receives `yt:player-state` with `detail: { songId, state }`, `state` ∈ `playing`, `paused`,
  `stopped`, `error`.
- The player publishes its height as `--player-height` on `<html>` while visible (the layout pads the page with it).

## Production server (`server.mjs`)

`node server.mjs` validates the environment (and logs a `[web] warning:` line when `SITE_URL` is unset or points at
this machine), then serves `dist/` through the adapter's request handler with its own HTTP(S) server:

- brotli/gzip for text responses (HTML, CSS, JS, JSON, SVG…) of at least 1 KiB or of unknown length, with
  `Vary: Accept-Encoding`; strong ETags become weak when the body is compressed;
- `Cache-Control: public, max-age=31536000, immutable` for `/_astro/*`, one day for the files from `public/`; pages
  get `public, max-age=60, s-maxage=300, stale-while-revalidate=600` from the middleware (errors, redirects and pages
  rendered without their data: `no-store`);
- only `GET` and `HEAD` (anything else: 405); keep-alive 65 s;
- SIGTERM/SIGINT: stop accepting, let open requests finish for up to 10 s, then exit (a second signal exits at once);
  an uncaught exception drains the same way and exits 1.

The build is self-contained (`dist/server` imports only `node:*` modules), so at runtime the server needs only
`dist/`, `server.mjs` and `env.mjs` — no `src/`, no `node_modules` (plus `package.json` for `pnpm start`). Those
three, copied anywhere, are a complete deployment; start it with `node <dir>/server.mjs` from any working directory.
