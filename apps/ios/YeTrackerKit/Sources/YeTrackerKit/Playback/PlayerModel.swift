import Foundation
import Observation

/// The persistent player (`apps/web/src/components/Player.astro`): queue,
/// quality, fallbacks, buffering hint, duration and lock-screen metadata.
/// Audio itself is delegated to a `PlaybackEngine`.
@MainActor
@Observable
public final class PlayerModel {
  /// Shown on the state line while nothing else is happening.
  public static let idleStateLine = "Now playing"
  public static let bufferingStateLine = "Buffering…"
  public static let retryingOriginalStateLine = "Retrying original…"
  public static let convertingStateLine = "Converting for playback…"
  public static let loadFailedMessage = "Audio failed to load. The file may be missing or unavailable."
  public static let qualitySwitchFailedMessage = "Quality switch failed. Restored the previous stream."
  /// A stream gap must last this long before the state line changes at all.
  public static let bufferingHintDelay: Duration = .milliseconds(500)
  /// Bitrate used when the stored file cannot be played natively (e.g. Ogg/Opus).
  public static let fallbackBitrate = 256
  /// Lock-screen and keyboard seek step (web `seekOffset` default).
  public static let seekStep: Double = 5
  /// A live transcode that stalls this close to the known end has finished:
  /// AVPlayer does not always report the end of a stream without a length.
  public static let liveEndTolerance: Double = 1.5
  public static let liveEndGrace: Duration = .seconds(2)
  /// A seek in a transcode restarts it (a new server-side ffmpeg) only after
  /// seeks have paused this long, so a run of skip taps starts one, not one each.
  public static let seekRestartDelay: Duration = .milliseconds(300)
  /// The API refuses `start` beyond a day of audio.
  public static let maxTranscodeStart: Double = 86_400

  public private(set) var current: Track?
  public private(set) var queue: [Track] = []
  public private(set) var queueID: String?
  public private(set) var isPlaying = false
  /// "Now playing", "Buffering…", "Retrying original…", "Converting for playback…".
  public private(set) var stateLine = PlayerModel.idleStateLine
  public private(set) var errorMessage: String?
  /// Position in the song, in seconds.
  public private(set) var elapsed: Double = 0
  /// Song length in seconds; `0` while unknown.
  public private(set) var duration: Double = 0
  /// A quality change is loading.
  public private(set) var isSwitchingQuality = false
  /// The kind of stream playing right now.
  public private(set) var activeSource: PlaybackSource?

  @ObservationIgnored private let engine: PlaybackEngine
  @ObservationIgnored private let api: APIProvider
  @ObservationIgnored private let settings: AppSettings
  @ObservationIgnored private let sleeper: Sleeper
  @ObservationIgnored public weak var nowPlaying: NowPlayingSink?

  @ObservationIgnored private var attempts = Attempts()
  /// Original sources seek once ready.
  @ObservationIgnored private var pendingSeek: Double?
  @ObservationIgnored private var pendingSwitch: QualitySwitch?
  @ObservationIgnored private var engineDuration: Double?
  @ObservationIgnored private var hasEnded = false
  @ObservationIgnored private var generation = 0
  @ObservationIgnored var bufferingTask: Task<Void, Never>?
  @ObservationIgnored var endWatchdog: Task<Void, Never>?
  @ObservationIgnored private var isWaiting = false
  @ObservationIgnored var durationTask: Task<Void, Never>?
  /// A transcode restart waiting for `seekRestartDelay`.
  @ObservationIgnored var seekTask: Task<Void, Never>?
  /// Whether playback should be running: set by loading with autoplay and by
  /// play/pause, unlike `isPlaying`, which waits for the engine's `.playing`.
  /// Reloads (seeks, invalidation, quality switches) keep this state, so a
  /// seek while the first stream is still buffering does not pause the track.
  @ObservationIgnored private var wantsToPlay = false

  private struct Attempts {
    var original = false
    var transcode = false
  }

  private struct QualitySwitch {
    let previousSource: PlaybackSource
    let position: Double
    let wasPlaying: Bool
  }

  public init(
    engine: PlaybackEngine,
    api: @escaping APIProvider,
    settings: AppSettings,
    sleeper: @escaping Sleeper = Sleepers.live
  ) {
    self.engine = engine
    self.api = api
    self.settings = settings
    self.sleeper = sleeper
    engine.volume = settings.clampedVolume
    engine.eventHandler = { [weak self] event in self?.handle(event) }
  }

  // MARK: - Derived state

  public var isActive: Bool { current != nil }
  public var quality: PlaybackQuality { settings.quality }
  public var canGoNext: Bool { neighbor(1) != nil }
  public var canGoPrevious: Bool { neighbor(-1) != nil }
  public var progress: Double { duration > 0 ? min(1, max(0, elapsed / duration)) : 0 }

  /// Player volume, 0…1, persisted like the web's `yetracker:volume`.
  public var volume: Double {
    get { settings.clampedVolume }
    set {
      settings.volume = newValue
      engine.volume = settings.clampedVolume
    }
  }

  public func coverURL(for track: Track) -> URL? {
    track.eraID.map { api().coverURL(eraID: $0, version: track.coverKey) }
  }

  // MARK: - Transport

  /// Starts `track`; next/previous walk `queue` (the list it was started from).
  public func play(_ track: Track, queue: [Track], queueID: String? = nil) {
    self.queue = queue.contains(where: { $0.id == track.id }) ? queue : [track]
    self.queueID = queueID
    start(track)
  }

  /// Replaces the queue with a grown version of the same list (more pages
  /// loaded). Ignored for other lists or when the current track left the list.
  public func extendQueue(_ tracks: [Track], queueID: String) {
    guard queueID == self.queueID, let current, tracks.contains(where: { $0.id == current.id }) else { return }
    guard tracks != queue else { return }
    queue = tracks
    publishNowPlaying()
  }

  public func togglePlayPause() {
    guard current != nil else { return }
    if isPlaying {
      pause()
    } else {
      resume()
    }
  }

  public func resume() {
    guard current != nil, !isPlaying else { return }
    if hasEnded || errorMessage != nil || activeSource == nil {
      retry()
      return
    }
    wantsToPlay = true
    guard seekTask == nil else { return }  // The pending restart starts playing.
    engine.play()
  }

  public func pause() {
    guard current != nil else { return }
    wantsToPlay = false
    engine.pause()
    isPlaying = false
    publishNowPlaying()
  }

  public func next() {
    if let track = neighbor(1) { start(track) }
  }

  public func previous() {
    if let track = neighbor(-1) { start(track) }
  }

  /// Seeks within the song. Original files seek natively; a transcode is
  /// restarted at the new offset (after `seekRestartDelay`). Seeking a
  /// transcode to its very end finishes the song instead: a stream that
  /// starts there would be empty and read as a failure.
  public func seek(to target: Double) {
    guard current != nil, let source = activeSource, target.isFinite else { return }
    var position = max(0, target)
    if duration > 0 { position = min(position, duration) }
    hasEnded = false
    switch source.kind {
    case .original:
      if pendingSeek != nil {
        pendingSeek = position
      } else {
        engine.seek(to: position)
      }
    case .transcode(let bitrate):
      if duration > 0, position >= duration - Self.liveEndTolerance {
        cancelSeekRestart()
        handleEnded()
        return
      }
      scheduleRestart(bitrate: bitrate, at: min(position, Self.maxTranscodeStart))
    }
    elapsed = position
    publishNowPlaying()
  }

  private func scheduleRestart(bitrate: Int, at position: Double) {
    seekTask?.cancel()
    let expected = generation
    seekTask = Task { [weak self, sleeper] in
      do {
        try await sleeper(Self.seekRestartDelay)
      } catch {
        return
      }
      guard let self, !Task.isCancelled, self.generation == expected else { return }
      self.seekTask = nil
      self.load(self.transcodeSource(bitrate: bitrate, at: position), autoplay: self.wantsToPlay)
    }
  }

  private func cancelSeekRestart() {
    seekTask?.cancel()
    seekTask = nil
  }

  public func skip(by delta: Double) {
    seek(to: elapsed + delta)
  }

  /// Changes the quality preference and, mid-song, reloads at the same position.
  /// On failure the previous stream is restored (web behaviour).
  public func setQuality(_ quality: PlaybackQuality) {
    guard quality != settings.quality else { return }
    settings.quality = quality
    // Nothing to reload when the stream already matches (e.g. after a fallback).
    guard current != nil, let previous = activeSource, previous.kind != sourceKind(for: quality) else { return }
    let position = elapsed
    let wasPlaying = wantsToPlay
    errorMessage = nil
    pendingSwitch = QualitySwitch(previousSource: previous, position: position, wasPlaying: wasPlaying)
    isSwitchingQuality = true
    attempts = Attempts()
    load(source(for: quality, at: position), autoplay: wasPlaying, resumeAt: position)
  }

  /// Reloads the current track from the start with the preferred quality.
  public func retry() {
    guard current != nil else { return }
    errorMessage = nil
    hasEnded = false
    pendingSwitch = nil
    isSwitchingQuality = false
    attempts = Attempts()
    clearBuffering()
    elapsed = 0
    load(source(for: settings.quality, at: 0), autoplay: true)
    publishNowPlaying()
  }

  /// Stops and forgets the current track (e.g. the server changed).
  public func stop() {
    generation += 1
    cancelSeekRestart()
    wantsToPlay = false
    stopWaiting()
    engine.stop()
    bufferingTask?.cancel()
    bufferingTask = nil
    durationTask?.cancel()
    durationTask = nil
    current = nil
    queue = []
    queueID = nil
    isPlaying = false
    errorMessage = nil
    elapsed = 0
    duration = 0
    activeSource = nil
    pendingSeek = nil
    pendingSwitch = nil
    isSwitchingQuality = false
    stateLine = Self.idleStateLine
    nowPlaying?.update(nil)
  }

  // MARK: - Loading

  private func start(_ track: Track) {
    generation += 1
    current = track
    errorMessage = nil
    hasEnded = false
    pendingSwitch = nil
    isSwitchingQuality = false
    pendingSeek = nil
    attempts = Attempts()
    engineDuration = nil
    elapsed = 0
    duration = track.durationHint ?? 0
    clearBuffering()
    load(source(for: settings.quality, at: 0), autoplay: true)
    publishNowPlaying()
    durationTask?.cancel()
    durationTask = nil
    if duration <= 0 { fetchDuration(for: track) }
  }

  private func sourceKind(for quality: PlaybackQuality) -> PlaybackSource.Kind {
    quality.bitrate.map { .transcode(bitrate: $0) } ?? .original
  }

  private func source(for quality: PlaybackQuality, at position: Double) -> PlaybackSource {
    if let bitrate = quality.bitrate { return transcodeSource(bitrate: bitrate, at: position) }
    return originalSource()
  }

  private func originalSource() -> PlaybackSource {
    PlaybackSource(kind: .original, url: api().streamURL(songID: current?.id ?? 0))
  }

  private func transcodeSource(bitrate: Int, at position: Double) -> PlaybackSource {
    let start = max(0, position.isFinite ? position : 0)
    let transcode = Transcode(bitrate: bitrate, format: .aac, start: start)
    return PlaybackSource(
      kind: .transcode(bitrate: bitrate),
      url: api().streamURL(songID: current?.id ?? 0, transcode: transcode),
      offset: start,
      mimeType: "audio/aac")
  }

  /// Loads a source. Originals seek to `resumeAt` once ready; transcodes start
  /// there already (their offset).
  private func load(_ source: PlaybackSource, autoplay: Bool, resumeAt: Double = 0) {
    cancelSeekRestart()
    wantsToPlay = autoplay
    activeSource = source
    switch source.kind {
    case .original:
      attempts.original = true
      pendingSeek = resumeAt > 0 ? resumeAt : nil
      elapsed = max(0, resumeAt)
    case .transcode:
      attempts.transcode = true
      pendingSeek = nil
      elapsed = source.offset
    }
    engine.load(source, autoplay: autoplay)
  }

  // MARK: - Engine events

  private func handle(_ event: PlaybackEngineEvent) {
    guard current != nil else { return }
    switch event {
    case .ready(let reported):
      stopWaiting()
      if case .original = activeSource?.kind, let reported, reported.isFinite, reported > 0 {
        engineDuration = reported
        duration = reported
      }
      if let position = pendingSeek {
        pendingSeek = nil
        engine.seek(to: position)
        elapsed = position
      }
      if pendingSwitch != nil {
        pendingSwitch = nil
        isSwitchingQuality = false
      }
      clearBuffering()
      publishNowPlaying()
    case .playing:
      stopWaiting()
      wantsToPlay = true
      isPlaying = true
      hasEnded = false
      errorMessage = nil
      clearBuffering()
      publishNowPlaying()
    case .paused:
      stopWaiting()
      // Only a pause after playback started is the user's (lock screen,
      // interruption); the engine also reports paused while a stream loads.
      guard isPlaying else { return }
      wantsToPlay = false
      isPlaying = false
      publishNowPlaying()
    case .waiting:
      isWaiting = true
      showBuffering()
      armEndWatchdog()
    case .time(let time):
      // The old stream keeps reporting its position until a pending restart loads.
      guard let source = activeSource, time.isFinite, pendingSeek == nil, seekTask == nil else { return }
      let position = source.offset + max(0, time)
      elapsed = duration > 0 ? min(position, duration) : position
    case .ended:
      handleEnded()
    case .failed:
      handleFailure()
    case .invalidated:
      reloadAfterInvalidation()
    }
  }

  /// Same kind of stream, same position, same play/pause state.
  private func reloadAfterInvalidation() {
    guard let source = activeSource else { return }
    let position = elapsed
    let wasPlaying = wantsToPlay
    stopWaiting()
    clearBuffering()
    switch source.kind {
    case .original:
      load(originalSource(), autoplay: wasPlaying, resumeAt: position)
    case .transcode(let bitrate):
      load(transcodeSource(bitrate: bitrate, at: position), autoplay: wasPlaying)
    }
    publishNowPlaying()
  }

  private func handleEnded() {
    stopWaiting()
    if let track = neighbor(1) {
      start(track)
      return
    }
    wantsToPlay = false
    isPlaying = false
    hasEnded = true
    if duration > 0 { elapsed = duration }
    clearBuffering()
    publishNowPlaying()
  }

  private func handleFailure() {
    guard let failed = activeSource else { return }
    if let pending = pendingSwitch {
      // Restore the previous working stream instead of leaving a broken one.
      pendingSwitch = nil
      isSwitchingQuality = false
      let restored: PlaybackSource =
        switch pending.previousSource.kind {
        case .original: originalSource()
        case .transcode(let bitrate): transcodeSource(bitrate: bitrate, at: pending.position)
        }
      clearBuffering()
      load(restored, autoplay: pending.wasPlaying, resumeAt: pending.position)
      errorMessage = Self.qualitySwitchFailedMessage
      publishNowPlaying()
      return
    }

    let position = elapsed
    switch failed.kind {
    case .transcode where !attempts.original:
      // A failed transcode is the common case (capacity, odd container): serve the stored file.
      errorMessage = nil
      clearBuffering()
      stateLine = Self.retryingOriginalStateLine
      load(originalSource(), autoplay: true, resumeAt: position)
    case .original where !attempts.transcode:
      // AVPlayer cannot decode some stored files (Ogg/Opus from yt-dlp): convert them.
      errorMessage = nil
      clearBuffering()
      stateLine = Self.convertingStateLine
      load(transcodeSource(bitrate: Self.fallbackBitrate, at: position), autoplay: true)
    default:
      clearBuffering()
      wantsToPlay = false
      isPlaying = false
      errorMessage = Self.loadFailedMessage
    }
    publishNowPlaying()
  }

  // MARK: - Live stream end

  private func armEndWatchdog() {
    guard endWatchdog == nil, let source = activeSource, source.isTranscode, duration > 0,
      elapsed >= duration - Self.liveEndTolerance
    else { return }
    let expected = generation
    endWatchdog = Task { [weak self, sleeper] in
      do {
        try await sleeper(Self.liveEndGrace)
      } catch {
        return
      }
      guard let self, !Task.isCancelled, self.generation == expected, self.isWaiting,
        self.activeSource == source
      else { return }
      self.endWatchdog = nil
      self.handleEnded()
    }
  }

  private func stopWaiting() {
    isWaiting = false
    endWatchdog?.cancel()
    endWatchdog = nil
  }

  // MARK: - Buffering hint

  private func showBuffering() {
    guard bufferingTask == nil else { return }
    bufferingTask = Task { [weak self, sleeper] in
      do {
        try await sleeper(Self.bufferingHintDelay)
      } catch {
        return
      }
      guard let self, !Task.isCancelled, self.bufferingTask != nil else { return }
      self.stateLine = Self.bufferingStateLine
    }
  }

  private func clearBuffering() {
    bufferingTask?.cancel()
    bufferingTask = nil
    stateLine = Self.idleStateLine
  }

  // MARK: - Helpers

  private func neighbor(_ direction: Int) -> Track? {
    guard let current, let index = queue.firstIndex(where: { $0.id == current.id }) else { return nil }
    let target = index + direction
    return queue.indices.contains(target) ? queue[target] : nil
  }

  /// Web: when the catalog has no length, ask the API to probe the file.
  private func fetchDuration(for track: Track) {
    let expected = generation
    durationTask = Task { [weak self] in
      guard let self else { return }
      let probed = try? await self.api().duration(songID: track.id)
      guard let probed, self.generation == expected, self.current?.id == track.id, self.engineDuration == nil
      else { return }
      self.duration = probed
      self.publishNowPlaying()
    }
  }

  private func publishNowPlaying() {
    guard let nowPlaying else { return }
    guard let track = current else {
      nowPlaying.update(nil)
      return
    }
    let eraName = track.eraName.nonBlank
    nowPlaying.update(
      NowPlayingInfo(
        trackID: track.id,
        title: track.title.nonBlank ?? "Ye Tracker",
        artist: eraName ?? "Ye Tracker",
        album: eraName ?? "Ye Tracker",
        artworkURL: coverURL(for: track),
        duration: duration > 0 ? duration : nil,
        elapsed: elapsed,
        isPlaying: isPlaying,
        canGoPrevious: canGoPrevious,
        canGoNext: canGoNext))
  }
}
