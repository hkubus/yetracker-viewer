import SwiftUI
import YeTrackerKit

/// Tabs with one navigation stack per browsing tab, the search tab in the tab
/// bar, the mini player as the tab bar's accessory, and the Now Playing sheet
/// that zooms out of it.
struct RootView: View {
  @Environment(AppModel.self) private var app
  @Namespace private var playerTransition

  var body: some View {
    @Bindable var router = app.router
    @Bindable var search = app.search
    TabView(selection: $router.selectedTab) {
      Tab("Eras", systemImage: "square.grid.2x2", value: AppTab.home) {
        BrowsingTab(tab: .home, path: $router.homePath) {
          HomeView()
        }
      }

      Tab("Settings", systemImage: "gearshape", value: AppTab.settings) {
        NavigationStack {
          SettingsView()
        }
        .downloadBannerInset()
      }

      Tab("Search", systemImage: "magnifyingglass", value: AppTab.search, role: .search) {
        BrowsingTab(tab: .search, path: $router.searchPath) {
          SearchView()
        }
        .searchable(text: $search.query, prompt: "Songs, artists, notes")
      }
    }
    .tabBarMinimizeBehavior(.onScrollDown)
    .modifier(MiniPlayerAccessory(isActive: app.player.isActive, transition: playerTransition))
    .sheet(isPresented: $router.isNowPlayingPresented) {
      NowPlayingView()
        .navigationTransition(.zoom(sourceID: MiniPlayerAccessory.transitionID, in: playerTransition))
    }
    .background {
      if app.player.isActive {
        PlayerKeyboardShortcuts(player: app.player)
      }
    }
  }
}

/// A tab with its own navigation stack of `AppRoute`s.
private struct BrowsingTab<Root: View>: View {
  @Environment(AppModel.self) private var app
  let tab: AppTab
  @Binding var path: [AppRoute]
  @ViewBuilder let root: () -> Root

  var body: some View {
    NavigationStack(path: $path) {
      root()
        .navigationDestination(for: AppRoute.self) { route in
          switch route {
          case .era(let eraRoute):
            EraDetailView(route: eraRoute, tab: tab, directory: app.directory, api: { [app] in app.api })
              .id(eraRoute)
          }
        }
    }
    .downloadBannerInset()
  }
}

/// The mini player in the tab bar's bottom accessory, like Music; hidden while
/// nothing plays.
private struct MiniPlayerAccessory: ViewModifier {
  static let transitionID = "now-playing"
  let isActive: Bool
  let transition: Namespace.ID

  func body(content: Content) -> some View {
    content.tabViewBottomAccessory(isEnabled: isActive) {
      MiniPlayerView()
        .matchedTransitionSource(id: Self.transitionID, in: transition)
    }
  }
}

extension View {
  /// Reserves room for the download banner above the tab bar.
  func downloadBannerInset() -> some View {
    modifier(DownloadBannerInset())
  }
}

private struct DownloadBannerInset: ViewModifier {
  @Environment(AppModel.self) private var app
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  func body(content: Content) -> some View {
    content.safeAreaInset(edge: .bottom, spacing: 0) {
      // Out of the way while typing: the inset would sit on top of the keyboard.
      if app.downloads.job != nil, !app.keyboard.isVisible {
        DownloadBanner()
          .padding(.horizontal, 16)
          .padding(.bottom, 8)
          .transition(.move(edge: .bottom).combined(with: .opacity))
      }
    }
    .animation(reduceMotion ? nil : .snappy, value: app.downloads.job != nil)
  }
}

/// Hardware keyboard control on iPad: Space toggles, ←/→ seek 5 s,
/// ⌘←/⌘→ change track. Text fields keep their own keys.
private struct PlayerKeyboardShortcuts: View {
  let player: PlayerModel

  var body: some View {
    VStack {
      Button("Play or Pause") { player.togglePlayPause() }
        .keyboardShortcut(.space, modifiers: [])
      Button("Seek Back 5 Seconds") { player.skip(by: -PlayerModel.seekStep) }
        .keyboardShortcut(.leftArrow, modifiers: [])
      Button("Seek Forward 5 Seconds") { player.skip(by: PlayerModel.seekStep) }
        .keyboardShortcut(.rightArrow, modifiers: [])
      Button("Previous Track") { player.previous() }
        .keyboardShortcut(.leftArrow, modifiers: .command)
      Button("Next Track") { player.next() }
        .keyboardShortcut(.rightArrow, modifiers: .command)
    }
    .opacity(0)
    .frame(width: 0, height: 0)
    .accessibilityHidden(true)
    .allowsHitTesting(false)
  }
}
