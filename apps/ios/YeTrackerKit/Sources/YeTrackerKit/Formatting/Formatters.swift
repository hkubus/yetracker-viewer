import Foundation

/// Display formatting shared by every screen. Mirrors `convertDuration`,
/// `formatLength` and the date helpers in `apps/web`.
public enum Formatters {
  /// "m:ss", or "—" for unknown/invalid values (web `convertDuration`).
  public static func duration(_ seconds: Double?) -> String {
    guard let seconds, seconds.isFinite, seconds >= 0 else { return "—" }
    let total = Int(seconds.rounded(.down))
    return "\(total / 60):\(String(format: "%02d", total % 60))"
  }

  /// "Snippet - 1:05": availability and a positive duration, either optional.
  public static func lengthLabel(availableLength: String?, seconds: Double?) -> String {
    var parts: [String] = []
    if let availableLength, !availableLength.isEmpty { parts.append(availableLength) }
    if let seconds, seconds.isFinite, seconds > 0 { parts.append(duration(seconds)) }
    return parts.joined(separator: " - ")
  }

  /// A catalog date as a `Date`, or `nil` when missing (`0`) or invalid.
  public static func date(fromUnix value: Int?) -> Date? {
    guard let value, value > 0 else { return nil }
    return Date(timeIntervalSince1970: TimeInterval(value))
  }

  /// Short numeric date ("9/30/26" in en-US) for the song rows, or `nil` when unknown.
  ///
  /// Catalog dates are stored as UTC midnight, so they are formatted in UTC;
  /// the local zone would show the previous day west of Greenwich.
  public static func shortDate(_ value: Int?, locale: Locale = .current) -> String? {
    guard let date = date(fromUnix: value) else { return nil }
    return date.formatted(
      Date.FormatStyle(locale: locale, calendar: utcCalendar, timeZone: utc)
        .month(.defaultDigits).day(.defaultDigits).year(.twoDigits))
  }

  /// Medium date ("Sep 30, 2026" in en-US), or "Unknown date" (web `RecentLeaks`).
  public static func mediumDate(_ value: Int?, locale: Locale = .current) -> String {
    guard let date = date(fromUnix: value) else { return "Unknown date" }
    return date.formatted(
      Date.FormatStyle(locale: locale, calendar: utcCalendar, timeZone: utc)
        .month(.abbreviated).day(.defaultDigits).year(.defaultDigits))
  }

  /// Elapsed-time description for VoiceOver ("1:05 elapsed").
  public static func elapsedDescription(_ seconds: Double) -> String {
    guard seconds.isFinite, seconds >= 0 else { return "unknown position" }
    return "\(duration(seconds)) elapsed"
  }

  private static let utc = TimeZone(identifier: "UTC") ?? TimeZone(secondsFromGMT: 0)!

  private static var utcCalendar: Calendar {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = utc
    return calendar
  }
}

/// Search-text helpers shared with the API's normalisation rules.
public enum TextNormalization {
  /// The API rejects longer queries with `400 Search query is too long`.
  public static let maxQueryLength = 100

  /// Trims and collapses every whitespace run to one space.
  public static func collapseWhitespace(_ value: String) -> String {
    value.split(whereSeparator: { $0.isWhitespace || $0.isNewline }).joined(separator: " ")
  }

  /// Web `normalize`: collapse whitespace, lowercase.
  public static func normalize(_ value: String) -> String {
    collapseWhitespace(value).lowercased()
  }

  /// A query the API accepts: normalised and cut to `maxQueryLength` characters.
  public static func apiQuery(_ value: String) -> String {
    let normalized = normalize(value)
    guard normalized.count > maxQueryLength else { return normalized }
    return String(normalized.prefix(maxQueryLength)).trimmingCharacters(in: .whitespaces)
  }

  /// Cuts to `limit` characters, ending with "…" when shortened (web `truncateDescription`).
  public static func truncate(_ value: String, limit: Int) -> String {
    guard limit > 1, value.count > limit else { return value }
    let prefix = value.prefix(limit - 1)
    return String(prefix).trimmingCharacters(in: .whitespaces) + "…"
  }

  /// Substring test on UTF-8 bytes, like SQL `LIKE`/`instr` and JS `includes`.
  ///
  /// `String.contains` compares grapheme clusters, so "⭐" would not be found in
  /// "⭐️" (with a variation selector) and "love" not in "love\u{301}"; the API
  /// matches both, and the instant filters must agree with it.
  public static func contains(_ haystack: some StringProtocol, _ needle: some StringProtocol) -> Bool {
    let needleBytes = Array(needle.utf8)
    guard !needleBytes.isEmpty else { return true }
    return Array(haystack.utf8).firstRange(of: needleBytes) != nil
  }

  /// Plural helper: "1 song", "2 songs".
  public static func count(_ value: Int, _ singular: String, _ plural: String) -> String {
    "\(value.formatted()) \(value == 1 ? singular : plural)"
  }
}
