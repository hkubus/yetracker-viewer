# iOS App Plan — `apps/ios` (SwiftUI)

> Status: M0–M6 done, plus CI/SideStore distribution, the iOS 26 redesign
> (iOS/Xcode 26.1+, Liquid Glass) and playback/caching work (§9). CI
> builds an unsigned IPA on every change (§8); `ios-v1.0.0` tags `b22262b`,
> from before the redesign. Left (M7): on-device QA, light mode (still forced
> off, §7) and two App Store–only items.

Goal: a native iPhone/iPad client with **feature parity with `apps/web`**,
talking to the same public, read-only API (`apps/api-rs`). The web app and the
API contract stay authoritative. The only server change made for iOS is
additive and backward-compatible: `format=aac` and `start` on
`/songs/:id/stream` (§5), which AVPlayer needs.

## 1. Constraints that shape the design

| Constraint | Consequence |
|---|---|
| API is GET-only, public, read-only (`AGENTS.md`) | No accounts or sync; all state is local (UserDefaults). |
| Covers are served as **AVIF** | ImageIO decodes AVIF from iOS 16. The app's minimum is **iOS 26.1** (Liquid Glass, the search tab, `tabViewBottomAccessory(isEnabled:)`); `YeTrackerKit` itself declares iOS 17 / macOS 14. |
| `?quality=N` transcodes to **Ogg/Opus**; a live transcode is chunked with `Accept-Ranges: none` | AVPlayer cannot play Ogg at all. The server gained `format=aac` (ADTS AAC) plus `start=<sec>`, so a live transcode can be "seeked" by restarting it at an offset (§5). Finished transcodes are cached and served with ranges. |
| YouTube/Instagram/Twitter media is stored as **`.ogg` (Opus)** by `yt-dlp` | "Original" fails in AVPlayer for these files. The player falls back to an AAC transcode automatically. |
| Dates are stored as **UTC midnight** (`importer.rs::parse_date`) | Dates are formatted in UTC, or they would show the previous day west of Greenwich. |
| Development happens on Linux (Swift 6.1 toolchain, no Xcode) | All logic lives in a Swift package that compiles and is tested on Linux. The app target only holds views and Apple-framework glue, and builds on macOS (CI or Xcode). |

## 2. Feature inventory: web → iOS

Every row is a web behaviour (source file in parentheses) and where it lives on iOS.

### Home (`pages/index.astro`, `Era.astro`, `RecentLeaks.astro`)
| Web | iOS |
|---|---|
| Header "Ye Tracker", "N eras · M tracks" (sum of `songsCount`) | Eras tab: large title plus subtitle (`HomeModel.headerSummary`) |
| Catalog failure notice + "Try again", page not cached | Inline "The catalog could not be loaded." notice with Try Again; pull-to-refresh revalidates the HTTP cache |
| Era cards: cover (`?v=coverVersion`), name, "1 song"/"N songs", dominant-colour tints | `EraCard` (cover, name, count) in an adaptive `LazyVGrid`; cover URLs carry `?v=` |
| Era name filter (only when > 3 eras), "X of Y eras", empty message | `.searchable` "Filter eras" (the count shows with > 3 eras), same count text and empty message |
| "Recently leaked": `/songs?playable=true&sort=leak-newest&limit=8`, title deep-links into the era, meta "Era • Sep 30, 2026", play button, its own play scope; hidden on failure | "Recently Leaked" section. Tapping the row plays; the chevron opens the era at the song. The queue is those 8 tracks. Hidden on failure and while filtering eras. |

### Global track search (`GlobalSongSearch.astro`)
| Web | iOS |
|---|---|
| Debounced (200 ms), aborts stale requests, 10 s timeout, ignores out-of-date responses | `GlobalSearchModel`: 150 ms debounce, task cancellation plus a request-signature guard, 10 s timeout. The field is the tab bar's search tab. |
| Only searches when the text is non-empty; filters refine a text search | Same |
| Era range: two-thumb slider over the ordered era list, gradient between the two eras' colours, names under the thumbs, "All eras / 1 era / N eras", `eraFrom`/`eraTo` only when narrowed | `EraRangeSlider` (custom two-thumb control, colour gradient, VoiceOver adjustable per thumb) |
| "Playable only" → `playable=true` | Toggle |
| Clear (disabled when there are no filters) | "Clear Filters" with the same enablement |
| Count: "N songs" idle, "Searching…", "X of Y matches" (> 50), "N match(es)", "Search failed" | Same strings (`GlobalSearchModel.countLabel`) |
| Result card: title, era name, notes truncated to 120 chars, era colour; links to `/eras/:id?page=⌈eraPosition/100⌉#song-:id` | Result row pushes the era screen focused on that song (loads through `eraPosition`, scrolls and highlights). A context menu adds Play when `playable`, and Show in Era. |
| Empty: "No songs match your search." / "Search is temporarily unavailable." | Same |

### Era page (`pages/eras/[id].astro`, `SongList.astro`, `Pagination.astro`, `BackButton.astro`)
| Web | iOS |
|---|---|
| Back to all eras | Navigation back button |
| Cover, name, description, notes (pre-line, scrollable) with era tints | `EraHeaderView` over an era-colour wash; long notes collapse with More/Less |
| Previous/next era links (from `/eras` order; failure disables them) | Glass buttons with the neighbour's name that replace the current era in place |
| 100 songs per page, "Showing X–Y of Z songs", Prev/Next/numbered pages, out-of-range page → last page | Infinite scroll in 100-song pages (`X-Total-Count`), footer "Showing X of Z songs"; a deep-link page/position loads through the target |
| Server-side `q` / `category` / `sort` form, "Clear all filters" | `.searchable` (300 ms debounced server query) plus a toolbar menu with Category and Sort pickers and "Clear all filters"; active filters show as removable chips |
| Typing also filters the current page instantly; "X of Y on this page…"; empty hint | The loaded rows are filtered instantly while the server query is in flight |
| Row actions: play (if playable), download original, else source link, else "—" with the reason | Play and download buttons, a source link otherwise, else "—". Context menu (Download Original, Open Source, Copy Title) and the VoiceOver hint carry the unavailable reason |
| Title, notes (3-line clamp + More/Less), quality, "Snippet - 1:05", file date, leak date (short) | `SongRow` with expandable notes and a metadata line |
| Playing row highlight "♪" | Era-colour row fill plus an animated waveform glyph |
| `#song-ID` target highlight, centred above the player | `ScrollViewReader` scroll-to-centre plus a highlight |
| Unknown era → branded 404; API failure → 500 page | "That era does not exist" (Back to all eras) / "Something went wrong" (Try again) |

### Player (`Player.astro`)
| Web | iOS |
|---|---|
| Persistent player across navigation | Mini player in the tab bar's bottom accessory (inline while the tab bar is minimized); tap for the Now Playing sheet, which zooms out of it |
| Cover, state line ("Now playing" / "Buffering…" after 500 ms / "Retrying original…"), title, era link | Same state line. The era link closes the sheet and pushes the era, focused on the track, in the current tab (Eras from Settings). |
| Prev / Play-Pause / Next walk the list the track came from; disabled at the ends; auto-advance at the end | `PlayerModel` queue snapshot from the originating list, extended when that list loads more pages |
| Seekable progress bar, elapsed/duration labels | Slider plus labels. Native seek for the original and for a finished transcode; a live transcode restarts at the offset. |
| Volume slider (persisted `yetracker:volume`) | Volume slider (AVPlayer volume, persisted) plus an AirPlay route picker |
| Quality: Original/64/128/192/256/320 kbps, default 128, persisted | Same options and default, persisted; also in Settings → Playback |
| Mid-song quality switch resumes at the same position; on failure restores the previous stream: "Quality switch failed. Restored the previous stream." | Same |
| Transcode failure → "Retrying original…" | Same, plus the reverse: an unsupported original (Ogg/Opus) → "Converting for playback…" via AAC |
| Error "Audio failed to load…" + Retry; "Cannot play this song: missing song id." | The first, with Retry (iOS tracks always have an id) |
| Duration: catalog `duration`/`trackLength`, else `GET /songs/:id/duration` | Same, overridden by the engine's duration for the original file |
| Media Session: title/artist/album/artwork; play, pause, seek ±, seek-to, next, previous | `MPNowPlayingInfoCenter` + `MPRemoteCommandCenter` (no ± skip, which would replace next/previous on the lock screen), background audio, interruption handling |
| Keyboard: Space toggles, ←/→ seek ±5 s | iPad hardware keyboard: the same, plus ⌘←/⌘→ for tracks |

### Cross-cutting
| Web | iOS |
|---|---|
| `PUBLIC_API_URL` (may carry a path prefix such as `/api`) | Settings → Server URL (validated, path prefixes kept, "Test Connection" via `/health` and `/eras`, "Use Default"). The default comes from the `YT_API_BASE_URL` build setting. |
| Download original (`/songs/:id/download`, `Content-Disposition` name) | Download with progress, then the share sheet (Save to Files, AirDrop…) |
| Source links open in a new tab | `SFSafariViewController` |
| Dark UI, `#181818`, `color-mix` tints in sRGB/OKLab | System light/dark colours, era tints from `EraPalette` (`RGBColor.mix` in sRGB/OKLab); see §7 |
| a11y: labels, live counts, reduced motion | VoiceOver labels/values, adjustable controls, Dynamic Type, Reduce Motion |
| Security headers/CSP, edge caching | N/A on iOS. `URLCache` honours the API's `Cache-Control`. |

## 3. Architecture

```
apps/ios/
  README.md                    build/run/config/testing, manual QA checklist
  Config/Shared.xcconfig       bundle id, version, API default URL (+ optional Local.xcconfig, gitignored)
  Config/Info.plist            background audio, ATS, URL scheme, one scene
  YeTracker.xcodeproj          hand-written, synchronized folders (object version 77)
  YeTracker/                   app target (SwiftUI, iOS 26.1+)
    App/        YeTrackerApp, AppModel (composition root), RootView (tabs, mini player, Now Playing)
    Playback/   AVPlayerEngine (PlaybackEngine + audio session), NowPlayingController
    Views/      Home/, Search/, Era/, Player/, Settings/, Components/
    Support/    Theme (EraPalette), ImagePipeline (+ CoverImage), DownloadCenter, KeyboardObserver, Presenter
    Assets.xcassets            AppIcon (one 1024 px PNG), AccentColor, LaunchBackground
  YeTrackerKit/                Swift package, Foundation + Observation only
    Sources/YeTrackerKit/
      Models/       Era, EraSong, SearchSong, SongCategory, SongSort, PlaybackQuality
      Networking/   APIClient, APIError, HTTPClient, QueryEncoding
      Formatting/   duration, dates (UTC), text normalisation, RGBColor (sRGB/OKLab mix)
      Routing/      AppRoute, DeepLink parser (yetracker:// and web-style URLs), Router
      Settings/     AppSettings over SettingsStorage (UserDefaults)
      Features/     HomeModel, GlobalSearchModel, EraDetailModel, EraDirectory
      Playback/     PlayerModel, PlaybackEngine, NowPlayingSink, Track, queue
      Support/      lenient decoding, injectable sleeper
    Tests/YeTrackerKitTests/   swift-testing; fake HTTP + fake engine; JSON fixtures from the real API
```

* **State**: `@Observable @MainActor` models in Kit. Views own them with
  `@State` and read `AppModel` (router, player, shared models) from the
  environment. No Combine.
* **Navigation**: `TabView` with Eras, Settings and a Search tab
  (`role: .search`). Eras and Search each have a `NavigationStack` bound to a
  `[AppRoute]` path owned by `Router`. Deep links are parsed in Kit and
  resolved by `Router`.
* **Networking**: `APIClient` builds URLs the same way as the web's `apiUrl()`
  (keeps a base path prefix). Queries are percent-encoded so `+` stays `%2B`
  (axum decodes `+` as a space). It classifies `text/plain` route errors,
  `{"error":…}` JSON, and transport errors (including ATS) into `APIError` with
  user-facing copy. The 10 s timeout matches the web. Pull-to-refresh
  revalidates cached responses (`ETag`/`Last-Modified` → `304`).
* **Playback**: `PlayerModel` (Kit) owns the queue, quality, fallbacks, the
  buffering hint, duration resolution, finished-transcode handoff and Now
  Playing data. It drives a `PlaybackEngine` protocol; the app implements it
  with `AVPlayer` (KVO → events). The whole state machine is unit-tested on
  Linux with a fake engine.
* **Images**: an `NSCache` of decoded covers over a `URLSession` with its own
  150 MB disk `URLCache`; views showing the same cover share one download.
  Cover URLs carry `?v=coverVersion`, so entries never go stale (the API sends
  `immutable` for them).

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
| `HEAD /songs/:id/stream?quality&format=aac` | Is the whole-song transcode finished? (`TranscodeState`) |
| `GET` of the same, cancelled after the headers | Starts a transcode nobody reads (a seeked stream, the next song) |
| `GET /songs/:id/duration` | Duration fallback |
| `GET /songs/:id/download` | Download original |
| `GET /health` + `GET /eras` | Settings → Test Connection |

## 5. Server addition: AVPlayer-compatible transcodes

`GET /songs/:id/stream` has two optional parameters for iOS. Both apply only
when `quality` is present. Requests without them behave as before.

| Param | Values | Effect |
|---|---|---|
| `format` | `opus` (default) \| `aac` | `aac` → `ffmpeg … -c:a aac -f adts -b:a {q}k`, `Content-Type: audio/aac`. Anything else → `400 Invalid format`. |
| `start` | seconds, plain decimal with ≤ 3 decimals, `0`–`86400` | `-ss {start}` before `-i` (fast input seek). Invalid → `400 Invalid start`. |

AVPlayer plays a live ADTS stream like Icecast radio: no ranges, indefinite
duration. Seeking one restarts it with `start`, and the UI shows
`start + currentTime` (Subsonic's `timeOffset` pattern). A finished whole-song
transcode is cached and served like a stored file (ranges, seekable); a
`format=aac&start=` request on it is cut from the cached file without ffmpeg.
Covered by Rust tests and the contract suite; details in `API.md`.

## 6. Playback state machine (PlayerModel)

```
play(track, queue) ─► load(stream(quality)) ──ready──► playing ◄─► paused
        │                    │ failed
        │                    ├─ transcode failed  → "Retrying original…"      → load(original @pos)
        │                    ├─ original failed and no transcode tried yet
        │                    │                    → "Converting for playback…" → load(aac 256k @pos)
        │                    └─ otherwise         → error "Audio failed to load…" [Retry]
        ├─ quality switch mid-song → load(new @pos); on failure restore the previous source + error
        ├─ seek: original or finished transcode → engine.seek;
        │        live transcode → reload with start=target once seeks pause for 300 ms
        ├─ live transcode → poll HEAD every 2 s until it is finished, then reload the finished
        │   file at the same position while nothing is heard (paused, stalled, not started)
        ├─ playing a finished transcode → transcode the next song in the queue in advance
        ├─ waiting > 500 ms → "Buffering…"
        ├─ live transcode waiting within 1.5 s of the end for 2 s → treated as ended
        │   (AVPlayer does not always report the end of a length-less stream)
        └─ ended → next in queue, else stop
```

## 7. UI

* iOS 26 design: Liquid Glass buttons, chips and banners; a tab bar that
  minimizes on scroll; Search as the tab bar's search tab; the mini player as
  the tab bar's bottom accessory; a Now Playing sheet that zooms out of it.
* Light and dark: system colours, plus `EraPalette`, which turns each era's
  `dominantColor` into tints (mixed toward white in dark, black in light, in
  OKLab) and translucent washes and fills. Now Playing is always dark, with
  the web player's `color-mix` (40 % accent over `#181818`).
  **Light mode never shows yet**: both target configurations in
  `project.pbxproj` still set `INFOPLIST_KEY_UIUserInterfaceStyle = Dark`,
  left over from the dark-only version (M7).
* iPhone: adaptive era grid (two columns on most phones), list rows. iPad: the
  grid widens and the era header goes side by side.
* One window on iPad (`UIApplicationSupportsMultipleScenes = NO`): the player,
  tabs and navigation are app-wide state, and a second window would mirror
  them. The Now Playing sheet has large artwork, the era link, scrubber,
  transport, volume, AirPlay, quality and errors.

## 8. Testing strategy

* `YeTrackerKit`: `swift test` on Linux (Swift 6.1) and macOS/Xcode. Coverage:
  JSON decoding against fixtures captured from a real import, URL/query
  building, formatting, colour maths, deep links, settings, and every feature
  model with a fake `HTTPClient`/`PlaybackEngine` and an injectable sleeper.
  The Linux toolchain needs `-Xlinker --allow-shlib-undefined` (its
  `libswiftObservation.so` references an unexported symbol). See the README.
* Live smoke tests (`YT_LIVE_API_URL=http://127.0.0.1:3000`) run the real
  client against a running API.
* API: `cargo test` plus the contract suite for the stream options.
* CI (`.github/workflows/ios.yml`, GitHub mirror only: Forgejo has no macOS
  runner): Kit tests, then an unsigned Release IPA built with Xcode 26. A tag
  `ios-v<version>` also makes a GitHub release and deploys the SideStore
  source (`scripts/sidestore-source.mjs`) to GitHub Pages.
* App target: no UI tests; on-device QA uses the README's checklist (M7).

## 9. Milestones

- [x] **M0** Plan (this document)
- [x] **M1** Kit foundation: models, API client, formatting, colour, routing, settings, tests
- [x] **M2** Kit features: home, search, era detail, player state machine, tests
- [x] **M3** API: `format=aac` + `start` on `/stream`, Rust tests, contract tests, `API.md`
- [x] **M4** App shell: Xcode project, xcconfig, Info.plist (background audio, ATS, URL scheme), assets, composition root, theme, image cache
- [x] **M5** Screens: Home, Search (+ era range slider), Era detail, mini player + Now Playing, Settings, downloads
- [x] **M6** System integration: AVPlayer engine, lock screen/remote commands, audio session interruptions, keyboard shortcuts, deep links
- [x] **CI and distribution** (`b22262b`): Xcode build, unsigned IPA, GitHub release + SideStore source per `ios-v*` tag; `ios-v1.0.0` tagged
- [x] **iOS 26 redesign** (`7055cb1`): iOS/Xcode 26.1, Liquid Glass, light/dark palettes, search tab, tab-bar mini player, Now Playing zoom
- [x] **Playback and caching** (`f664fa6`, `18c8c80`): handoff from a live transcode to the finished file, next-song prefetch, revalidating refresh, shared cover downloads
- [ ] **M7** What's left
  - [ ] On-device QA against a real server: the README's "Manual QA checklist" (`apps/ios/README.md`), nothing ticked yet
  - [ ] Light mode: remove `INFOPLIST_KEY_UIUserInterfaceStyle = Dark` from both target configurations (the checklist's light/dark item fails until then)
  - [x] App icon: dark and tinted variants (`AppIcon-Dark.png`, `AppIcon-Tinted.png`), cut exactly from `AppIcon.png` by `scripts/ios-icon-variants.mjs`; seeing them on a device is part of the checklist. The SideStore source publishes the default PNG.
  - Optional, App Store build only:
    - [ ] Universal links: an Associated Domains entitlement and an `apple-app-site-association` file on the site (`DeepLink` already parses web URLs)
    - [ ] Network security: narrow `NSAllowsArbitraryLoads` to `NSAllowsLocalNetworking` or per-host `NSExceptionDomains`, and fix Settings' Server footer to match: it says iOS only allows plain http:// for localhost and local network names, which isn't true of the current Info.plist.

## 10. Risks and open questions

* **AAC live streams in AVPlayer**: not yet checked on a device (M7). The
  Icecast precedent says this works. The player leaves a live stream for the
  finished file as soon as it can (§6), and degrades to "Original" when a
  transcode fails.
* **Transcode capacity** (`MAX_CONCURRENT_TRANSCODES`, default: the number of
  CPUs, at least 2; seeks use at most half): a `503` falls back to the original
  file, exactly like the web.
* **ATS**: `NSAllowsArbitraryLoads`, because the server is user-chosen (plain
  HTTP to any host works). An App Store build would have to narrow this (M7).
