import Foundation
import Testing

@testable import YeTrackerKit

@Suite("Deep links")
struct DeepLinkTests {
  func parse(_ string: String) -> DeepLinkTarget? {
    DeepLink.parse(URL(string: string)!)
  }

  @Test func customSchemeEraLinks() {
    #expect(parse("yetracker://eras/12") == .era(EraRoute(eraID: 12)))
    #expect(parse("yetracker:///eras/12") == .era(EraRoute(eraID: 12)))
    #expect(
      parse("yetracker://eras/12?page=3#song-45")
        == .era(EraRoute(eraID: 12, focus: SongFocus(songID: 45, loadThrough: 300))))
  }

  @Test func webURLsWithFilters() {
    let target = parse(
      "https://tracker.example/eras/7?page=2&q=love%20lockdown&category=grails&sort=leak-newest#song-9")
    #expect(
      target
        == .era(
          EraRoute(
            eraID: 7, focus: SongFocus(songID: 9, loadThrough: 200), query: "love lockdown", category: .grails,
            sort: .leakNewest)))
  }

  @Test func pageOnlyLinksRevealTheFirstRowOfThePage() {
    #expect(
      parse("yetracker://eras/7?page=4")
        == .era(EraRoute(eraID: 7, focus: SongFocus(songID: nil, loadThrough: 400, rowIndex: 300))))
  }

  @Test func invalidValuesAreDroppedLikeTheWeb() {
    #expect(
      parse("yetracker://eras/7?page=abc&category=bogus&sort=nope#song-x")
        == .era(EraRoute(eraID: 7)))
  }

  @Test func homeAndRejects() {
    #expect(parse("yetracker://") == .home)
    #expect(parse("yetracker://home") == .home)
    #expect(parse("https://tracker.example/") == .home)
    #expect(parse("yetracker://eras/0") == nil)
    #expect(parse("yetracker://eras/-1") == nil)
    #expect(parse("yetracker://songs/5") == nil)
    #expect(parse("mailto:someone@example.com") == nil)
  }

  @Test func focusFromEraPosition() {
    #expect(SongFocus(songID: 3, position: 441).loadThrough == 441)
    #expect(SongFocus(songID: 3, position: nil).loadThrough == 1)
  }
}

@Suite("Router")
@MainActor
struct RouterTests {
  @Test func deepLinksResetHomeStack() {
    let router = Router()
    router.selectedTab = .search
    router.isNowPlayingPresented = true
    router.homePath = [.era(EraRoute(eraID: 1)), .era(EraRoute(eraID: 2))]
    #expect(router.open(URL(string: "yetracker://eras/5#song-9")!))
    #expect(router.selectedTab == .home)
    #expect(!router.isNowPlayingPresented)
    #expect(router.homePath == [.era(EraRoute(eraID: 5, focus: SongFocus(page: 1, songID: 9)))])
    #expect(!router.open(URL(string: "https://example.com/nope/1")!))
  }

  @Test func showPushesOntoTheCurrentBrowsingTab() {
    let router = Router()
    router.selectedTab = .search
    router.show(EraRoute(eraID: 3))
    #expect(router.searchPath == [.era(EraRoute(eraID: 3))])
    router.selectedTab = .settings
    router.show(EraRoute(eraID: 4))
    #expect(router.selectedTab == .home)
    #expect(router.homePath == [.era(EraRoute(eraID: 4))])
  }

  @Test func replaceTopSwapsNeighbours() {
    let router = Router()
    router.homePath = [.era(EraRoute(eraID: 1))]
    router.replaceTop(with: EraRoute(eraID: 2), in: .home)
    #expect(router.homePath == [.era(EraRoute(eraID: 2))])
  }

  @Test func playerLinksCloseTheSheet() {
    let router = Router()
    router.isNowPlayingPresented = true
    router.showFromPlayer(EraRoute(eraID: 8))
    #expect(!router.isNowPlayingPresented)
    #expect(router.homePath.count == 1)
  }
}

@Suite("Settings")
@MainActor
struct SettingsTests {
  @Test func defaultsMatchTheWeb() {
    let settings = makeSettings()
    #expect(settings.quality == .kbps128)
    #expect(settings.volume == 1)
    #expect(settings.apiBaseURL == testBaseURL)
  }

  @Test func storedPreferencesAreRestored() {
    let settings = makeSettings([AppSettings.qualityKey: "", AppSettings.volumeKey: "0.35"])
    #expect(settings.quality == .original)
    #expect(settings.volume == 0.35)
  }

  @Test func invalidStoredValuesFallBack() {
    let settings = makeSettings([AppSettings.qualityKey: "96", AppSettings.volumeKey: "7"])
    #expect(settings.quality == .kbps128)
    #expect(settings.volume == 1)
  }

  @Test func changesPersist() {
    let storage = InMemorySettingsStorage()
    let settings = AppSettings(storage: storage, defaultAPIBaseURL: testBaseURL)
    settings.quality = .kbps320
    settings.volume = 1.5
    #expect(storage.string(forKey: AppSettings.qualityKey) == "320")
    #expect(storage.string(forKey: AppSettings.volumeKey) == "1.0")
    #expect(settings.clampedVolume == 1)
  }

  @Test func serverURLOverrides() throws {
    let storage = InMemorySettingsStorage()
    let settings = AppSettings(storage: storage, defaultAPIBaseURL: testBaseURL)
    try settings.setAPIBaseURL("tracker.example/api/")
    #expect(settings.apiBaseURL.absoluteString == "https://tracker.example/api")
    #expect(storage.string(forKey: AppSettings.apiBaseURLKey) == "https://tracker.example/api")
    #expect(throws: AppSettings.ServerURLError.invalid) { try settings.setAPIBaseURL("not a url") }
    try settings.setAPIBaseURL("")
    #expect(settings.customAPIBaseURL == nil)
    #expect(storage.string(forKey: AppSettings.apiBaseURLKey) == nil)
    try settings.setAPIBaseURL("http://test.local/")
    #expect(settings.customAPIBaseURL == nil)
  }
}
