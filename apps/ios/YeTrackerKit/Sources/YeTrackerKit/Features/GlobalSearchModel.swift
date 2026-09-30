import Foundation
import Observation

/// "Find a track": catalog-wide search with an era range and a playable filter
/// (`apps/web/src/components/GlobalSongSearch.astro`).
@MainActor
@Observable
public final class GlobalSearchModel {
  public enum Phase: Equatable, Sendable {
    /// No query: nothing is shown.
    case idle
    case searching
    case loaded
    case failed
  }

  public static let maxResults = APIClient.maxSearchLimit
  public static let debounce: Duration = .milliseconds(150)

  public let directory: EraDirectory

  public var query = "" {
    didSet { if query != oldValue { scheduleSearch() } }
  }

  public var playableOnly = false {
    didSet { if playableOnly != oldValue { scheduleSearch() } }
  }

  /// Earliest selected era, as an index into `directory.eras`.
  public private(set) var lowerIndex = 0
  /// Latest selected era; `nil` follows the last era (so the range survives reloads).
  public private(set) var upperIndexOverride: Int?

  public private(set) var results: [SearchSong] = []
  /// Matches before the 50-result cap.
  public private(set) var total = 0
  public private(set) var phase: Phase = .idle

  @ObservationIgnored private let api: APIProvider
  @ObservationIgnored private let sleeper: Sleeper
  @ObservationIgnored var pendingSearch: Task<Void, Never>?

  public init(directory: EraDirectory, api: @escaping APIProvider, sleeper: @escaping Sleeper = Sleepers.live) {
    self.directory = directory
    self.api = api
    self.sleeper = sleeper
  }

  // MARK: - Era range

  public var lastIndex: Int { max(directory.eras.count - 1, 0) }
  public var upperIndex: Int { min(upperIndexOverride ?? lastIndex, lastIndex) }
  public var effectiveLowerIndex: Int { min(lowerIndex, upperIndex) }

  /// Moves the start thumb; dragging past the end drags the end along (web behaviour).
  public func setLowerIndex(_ index: Int) {
    let clamped = min(max(0, index), lastIndex)
    guard clamped != lowerIndex || clamped > upperIndex else { return }
    lowerIndex = clamped
    if clamped > upperIndex { upperIndexOverride = clamped == lastIndex ? nil : clamped }
    scheduleSearch()
  }

  /// Moves the end thumb; dragging before the start drags the start along.
  public func setUpperIndex(_ index: Int) {
    let clamped = min(max(0, index), lastIndex)
    let override = clamped == lastIndex ? nil : clamped
    guard override != upperIndexOverride || clamped < lowerIndex else { return }
    upperIndexOverride = override
    if clamped < lowerIndex { lowerIndex = clamped }
    scheduleSearch()
  }

  public var lowerEra: Era? { era(at: effectiveLowerIndex) }
  public var upperEra: Era? { era(at: upperIndex) }

  public var isFullRange: Bool { effectiveLowerIndex == 0 && upperIndex == lastIndex }

  /// "All eras", "1 era" or "N eras".
  public var rangeStatus: String {
    if isFullRange { return "All eras" }
    let count = upperIndex - effectiveLowerIndex + 1
    return count == 1 ? "1 era" : "\(count) eras"
  }

  // MARK: - Derived text

  /// Enables "Clear".
  public var hasFilters: Bool {
    !TextNormalization.normalize(query).isEmpty || !isFullRange || playableOnly
  }

  /// The header count (web `count.textContent`).
  public var countLabel: String {
    switch phase {
    case .idle:
      TextNormalization.count(directory.totalSongs, "song", "songs")
    case .searching:
      "Searching…"
    case .loaded:
      total > Self.maxResults
        ? "\(results.count.formatted()) of \(total.formatted()) matches"
        : TextNormalization.count(total, "match", "matches")
    case .failed:
      "Search failed"
    }
  }

  /// The empty-state message, when one applies.
  public var emptyMessage: String? {
    switch phase {
    case .failed: "Search is temporarily unavailable."
    case .loaded where total == 0: "No songs match your search."
    default: nil
    }
  }

  // MARK: - Actions

  public func clear() {
    query = ""
    playableOnly = false
    lowerIndex = 0
    upperIndexOverride = nil
    scheduleSearch()
  }

  /// A deep link into the song's era.
  public func route(for song: SearchSong) -> EraRoute? {
    guard let eraID = song.eraId else { return nil }
    return EraRoute(eraID: eraID, focus: song.focus)
  }

  public func track(for song: SearchSong) -> Track? {
    guard song.isPlayable else { return nil }
    return Track(song: song, coverVersion: directory.era(id: song.eraId)?.coverVersion)
  }

  /// Plays a result; the playable results form the queue.
  public func play(_ song: SearchSong, with player: PlayerModel) {
    guard let track = track(for: song) else { return }
    let queue = results.compactMap(track(for:))
    player.play(track, queue: queue, queueID: "search:\(currentRequest().signature)")
  }

  /// The request the current inputs describe.
  public func currentRequest() -> SongSearchRequest {
    let eras = directory.eras
    let lower = effectiveLowerIndex
    let upper = upperIndex
    return SongSearchRequest(
      query: TextNormalization.apiQuery(query),
      eraFrom: lower > 0 && eras.indices.contains(lower) ? eras[lower].id : nil,
      eraTo: upper < lastIndex && eras.indices.contains(upper) ? eras[upper].id : nil,
      playableOnly: playableOnly,
      limit: Self.maxResults)
  }

  /// Re-runs the current search (e.g. after the era list reloaded).
  public func refresh() {
    scheduleSearch()
  }

  // MARK: - Private

  private func era(at index: Int) -> Era? {
    directory.eras.indices.contains(index) ? directory.eras[index] : nil
  }

  private func scheduleSearch() {
    pendingSearch?.cancel()
    let request = currentRequest()
    guard !request.query.isEmpty else {
      pendingSearch = nil
      results = []
      total = 0
      phase = .idle
      return
    }
    phase = .searching
    pendingSearch = Task { [weak self, sleeper] in
      do {
        try await sleeper(Self.debounce)
      } catch {
        return
      }
      guard !Task.isCancelled else { return }
      await self?.perform(request)
    }
  }

  private func perform(_ request: SongSearchRequest) async {
    do {
      let response = try await api().searchSongs(request)
      guard !Task.isCancelled, request.signature == currentRequest().signature else { return }
      results = response.songs
      total = response.total
      phase = .loaded
    } catch {
      let apiError = APIError(transportError: error)
      guard !apiError.isCancellation, !Task.isCancelled, request.signature == currentRequest().signature else {
        return
      }
      results = []
      total = 0
      phase = .failed
    }
  }
}
