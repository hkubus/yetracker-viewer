import Foundation
import Testing

@testable import YeTrackerKit

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

/// A small fake of `/eras/:id/songs` over a generated era: honours limit, offset,
/// q (name substring), category (⭐ prefix) and sort (name / id).
final class FakeEraServer: @unchecked Sendable {
  let eraID: Int
  let names: [Int: String]
  let playable: Bool

  init(eraID: Int = 2, count: Int = 250, playable: Bool = false) {
    self.eraID = eraID
    self.playable = playable
    var names: [Int: String] = [:]
    for id in 1...count { names[id] = id % 10 == 0 ? "⭐ Song \(id)" : "Song \(id)" }
    self.names = names
  }

  func install(on http: FakeHTTPClient) {
    http.on(
      "/eras",
      respond: .json("[\(eraJSON(1, name: "First")),\(eraJSON(2, name: "Middle")),\(eraJSON(3, name: "Last"))]"))
    http.on("/eras/\(eraID)", respond: .json(eraJSON(eraID, name: "Middle", color: "336699")))
    http.on("/eras/\(eraID)/songs") { [self] request in
      let query = request.query
      var ids = names.keys.sorted()
      if let q = query["q"], !q.isEmpty { ids = ids.filter { names[$0]!.lowercased().contains(q) } }
      if query["category"] == "best-of" { ids = ids.filter { names[$0]!.hasPrefix("⭐") } }
      if query["sort"] == "name" { ids.sort { names[$0]!.lowercased() < names[$1]!.lowercased() } }
      let offset = Int(query["offset"] ?? "0") ?? 0
      let limit = min(Int(query["limit"] ?? "100") ?? 100, 500)
      let page = Array(ids.dropFirst(offset).prefix(limit))
      let body =
        "["
        + page.map { songJSON($0, era: self.eraID, name: self.names[$0], playable: self.playable) }
        .joined(separator: ",") + "]"
      return .json(body, headers: ["X-Total-Count": page.isEmpty ? "0" : String(ids.count)])
    }
  }
}

@Suite("EraDetailModel")
@MainActor
struct EraDetailModelTests {
  let http = FakeHTTPClient()

  func makeModel(_ route: EraRoute = EraRoute(eraID: 2), server: FakeEraServer = FakeEraServer()) -> EraDetailModel {
    server.install(on: http)
    let api = makeAPI(http)
    return EraDetailModel(route: route, directory: EraDirectory(api: api), api: api, sleeper: immediateSleeper)
  }

  func songRequests() -> [[String: String]] {
    http.requests(to: "/eras/2/songs").map(\.query)
  }

  @Test func initialLoadShowsTheFirstPageAndNeighbours() async {
    let model = makeModel()
    await model.load()
    #expect(model.phase == .loaded)
    #expect(model.title == "Middle")
    #expect(model.color.hex == "336699")
    #expect(model.coverURL.absoluteString == "http://test.local/eras/2/cover?v=cv2")
    #expect(model.songs.count == 100)
    #expect(model.total == 250)
    #expect(model.countLabel == "250 songs")
    #expect(model.footerLabel == "Showing 100 of 250 songs")
    #expect(model.previousEra?.id == 1)
    #expect(model.nextEra?.id == 3)
    #expect(songRequests() == [["limit": "100", "offset": "0"]])
    #expect(!model.hasActiveFilters)
  }

  @Test func scrollingNearTheEndLoadsTheNextPage() async {
    let model = makeModel()
    await model.load()
    model.loadMoreIfNeeded(currentSongID: 10)
    #expect(model.pageTask == nil)

    model.loadMoreIfNeeded(currentSongID: 90)
    await model.pageTask?.value
    #expect(model.songs.count == 200)
    #expect(songRequests().last == ["limit": "100", "offset": "100"])

    model.loadMoreIfNeeded(currentSongID: 200)
    await model.pageTask?.value
    #expect(model.songs.count == 250)
    #expect(model.paging == .complete)
    #expect(model.footerLabel == nil)
    let requests = songRequests().count
    model.loadMoreIfNeeded(currentSongID: 250)
    #expect(songRequests().count == requests)
  }

  @Test func pagingFailuresCanBeRetried() async {
    let model = makeModel()
    await model.load()
    http.on("/eras/2/songs", respond: .text("boom", status: 500))
    model.loadNextPage()
    await model.pageTask?.value
    #expect(model.paging == .failed(.http(status: 500, message: "boom")))
    FakeEraServer().install(on: http)
    model.retryPaging()
    await model.pageTask?.value
    #expect(model.songs.count == 200)
  }

  @Test func unknownErasAreNotFound() async {
    http.on("/eras/404", respond: .text("Era does not exist", status: 404))
    http.on("/eras/404/songs", respond: .text("Era does not exist", status: 404))
    let api = makeAPI(http)
    let model = EraDetailModel(route: EraRoute(eraID: 404), directory: EraDirectory(api: api), api: api)
    await model.load()
    #expect(model.phase == .notFound)
  }

  @Test func serverFailuresAreReported() async {
    http.on("/eras/2", respond: .text("Internal server error", status: 500))
    let api = makeAPI(http)
    let model = EraDetailModel(route: EraRoute(eraID: 2), directory: EraDirectory(api: api), api: api)
    await model.load()
    #expect(model.phase == .failed(.http(status: 500, message: "Internal server error")))
  }

  @Test func categoryAndSortRequeryFromTheTop() async {
    let model = makeModel()
    await model.load()
    model.category = .bestOf
    await model.filterTask?.value
    #expect(songRequests().last == ["limit": "100", "offset": "0", "category": "best-of"])
    #expect(model.total == 25)
    #expect(model.countLabel == "25 matches")
    #expect(model.scrollRequest?.target == .top)
    #expect(model.hasActiveFilters)

    model.sort = .name
    await model.filterTask?.value
    #expect(songRequests().last == ["limit": "100", "offset": "0", "category": "best-of", "sort": "name"])
  }

  @Test func pagingWaitsWhileNewFiltersLoad() async {
    let model = makeModel()
    await model.load()
    let gate = Gate()
    let server = FakeEraServer()
    http.on("/eras/2/songs") { request in
      if request.query["category"] == "best-of" { await gate.wait() }
      return try await Self.respond(server, request)
    }
    model.category = .bestOf
    await settle()
    #expect(model.isApplyingFilters)
    model.loadNextPage()
    #expect(model.pageTask == nil, "a page of the old filters must not be appended to the new list")
    gate.open()
    await model.filterTask?.value
    #expect(model.total == 25)
    #expect(model.songs.allSatisfy { $0.name?.hasPrefix("⭐") == true })
  }

  /// Runs the generated era server for one request.
  static func respond(_ server: FakeEraServer, _ request: URLRequest) async throws -> HTTPResponse {
    let http = FakeHTTPClient()
    server.install(on: http)
    return try await http.send(request)
  }

  @Test func typingFiltersInstantlyThenSearchesTheEra() async {
    let model = makeModel()
    await model.load()
    model.searchText = "Song 12"
    #expect(model.visibleSongs.map(\.id) == [12])
    #expect(model.isQueryPending)
    #expect(model.countLabel == "1 of 100 loaded — searching the whole era…")

    await model.filterTask?.value
    #expect(songRequests().last == ["limit": "100", "offset": "0", "q": "song 12"])
    #expect(model.songs.map(\.id) == [12] + Array(120...129))
    #expect(!model.isQueryPending)
    #expect(model.countLabel == "11 matches")
  }

  @Test func instantFilterFindsEmojiWithVariationSelectors() async {
    let model = makeModel()
    // Later registrations win: replace the generated era with two rows.
    http.on(
      "/eras/2/songs", respond: .json("[\(songJSON(1, era: 2, name: "⭐\u{FE0F} Starred")),\(songJSON(2, era: 2))]"))
    await model.load()
    model.searchText = "⭐"
    #expect(model.visibleSongs.map(\.id) == [1])
  }

  @Test func whitespaceOnlyEditsDoNotRequery() async {
    let model = makeModel()
    await model.load()
    model.searchText = "   "
    await model.filterTask?.value
    #expect(songRequests().count == 1)
  }

  @Test func emptyResultsExplainThemselves() async {
    let model = makeModel()
    await model.load()
    model.searchText = "nothing like this"
    #expect(model.emptyMessage == "Searching the whole era…")
    await model.filterTask?.value
    #expect(model.emptyMessage == "No songs in this era match these filters.")
  }

  @Test func clearFiltersIssuesASingleRequest() async {
    let model = makeModel(EraRoute(eraID: 2, query: "song 1", category: .bestOf, sort: .name))
    await model.load()
    #expect(songRequests() == [["limit": "100", "offset": "0", "q": "song 1", "category": "best-of", "sort": "name"]])
    model.clearFilters()
    await model.filterTask?.value
    #expect(songRequests().count == 2)
    #expect(songRequests().last == ["limit": "100", "offset": "0"])
    #expect(!model.hasActiveFilters)
  }

  @Test func deepLinkLoadsThroughTheSongAndHighlightsIt() async {
    let model = makeModel(EraRoute(eraID: 2, focus: SongFocus(songID: 180, position: 180)))
    await model.load()
    #expect(songRequests() == [["limit": "200", "offset": "0"]])
    #expect(model.highlightedSongID == 180)
    #expect(model.scrollRequest?.target == .song(180))
  }

  @Test func deepLinkKeepsLoadingWhenThePositionIsStale() async {
    let model = makeModel(EraRoute(eraID: 2, focus: SongFocus(songID: 240, position: 30)))
    await model.load()
    #expect(songRequests() == [["limit": "100", "offset": "0"], ["limit": "500", "offset": "100"]])
    #expect(model.highlightedSongID == 240)
    #expect(model.paging == .complete)
  }

  @Test func pageOnlyLinksRevealThePagesFirstRow() async {
    let model = makeModel(EraRoute(eraID: 2, focus: SongFocus(page: 2, songID: nil)))
    await model.load()
    #expect(model.scrollRequest?.target == .song(101))
    #expect(model.highlightedSongID == nil)
  }

  @Test func missingDeepLinkTargetsDoNotHighlight() async {
    let model = makeModel(EraRoute(eraID: 2, focus: SongFocus(songID: 9999, position: 1)))
    await model.load()
    #expect(model.highlightedSongID == nil)
    #expect(model.songs.count == 250)
  }

  @Test func playingQueuesTheVisibleRowsAndGrowsWithPages() async {
    let model = makeModel(server: FakeEraServer(playable: true))
    await model.load()
    let player = PlayerModel(
      engine: FakePlaybackEngine(), api: makeAPI(http), settings: makeSettings(), sleeper: foreverSleeper)
    model.player = player
    model.play(model.songs[0])
    #expect(player.current?.id == 1)
    #expect(player.current?.eraName == "Middle")
    #expect(player.current?.durationHint == 120.5)
    #expect(player.queue.count == 100)

    model.loadNextPage()
    await model.pageTask?.value
    #expect(player.queue.count == 200)

    model.category = .bestOf
    await model.filterTask?.value
    #expect(player.queue.count == 200, "a differently filtered list is a different queue")
  }

  @Test func unplayableRowsAreSkippedInTheQueue() async {
    let model = makeModel()
    await model.load()
    #expect(model.track(for: model.songs[0]) == nil)
    #expect(model.playQueue.isEmpty)
  }
}
