import Foundation

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

/// Typed access to the YeTracker API (`API.md`, `apps/api-rs/tests/*.test.mjs`).
public struct APIClient: Sendable {
  /// Same bound as the web's SSR fetches.
  public static let requestTimeout: TimeInterval = 10
  /// `ERA_PAGE_SIZE` in `apps/web/src/config.ts`; deep-link page numbers assume it.
  public static let eraPageSize = 100
  /// `GET /eras/:id/songs` clamps `limit` to this.
  public static let maxEraSongsLimit = 500
  /// `GET /songs` search mode clamps `limit` to this.
  public static let maxSearchLimit = 50

  public let baseURL: URL
  private let http: any HTTPClient
  private let userAgent: String

  public init(baseURL: URL, http: any HTTPClient = URLSessionHTTPClient(), userAgent: String = "YeTracker-iOS/1.0") {
    self.baseURL = baseURL
    self.http = http
    self.userAgent = userAgent
  }

  // MARK: - URLs

  /// `apiUrl(base, path)` from the web: keeps any path prefix on the base (e.g. `/api`).
  public func endpoint(_ path: String, query: [(String, String)] = []) -> URL {
    var base = baseURL.absoluteString
    while base.hasSuffix("/") { base.removeLast() }
    let cleanPath = path.hasPrefix("/") ? String(path.dropFirst()) : path
    var string = "\(base)/\(cleanPath)"
    if !query.isEmpty { string += "?\(QueryEncoding.encode(query))" }
    return URL(string: string) ?? baseURL
  }

  /// Era artwork (AVIF). The version makes the URL immutable for caching.
  public func coverURL(eraID: Int, version: String?) -> URL {
    let version = version?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    return endpoint("/eras/\(eraID)/cover", query: version.isEmpty ? [] : [("v", version)])
  }

  /// The stored file (range-capable), or a live transcode.
  public func streamURL(songID: Int, transcode: Transcode? = nil) -> URL {
    endpoint("/songs/\(songID)/stream", query: transcode?.queryItems ?? [])
  }

  /// The stored file as an attachment (`Content-Disposition` carries the name).
  public func downloadURL(songID: Int) -> URL {
    endpoint("/songs/\(songID)/download")
  }

  // MARK: - Endpoints

  /// `GET /eras`: main eras in catalog order, with `songsCount`.
  public func eras(reload: Bool = false) async throws -> [Era] {
    let response = try await get("/eras", reload: reload)
    return try decode(LossyArray<Era>.self, from: response).elements
  }

  /// `GET /eras/:id`. Throws `.http(404, "Era does not exist")` for unknown ids.
  public func era(id: Int, reload: Bool = false) async throws -> Era {
    let response = try await get("/eras/\(id)", reload: reload)
    return try decode(Era.self, from: response)
  }

  /// `GET /eras/:id/songs` with the total from `X-Total-Count`.
  public func eraSongs(eraID: Int, request: EraSongsRequest, reload: Bool = false) async throws -> EraSongsPage {
    let response = try await get("/eras/\(eraID)/songs", query: request.queryItems, reload: reload)
    let songs = try decode(LossyArray<EraSong>.self, from: response).elements
    let headerTotal = response.header("x-total-count").flatMap { Int($0.trimmingCharacters(in: .whitespaces)) }
    let total = max(0, headerTotal ?? (request.offset + songs.count))
    return EraSongsPage(songs: songs, total: total, offset: request.offset)
  }

  /// `GET /songs` in search/filter mode.
  public func searchSongs(_ request: SongSearchRequest) async throws -> SearchResponse {
    let response = try await get("/songs", query: request.queryItems)
    return try decode(SearchResponse.self, from: response)
  }

  /// The home page's "Recently leaked" strip: newest playable leaks.
  public func recentLeaks(limit: Int = 8, reload: Bool = false) async throws -> [SearchSong] {
    var request = SongSearchRequest()
    request.playableOnly = true
    request.sort = .leakNewest
    request.limit = limit
    let response = try await get("/songs", query: request.queryItems, reload: reload)
    return try decode(SearchResponse.self, from: response).songs
  }

  /// `GET /songs/:id/duration`: probed length in seconds, or `nil` when the server has none.
  public func duration(songID: Int) async throws -> Double? {
    struct Body: Decodable {
      let duration: Double?
    }
    let response = try await get("/songs/\(songID)/duration")
    guard let value = try decode(Body.self, from: response).duration, value.isFinite, value > 0 else { return nil }
    return value
  }

  /// `GET /health`; succeeds only for `{"status":"ok"}`.
  public func health() async throws {
    struct Body: Decodable {
      let status: String?
    }
    let response = try await get("/health", cachePolicy: .reloadIgnoringLocalCacheData)
    guard (try? decode(Body.self, from: response))?.status == "ok" else {
      throw APIError.decoding("The server did not report a healthy status.")
    }
  }

  // MARK: - Plumbing

  /// `reload` (pull to refresh) skips the HTTP cache's freshness but still
  /// revalidates what it holds (see `URLSessionHTTPClient`), so an unchanged
  /// list costs a `304` instead of the whole body.
  private func get(_ path: String, query: [(String, String)] = [], reload: Bool = false) async throws -> HTTPResponse {
    try await get(path, query: query, cachePolicy: reload ? .reloadRevalidatingCacheData : .useProtocolCachePolicy)
  }

  private func get(
    _ path: String, query: [(String, String)] = [], cachePolicy: URLRequest.CachePolicy
  ) async throws -> HTTPResponse {
    var request = URLRequest(
      url: endpoint(path, query: query), cachePolicy: cachePolicy, timeoutInterval: Self.requestTimeout)
    request.httpMethod = "GET"
    request.setValue("application/json", forHTTPHeaderField: "Accept")
    request.setValue(userAgent, forHTTPHeaderField: "User-Agent")

    let response: HTTPResponse
    do {
      response = try await http.send(request)
    } catch {
      throw APIError(transportError: error)
    }
    guard (200..<300).contains(response.status) else {
      throw APIError.http(status: response.status, message: Self.errorMessage(from: response))
    }
    return response
  }

  private func decode<T: Decodable>(_ type: T.Type, from response: HTTPResponse) throws -> T {
    do {
      return try JSONDecoder().decode(T.self, from: response.body)
    } catch {
      throw APIError.decoding(String(describing: error))
    }
  }

  /// Route errors are `text/plain` messages; unknown routes send `{"error":"Not found"}`.
  static func errorMessage(from response: HTTPResponse) -> String {
    struct JSONError: Decodable {
      let error: String?
    }
    if let parsed = try? JSONDecoder().decode(JSONError.self, from: response.body), let message = parsed.error.nonBlank
    {
      return message
    }
    let text = String(decoding: response.body, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
    // Never surface an HTML error page or a huge body as a message.
    guard !text.isEmpty, !text.hasPrefix("<"), text.count <= 300 else { return "" }
    return text
  }
}

/// One page of an era's songs.
public struct EraSongsPage: Hashable, Sendable {
  public let songs: [EraSong]
  /// Songs matching the request before `limit`/`offset`.
  public let total: Int
  public let offset: Int

  public init(songs: [EraSong], total: Int, offset: Int) {
    self.songs = songs
    self.total = total
    self.offset = offset
  }
}

/// Query for `GET /eras/:id/songs`.
public struct EraSongsRequest: Hashable, Sendable {
  public var limit: Int
  public var offset: Int
  /// Normalised search text (`TextNormalization.apiQuery`); empty for none.
  public var query: String
  public var category: SongCategory?
  public var sort: SongSort

  public init(
    limit: Int = APIClient.eraPageSize,
    offset: Int = 0,
    query: String = "",
    category: SongCategory? = nil,
    sort: SongSort = .default
  ) {
    self.limit = limit
    self.offset = offset
    self.query = query
    self.category = category
    self.sort = sort
  }

  /// Same parameters, in the same order, as the web era page.
  public var queryItems: [(String, String)] {
    var items: [(String, String)] = [
      ("limit", String(min(max(1, limit), APIClient.maxEraSongsLimit))),
      ("offset", String(max(0, offset))),
    ]
    if !query.isEmpty { items.append(("q", query)) }
    if let category { items.append(("category", category.rawValue)) }
    if sort != .default { items.append(("sort", sort.rawValue)) }
    return items
  }
}

/// Query for `GET /songs` in search/filter mode.
public struct SongSearchRequest: Hashable, Sendable {
  public var query: String
  public var eraFrom: Int?
  public var eraTo: Int?
  public var playableOnly: Bool
  public var sort: SongSort?
  public var limit: Int

  public init(
    query: String = "",
    eraFrom: Int? = nil,
    eraTo: Int? = nil,
    playableOnly: Bool = false,
    sort: SongSort? = nil,
    limit: Int = APIClient.maxSearchLimit
  ) {
    self.query = query
    self.eraFrom = eraFrom
    self.eraTo = eraTo
    self.playableOnly = playableOnly
    self.sort = sort
    self.limit = limit
  }

  public var queryItems: [(String, String)] {
    var items: [(String, String)] = [("limit", String(min(max(1, limit), APIClient.maxSearchLimit)))]
    if !query.isEmpty { items.append(("q", query)) }
    if let eraFrom { items.append(("eraFrom", String(eraFrom))) }
    if let eraTo { items.append(("eraTo", String(eraTo))) }
    if playableOnly { items.append(("playable", "true")) }
    if let sort { items.append(("sort", sort.rawValue)) }
    return items
  }

  /// Stable identity of the request, used to drop out-of-date responses.
  public var signature: String { QueryEncoding.encode(queryItems) }
}

/// Live transcode parameters for `/songs/:id/stream`.
public struct Transcode: Hashable, Sendable {
  public enum Format: String, Sendable {
    /// Ogg/Opus (the web's format; AVPlayer cannot play it).
    case opus
    /// ADTS AAC, playable by AVPlayer.
    case aac
  }

  public var bitrate: Int
  public var format: Format
  /// Seconds into the source to start from (restart-at-offset seeking).
  public var start: Double

  public init(bitrate: Int, format: Format = .aac, start: Double = 0) {
    self.bitrate = bitrate
    self.format = format
    self.start = start
  }

  public var queryItems: [(String, String)] {
    var items: [(String, String)] = [("quality", String(Self.supportedBitrate(bitrate)))]
    if format != .opus { items.append(("format", format.rawValue)) }
    if start.isFinite, start > 0 { items.append(("start", Self.formatSeconds(start))) }
    return items
  }

  /// Bitrates the API transcodes to (anything else is `400 Invalid quality for file`).
  public static let supportedBitrates = [64, 128, 192, 256, 320]

  /// The nearest supported bitrate (the lower one on a tie).
  static func supportedBitrate(_ bitrate: Int) -> Int {
    supportedBitrates.min { abs($0 - bitrate) < abs($1 - bitrate) } ?? 128
  }

  /// Plain decimal seconds with at most three fractional digits ("93.5", never "9.35e1").
  static func formatSeconds(_ value: Double) -> String {
    let millis = Int((value * 1000).rounded())
    let whole = millis / 1000
    let fraction = millis % 1000
    guard fraction > 0 else { return String(whole) }
    var digits = String(format: "%03d", fraction)
    while digits.hasSuffix("0") { digits.removeLast() }
    return "\(whole).\(digits)"
  }
}

/// Percent-encoding for query strings.
///
/// Only RFC 3986 unreserved characters are left as-is. In particular `+` is
/// encoded (the API's form decoder reads a bare `+` as a space) and spaces
/// become `%20`, matching what `URLSearchParams` produces for the web client.
public enum QueryEncoding {
  private static let unreserved: Set<Character> = Set(
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")

  public static func encode(_ items: [(String, String)]) -> String {
    items.map { "\(escape($0.0))=\(escape($0.1))" }.joined(separator: "&")
  }

  public static func escape(_ value: String) -> String {
    var result = ""
    for character in value {
      if unreserved.contains(character) {
        result.append(character)
      } else {
        for byte in String(character).utf8 {
          result += String(format: "%%%02X", byte)
        }
      }
    }
    return result
  }
}

/// Validation for the user-configurable server URL.
public enum APIBaseURL {
  /// Normalises user input: assumes `https://` without a scheme, requires an
  /// http(s) host, drops query/fragment and trailing slashes. Path prefixes
  /// (`https://example.com/api`) are kept, as with the web's `PUBLIC_API_URL`.
  public static func parse(_ input: String) -> URL? {
    var text = input.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !text.isEmpty, !text.contains(where: { $0.isWhitespace }) else { return nil }
    if !text.contains("://") { text = "https://\(text)" }
    guard var components = URLComponents(string: text),
      let scheme = components.scheme?.lowercased(), scheme == "http" || scheme == "https",
      let host = components.host, !host.isEmpty
    else { return nil }
    components.scheme = scheme
    components.query = nil
    components.fragment = nil
    components.user = nil
    components.password = nil
    var path = components.path
    while path.hasSuffix("/") { path.removeLast() }
    components.path = path
    return components.url
  }
}
