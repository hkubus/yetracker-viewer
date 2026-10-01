import Foundation

/// Something the player can play: a song plus what the player UI shows for it.
public struct Track: Hashable, Identifiable, Sendable {
  public let id: Int
  public let title: String
  public let eraID: Int?
  public let eraName: String?
  /// Era accent as hex (tints the player).
  public let colorHex: String?
  /// Cover version for the artwork URL.
  public let coverVersion: String?
  /// Known length in seconds, if any (duration, else catalog length).
  public let durationHint: Double?

  public init(
    id: Int,
    title: String,
    eraID: Int?,
    eraName: String?,
    colorHex: String? = nil,
    coverVersion: String? = nil,
    durationHint: Double? = nil
  ) {
    self.id = id
    self.title = title
    self.eraID = eraID
    self.eraName = eraName
    self.colorHex = colorHex
    self.coverVersion = coverVersion
    self.durationHint = durationHint.flatMap { $0.isFinite && $0 > 0 ? $0 : nil }
  }

  /// A row of an era list.
  public init(song: EraSong, era: Era?) {
    self.init(
      id: song.id,
      title: song.displayTitle,
      eraID: song.eraId ?? era?.id,
      eraName: era?.name.nonBlank,
      colorHex: era?.dominantColor,
      coverVersion: era?.coverVersion,
      durationHint: song.bestDuration)
  }

  /// A search result or recent leak. Its era's cover version comes with it
  /// from current APIs; `coverVersion` (the era list's) covers older ones.
  public init(song: SearchSong, coverVersion: String?) {
    self.init(
      id: song.id,
      title: song.displayTitle,
      eraID: song.eraId,
      eraName: song.eraName.nonBlank,
      colorHex: song.dominantColor,
      coverVersion: song.eraCoverVersion.nonBlank ?? coverVersion,
      durationHint: song.bestDuration)
  }

  public var color: RGBColor { RGBColor(hex: colorHex) ?? .fallbackAccent }

  /// Same cover cache key as `Era.coverKey`.
  public var coverKey: String { coverVersion.nonBlank ?? color.hex }
}

/// One loadable stream for the current track.
public struct PlaybackSource: Hashable, Sendable {
  public enum Kind: Hashable, Sendable {
    /// The stored file: range-capable, natively seekable.
    case original
    /// A live AAC transcode: not seekable, restarted at an offset instead.
    case transcode(bitrate: Int)
  }

  public let kind: Kind
  public let url: URL
  /// Song time at which this source's time zero starts (the transcode's `start`).
  public let offset: Double
  /// MIME type hint for URLs without a file extension.
  public let mimeType: String?

  public init(kind: Kind, url: URL, offset: Double = 0, mimeType: String? = nil) {
    self.kind = kind
    self.url = url
    self.offset = max(0, offset)
    self.mimeType = mimeType
  }

  public var isTranscode: Bool {
    if case .transcode = kind { return true }
    return false
  }
}

/// What an engine reports about the source it was last asked to load.
/// Engines must drop events from sources that have since been replaced.
public enum PlaybackEngineEvent: Equatable, Sendable {
  /// Ready to play; `duration` is `nil` for live streams.
  case ready(duration: Double?)
  case playing
  case paused
  /// Playback wants to play but is waiting for data.
  case waiting
  /// Periodic position in the source's own timeline.
  case time(Double)
  case ended
  case failed(String)
  /// The engine lost its source through no fault of the stream (e.g. iOS reset
  /// its media services) and needs it loaded again.
  case invalidated
}

/// The audio backend (AVPlayer in the app, a fake in tests).
@MainActor
public protocol PlaybackEngine: AnyObject {
  var eventHandler: (@MainActor @Sendable (PlaybackEngineEvent) -> Void)? { get set }
  var volume: Double { get set }
  func load(_ source: PlaybackSource, autoplay: Bool)
  func play()
  func pause()
  /// Seeks within the current source's timeline.
  func seek(to seconds: Double)
  func stop()
}

/// Lock screen / Control Center metadata (the web's Media Session).
public struct NowPlayingInfo: Equatable, Sendable {
  public let trackID: Int
  public let title: String
  public let artist: String
  public let album: String
  public let artworkURL: URL?
  public let duration: Double?
  public let elapsed: Double
  public let isPlaying: Bool
  public let canGoPrevious: Bool
  public let canGoNext: Bool

  public init(
    trackID: Int,
    title: String,
    artist: String,
    album: String,
    artworkURL: URL?,
    duration: Double?,
    elapsed: Double,
    isPlaying: Bool,
    canGoPrevious: Bool,
    canGoNext: Bool
  ) {
    self.trackID = trackID
    self.title = title
    self.artist = artist
    self.album = album
    self.artworkURL = artworkURL
    self.duration = duration
    self.elapsed = elapsed
    self.isPlaying = isPlaying
    self.canGoPrevious = canGoPrevious
    self.canGoNext = canGoNext
  }
}

@MainActor
public protocol NowPlayingSink: AnyObject {
  /// `nil` clears the lock-screen entry.
  func update(_ info: NowPlayingInfo?)
}
