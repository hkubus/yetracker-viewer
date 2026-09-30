import Foundation
import Observation

/// One era: header, neighbours, and its song list with server-side search,
/// category and sort, an instant local filter, infinite paging and deep-link
/// focus (`apps/web/src/pages/eras/[id].astro`, `SongList.astro`, `Pagination.astro`).
@MainActor
@Observable
public final class EraDetailModel {
  public enum Phase: Equatable, Sendable {
    case loading
    case loaded
    /// `404 Era does not exist`.
    case notFound
    case failed(APIError)
  }

  public enum Paging: Equatable, Sendable {
    case idle
    case loading
    case failed(APIError)
    case complete
  }

  public enum ScrollTarget: Equatable, Sendable {
    case song(Int)
    case top
  }

  /// A scroll the view should perform; the token makes repeats distinct.
  public struct ScrollRequest: Equatable, Sendable {
    public let target: ScrollTarget
    public let token: Int
  }

  public static let pageSize = APIClient.eraPageSize
  /// Deep links load up to this many rows per request to reach their target.
  public static let focusChunk = APIClient.maxEraSongsLimit
  /// The local filter is instant; the whole-era server query waits for a typing pause.
  public static let queryDebounce: Duration = .milliseconds(300)
  /// Rows from the end of the list at which the next page is requested.
  public static let prefetchDistance = 15

  public let eraID: Int
  public let directory: EraDirectory

  public private(set) var era: Era?
  /// Rows loaded so far for the applied filters.
  public private(set) var songs: [EraSong] = []
  /// Rows matching the applied filters (`X-Total-Count`).
  public private(set) var total = 0
  public private(set) var phase: Phase = .loading
  public private(set) var paging: Paging = .idle
  /// New filters are loading; the old rows stay visible meanwhile.
  public private(set) var isApplyingFilters = false
  /// The deep-linked row (web `:target`).
  public private(set) var highlightedSongID: Int?
  public private(set) var scrollRequest: ScrollRequest?

  /// Search field text: filters loaded rows instantly, then queries the whole era.
  public var searchText: String {
    didSet { if searchText != oldValue, !isBatchUpdating { scheduleQuery() } }
  }

  public var category: SongCategory? {
    didSet { if category != oldValue, !isBatchUpdating { applyFiltersNow() } }
  }

  public var sort: SongSort {
    didSet { if sort != oldValue, !isBatchUpdating { applyFiltersNow() } }
  }

  /// Receives play requests and queue growth.
  @ObservationIgnored public weak var player: PlayerModel?

  @ObservationIgnored private let api: APIProvider
  @ObservationIgnored private let sleeper: Sleeper
  /// The filters `songs` reflects (limit/offset unused).
  @ObservationIgnored private var applied: EraSongsRequest
  @ObservationIgnored private var pendingFocus: SongFocus?
  @ObservationIgnored private var generation = 0
  @ObservationIgnored private var scrollToken = 0
  @ObservationIgnored private var isBatchUpdating = false
  @ObservationIgnored private var haystacks: [Int: [UInt8]] = [:]
  @ObservationIgnored var filterTask: Task<Void, Never>?
  @ObservationIgnored var pageTask: Task<Void, Never>?

  public init(
    route: EraRoute,
    directory: EraDirectory,
    api: @escaping APIProvider,
    sleeper: @escaping Sleeper = Sleepers.live
  ) {
    eraID = route.eraID
    self.directory = directory
    self.api = api
    self.sleeper = sleeper
    searchText = route.query
    category = route.category
    sort = route.sort
    applied = EraSongsRequest(
      query: TextNormalization.apiQuery(route.query), category: route.category, sort: route.sort)
    pendingFocus = route.focus
    // Show the header straight away from the cached era list.
    era = directory.era(id: route.eraID)
  }

  // MARK: - Loading

  /// Initial load: era, first page (enough rows for a deep-link focus) and neighbours.
  public func load() async {
    guard phase != .loaded else { return }
    await loadAll(reload: false)
  }

  /// Pull to refresh: everything again, bypassing the HTTP cache.
  public func refresh() async {
    await loadAll(reload: true)
  }

  public func loadMoreIfNeeded(currentSongID id: Int) {
    guard phase == .loaded, paging == .idle, !isApplyingFilters, songs.count < total else { return }
    let visible = visibleSongs
    guard let index = visible.firstIndex(where: { $0.id == id }), index >= visible.count - Self.prefetchDistance
    else { return }
    loadNextPage()
  }

  /// Ignored while new filters load: the page would come from the old filters.
  public func loadNextPage() {
    guard paging != .loading, phase == .loaded, !isApplyingFilters, songs.count < total else { return }
    pageTask = Task { await self.fetchNextPage(limit: Self.pageSize) }
  }

  public func retryPaging() {
    guard case .failed = paging else { return }
    paging = .idle
    loadNextPage()
  }

  /// "Clear all filters".
  public func clearFilters() {
    isBatchUpdating = true
    searchText = ""
    category = nil
    sort = .default
    isBatchUpdating = false
    applyFiltersNow()
  }

  // MARK: - Derived state

  public var title: String { era?.displayName ?? directory.era(id: eraID)?.displayName ?? "Era" }
  public var color: RGBColor { (era ?? directory.era(id: eraID))?.color ?? .fallbackAccent }

  public var coverURL: URL {
    api().coverURL(eraID: eraID, version: (era ?? directory.era(id: eraID))?.coverKey)
  }

  public var previousEra: Era? { directory.neighbors(of: eraID).previous }
  public var nextEra: Era? { directory.neighbors(of: eraID).next }

  public var normalizedSearch: String { TextNormalization.normalize(searchText) }

  /// Loaded rows passing the instant filter.
  public var visibleSongs: [EraSong] {
    let query = normalizedSearch
    guard !query.isEmpty else { return songs }
    let needle = Array(query.utf8)
    return songs.filter { (haystacks[$0.id] ?? Array($0.searchHaystack.utf8)).firstRange(of: needle) != nil }
  }

  /// The typed text has not reached the server yet.
  public var isQueryPending: Bool { TextNormalization.apiQuery(searchText) != applied.query }

  /// Shows "Clear all filters".
  public var hasActiveFilters: Bool { !normalizedSearch.isEmpty || category != nil || sort != .default }

  /// "523 songs" / "12 matches" / "4 of 100 loaded — searching the whole era…".
  public var countLabel: String {
    if isQueryPending, !normalizedSearch.isEmpty {
      return "\(visibleSongs.count.formatted()) of \(songs.count.formatted()) loaded — searching the whole era…"
    }
    let filtered = !applied.query.isEmpty || applied.category != nil
    return filtered
      ? TextNormalization.count(total, "match", "matches") : TextNormalization.count(total, "song", "songs")
  }

  /// "Showing 100 of 523 songs" while more rows remain.
  public var footerLabel: String? {
    guard phase == .loaded, songs.count < total else { return nil }
    return "Showing \(songs.count.formatted()) of \(total.formatted()) songs"
  }

  public var emptyMessage: String? {
    guard phase == .loaded, visibleSongs.isEmpty, !isApplyingFilters else { return nil }
    if !songs.isEmpty { return isQueryPending ? "Searching the whole era…" : "No songs match your filter." }
    if isQueryPending { return "Searching the whole era…" }
    return hasActiveFilters ? "No songs in this era match these filters." : "This era has no songs yet."
  }

  // MARK: - Songs and playback

  public func track(for song: EraSong) -> Track? {
    guard song.isPlayable else { return nil }
    return Track(song: song, era: era ?? directory.era(id: eraID))
  }

  /// The visible playable rows, in order: next/previous walk this list.
  public var playQueue: [Track] { visibleSongs.compactMap(track(for:)) }

  /// Identifies this list (era + filters) so the player can grow the queue as pages load.
  public var queueID: String {
    "era:\(eraID)|q=\(applied.query)|c=\(applied.category?.rawValue ?? "")|s=\(applied.sort.rawValue)|f=\(normalizedSearch)"
  }

  public func play(_ song: EraSong) {
    guard let player, let track = track(for: song) else { return }
    player.play(track, queue: playQueue, queueID: queueID)
  }

  public func downloadURL(for song: EraSong) -> URL {
    api().downloadURL(songID: song.id)
  }

  // MARK: - Private

  private func currentFilters() -> EraSongsRequest {
    EraSongsRequest(query: TextNormalization.apiQuery(searchText), category: category, sort: sort)
  }

  private func loadAll(reload: Bool) async {
    generation += 1
    let current = generation
    filterTask?.cancel()
    pageTask?.cancel()
    let filters = currentFilters()
    let focus = pendingFocus
    var firstPage = filters
    firstPage.limit = focus.map { min(Self.focusChunk, roundUpToPage($0.loadThrough)) } ?? Self.pageSize
    if songs.isEmpty { phase = .loading } else { isApplyingFilters = true }
    paging = .idle

    let client = api()
    let eraID = eraID
    let neighbours = Task { await self.directory.load() }
    do {
      async let loadedEra = client.era(id: eraID, reload: reload)
      async let loadedPage = client.eraSongs(eraID: eraID, request: firstPage, reload: reload)
      let (newEra, page) = try await (loadedEra, loadedPage)
      guard current == generation else { return }
      era = newEra
      applied = filters
      setSongs(page.songs, total: page.total)
      isApplyingFilters = false
      phase = .loaded
      syncQueue()
      if let focus { await reveal(focus, generation: current) }
    } catch {
      let apiError = APIError(transportError: error)
      guard current == generation, !apiError.isCancellation else { return }
      isApplyingFilters = false
      phase = apiError.isNotFound ? .notFound : .failed(apiError)
    }
    await neighbours.value
  }

  private func applyFiltersNow() {
    filterTask?.cancel()
    filterTask = Task { await self.applyFilters(scrollToTop: true) }
  }

  private func scheduleQuery() {
    filterTask?.cancel()
    guard TextNormalization.apiQuery(searchText) != applied.query || isApplyingFilters else { return }
    filterTask = Task { [weak self, sleeper] in
      do {
        try await sleeper(Self.queryDebounce)
      } catch {
        return
      }
      guard !Task.isCancelled else { return }
      await self?.applyFilters(scrollToTop: false)
    }
  }

  /// Page one for the current search/category/sort. The old rows stay until it lands.
  private func applyFilters(scrollToTop: Bool) async {
    generation += 1
    let current = generation
    pageTask?.cancel()
    pendingFocus = nil
    highlightedSongID = nil
    var request = currentFilters()
    request.limit = Self.pageSize
    isApplyingFilters = true
    paging = .idle
    do {
      if era == nil { era = try? await api().era(id: eraID) }
      let page = try await api().eraSongs(eraID: eraID, request: request)
      guard current == generation else { return }
      applied = request
      setSongs(page.songs, total: page.total)
      isApplyingFilters = false
      phase = .loaded
      if scrollToTop { requestScroll(.top) }
      syncQueue()
    } catch {
      let apiError = APIError(transportError: error)
      guard current == generation, !apiError.isCancellation else { return }
      isApplyingFilters = false
      phase = apiError.isNotFound ? .notFound : .failed(apiError)
    }
  }

  /// Appends the next rows for the applied filters. Returns whether new rows arrived.
  @discardableResult
  private func fetchNextPage(limit: Int) async -> Bool {
    guard paging != .loading, !isApplyingFilters, songs.count < total else { return false }
    let current = generation
    var request = applied
    request.offset = songs.count
    request.limit = limit
    paging = .loading
    do {
      let page = try await api().eraSongs(eraID: eraID, request: request)
      guard current == generation else { return false }
      let added = appendSongs(page.songs, total: page.total)
      paging = (songs.count >= total || added == 0) ? .complete : .idle
      syncQueue()
      return added > 0
    } catch {
      let apiError = APIError(transportError: error)
      guard current == generation else { return false }
      paging = apiError.isCancellation ? .idle : .failed(apiError)
      return false
    }
  }

  /// Loads through the deep-link target, then scrolls to and highlights it.
  private func reveal(_ focus: SongFocus, generation current: Int) async {
    pendingFocus = nil
    if let songID = focus.songID {
      while !songs.contains(where: { $0.id == songID }), songs.count < total, current == generation {
        guard await fetchFocusChunk() else { break }
      }
      guard current == generation, songs.contains(where: { $0.id == songID }) else { return }
      highlightedSongID = songID
      requestScroll(.song(songID))
    } else if let row = focus.rowIndex, total > 0 {
      let target = min(row, total - 1)
      while songs.count <= target, songs.count < total, current == generation {
        guard await fetchFocusChunk() else { break }
      }
      guard current == generation, songs.indices.contains(target) else { return }
      requestScroll(.song(songs[target].id))
    }
  }

  /// One deep-link chunk, first letting a scroll-triggered page finish.
  private func fetchFocusChunk() async -> Bool {
    if paging == .loading, let pageTask {
      await pageTask.value
      return true
    }
    return await fetchNextPage(limit: Self.focusChunk)
  }

  private func setSongs(_ newSongs: [EraSong], total newTotal: Int) {
    var seen = Set<Int>()
    songs = newSongs.filter { seen.insert($0.id).inserted }
    haystacks = Dictionary(uniqueKeysWithValues: songs.map { ($0.id, Array($0.searchHaystack.utf8)) })
    total = max(newTotal, songs.count)
    paging = songs.count >= total ? .complete : .idle
  }

  /// Appends rows not already present; returns how many were added.
  private func appendSongs(_ newSongs: [EraSong], total newTotal: Int) -> Int {
    var seen = Set(songs.map(\.id))
    let fresh = newSongs.filter { seen.insert($0.id).inserted }
    songs += fresh
    for song in fresh { haystacks[song.id] = Array(song.searchHaystack.utf8) }
    total = max(newTotal, songs.count)
    return fresh.count
  }

  private func requestScroll(_ target: ScrollTarget) {
    scrollToken += 1
    scrollRequest = ScrollRequest(target: target, token: scrollToken)
  }

  private func syncQueue() {
    player?.extendQueue(playQueue, queueID: queueID)
  }

  private func roundUpToPage(_ rows: Int) -> Int {
    let pages = (max(1, rows) + Self.pageSize - 1) / Self.pageSize
    return pages * Self.pageSize
  }
}
