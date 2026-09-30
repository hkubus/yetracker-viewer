import Foundation

/// A song to reveal when an era opens (the web's `?page=N#song-ID`).
public struct SongFocus: Hashable, Sendable {
  /// The row to scroll to and highlight, when known.
  public var songID: Int?
  /// Rows (in catalog order) that must be loaded for the focus to be in the list.
  public var loadThrough: Int
  /// 0-based row to reveal for a page-only link without a song id.
  public var rowIndex: Int?

  public init(songID: Int?, loadThrough: Int, rowIndex: Int? = nil) {
    self.songID = songID
    self.loadThrough = max(1, loadThrough)
    self.rowIndex = rowIndex
  }

  /// A song at a known 1-based era position (search results' `eraPosition`).
  public init(songID: Int, position: Int?) {
    self.init(songID: songID, loadThrough: position ?? 1)
  }

  /// A web deep link: page number (100 rows each) and optional `#song-ID`.
  public init(page: Int, songID: Int?) {
    let page = max(1, page)
    self.init(
      songID: songID,
      loadThrough: page * APIClient.eraPageSize,
      rowIndex: songID == nil ? (page - 1) * APIClient.eraPageSize : nil)
  }
}

/// Everything needed to open an era screen.
public struct EraRoute: Hashable, Sendable {
  public var eraID: Int
  public var focus: SongFocus?
  public var query: String
  public var category: SongCategory?
  public var sort: SongSort

  public init(
    eraID: Int,
    focus: SongFocus? = nil,
    query: String = "",
    category: SongCategory? = nil,
    sort: SongSort = .default
  ) {
    self.eraID = eraID
    self.focus = focus
    self.query = query
    self.category = category
    self.sort = sort
  }
}

/// Values pushed onto a tab's navigation stack.
public enum AppRoute: Hashable, Sendable {
  case era(EraRoute)
}

/// Where an incoming URL leads.
public enum DeepLinkTarget: Hashable, Sendable {
  case home
  case era(EraRoute)
}

/// Parses `yetracker://eras/12?page=3#song-45` and the web's own URLs
/// (`https://host/eras/12?page=3&q=…&category=…&sort=…#song-45`).
public enum DeepLink {
  public static let scheme = "yetracker"

  public static func parse(_ url: URL) -> DeepLinkTarget? {
    guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
      let scheme = components.scheme?.lowercased()
    else { return nil }

    var segments: [String]
    switch scheme {
    case Self.scheme:
      // `yetracker://eras/12` puts "eras" in the host; `yetracker:///eras/12` in the path.
      segments = components.host.map { [$0] } ?? []
    case "http", "https":
      segments = []
    default:
      return nil
    }
    segments += components.path.split(separator: "/").map(String.init)
    segments = segments.filter { !$0.isEmpty }

    if segments.isEmpty || segments == ["home"] { return .home }
    guard segments.count == 2, segments[0] == "eras", let eraID = positiveInteger(segments[1]) else { return nil }

    let items = components.queryItems ?? []
    func value(_ name: String) -> String? { items.last(where: { $0.name == name })?.value }

    let page = value("page").flatMap(positiveInteger) ?? 1
    let songID = components.fragment.flatMap { fragment -> Int? in
      guard fragment.hasPrefix("song-") else { return nil }
      return positiveInteger(String(fragment.dropFirst("song-".count)))
    }
    // The web page slices the raw query to 200 characters before sending it on.
    let query = String((value("q") ?? "").trimmingCharacters(in: .whitespacesAndNewlines).prefix(200))
    let category = value("category").flatMap { SongCategory(rawValue: $0.trimmingCharacters(in: .whitespaces)) }
    let sort = SongSort(normalizing: value("sort"))
    let focus = (songID != nil || page > 1) ? SongFocus(page: page, songID: songID) : nil

    return .era(EraRoute(eraID: eraID, focus: focus, query: query, category: category, sort: sort))
  }

  /// `positiveInteger` from the API: ASCII digits, at least 1.
  static func positiveInteger(_ value: String) -> Int? {
    guard !value.isEmpty, value.allSatisfy({ $0.isASCII && $0.isNumber }), let parsed = Int(value), parsed >= 1
    else { return nil }
    return parsed
  }
}
