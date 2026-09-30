import Foundation
import Testing

@testable import YeTrackerKit

/// Runs the real client against a running API. Opt-in:
///
///     YT_LIVE_API_URL=http://127.0.0.1:3000 swift test --filter LiveAPITests
private let liveBaseURL = ProcessInfo.processInfo.environment["YT_LIVE_API_URL"].flatMap(APIBaseURL.parse)

@Suite("Live API", .enabled(if: liveBaseURL != nil, "set YT_LIVE_API_URL to run"))
struct LiveAPITests {
  let api = APIClient(baseURL: liveBaseURL ?? testBaseURL)

  @Test func healthAndCatalog() async throws {
    try await api.health()
    let eras = try await api.eras(reload: true)
    #expect(!eras.isEmpty)
    let era = try #require(eras.first)
    let detail = try await api.era(id: era.id)
    #expect(detail.id == era.id)
    #expect(detail.name == era.name)
  }

  @Test func eraSongsPageAndTotal() async throws {
    let era = try #require(try await api.eras().max { ($0.songsCount ?? 0) < ($1.songsCount ?? 0) })
    let page = try await api.eraSongs(eraID: era.id, request: EraSongsRequest(limit: 100, offset: 0))
    #expect(page.total == era.songsCount)
    #expect(page.songs.count == min(100, page.total))
    let sorted = try await api.eraSongs(eraID: era.id, request: EraSongsRequest(limit: 5, sort: .leakNewest))
    let dates = sorted.songs.compactMap(\.leakDate).filter { $0 > 0 }
    #expect(dates == dates.sorted(by: >))
    let starred = try await api.eraSongs(eraID: era.id, request: EraSongsRequest(limit: 20, category: .bestOf))
    // The API matches the base code point, so "⭐️" (with a variation selector) counts too.
    #expect(starred.songs.allSatisfy { TextNormalization.contains($0.name ?? "", "\u{2B50}") })
  }

  @Test func searchAndSpecialCharacters() async throws {
    let response = try await api.searchSongs(SongSearchRequest(query: "love"))
    #expect(response.songs.count <= 50)
    #expect(response.total >= response.songs.count)
    // `+` and `&` must reach the server literally.
    _ = try await api.searchSongs(SongSearchRequest(query: "c++ & more"))
    let leaks = try await api.recentLeaks()
    #expect(leaks.allSatisfy { $0.isPlayable })
  }

  @Test func routeErrorsAreReadable() async throws {
    await #expect(throws: APIError.http(status: 404, message: "Era does not exist")) {
      _ = try await api.era(id: 999_999_999)
    }
    await #expect(throws: APIError.http(status: 400, message: "Search query is too long")) {
      var request = SongSearchRequest()
      request.query = String(repeating: "x", count: 101)
      _ = try await api.searchSongs(request)
    }
  }

  @Test func eraDetailModelEndToEnd() async throws {
    let provider: APIProvider = { [api] in api }
    let eras = try await api.eras()
    let era = try #require(eras.first)
    let summary = await Self.loadModel(eraID: era.id, provider: provider)
    #expect(summary.phase == .loaded)
    #expect(summary.total == era.songsCount)
    #expect(summary.count == min(100, era.songsCount ?? 0))
  }

  @MainActor
  static func loadModel(
    eraID: Int, provider: @escaping APIProvider
  ) async -> (
    phase: EraDetailModel.Phase, count: Int, total: Int
  ) {
    let model = EraDetailModel(route: EraRoute(eraID: eraID), directory: EraDirectory(api: provider), api: provider)
    await model.load()
    return (model.phase, model.songs.count, model.total)
  }
}
