import Foundation

/// Emoji categories for `GET /eras/:id/songs?category=`. Mirrors
/// `apps/web/src/songCategories.ts` (the API matches on the emoji's base codepoint).
public enum SongCategory: String, CaseIterable, Identifiable, Hashable, Sendable {
  case bestOf = "best-of"
  case special
  case grails
  case wanted
  case worstOf = "worst-of"
  case ai

  public var id: String { rawValue }

  public var label: String {
    switch self {
    case .bestOf: "Best Of"
    case .special: "Special"
    case .grails: "Grails"
    case .wanted: "Wanted"
    case .worstOf: "Worst Of"
    case .ai: "AI"
    }
  }

  public var emoji: String {
    switch self {
    case .bestOf: "⭐"
    case .special: "✨"
    case .grails: "🏆"
    case .wanted: "🏅"
    case .worstOf: "🗑️"
    case .ai: "🤖"
    }
  }

  /// "⭐ Best Of", as in the web's category select.
  public var title: String { "\(emoji) \(label)" }
}

/// Sort keys for song listings. Mirrors `SORT_KEYS` in `apps/api-rs/src/request.rs`
/// and `apps/web/src/songSorts.ts`.
public enum SongSort: String, CaseIterable, Identifiable, Hashable, Sendable {
  case catalog = "id"
  case category
  case leakNewest = "leak-newest"
  case leakOldest = "leak-oldest"
  case fileNewest = "file-newest"
  case name

  public static let `default`: SongSort = .catalog

  public var id: String { rawValue }

  public var label: String {
    switch self {
    case .catalog: "Catalog order"
    case .category: "Category (best first)"
    case .leakNewest: "Newest leak"
    case .leakOldest: "Oldest leak"
    case .fileNewest: "Newest file"
    case .name: "Title A–Z"
    }
  }

  /// Web `normalizeSongSort`: unknown values fall back to catalog order.
  public init(normalizing value: String?) {
    let trimmed = (value ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
    self = SongSort(rawValue: trimmed) ?? .default
  }
}

/// The player's quality menu. Raw values match the web `<select>` (and its
/// `yetracker:quality` localStorage values): `""` is the stored original file.
public enum PlaybackQuality: String, CaseIterable, Identifiable, Hashable, Sendable {
  case original = ""
  case kbps64 = "64"
  case kbps128 = "128"
  case kbps192 = "192"
  case kbps256 = "256"
  case kbps320 = "320"

  /// 128 kbps for first-time users, as on the web.
  public static let `default`: PlaybackQuality = .kbps128

  public var id: String { rawValue }

  /// Transcode bitrate in kbps; `nil` streams the stored file.
  public var bitrate: Int? { Int(rawValue) }

  public var label: String {
    guard let bitrate else { return "Original" }
    return "\(bitrate) kbps"
  }
}
