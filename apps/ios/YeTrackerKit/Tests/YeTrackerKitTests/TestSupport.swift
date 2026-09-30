import Foundation
import Testing

@testable import YeTrackerKit

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

// MARK: - Fixtures

enum Fixture {
  static func data(_ name: String) throws -> Data {
    let url = try #require(
      Bundle.module.url(forResource: name, withExtension: "json", subdirectory: "Fixtures"),
      "missing fixture \(name).json")
    return try Data(contentsOf: url)
  }

  static func decode<T: Decodable>(_ type: T.Type, _ name: String) throws -> T {
    try JSONDecoder().decode(T.self, from: data(name))
  }
}

// MARK: - HTTP

extension HTTPResponse {
  static func json(_ body: Data, status: Int = 200, headers: [String: String] = [:]) -> HTTPResponse {
    HTTPResponse(
      status: status, headers: headers.merging(["Content-Type": "application/json"]) { a, _ in a }, body: body)
  }

  static func json(_ string: String, status: Int = 200, headers: [String: String] = [:]) -> HTTPResponse {
    json(Data(string.utf8), status: status, headers: headers)
  }

  static func fixture(_ name: String, headers: [String: String] = [:]) throws -> HTTPResponse {
    json(try Fixture.data(name), headers: headers)
  }

  static func text(_ message: String, status: Int) -> HTTPResponse {
    HTTPResponse(status: status, headers: ["Content-Type": "text/plain;charset=UTF-8"], body: Data(message.utf8))
  }
}

/// Canned responses keyed by path. Unmatched requests get the API's JSON 404.
final class FakeHTTPClient: HTTPClient, @unchecked Sendable {
  typealias Responder = @Sendable (URLRequest) async throws -> HTTPResponse

  private let lock = NSLock()
  private var routes: [(path: String, responder: Responder)] = []
  private var recorded: [URLRequest] = []

  var requests: [URLRequest] { lock.withLock { recorded } }

  func requests(to path: String) -> [URLRequest] {
    requests.filter { $0.url?.path == path }
  }

  /// Later registrations win, so tests can override a default.
  func on(_ path: String, _ responder: @escaping Responder) {
    lock.withLock { routes.append((path, responder)) }
  }

  func on(_ path: String, respond response: HTTPResponse) {
    on(path) { _ in response }
  }

  func send(_ request: URLRequest) async throws -> HTTPResponse {
    let responder: Responder? = lock.withLock {
      recorded.append(request)
      return routes.last(where: { $0.path == request.url?.path })?.responder
    }
    guard let responder else { return .json(#"{"error":"Not found"}"#, status: 404) }
    return try await responder(request)
  }
}

extension URLRequest {
  /// Query items as a dictionary (last value wins).
  var query: [String: String] {
    guard let url, let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems else { return [:] }
    var result: [String: String] = [:]
    for item in items { result[item.name] = item.value ?? "" }
    return result
  }
}

/// A one-shot gate a fake responder can wait on, so tests control response order.
final class Gate: @unchecked Sendable {
  private let lock = NSLock()
  private var isOpen = false
  private var waiters: [CheckedContinuation<Void, Never>] = []

  func wait() async {
    await withCheckedContinuation { continuation in
      let resumeNow = lock.withLock { () -> Bool in
        if isOpen { return true }
        waiters.append(continuation)
        return false
      }
      if resumeNow { continuation.resume() }
    }
  }

  func open() {
    let pending = lock.withLock { () -> [CheckedContinuation<Void, Never>] in
      isOpen = true
      defer { waiters = [] }
      return waiters
    }
    pending.forEach { $0.resume() }
  }
}

// MARK: - Clients and models

let testBaseURL = URL(string: "http://test.local")!

@MainActor
func makeAPI(_ http: FakeHTTPClient, base: URL = testBaseURL) -> APIProvider {
  let client = APIClient(baseURL: base, http: http)
  return { client }
}

/// Returns immediately: debounces fire on the next suspension point.
let immediateSleeper: Sleeper = { _ in await Task.yield() }

/// Never returns until cancelled.
let foreverSleeper: Sleeper = { _ in try await Task.sleep(for: .seconds(3600)) }

/// Lets queued main-actor work run.
@MainActor
func settle(_ rounds: Int = 20) async {
  for _ in 0..<rounds { await Task.yield() }
}

// MARK: - Playback

@MainActor
final class FakePlaybackEngine: PlaybackEngine {
  enum Call: Equatable {
    case load(URL, autoplay: Bool)
    case play
    case pause
    case seek(Double)
    case stop
  }

  var eventHandler: (@MainActor @Sendable (PlaybackEngineEvent) -> Void)?
  var volume: Double = 1
  private(set) var calls: [Call] = []
  private(set) var loaded: [PlaybackSource] = []

  var lastSource: PlaybackSource? { loaded.last }

  func load(_ source: PlaybackSource, autoplay: Bool) {
    loaded.append(source)
    calls.append(.load(source.url, autoplay: autoplay))
  }

  func play() { calls.append(.play) }
  func pause() { calls.append(.pause) }
  func seek(to seconds: Double) { calls.append(.seek(seconds)) }
  func stop() { calls.append(.stop) }

  func emit(_ event: PlaybackEngineEvent) { eventHandler?(event) }
}

@MainActor
final class RecordingNowPlaying: NowPlayingSink {
  private(set) var updates: [NowPlayingInfo?] = []
  var last: NowPlayingInfo? { updates.last ?? nil }
  func update(_ info: NowPlayingInfo?) { updates.append(info) }
}

@MainActor
func makeSettings(_ values: [String: String] = [:]) -> AppSettings {
  AppSettings(storage: InMemorySettingsStorage(values), defaultAPIBaseURL: testBaseURL)
}

func track(_ id: Int, era: Int = 1, duration: Double? = nil) -> Track {
  Track(
    id: id, title: "Song \(id)", eraID: era, eraName: "Era \(era)", colorHex: "336699", coverVersion: "v\(era)",
    durationHint: duration)
}

func eraJSON(_ id: Int, name: String, songs: Int = 10, color: String = "666666") -> String {
  #"{"id":\#(id),"name":"\#(name)","notes":"","description":"","dominantColor":"\#(color)","songsCount":\#(songs),"coverVersion":"cv\#(id)"}"#
}

func songJSON(_ id: Int, era: Int, name: String? = nil, playable: Bool = false, notes: String = "") -> String {
  let title = name ?? "Song \(id)"
  return
    #"{"id":\#(id),"eraId":\#(era),"catalogId":"unreleased","name":"\#(title)","notes":"\#(notes)","fileDate":0,"leakDate":0,"availableLength":"Full","trackLength":120,"quality":"CD Quality","url":"https://pillows.su/f/\#(id)","downloaded":\#(playable ? "1" : "null"),"playable":\#(playable),"duration":\#(playable ? "120.5" : "null")}"#
}

func songsPage(_ ids: ClosedRange<Int>, era: Int, playable: Bool = false) -> String {
  "[" + ids.map { songJSON($0, era: era, playable: playable) }.joined(separator: ",") + "]"
}
