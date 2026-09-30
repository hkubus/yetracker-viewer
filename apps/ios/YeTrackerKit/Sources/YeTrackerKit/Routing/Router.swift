import Foundation
import Observation

/// The app's tabs.
public enum AppTab: String, Hashable, Sendable, CaseIterable {
  case home
  case search
  case settings
}

/// Tab selection, one navigation path per browsing tab, and the Now Playing sheet.
@MainActor
@Observable
public final class Router {
  public var selectedTab: AppTab = .home
  public var homePath: [AppRoute] = []
  public var searchPath: [AppRoute] = []
  public var isNowPlayingPresented = false

  public init() {}

  /// Handles an incoming URL. Returns `false` when the URL is not ours.
  @discardableResult
  public func open(_ url: URL) -> Bool {
    guard let target = DeepLink.parse(url) else { return false }
    isNowPlayingPresented = false
    selectedTab = .home
    switch target {
    case .home:
      homePath = []
    case .era(let route):
      homePath = [.era(route)]
    }
    return true
  }

  /// Pushes an era onto the given tab's stack (the current tab by default;
  /// Settings has no stack, so Home is used instead).
  public func show(_ route: EraRoute, in tab: AppTab? = nil) {
    let target = browsingTab(tab ?? selectedTab)
    selectedTab = target
    append(.era(route), to: target)
  }

  /// Opens an era from the player: closes the sheet first.
  public func showFromPlayer(_ route: EraRoute) {
    isNowPlayingPresented = false
    show(route)
  }

  /// Swaps the top era for a neighbour (previous/next era) without growing the stack.
  public func replaceTop(with route: EraRoute, in tab: AppTab) {
    let target = browsingTab(tab)
    var path = self.path(for: target)
    if path.isEmpty {
      path = [.era(route)]
    } else {
      path[path.count - 1] = .era(route)
    }
    setPath(path, for: target)
  }

  public func popToRoot(_ tab: AppTab) {
    setPath([], for: browsingTab(tab))
  }

  public func path(for tab: AppTab) -> [AppRoute] {
    switch browsingTab(tab) {
    case .search: searchPath
    default: homePath
    }
  }

  private func append(_ route: AppRoute, to tab: AppTab) {
    var path = self.path(for: tab)
    path.append(route)
    setPath(path, for: tab)
  }

  private func setPath(_ path: [AppRoute], for tab: AppTab) {
    switch browsingTab(tab) {
    case .search: searchPath = path
    default: homePath = path
    }
  }

  private func browsingTab(_ tab: AppTab) -> AppTab {
    tab == .settings ? .home : tab
  }
}
