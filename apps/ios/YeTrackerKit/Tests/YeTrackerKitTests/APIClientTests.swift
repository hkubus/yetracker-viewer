import Foundation
import Testing

@testable import YeTrackerKit

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

@Suite("APIClient")
struct APIClientTests {
  let http = FakeHTTPClient()

  func client(_ base: String = "http://test.local") -> APIClient {
    APIClient(baseURL: URL(string: base)!, http: http)
  }

  @Test func endpointsKeepBasePathPrefix() {
    let api = client("https://example.com/api/")
    #expect(api.endpoint("/eras").absoluteString == "https://example.com/api/eras")
    #expect(api.endpoint("eras/1").absoluteString == "https://example.com/api/eras/1")
    #expect(
      api.coverURL(eraID: 3, version: "abc123").absoluteString == "https://example.com/api/eras/3/cover?v=abc123")
    #expect(api.coverURL(eraID: 3, version: "  ").absoluteString == "https://example.com/api/eras/3/cover")
    #expect(api.downloadURL(songID: 9).absoluteString == "https://example.com/api/songs/9/download")
    #expect(api.streamURL(songID: 9).absoluteString == "https://example.com/api/songs/9/stream")
  }

  @Test func transcodeURLs() {
    let api = client()
    #expect(
      api.streamURL(songID: 5, transcode: Transcode(bitrate: 128)).absoluteString
        == "http://test.local/songs/5/stream?quality=128&format=aac")
    #expect(
      api.streamURL(songID: 5, transcode: Transcode(bitrate: 64, format: .opus, start: 93.5)).absoluteString
        == "http://test.local/songs/5/stream?quality=64&start=93.5")
    #expect(
      api.streamURL(songID: 5, transcode: Transcode(bitrate: 999, start: 12.0004)).absoluteString
        == "http://test.local/songs/5/stream?quality=320&format=aac&start=12")
    // Only the bitrates the API accepts: the nearest one, the lower on a tie.
    #expect(Transcode(bitrate: 32).queryItems.first! == ("quality", "64"))
    #expect(Transcode(bitrate: 96).queryItems.first! == ("quality", "64"))
    #expect(Transcode(bitrate: 97).queryItems.first! == ("quality", "128"))
    #expect(Transcode(bitrate: 250).queryItems.first! == ("quality", "256"))
  }

  @Test func startSecondsNeverUseExponents() {
    #expect(Transcode.formatSeconds(0.25) == "0.25")
    #expect(Transcode.formatSeconds(1e-7) == "0")
    #expect(Transcode.formatSeconds(12345.6789) == "12345.679")
    #expect(Transcode.formatSeconds(60) == "60")
  }

  @Test func queryEncodingMatchesURLSearchParams() {
    #expect(QueryEncoding.escape("c++ & you") == "c%2B%2B%20%26%20you")
    #expect(QueryEncoding.escape("⭐ love") == "%E2%AD%90%20love")
    #expect(QueryEncoding.escape("a-b_c.d~e") == "a-b_c.d~e")
    #expect(QueryEncoding.encode([("q", "a=b"), ("limit", "5")]) == "q=a%3Db&limit=5")
  }

  @Test func eraSongsRequestParameters() {
    let request = EraSongsRequest(limit: 900, offset: 200, query: "love", category: .bestOf, sort: .leakNewest)
    let items = request.queryItems.map { "\($0.0)=\($0.1)" }
    #expect(items == ["limit=500", "offset=200", "q=love", "category=best-of", "sort=leak-newest"])
    #expect(EraSongsRequest().queryItems.map(\.0) == ["limit", "offset"])
  }

  @Test func searchRequestParameters() {
    let request = SongSearchRequest(query: "love", eraFrom: 3, eraTo: 9, playableOnly: true)
    #expect(
      request.queryItems.map { "\($0.0)=\($0.1)" } == ["limit=50", "q=love", "eraFrom=3", "eraTo=9", "playable=true"])
    #expect(SongSearchRequest(query: "x", limit: 500).queryItems.first?.1 == "50")
  }

  @Test func erasAreFetchedAndDecoded() async throws {
    http.on("/eras", respond: try .fixture("eras"))
    let eras = try await client().eras()
    #expect(eras.count == 43)
    let request = try #require(http.requests.first)
    #expect(request.value(forHTTPHeaderField: "Accept") == "application/json")
    #expect(request.timeoutInterval == 10)
  }

  @Test func eraSongsReadTotalHeader() async throws {
    http.on("/eras/31/songs", respond: try .fixture("era-31-songs-nebraska", headers: ["X-Total-Count": "956"]))
    let page = try await client().eraSongs(eraID: 31, request: EraSongsRequest(offset: 100))
    #expect(page.total == 956)
    #expect(page.songs.count == 6)
    #expect(page.offset == 100)
    #expect(http.requests.first?.query == ["limit": "100", "offset": "100"])
  }

  @Test func missingTotalHeaderFallsBackToRowsSeen() async throws {
    http.on("/eras/31/songs", respond: try .fixture("era-31-songs-nebraska"))
    let page = try await client().eraSongs(eraID: 31, request: EraSongsRequest(offset: 100))
    #expect(page.total == 106)
  }

  @Test func routeErrorsCarryPlainTextMessage() async {
    http.on("/eras/999", respond: .text("Era does not exist", status: 404))
    await #expect(throws: APIError.http(status: 404, message: "Era does not exist")) {
      _ = try await client().era(id: 999)
    }
  }

  @Test func unknownRoutesCarryJSONMessage() async {
    await #expect(throws: APIError.http(status: 404, message: "Not found")) {
      _ = try await client().era(id: 1)
    }
  }

  @Test func htmlErrorBodiesAreNotShown() {
    let response = HTTPResponse(status: 502, headers: [:], body: Data("<html>Bad gateway</html>".utf8))
    #expect(APIClient.errorMessage(from: response) == "")
    #expect(APIError.http(status: 502, message: "").userMessage == "The server answered with status 502.")
  }

  @Test func malformedJSONIsADecodingError() async {
    http.on("/eras/1", respond: .json("{nope"))
    do {
      _ = try await client().era(id: 1)
      Issue.record("expected a decoding error")
    } catch let error as APIError {
      guard case .decoding = error else {
        Issue.record("unexpected \(error)")
        return
      }
    } catch {
      Issue.record("unexpected \(error)")
    }
  }

  @Test func durationAndHealth() async throws {
    http.on("/songs/5/duration", respond: .json(#"{"duration":65.12}"#))
    http.on("/songs/6/duration", respond: .json(#"{"duration":0}"#))
    http.on("/health", respond: .json(#"{"status":"ok"}"#))
    #expect(try await client().duration(songID: 5) == 65.12)
    #expect(try await client().duration(songID: 6) == nil)
    try await client().health()
    #expect(http.requests(to: "/health").first?.cachePolicy == .reloadIgnoringLocalCacheData)
  }

  @Test func recentLeaksUseTheHomePageQuery() async throws {
    http.on("/songs", respond: try .fixture("recent-leaks"))
    let songs = try await client().recentLeaks()
    #expect(!songs.isEmpty)
    #expect(http.requests.first?.query == ["playable": "true", "sort": "leak-newest", "limit": "8"])
  }

  @Test func transportErrorsAreClassified() {
    #expect(APIError(transportError: URLError(.timedOut)) == .timedOut)
    #expect(APIError(transportError: URLError(.notConnectedToInternet)) == .offline)
    #expect(APIError(transportError: URLError(.cannotConnectToHost)) == .unreachable)
    #expect(APIError(transportError: URLError(.cancelled)) == .cancelled)
    #expect(
      APIError(transportError: URLError(.appTransportSecurityRequiresSecureConnection)) == .insecureConnectionBlocked)
    #expect(APIError(transportError: URLError(.serverCertificateUntrusted)) == .secureConnectionFailed)
    #expect(APIError(transportError: CancellationError()) == .cancelled)
    #expect(APIError.http(status: 404, message: "x").isNotFound)
  }

  @Test func baseURLValidation() {
    #expect(APIBaseURL.parse("example.com/api/")?.absoluteString == "https://example.com/api")
    #expect(APIBaseURL.parse(" http://127.0.0.1:3000/ ")?.absoluteString == "http://127.0.0.1:3000")
    #expect(APIBaseURL.parse("HTTPS://Example.com?x=1#y")?.absoluteString == "https://Example.com")
    #expect(APIBaseURL.parse("ftp://example.com") == nil)
    #expect(APIBaseURL.parse("http://") == nil)
    #expect(APIBaseURL.parse("exa mple.com") == nil)
    #expect(APIBaseURL.parse("") == nil)
  }
}
