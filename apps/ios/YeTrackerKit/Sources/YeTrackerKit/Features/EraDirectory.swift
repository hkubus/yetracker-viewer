import Foundation
import Observation

/// The shared `GET /eras` list: the home grid, the search era range and the
/// era screen's previous/next links all read it.
@MainActor
@Observable
public final class EraDirectory {
  public enum State: Equatable, Sendable {
    case idle
    case loading
    case loaded
    /// Nothing could be loaded. A failed refresh keeps the old list instead.
    case failed(APIError)
  }

  public private(set) var eras: [Era] = []
  public private(set) var state: State = .idle
  /// Set when a refresh failed but an older list is still shown.
  public private(set) var refreshError: APIError?

  @ObservationIgnored private let api: APIProvider
  @ObservationIgnored private var inflight: Task<Void, Never>?
  @ObservationIgnored private var generation = 0

  public init(api: @escaping APIProvider) {
    self.api = api
  }

  /// Loads the list once. `force` refetches, bypassing the HTTP cache.
  public func load(force: Bool = false) async {
    if let inflight {
      await inflight.value
      if !force { return }
    }
    if !force, state == .loaded { return }
    generation += 1
    let current = generation
    let task = Task { await self.fetch(reload: force, generation: current) }
    inflight = task
    await task.value
    if generation == current { inflight = nil }
  }

  /// Forgets everything (the server changed).
  public func reset() {
    generation += 1
    inflight?.cancel()
    inflight = nil
    eras = []
    state = .idle
    refreshError = nil
  }

  public func era(id: Int?) -> Era? {
    guard let id else { return nil }
    return eras.first { $0.id == id }
  }

  public func index(of id: Int) -> Int? {
    eras.firstIndex { $0.id == id }
  }

  /// Neighbours in catalog order (web era navigation). Both `nil` when the list is unavailable.
  public func neighbors(of id: Int) -> (previous: Era?, next: Era?) {
    guard let index = index(of: id) else { return (nil, nil) }
    let previous = index > 0 ? eras[index - 1] : nil
    let next = index < eras.count - 1 ? eras[index + 1] : nil
    return (previous, next)
  }

  /// Sum of `songsCount` (the home header's track count).
  public var totalSongs: Int {
    eras.reduce(0) { $0 + max(0, $1.songsCount ?? 0) }
  }

  private func fetch(reload: Bool, generation: Int) async {
    if eras.isEmpty { state = .loading }
    do {
      let loaded = try await api().eras(reload: reload)
      guard generation == self.generation else { return }
      eras = loaded
      state = .loaded
      refreshError = nil
    } catch {
      let apiError = APIError(transportError: error)
      guard generation == self.generation, !apiError.isCancellation else { return }
      if eras.isEmpty {
        state = .failed(apiError)
      } else {
        refreshError = apiError
      }
    }
  }
}
