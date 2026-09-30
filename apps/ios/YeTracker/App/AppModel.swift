import Foundation
import Observation
import YeTrackerKit

/// Composition root: owns the settings, the API transport, the shared feature
/// models and the platform services, and rewires them when the server changes.
@MainActor
@Observable
final class AppModel {
  let settings: AppSettings
  let router = Router()
  let directory: EraDirectory
  let home: HomeModel
  let search: GlobalSearchModel
  let player: PlayerModel
  let downloads: DownloadCenter
  let keyboard = KeyboardObserver()

  @ObservationIgnored private let http: URLSessionHTTPClient
  @ObservationIgnored private let nowPlaying: NowPlayingController
  @ObservationIgnored private let apiProvider: APIProvider

  init() {
    let settings = AppSettings(storage: UserDefaults.standard, defaultAPIBaseURL: Self.defaultAPIBaseURL())
    let configuration = URLSessionConfiguration.default
    // Honour the API's Cache-Control (60 s for JSON) across launches, in a
    // directory of its own (covers have a separate cache).
    let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first
    configuration.urlCache = URLCache(
      memoryCapacity: 8 << 20,
      diskCapacity: 32 << 20,
      directory: caches?.appendingPathComponent("APIResponses", isDirectory: true))
    configuration.waitsForConnectivity = false
    let http = URLSessionHTTPClient(session: URLSession(configuration: configuration))
    let apiProvider: APIProvider = { APIClient(baseURL: settings.apiBaseURL, http: http) }

    self.settings = settings
    self.http = http
    self.apiProvider = apiProvider
    directory = EraDirectory(api: apiProvider)
    home = HomeModel(directory: directory, api: apiProvider)
    search = GlobalSearchModel(directory: directory, api: apiProvider)
    player = PlayerModel(engine: AVPlayerEngine(), api: apiProvider, settings: settings)
    downloads = DownloadCenter()
    nowPlaying = NowPlayingController()
    nowPlaying.attach(to: player)
  }

  /// A client for the current server.
  var api: APIClient { apiProvider() }

  // MARK: - Server

  /// Validates and applies a new server URL; an empty string restores the default.
  func updateServer(_ input: String) throws(AppSettings.ServerURLError) {
    let before = settings.apiBaseURL
    try settings.setAPIBaseURL(input)
    if settings.apiBaseURL != before { serverDidChange() }
  }

  func resetServer() {
    let before = settings.apiBaseURL
    settings.resetAPIBaseURL()
    if settings.apiBaseURL != before { serverDidChange() }
  }

  /// Everything cached or playing belongs to the old server.
  private func serverDidChange() {
    player.stop()
    downloads.cancel()
    directory.reset()
    search.clear()
    router.homePath = []
    router.searchPath = []
    Task { await home.load(force: true) }
  }

  // MARK: - Links and downloads

  /// Opens a catalog source link in an in-app browser.
  func openWebLink(_ url: URL) {
    Presenter.presentSafari(url)
  }

  /// The cover of a search result's era (`nil` until the era list is loaded).
  func coverURL(for song: SearchSong) -> URL? {
    guard let era = directory.era(id: song.eraId) else { return nil }
    return api.coverURL(eraID: era.id, version: era.coverKey)
  }

  func download(songID: Int, title: String) {
    downloads.start(songID: songID, title: title, from: api.downloadURL(songID: songID))
  }

  // MARK: - Private

  /// `YTDefaultAPIBaseURL` from Info.plist (the `YT_API_BASE_URL` build setting).
  private static func defaultAPIBaseURL() -> URL {
    let configured = Bundle.main.object(forInfoDictionaryKey: "YTDefaultAPIBaseURL") as? String
    return configured.flatMap(APIBaseURL.parse) ?? URL(string: "http://localhost:3000")!
  }
}
