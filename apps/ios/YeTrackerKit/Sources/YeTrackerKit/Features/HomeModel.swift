import Foundation
import Observation

/// The home screen: header stats, "Recently leaked" and the filterable era grid
/// (`apps/web/src/pages/index.astro`, `RecentLeaks.astro`).
@MainActor
@Observable
public final class HomeModel {
  /// Web `RecentLeaks` `LIMIT`.
  public static let recentLeaksLimit = 8

  public let directory: EraDirectory
  /// Text in the "Filter eras…" field.
  public var eraFilter = ""
  /// Newest playable leaks. Decoration only: failures leave it empty.
  public private(set) var recentLeaks: [SearchSong] = []

  @ObservationIgnored private let api: APIProvider

  public init(directory: EraDirectory, api: @escaping APIProvider) {
    self.directory = directory
    self.api = api
  }

  /// Loads the eras and the recent leaks together. `force` bypasses the HTTP cache (pull to refresh).
  public func load(force: Bool = false) async {
    let leaks = Task { await self.loadRecentLeaks(force: force) }
    await directory.load(force: force)
    await leaks.value
  }

  /// "43 eras · 9,649 tracks".
  public var headerSummary: String {
    "\(TextNormalization.count(directory.eras.count, "era", "eras")) · "
      + TextNormalization.count(directory.totalSongs, "track", "tracks")
  }

  /// The web only offers the era filter with more than three eras.
  public var showsEraFilter: Bool { directory.eras.count > 3 }

  public var filteredEras: [Era] {
    let query = TextNormalization.normalize(eraFilter)
    guard !query.isEmpty else { return directory.eras }
    return directory.eras.filter { TextNormalization.contains(($0.name ?? "").lowercased(), query) }
  }

  /// "43 eras" or "5 of 43 eras".
  public var eraCountLabel: String {
    let total = directory.eras.count
    guard !TextNormalization.normalize(eraFilter).isEmpty else {
      return TextNormalization.count(total, "era", "eras")
    }
    return "\(filteredEras.count.formatted()) of \(TextNormalization.count(total, "era", "eras"))"
  }

  /// Shown when the filter hides every era.
  public var eraFilterEmptyMessage: String? {
    guard !directory.eras.isEmpty, filteredEras.isEmpty else { return nil }
    return "No eras match that filter."
  }

  /// A playable track for a recent leak (the cover version comes from the era list).
  public func track(for song: SearchSong) -> Track? {
    guard song.isPlayable else { return nil }
    return Track(song: song, coverVersion: directory.era(id: song.eraId)?.coverVersion)
  }

  /// The strip is its own play scope, as on the web.
  public var recentLeaksQueue: [Track] { recentLeaks.compactMap(track(for:)) }

  public static let recentLeaksQueueID = "recent-leaks"

  public func play(_ song: SearchSong, with player: PlayerModel) {
    guard let track = track(for: song) else { return }
    player.play(track, queue: recentLeaksQueue, queueID: Self.recentLeaksQueueID)
  }

  /// Opens the era scrolled to the song, like the web's deep link.
  public func route(for song: SearchSong) -> EraRoute? {
    guard let eraID = song.eraId else { return nil }
    return EraRoute(eraID: eraID, focus: song.focus)
  }

  private func loadRecentLeaks(force: Bool) async {
    do {
      recentLeaks = try await api().recentLeaks(limit: Self.recentLeaksLimit, reload: force)
    } catch {
      if !APIError(transportError: error).isCancellation { recentLeaks = [] }
    }
  }
}
