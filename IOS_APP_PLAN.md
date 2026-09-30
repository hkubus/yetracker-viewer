# iOS App Plan — `apps/ios` (SwiftUI)

> Status: M0–M6 implemented. `YeTrackerKit` builds and its 125 tests pass on
> Linux (Swift 6.1), 5 of them against a live API. The API change is covered by
> Rust unit tests and the contract suite (47/47). The SwiftUI/AVFoundation app
> target has **not been compiled yet**: it was written on Linux, where SwiftUI,
> AVFoundation and MediaPlayer do not exist. Its project file was validated with
> tuist/XcodeProj. The first Xcode build is the first task of M7 (§9).

Goal: a native iPhone/iPad client with **feature parity with `apps/web`**,
talking to the same public, read-only API (`apps/api-rs`). The web app and the
API contract stay authoritative. The only server change is one additive,
backward-compatible option on `/songs/:id/stream` (§5), which AVPlayer needs.

## 1. Constraints that shape the design

| Constraint | Consequence |
|---|---|
| API is GET-only, public, read-only (`AGENTS.md`) | No accounts or sync; all state is local (UserDefaults). |
| Covers are served as **AVIF** | iOS 16+ decodes AVIF through ImageIO, so the minimum is **iOS 17** (also required for `@Observable`). |
| `?quality=N` transcodes to **Ogg/Opus**, chunked, `Accept-Ranges: none` | AVPlayer cannot play Ogg at all. The server gains `format=aac` (ADTS AAC) plus `start=<sec>` so a live transcode can be "seeked" by restarting it at an offset (§5). |
| YouTube/Instagram/Twitter media is stored as **`.ogg` (Opus)** by `yt-dlp` | "Original" fails in AVPlayer for these files. The player falls back to an AAC transcode automatically. |
| Dates are stored as **UTC midnight** (`importer.rs::parse_date`) | Dates are formatted in UTC, or they would show the previous day west of Greenwich. |
| Development happens on Linux (Swift 6.1 toolchain, no Xcode) | All logic lives in a Swift package that compiles and is tested on Linux. The app target only holds views and Apple-framework glue. |

## 2. Feature inventory: web → iOS

Every row is a web behaviour (source file in parentheses) and where it lives on iOS.

### Home (`pages/index.astro`, `Era.astro`, `RecentLeaks.astro`)
| Web | iOS |
|---|---|
| Header "Ye Tracker", "N eras · M tracks" (sum of `songsCount`) | Home tab large title + subtitle (`HomeModel.headerSummary`) |
| Catalog failure notice + "Try again", page not cached | Full-screen error state with Retry; pull-to-refresh bypasses the HTTP cache |
| Era cards: cover (`?v=coverVersion`), name, "1 song"/"N songs", dominant-colour tints | `EraCard` in an adaptive `LazyVGrid`; cover cache keyed by `coverVersion` |
| Era name filter (only when > 3 eras), "X of Y eras", empty message | `.searchable("Filter eras…")`, same count text and empty state |
| "Recently leaked": `/songs?playable=true&sort=leak-newest&limit=8`, title deep-links into the era, meta "Era • Sep 30, 2026", play button, its own play scope; hidden on failure | "Recently leaked" section. Tapping the row plays; the era button deep-links. The queue is those 8 tracks. Hidden on failure. |

### Global track search (`GlobalSongSearch.astro`)
| Web | iOS |
|---|---|
| Debounced (150 ms), aborts stale requests, 10 s timeout, ignores out-of-date responses | `GlobalSearchModel`: 150 ms debounce, task cancellation plus a request-signature guard, 10 s timeout |
| Only searches when the text is non-empty; filters refine a text search | Same |
| Era range: two-thumb slider over the ordered era list, gradient between the two eras' colours, names under the thumbs, "All eras / 1 era / N eras", `eraFrom`/`eraTo` only when narrowed | `EraRangeSlider` (custom two-thumb control, colour gradient, VoiceOver adjustable per thumb) |
| "Playable only" → `playable=true` | Toggle |
| Clear (disabled when there are no filters) | Clear button with the same enablement |
| Count: "N songs" idle, "Searching…", "X of Y matches" (> 50), "N match(es)", "Search failed" | Same strings (`GlobalSearchModel.countLabel`) |
| Result card: title, era name, notes truncated to 120 chars, era colour; links to `/eras/:id?page=⌈eraPosition/100⌉#song-:id` | Result row pushes the era screen focused on that song (loads through `eraPosition`, scrolls and highlights). A context menu adds Play when `playable`. |
| Empty: "No songs match your search." / "Search is temporarily unavailable." | Same |

### Era page (`pages/eras/[id].astro`, `SongList.astro`, `Pagination.astro`, `BackButton.astro`)
| Web | iOS |
|---|---|
| Back to all eras | Navigation back button |
| Cover, name, description, notes (pre-line, scrollable) with era tints | `EraHeaderView`; long notes collapse with More/Less |
| Previous/next era links (from `/eras` order; failure disables them) | ‹ › buttons that replace the current era in place |
| 100 songs per page, "Showing X–Y of Z songs", Prev/Next/numbered pages, out-of-range page → last page | Infinite scroll in 100-song pages (`X-Total-Count`), footer "Showing X of Z songs"; a deep-link page/position loads through the target |
| Server-side `q` / `category` / `sort` form, "Clear all filters" | `.searchable` (debounced server query) plus a toolbar menu with Category and Sort pickers and "Clear all filters" |
| Typing also filters the current page instantly; "X of Y on this page…"; empty hint | The loaded rows are filtered instantly while the server query is in flight |
| Row actions: play (if playable), download original, else source link, else "—" with the reason | Play and download buttons, a source link otherwise, else "—". Context menu (Download, Open Source, Copy Title) and the VoiceOver hint carry the unavailable reason |
| Title, notes (3-line clamp + More/Less), quality, "Snippet - 1:05", file date, leak date (short) | `SongRow` with expandable notes and a metadata line |
| Playing row highlight "♪" | Highlight plus an equalizer glyph |
| `#song-ID` target highlight, centred above the player | `ScrollViewReader` scroll-to-centre plus a highlight |
| Unknown era → branded 404; API failure → 500 page | "That era does not exist" / "Something went wrong" states with Retry and Back |

### Player (`Player.astro`)
| Web | iOS |
|---|---|
| Persistent player across navigation | Mini player inset above the tab bar in every tab; tap for the full Now Playing sheet |
| Cover, state line ("Now playing" / "Buffering…" after 500 ms / "Retrying original…"), title, era link | Same state line. The era link switches to Home and pushes the era focused on the track. |
| Prev / Play-Pause / Next walk the list the track came from; disabled at the ends; auto-advance at the end | `PlayerModel` queue snapshot from the originating list, extended when that list loads more pages |
| Seekable progress bar, elapsed/duration labels | Slider plus labels. Native seek for the original file; restart-at-offset for transcodes. |
| Volume slider (persisted `yetracker:volume`) | Volume slider (AVPlayer volume, persisted) plus an AirPlay route picker |
| Quality: Original/64/128/192/256/320 kbps, default 128, persisted | Same options and default, persisted |
| Mid-song quality switch resumes at the same position; on failure restores the previous stream: "Quality switch failed. Restored the previous stream." | Same |
| Transcode failure → "Retrying original…" | Same, plus the reverse: an unsupported original (Ogg/Opus) → "Converting for playback…" via AAC |
| Error "Audio failed to load…" + Retry; "Cannot play this song: missing song id." | Same |
| Duration: catalog `duration`/`trackLength`, else `GET /songs/:id/duration` | Same, overridden by the engine's duration when the stream knows it |
| Media Session: title/artist/album/artwork; play, pause, seek ±, seek-to, next, previous | `MPNowPlayingInfoCenter` + `MPRemoteCommandCenter`, background audio, interruption handling |
| Keyboard: Space toggles, ←/→ seek ±5 s | iPad hardware-keyboard shortcuts |

### Cross-cutting
| Web | iOS |
|---|---|
| `PUBLIC_API_URL` (may carry a path prefix such as `/api`) | Settings → Server URL (validated, path prefixes kept, "Test connection" via `/health`). The default comes from the `YT_API_BASE_URL` build setting. |
| Download original (`/songs/:id/download`, `Content-Disposition` name) | Download with progress, then the share sheet (Save to Files, AirDrop…) |
| Source links open in a new tab | `SFSafariViewController` |
| Dark UI, `#181818`, `color-mix` tints in sRGB/OKLab | Same palette. `RGBColor.mix(_:_:in:)` in Kit implements both spaces. |
| a11y: labels, live counts, reduced motion | VoiceOver labels/values, adjustable controls, Dynamic Type, Reduce Motion |
| Security headers/CSP, edge caching | N/A on iOS. `URLCache` honours the API's `Cache-Control`. |

## 3. Architecture

```
apps/ios/
  README.md                    build/run/config/testing
  Config/Shared.xcconfig       bundle id, API default URL (+ optional Local.xcconfig, gitignored)
  YeTracker.xcodeproj          hand-written, Xcode 16+ synchronized folders
  YeTracker/                   app target (SwiftUI, iOS 17+)
    App/        YeTrackerApp, AppModel (composition root), Router, RootView
    Playback/   AVPlayerEngine (PlaybackEngine + audio session), NowPlayingController
    Views/      Home/, Search/, Era/, Player/, Settings/, Components/
    Support/    Theme, ImagePipeline (+ CoverImage), DownloadCenter, Presenter (Safari, share sheet)
  YeTrackerKit/                Swift package, Foundation + Observation only
    Sources/YeTrackerKit/
      Models/       Era, EraSong, SearchSong, SongCategory, SongSort, PlaybackQuality
      Networking/   APIClient, APIError, HTTPClient, QueryEncoding
      Formatting/   duration, dates (UTC), text normalisation, RGBColor (sRGB/OKLab mix)
      Routing/      AppRoute, DeepLink parser (yetracker:// and web-style URLs)
      Settings/     AppSettings over a KeyValueStore
      Features/     HomeModel, GlobalSearchModel, EraDetailModel, EraDirectory
      Playback/     PlayerModel, PlaybackEngine, NowPlayingSink, Track, queue
    Tests/YeTrackerKitTests/   swift-testing; fake HTTP + fake engine; JSON fixtures from the real API
```

* **State**: `@Observable @MainActor` models in Kit. Views own them with
  `@State` and read shared services (`AppModel`, `PlayerModel`, `Router`) from
  the environment. No Combine.
* **Navigation**: `TabView` (Home, Search, Settings), each with a
  `NavigationStack` bound to a `[AppRoute]` path owned by `Router`. Deep links
  are parsed in Kit and resolved by `Router`.
* **Networking**: `APIClient` builds URLs the same way as the web's `apiUrl()`
  (keeps a base path prefix). Queries are percent-encoded so `+` stays `%2B`
  (axum decodes `+` as a space). It classifies `text/plain` route errors,
  `{"error":…}` JSON, and transport errors (including ATS) into `APIError` with
  user-facing copy. The 10 s timeout matches the web.
* **Playback**: `PlayerModel` (Kit) owns the queue, quality, fallbacks, the
  buffering hint, duration resolution and Now Playing data. It drives a
  `PlaybackEngine` protocol; the app implements it with `AVPlayer`
  (KVO → events). The whole state machine is unit-tested on Linux with a fake
  engine.
* **Images**: an `NSCache`-backed loader over a `URLSession` with a 100 MB
  disk `URLCache`. Cover URLs carry `?v=coverVersion`, so entries never go
  stale (the API sends `immutable`).

## 4. API usage (unchanged contract)

| Call | Used by |
|---|---|
| `GET /eras` | Home grid, search era range, era neighbours (shared `EraDirectory` cache) |
| `GET /eras/:id` | Era header |
| `GET /eras/:id/songs?limit&offset&q&category&sort` (+ `X-Total-Count`) | Era song list and paging |
| `GET /eras/:id/cover?v=` | Cards, header, player, lock-screen artwork |
| `GET /songs?q&eraFrom&eraTo&playable&limit=50` | Global search |
| `GET /songs?playable=true&sort=leak-newest&limit=8` | Recently leaked |
| `GET /songs/:id/stream[?quality&format=aac&start]` | Playback |
| `GET /songs/:id/duration` | Duration fallback |
| `GET /songs/:id/download` | Download original |
| `GET /health` | Settings → Test connection |

## 5. Server addition: AVPlayer-compatible transcodes

`GET /songs/:id/stream` gains two optional parameters. Both apply only when
`quality` is present. Existing requests behave exactly as before.

| Param | Values | Effect |
|---|---|---|
| `format` | `opus` (default) \| `aac` | `aac` → `ffmpeg … -vn -f adts -c:a aac -b:a {q}k`, `Content-Type: audio/aac`. Anything else → `400 Invalid format`. |
| `start` | seconds, `^\d+(\.\d+)?$`, ≤ 86400 | `-ss {start}` before `-i` (fast input seek). Invalid → `400 Invalid start`. |

AVPlayer plays ADTS over chunked HTTP the way it plays Icecast radio: no
ranges, indefinite duration. Seeking a transcode restarts it with `start`,
and the UI shows `start + currentTime`. This is the same pattern Subsonic uses
with `timeOffset`. Covered by Rust unit tests and new contract tests; `API.md`
is updated.

## 6. Playback state machine (PlayerModel)

```
play(track, queue) ─► load(stream(quality)) ──ready──► playing ◄─► paused
        │                    │ failed
        │                    ├─ transcode failed  → "Retrying original…"      → load(original @pos)
        │                    ├─ original failed and no transcode tried yet
        │                    │                    → "Converting for playback…" → load(aac 256k @pos)
        │                    └─ otherwise         → error "Audio failed to load…" [Retry]
        ├─ quality switch mid-song → load(new @pos); on failure restore the previous source + error
        ├─ seek: original → engine.seek; transcode → reload with start=target
        ├─ waiting > 500 ms → "Buffering…"
        ├─ transcode waiting within 1.5 s of the end for 2 s → treated as ended
        │   (AVPlayer does not always report the end of a length-less stream)
        └─ ended → next in queue, else stop
```

## 7. UI

* Dark only (`#181818`). Each era's `dominantColor` drives the tints with the
  same `color-mix` ratios as the web (card 25 % fill / 70 % border, text 30 %
  toward white in OKLab, player 40 % over the background).
* iPhone: two-column era grid, card-style song rows. iPad: the adaptive grid
  widens and all screens use readable-width layouts.
* Mini player: 64 pt bar with cover, title/state, play/pause and next, plus a
  thin progress line. It steps aside while the software keyboard is up.
* One window on iPad (`UIApplicationSupportsMultipleScenes = NO`): the player,
  tabs and navigation are app-wide state, and a second window would mirror them. The full sheet has large artwork, the era link,
  scrubber, transport, volume, AirPlay, quality and errors.

## 8. Testing strategy

* `YeTrackerKit`: `swift test` on Linux (Swift 6.1) and macOS/Xcode. Coverage:
  JSON decoding against fixtures captured from a real import, URL/query
  building, formatting, colour maths, deep links, settings, and every feature
  model with a fake `HTTPClient`/`PlaybackEngine` and an injectable sleeper.
  The Linux toolchain needs `-Xlinker --allow-shlib-undefined` (its
  `libswiftObservation.so` references an unexported symbol). See the README.
* A live smoke test (`YT_LIVE_API_URL=http://127.0.0.1:3100`) runs the real
  client against a running API.
* API: `cargo test --lib` plus the contract suite for the new stream options.
* App target: first build and manual QA in Xcode 16+ (checklist in the README).

## 9. Milestones

- [x] **M0** Plan (this document)
- [x] **M1** Kit foundation: models, API client, formatting, colour, routing, settings, tests
- [x] **M2** Kit features: home, search, era detail, player state machine, tests
- [x] **M3** API: `format=aac` + `start` on `/stream`, Rust tests, contract tests, `API.md`
- [x] **M4** App shell: Xcode project, xcconfig, Info.plist (background audio, ATS local networking, URL scheme), assets, composition root, theme, image cache
- [x] **M5** Screens: Home, Search (+ era range slider), Era detail, mini player + Now Playing, Settings, downloads
- [x] **M6** System integration: AVPlayer engine, lock screen/remote commands, audio session interruptions, keyboard shortcuts, deep links
- [ ] **M7** First Xcode build and fixes, device QA against a real server (AAC live stream, lock screen, AirPlay, downloads), app icon polish

## 10. Risks and open questions

* **Unbuilt UI layer**: expect small compile fixes on the first Xcode build.
  Logic bugs should be rare because the models are tested.
* **AAC live streams in AVPlayer**: the Icecast precedent says this works. If
  it doesn't, the player degrades to "Original" automatically, and the
  fallback is to download the transcode to a temp file before playing.
* **Transcode capacity** (`MAX_CONCURRENT_TRANSCODES`, default 2): a `503`
  falls back to the original file, exactly like the web.
* **ATS**: `NSAllowsArbitraryLoads`, because the server is user-chosen (plain
  HTTP to any host works). An App Store build would have to narrow this.
