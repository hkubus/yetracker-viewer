import Foundation
import Testing

@testable import YeTrackerKit

@Suite("HomeModel")
@MainActor
struct HomeModelTests {
  let http = FakeHTTPClient()

  func makeHome() -> HomeModel {
    let api = makeAPI(http)
    return HomeModel(directory: EraDirectory(api: api), api: api)
  }

  @Test func loadsErasAndRecentLeaks() async throws {
    http.on("/eras", respond: try .fixture("eras"))
    http.on("/songs", respond: try .fixture("recent-leaks"))
    let home = makeHome()
    await home.load()

    #expect(home.directory.state == .loaded)
    #expect(home.headerSummary == "43 eras · \(9649.formatted()) tracks")
    #expect(home.showsEraFilter)
    #expect(home.eraCountLabel == "43 eras")
    #expect(home.recentLeaks.count == 2)
    #expect(http.requests(to: "/songs").first?.query == ["playable": "true", "sort": "leak-newest", "limit": "8"])

    let queue = home.recentLeaksQueue
    #expect(queue.map(\.id) == [6751, 6752])
    #expect(queue.first?.coverVersion == home.directory.era(id: 31)?.coverVersion)
    #expect(home.route(for: home.recentLeaks[0]) == EraRoute(eraID: 31, focus: SongFocus(songID: 6751, position: 441)))
  }

  @Test func eraFilterMatchesNamesCaseInsensitively() async throws {
    http.on("/eras", respond: try .fixture("eras"))
    let home = makeHome()
    await home.load()
    home.eraFilter = "  DONDA "
    #expect(!home.filteredEras.isEmpty)
    #expect(home.filteredEras.allSatisfy { $0.name!.lowercased().contains("donda") })
    #expect(home.eraCountLabel == "\(home.filteredEras.count) of 43 eras")
    #expect(home.eraFilterEmptyMessage == nil)
    home.eraFilter = "zzzz"
    #expect(home.eraFilterEmptyMessage == "No eras match that filter.")
  }

  @Test func recentLeaksFailureOnlyHidesTheStrip() async throws {
    http.on("/eras", respond: try .fixture("eras"))
    http.on("/songs", respond: .text("boom", status: 500))
    let home = makeHome()
    await home.load()
    #expect(home.recentLeaks.isEmpty)
    #expect(home.directory.state == .loaded)
  }

  @Test func catalogFailureIsReported() async {
    http.on("/eras", respond: .text("Internal server error", status: 500))
    let home = makeHome()
    await home.load()
    #expect(home.directory.state == .failed(.http(status: 500, message: "Internal server error")))
  }

  @Test func failedRefreshKeepsTheOldList() async throws {
    http.on("/eras", respond: try .fixture("eras"))
    let home = makeHome()
    await home.load()
    http.on("/eras", respond: .text("down", status: 503))
    await home.load(force: true)
    #expect(home.directory.eras.count == 43)
    #expect(home.directory.state == .loaded)
    #expect(home.directory.refreshError == .http(status: 503, message: "down"))
    #expect(http.requests(to: "/eras").last?.cachePolicy == .reloadIgnoringLocalCacheData)
  }

  @Test func concurrentLoadsShareOneRequest() async throws {
    http.on("/eras", respond: try .fixture("eras"))
    let directory = EraDirectory(api: makeAPI(http))
    async let first: Void = directory.load()
    async let second: Void = directory.load()
    _ = await (first, second)
    await directory.load()
    #expect(http.requests(to: "/eras").count == 1)
  }

  @Test func playingARecentLeakQueuesTheStrip() async throws {
    http.on("/eras", respond: try .fixture("eras"))
    http.on("/songs", respond: try .fixture("recent-leaks"))
    let home = makeHome()
    await home.load()
    let engine = FakePlaybackEngine()
    let player = PlayerModel(engine: engine, api: makeAPI(http), settings: makeSettings(), sleeper: foreverSleeper)
    home.play(home.recentLeaks[1], with: player)
    #expect(player.current?.id == 6752)
    #expect(player.queueID == HomeModel.recentLeaksQueueID)
    #expect(player.canGoPrevious)
  }
}

@Suite("GlobalSearchModel")
@MainActor
struct GlobalSearchModelTests {
  let http = FakeHTTPClient()

  func makeSearch() async throws -> GlobalSearchModel {
    http.on("/eras", respond: try .fixture("eras"))
    let api = makeAPI(http)
    let directory = EraDirectory(api: api)
    await directory.load()
    return GlobalSearchModel(directory: directory, api: api, sleeper: immediateSleeper)
  }

  @Test func idleShowsTheCatalogCount() async throws {
    let search = try await makeSearch()
    #expect(search.phase == .idle)
    #expect(search.countLabel == "\(9649.formatted()) songs")
    #expect(search.rangeStatus == "All eras")
    #expect(!search.hasFilters)
    #expect(search.lowerEra?.id == 1)
    #expect(search.upperEra?.id == 44)
  }

  @Test func typingSearchesAfterTheDebounce() async throws {
    http.on("/songs", respond: try .fixture("search-love-lockdown"))
    let search = try await makeSearch()
    search.query = "  Love   LOCKDOWN "
    #expect(search.phase == .searching)
    #expect(search.countLabel == "Searching…")
    await search.pendingSearch?.value

    #expect(http.requests(to: "/songs").last?.query == ["limit": "50", "q": "love lockdown"])
    #expect(search.phase == .loaded)
    #expect(search.results.count == 5)
    #expect(search.countLabel == "\(search.total) matches")
    #expect(search.emptyMessage == nil)
    #expect(search.hasFilters)
  }

  @Test func blankQueriesNeverHitTheServer() async throws {
    let search = try await makeSearch()
    search.query = "   "
    search.playableOnly = true
    await search.pendingSearch?.value
    #expect(search.phase == .idle)
    #expect(http.requests(to: "/songs").isEmpty)
    #expect(search.hasFilters)
  }

  @Test func eraRangeOnlySendsNarrowedBounds() async throws {
    http.on("/songs", respond: .json(#"{"songs":[],"total":0}"#))
    let search = try await makeSearch()
    let eras = search.directory.eras
    search.query = "x"
    search.setLowerIndex(2)
    search.setUpperIndex(5)
    await search.pendingSearch?.value
    #expect(search.rangeStatus == "4 eras")
    #expect(
      http.requests(to: "/songs").last?.query
        == ["limit": "50", "q": "x", "eraFrom": String(eras[2].id), "eraTo": String(eras[5].id)])

    search.setUpperIndex(search.lastIndex)
    await search.pendingSearch?.value
    #expect(http.requests(to: "/songs").last?.query["eraTo"] == nil)
    #expect(search.upperIndexOverride == nil)
  }

  @Test func thumbsDragEachOtherAlong() async throws {
    let search = try await makeSearch()
    search.setLowerIndex(3)
    search.setUpperIndex(6)
    search.setLowerIndex(9)
    #expect(search.effectiveLowerIndex == 9)
    #expect(search.upperIndex == 9)
    #expect(search.rangeStatus == "1 era")
    search.setUpperIndex(1)
    #expect(search.effectiveLowerIndex == 1)
    #expect(search.upperIndex == 1)
    search.setLowerIndex(-5)
    search.setUpperIndex(999)
    #expect(search.isFullRange)
  }

  @Test func outOfDateResponsesAreDropped() async throws {
    let gate = Gate()
    http.on("/songs") { request in
      if request.query["q"] == "a" {
        await gate.wait()
        return .json(#"{"songs":[{"id":1,"name":"stale"}],"total":1}"#)
      }
      return .json(#"{"songs":[{"id":2,"name":"fresh"}],"total":1}"#)
    }
    let search = try await makeSearch()
    search.query = "a"
    let first = search.pendingSearch
    await settle()
    search.query = "ab"
    await search.pendingSearch?.value
    #expect(search.results.map(\.id) == [2])
    gate.open()
    await first?.value
    #expect(search.results.map(\.id) == [2])
    #expect(search.phase == .loaded)
  }

  @Test func failuresShowTheUnavailableMessage() async throws {
    http.on("/songs", respond: .text("Search query is too long", status: 400))
    let search = try await makeSearch()
    search.query = "love"
    await search.pendingSearch?.value
    #expect(search.phase == .failed)
    #expect(search.countLabel == "Search failed")
    #expect(search.emptyMessage == "Search is temporarily unavailable.")
  }

  @Test func countLabelsForLargeAndEmptyResults() async throws {
    let songs = (1...50).map { #"{"id":\#($0),"name":"s\#($0)"}"# }.joined(separator: ",")
    http.on("/songs", respond: .json(#"{"songs":[\#(songs)],"total":120}"#))
    let search = try await makeSearch()
    search.query = "s"
    await search.pendingSearch?.value
    #expect(search.countLabel == "50 of 120 matches")

    http.on("/songs", respond: .json(#"{"songs":[],"total":0}"#))
    search.query = "zzz"
    await search.pendingSearch?.value
    #expect(search.countLabel == "0 matches")
    #expect(search.emptyMessage == "No songs match your search.")

    http.on("/songs", respond: .json(#"{"songs":[{"id":1,"name":"one"}],"total":1}"#))
    search.query = "one"
    await search.pendingSearch?.value
    #expect(search.countLabel == "1 match")
  }

  @Test func clearResetsEverything() async throws {
    http.on("/songs", respond: try .fixture("search-love-lockdown"))
    let search = try await makeSearch()
    search.query = "love"
    search.playableOnly = true
    search.setLowerIndex(4)
    await search.pendingSearch?.value
    search.clear()
    #expect(search.query.isEmpty)
    #expect(!search.playableOnly)
    #expect(search.isFullRange)
    #expect(search.phase == .idle)
    #expect(search.results.isEmpty)
    #expect(!search.hasFilters)
  }

  @Test func resultsDeepLinkIntoTheirEra() async throws {
    http.on("/songs", respond: try .fixture("search-love-lockdown"))
    let search = try await makeSearch()
    search.query = "love lockdown"
    await search.pendingSearch?.value
    let first = try #require(search.results.first)
    #expect(search.route(for: first) == EraRoute(eraID: 14, focus: SongFocus(songID: 2194, loadThrough: 170)))
  }
}
