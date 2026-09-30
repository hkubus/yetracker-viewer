# Ye Tracker for iOS

A native SwiftUI client for the YeTracker API with the same features as
`apps/web`: era browsing, catalog-wide search, recently leaked tracks, and a
persistent player with lock-screen controls, AirPlay and downloads. The plan
and the web → iOS feature map are in [`IOS_APP_PLAN.md`](../../IOS_APP_PLAN.md).

## Requirements

- Xcode 16 or newer (the project uses synchronized folders, object version 77)
- iOS 17 or newer (device or simulator)
- A running API from this repo, including the `format=aac`/`start` stream
  options (see [Server requirement](#server-requirement))

## Run it

1. Start the API, e.g. `pnpm start:api` (listens on `127.0.0.1:3000`).
2. Open `apps/ios/YeTracker.xcodeproj`, pick an iOS 17+ simulator and run the
   **YeTracker** scheme. The simulator shares the Mac's network, so the default
   server `http://localhost:3000` works as is.

For a device, set a signing team and a reachable server in
`Config/Local.xcconfig` (gitignored, included by `Config/Shared.xcconfig`):

```xcconfig
DEVELOPMENT_TEAM = ABCDE12345
PRODUCT_BUNDLE_IDENTIFIER = net.example.yetracker
// `//` starts a comment in xcconfig files, hence the `$()`.
YT_API_BASE_URL = https:/$()/tracker.example.com/api
```

The server can also be changed at runtime in **Settings → Server**, which
validates the URL and offers **Test Connection** (`GET /health`). Path prefixes
such as `/api` are kept, like the web's `PUBLIC_API_URL`.

### Plain HTTP and App Transport Security

`Config/Info.plist` sets `NSAllowsArbitraryLoads`: the server is whatever the
user enters in **Settings → Server**, so plain `http://` works for any host
(`http://192.168.1.20:3000`, a VPS without TLS). HTTPS is still preferable (the
production setup in the root README has it). An App Store build would have to
narrow this to `NSAllowsLocalNetworking` or per-host `NSExceptionDomains`.

## Server requirement

AVPlayer cannot play Ogg, which is what the web player's `?quality=` transcode
produces, and what `yt-dlp` stores for YouTube/Instagram/Twitter sources. The
API therefore accepts two extra stream parameters (documented in `API.md`):

| Parameter | Meaning |
|---|---|
| `format=aac` | Transcode to ADTS AAC (`audio/aac`) instead of Ogg/Opus |
| `start=<seconds>` | Start the transcode at an offset, so the player can seek a live stream by restarting it |

Playback strategy (`PlayerModel`):

- The default quality is 128 kbps, as on the web. It streams
  `…/stream?quality=128&format=aac`. Seeking restarts the stream with `start=`.
- **Original** streams the stored file with HTTP ranges and seeks natively.
- A failed transcode (for example `503` when transcode slots are full) falls
  back to the original file, like the web does.
- An original the device cannot decode (Ogg/Opus) falls back to a 256 kbps AAC
  transcode.

Against an older API without these parameters, only originals in formats
AVPlayer supports (MP3, AAC/M4A, FLAC, WAV, AIFF) will play.

## Deep links

`yetracker://eras/12?page=3&q=love&category=best-of&sort=leak-newest#song-45`
opens era 12 with those filters, loads through page 3 and scrolls to and
highlights song 45. The parser also accepts the web app's own URLs
(`https://<site>/eras/12?page=3#song-45`), so universal links work once you add
an Associated Domains entitlement for your site.

## Architecture

```
YeTrackerKit/        Swift package: Foundation + Observation only (builds on Linux)
  Models/            Era, EraSong, SearchSong, categories, sorts, qualities
  Networking/        APIClient, error classification, query encoding
  Formatting/        durations, UTC dates, text matching, sRGB/OKLab colour mixing
  Routing/           deep links and per-tab navigation paths
  Settings/          server URL, quality and volume (web localStorage key names)
  Features/          HomeModel, GlobalSearchModel, EraDetailModel, EraDirectory
  Playback/          PlayerModel state machine behind a PlaybackEngine protocol
YeTracker/           App target: SwiftUI views and Apple-framework glue
  App/               entry point, composition root (AppModel), tabs
  Playback/          AVPlayer engine, lock screen / remote commands
  Support/           theme, cover cache, downloads, UIKit presentation
  Views/             Home, Search, Era, Player, Settings
```

All behaviour lives in `@Observable @MainActor` models in `YeTrackerKit`, so it
is unit-tested without a simulator. This covers paging, debounced search,
stale-response handling, deep-link focus, and the player's fallbacks,
quality switching, buffering hint and queue. The app target only renders
state and adapts AVFoundation, MediaPlayer and UIKit.

## Tests

```sh
# macOS (or the YeTracker scheme's Test action in Xcode)
swift test --package-path apps/ios/YeTrackerKit

# Linux (Swift 6.1): the toolchain's libswiftObservation.so references an
# unexported symbol, so let the linker leave it unresolved.
swift test --package-path apps/ios/YeTrackerKit -Xlinker --allow-shlib-undefined

# Also run the live smoke tests against a running API
YT_LIVE_API_URL=http://127.0.0.1:3000 swift test --package-path apps/ios/YeTrackerKit
```

Command-line build of the app on a Mac:

```sh
xcodebuild -project apps/ios/YeTracker.xcodeproj -scheme YeTracker \
  -destination 'platform=iOS Simulator,name=iPhone 16' build
```

## Manual QA checklist

- [ ] Home: stats header, recently leaked (tap plays, chevron opens the era), era grid, "Filter eras…"
- [ ] Era: header, previous/next era, search (instant local filter, then the whole era), category and sort menu, filter chips, "Clear all filters", infinite scroll, "Showing X of Y songs"
- [ ] Search: results deep-link into the era and highlight the song; era range slider, "Playable only", Clear; long-press a playable result to play it
- [ ] Player: mini player and Now Playing sheet, scrubbing at 128 kbps (restarts with `start=`) and at Original (native seek), quality switch mid-song, auto-advance, previous/next at list ends
- [ ] An Ogg/Opus original at **Original** quality shows "Converting for playback…" and then plays
- [ ] Lock screen and Control Center: artwork, play/pause, next/previous, scrubbing; a phone call interrupts and playback resumes afterwards
- [ ] AirPlay route picker, volume slider, background playback
- [ ] Download original → share sheet → Save to Files; source links open in Safari
- [ ] iPad: keyboard Space, ←/→ (±5 s), ⌘←/⌘→
- [ ] Settings: invalid URL message, Test Connection, Use Default, clear cover cache
