import MediaPlayer
import UIKit
import YeTrackerKit

/// Lock screen, Control Center and headphone controls (the web's Media Session):
/// publishes `NowPlayingInfo` and routes remote commands to the player.
@MainActor
final class NowPlayingController: NowPlayingSink {
  private weak var player: PlayerModel?
  private var artworkURL: URL?
  private var artwork: MPMediaItemArtwork?
  private var artworkTask: Task<Void, Never>?
  private var lastInfo: NowPlayingInfo?

  func attach(to player: PlayerModel) {
    self.player = player
    player.nowPlaying = self

    let center = MPRemoteCommandCenter.shared()
    Self.register(center.playCommand) { [weak self] _ in self?.player?.resume() }
    Self.register(center.pauseCommand) { [weak self] _ in self?.player?.pause() }
    Self.register(center.togglePlayPauseCommand) { [weak self] _ in self?.player?.togglePlayPause() }
    Self.register(center.nextTrackCommand) { [weak self] _ in self?.player?.next() }
    Self.register(center.previousTrackCommand) { [weak self] _ in self?.player?.previous() }
    Self.register(center.changePlaybackPositionCommand) { [weak self] position in
      guard let position else { return }
      self?.player?.seek(to: position)
    }
    // Interval skipping would replace next/previous on the lock screen.
    center.skipForwardCommand.isEnabled = false
    center.skipBackwardCommand.isEnabled = false
    center.seekForwardCommand.isEnabled = false
    center.seekBackwardCommand.isEnabled = false
  }

  func update(_ info: NowPlayingInfo?) {
    let center = MPNowPlayingInfoCenter.default()
    let commands = MPRemoteCommandCenter.shared()
    guard let info else {
      lastInfo = nil
      artworkTask?.cancel()
      center.nowPlayingInfo = nil
      commands.nextTrackCommand.isEnabled = false
      commands.previousTrackCommand.isEnabled = false
      return
    }
    lastInfo = info
    commands.nextTrackCommand.isEnabled = info.canGoNext
    commands.previousTrackCommand.isEnabled = info.canGoPrevious
    commands.changePlaybackPositionCommand.isEnabled = info.duration != nil

    var values: [String: Any] = [
      MPMediaItemPropertyTitle: info.title,
      MPMediaItemPropertyArtist: info.artist,
      MPMediaItemPropertyAlbumTitle: info.album,
      MPNowPlayingInfoPropertyElapsedPlaybackTime: info.elapsed,
      MPNowPlayingInfoPropertyPlaybackRate: info.isPlaying ? 1.0 : 0.0,
      MPNowPlayingInfoPropertyDefaultPlaybackRate: 1.0,
      MPNowPlayingInfoPropertyMediaType: MPNowPlayingInfoMediaType.audio.rawValue,
    ]
    if let duration = info.duration { values[MPMediaItemPropertyPlaybackDuration] = duration }
    if info.artworkURL == artworkURL, let artwork { values[MPMediaItemPropertyArtwork] = artwork }
    center.nowPlayingInfo = values

    if info.artworkURL != artworkURL { loadArtwork(info.artworkURL) }
  }

  private func loadArtwork(_ url: URL?) {
    artworkTask?.cancel()
    artworkURL = url
    artwork = nil
    guard let url else { return }
    artworkTask = Task { [weak self] in
      guard let image = await ImagePipeline.shared.image(for: url), !Task.isCancelled, let self,
        self.artworkURL == url
      else { return }
      self.artwork = Self.makeArtwork(image)
      if let info = self.lastInfo { self.update(info) }
    }
  }

  // MARK: - Factories outside the main actor
  //
  // MediaPlayer may invoke these closures off the main thread.

  private nonisolated static func makeArtwork(_ image: UIImage) -> MPMediaItemArtwork {
    MPMediaItemArtwork(boundsSize: image.size) { _ in image }
  }

  /// `action` receives the target position for seek commands, `nil` otherwise.
  private nonisolated static func register(
    _ command: MPRemoteCommand,
    _ action: @escaping @MainActor @Sendable (_ position: Double?) -> Void
  ) {
    command.isEnabled = true
    _ = command.addTarget { event in
      let position = (event as? MPChangePlaybackPositionCommandEvent)?.positionTime
      if Thread.isMainThread {
        MainActor.assumeIsolated { action(position) }
      } else {
        DispatchQueue.main.async { MainActor.assumeIsolated { action(position) } }
      }
      return .success
    }
  }
}
