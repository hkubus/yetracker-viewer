import AVFoundation
import YeTrackerKit

/// `PlaybackEngine` over `AVPlayer`.
///
/// The stored file streams with HTTP ranges (seekable). A transcode is an
/// ADTS AAC live stream without ranges, which AVPlayer plays the way it plays
/// internet radio; `PlayerModel` seeks those by reloading with `start=`.
@MainActor
final class AVPlayerEngine: PlaybackEngine {
  var eventHandler: (@MainActor @Sendable (PlaybackEngineEvent) -> Void)?

  var volume: Double = 1 {
    didSet { applyVolume() }
  }

  /// Replaced when iOS resets its media services, which invalidates every
  /// AVFoundation object.
  private var player = AVPlayer()
  private var item: AVPlayerItem?
  /// Bumped on every load so callbacks from replaced items are ignored.
  private var generation = 0
  private var isLiveSource = false
  private var reportedReady = false
  private var lastStatus: AVPlayer.TimeControlStatus?
  private var wantsPlayback = false
  private var resumeAfterInterruption = false

  private var itemObservation: NSKeyValueObservation?
  private var itemTokens: [NSObjectProtocol] = []
  private var playerObservation: NSKeyValueObservation?
  private var sessionTokens: [NSObjectProtocol] = []
  private var timeObserver: Any?

  init() {
    AudioSession.configure()
    configurePlayer()
    sessionTokens = Self.observeAudioSession(
      interruption: { [weak self] interruption in self?.handleInterruption(interruption) },
      reset: { [weak self] in self?.handleMediaServicesReset() })
  }

  // MARK: - PlaybackEngine

  func load(_ source: PlaybackSource, autoplay: Bool) {
    generation += 1
    let current = generation
    detachItem()
    isLiveSource = source.isTranscode
    reportedReady = false
    lastStatus = nil

    var options: [String: Any] = [:]
    if let mimeType = source.mimeType {
      // The stream URL has no extension: tell AVFoundation what the bytes are.
      options[AVURLAssetOverrideMIMETypeKey] = mimeType
    }
    let asset = AVURLAsset(url: source.url, options: options)
    let item = AVPlayerItem(asset: asset)
    self.item = item
    attach(item, generation: current)
    player.replaceCurrentItem(with: item)
    if autoplay {
      play()
    } else {
      wantsPlayback = false
      player.pause()
    }
  }

  func play() {
    wantsPlayback = true
    AudioSession.activate()
    player.play()
  }

  func pause() {
    wantsPlayback = false
    player.pause()
  }

  func seek(to seconds: Double) {
    guard item != nil, seconds.isFinite else { return }
    let tolerance = CMTime(seconds: 0.25, preferredTimescale: 600)
    player.seek(
      to: CMTime(seconds: max(0, seconds), preferredTimescale: 600),
      toleranceBefore: tolerance,
      toleranceAfter: tolerance)
  }

  func stop() {
    generation += 1
    wantsPlayback = false
    detachItem()
    player.pause()
    player.replaceCurrentItem(with: nil)
    item = nil
  }

  // MARK: - Player setup

  private func configurePlayer() {
    player.automaticallyWaitsToMinimizeStalling = true
    player.allowsExternalPlayback = true
    applyVolume()
    timeObserver = player.addPeriodicTimeObserver(
      forInterval: CMTime(seconds: 0.5, preferredTimescale: 600), queue: .main
    ) { [weak self] time in
      MainActor.assumeIsolated {
        guard let self else { return }
        self.handleTime(time)
      }
    }
    playerObservation = Self.observeTimeControl(of: player) { [weak self] in
      self?.handleTimeControlChange()
    }
  }

  private func tearDownPlayer() {
    if let timeObserver { player.removeTimeObserver(timeObserver) }
    timeObserver = nil
    playerObservation?.invalidate()
    playerObservation = nil
  }

  private func applyVolume() {
    player.volume = Float(min(1, max(0, volume)))
    player.isMuted = volume <= 0
  }

  // MARK: - Item events

  private func attach(_ item: AVPlayerItem, generation: Int) {
    itemObservation = Self.observeStatus(of: item) { [weak self] status, message in
      self?.handleStatus(status, message: message, generation: generation)
    }
    let center = NotificationCenter.default
    itemTokens = [
      center.addObserver(forName: AVPlayerItem.didPlayToEndTimeNotification, object: item, queue: .main) {
        [weak self] _ in
        MainActor.assumeIsolated {
          guard let self else { return }
          self.handleEnded(generation: generation)
        }
      },
      center.addObserver(forName: AVPlayerItem.failedToPlayToEndTimeNotification, object: item, queue: .main) {
        [weak self] notification in
        let error = notification.userInfo?[AVPlayerItemFailedToPlayToEndTimeErrorKey] as? Error
        let message = error?.localizedDescription ?? "Playback stopped unexpectedly."
        MainActor.assumeIsolated {
          guard let self else { return }
          self.handleFailure(message, generation: generation)
        }
      },
    ]
  }

  private func detachItem() {
    itemObservation?.invalidate()
    itemObservation = nil
    for token in itemTokens {
      NotificationCenter.default.removeObserver(token)
    }
    itemTokens = []
  }

  private func handleStatus(_ status: AVPlayerItem.Status, message: String?, generation: Int) {
    guard generation == self.generation else { return }
    switch status {
    case .readyToPlay:
      guard !reportedReady else { return }
      reportedReady = true
      emit(.ready(duration: isLiveSource ? nil : knownDuration()))
    case .failed:
      emit(.failed(message ?? "The audio could not be loaded."))
    default:
      break
    }
  }

  private func handleEnded(generation: Int) {
    guard generation == self.generation else { return }
    wantsPlayback = false
    emit(.ended)
  }

  private func handleFailure(_ message: String, generation: Int) {
    guard generation == self.generation else { return }
    emit(.failed(message))
  }

  private func handleTime(_ time: CMTime) {
    guard item != nil, time.isNumeric else { return }
    let seconds = time.seconds
    if seconds.isFinite { emit(.time(seconds)) }
  }

  /// Reads the state on the main thread when handled, so late callbacks from a
  /// replaced item report what the player is doing now.
  private func handleTimeControlChange() {
    guard item != nil else { return }
    let status = player.timeControlStatus
    guard status != lastStatus else { return }
    lastStatus = status
    switch status {
    case .playing: emit(.playing)
    case .paused: emit(.paused)
    case .waitingToPlayAtSpecifiedRate: emit(.waiting)
    @unknown default: break
    }
  }

  private func knownDuration() -> Double? {
    guard let duration = item?.duration, duration.isNumeric, !duration.isIndefinite else { return nil }
    let seconds = duration.seconds
    return seconds.isFinite && seconds > 0 ? seconds : nil
  }

  private func emit(_ event: PlaybackEngineEvent) {
    eventHandler?(event)
  }

  // MARK: - Audio session events

  private func handleInterruption(_ interruption: AudioSession.Interruption) {
    switch interruption {
    case .began:
      // The system has already paused the player.
      resumeAfterInterruption = wantsPlayback
    case .ended(let shouldResume):
      if shouldResume, resumeAfterInterruption, item != nil { play() }
      resumeAfterInterruption = false
    }
  }

  /// Rare, but after it the player, its items and the session are all dead:
  /// rebuild them and let `PlayerModel` reload the current stream in place.
  private func handleMediaServicesReset() {
    let hadItem = item != nil
    generation += 1
    detachItem()
    tearDownPlayer()
    player = AVPlayer()
    item = nil
    lastStatus = nil
    reportedReady = false
    AudioSession.configure()
    configurePlayer()
    if hadItem { emit(.invalidated) }
  }

  // MARK: - Observation factories
  //
  // KVO callbacks can arrive on any thread. The closures are created outside
  // the main actor and hop to it explicitly, in order.

  private nonisolated static func observeTimeControl(
    of player: AVPlayer,
    _ handler: @escaping @MainActor @Sendable () -> Void
  ) -> NSKeyValueObservation {
    player.observe(\.timeControlStatus, options: [.new]) { _, _ in
      DispatchQueue.main.async { MainActor.assumeIsolated { handler() } }
    }
  }

  private nonisolated static func observeStatus(
    of item: AVPlayerItem,
    _ handler: @escaping @MainActor @Sendable (AVPlayerItem.Status, String?) -> Void
  ) -> NSKeyValueObservation {
    item.observe(\.status, options: [.initial, .new]) { item, _ in
      let status = item.status
      let message = item.error?.localizedDescription
      DispatchQueue.main.async { MainActor.assumeIsolated { handler(status, message) } }
    }
  }

  private nonisolated static func observeAudioSession(
    interruption: @escaping @MainActor @Sendable (AudioSession.Interruption) -> Void,
    reset: @escaping @MainActor @Sendable () -> Void
  ) -> [NSObjectProtocol] {
    let center = NotificationCenter.default
    let session = AVAudioSession.sharedInstance()
    return [
      center.addObserver(forName: AVAudioSession.interruptionNotification, object: session, queue: .main) {
        notification in
        guard let event = AudioSession.Interruption(notification) else { return }
        MainActor.assumeIsolated { interruption(event) }
      },
      center.addObserver(forName: AVAudioSession.mediaServicesWereResetNotification, object: session, queue: .main) {
        _ in
        MainActor.assumeIsolated { reset() }
      },
    ]
  }
}

/// The app plays long-form audio in the background (`UIBackgroundModes: audio`).
enum AudioSession {
  static func configure() {
    try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .default, policy: .longFormAudio)
  }

  static func activate() {
    try? AVAudioSession.sharedInstance().setActive(true)
  }

  enum Interruption {
    case began
    case ended(shouldResume: Bool)

    init?(_ notification: Notification) {
      guard let raw = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
        let type = AVAudioSession.InterruptionType(rawValue: raw)
      else { return nil }
      switch type {
      case .began:
        self = .began
      case .ended:
        let options = (notification.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt) ?? 0
        self = .ended(shouldResume: AVAudioSession.InterruptionOptions(rawValue: options).contains(.shouldResume))
      @unknown default:
        return nil
      }
    }
  }
}
