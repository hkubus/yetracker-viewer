import Foundation
import Observation

/// Minimal string storage so settings can live in `UserDefaults` or in memory (tests).
public protocol SettingsStorage: AnyObject {
  func string(forKey key: String) -> String?
  func setString(_ value: String?, forKey key: String)
}

extension UserDefaults: SettingsStorage {
  public func setString(_ value: String?, forKey key: String) {
    if let value {
      set(value, forKey: key)
    } else {
      removeObject(forKey: key)
    }
  }
}

public final class InMemorySettingsStorage: SettingsStorage {
  private var values: [String: String]

  public init(_ values: [String: String] = [:]) {
    self.values = values
  }

  public func string(forKey key: String) -> String? { values[key] }

  public func setString(_ value: String?, forKey key: String) {
    values[key] = value
  }
}

/// User preferences. Quality and volume use the web's `localStorage` key names.
@MainActor
@Observable
public final class AppSettings {
  public static let apiBaseURLKey = "yetracker.apiBaseURL"
  public static let qualityKey = "yetracker:quality"
  public static let volumeKey = "yetracker:volume"

  /// Build-time default (`YT_API_BASE_URL`).
  public let defaultAPIBaseURL: URL
  /// A user override, or `nil` to follow the default.
  public private(set) var customAPIBaseURL: URL?

  public var apiBaseURL: URL { customAPIBaseURL ?? defaultAPIBaseURL }

  /// Remembered from the first explicit choice; 128 kbps until then (web behaviour).
  public var quality: PlaybackQuality {
    didSet { storage.setString(quality.rawValue, forKey: Self.qualityKey) }
  }

  /// Player volume, 0…1 (out-of-range writes are stored clamped; read `clampedVolume`).
  public var volume: Double {
    didSet { storage.setString(String(clampedVolume), forKey: Self.volumeKey) }
  }

  public var clampedVolume: Double { volume.isFinite ? min(1, max(0, volume)) : 1 }

  @ObservationIgnored private let storage: SettingsStorage

  public init(storage: SettingsStorage, defaultAPIBaseURL: URL) {
    self.storage = storage
    self.defaultAPIBaseURL = APIBaseURL.parse(defaultAPIBaseURL.absoluteString) ?? defaultAPIBaseURL
    customAPIBaseURL = storage.string(forKey: Self.apiBaseURLKey).flatMap(APIBaseURL.parse)
    quality = storage.string(forKey: Self.qualityKey).flatMap(PlaybackQuality.init(rawValue:)) ?? .default
    if let stored = storage.string(forKey: Self.volumeKey).flatMap(Double.init), stored.isFinite,
      (0...1).contains(stored)
    {
      volume = stored
    } else {
      volume = 1
    }
  }

  public enum ServerURLError: Error, Equatable {
    case invalid
  }

  /// Validates and stores a server URL. An empty string (or the default itself)
  /// clears the override.
  public func setAPIBaseURL(_ input: String) throws(ServerURLError) {
    let trimmed = input.trimmingCharacters(in: .whitespacesAndNewlines)
    if trimmed.isEmpty {
      resetAPIBaseURL()
      return
    }
    guard let url = APIBaseURL.parse(trimmed) else { throw .invalid }
    if url == defaultAPIBaseURL {
      resetAPIBaseURL()
      return
    }
    customAPIBaseURL = url
    storage.setString(url.absoluteString, forKey: Self.apiBaseURLKey)
  }

  public func resetAPIBaseURL() {
    customAPIBaseURL = nil
    storage.setString(nil, forKey: Self.apiBaseURLKey)
  }
}
