import SwiftUI
import YeTrackerKit

/// Tabs, one navigation stack per browsing tab, the mini player above the tab
/// bar and the Now Playing sheet.
struct RootView: View {
  @Environment(AppModel.self) private var app

  var body: some View {
    @Bindable var router = app.router
    TabView(selection: $router.selectedTab) {
      BrowsingTab(tab: .home, path: $router.homePath) {
        HomeView()
      }
      .tabItem { Label("Eras", systemImage: "square.grid.2x2.fill") }
      .tag(AppTab.home)

      BrowsingTab(tab: .search, path: $router.searchPath) {
        SearchView()
      }
      .tabItem { Label("Search", systemImage: "magnifyingglass") }
      .tag(AppTab.search)

      NavigationStack {
        SettingsView()
      }
      .miniPlayerInset()
      .tabItem { Label("Settings", systemImage: "gearshape.fill") }
      .tag(AppTab.settings)
    }
    .sheet(isPresented: $router.isNowPlayingPresented) {
      NowPlayingView()
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
    .miniPlayerInset()
  }
}

extension View {
  /// Reserves room for the download banner and the mini player above the tab bar.
  func miniPlayerInset() -> some View {
    modifier(MiniPlayerInset())
  }
}

private struct MiniPlayerInset: ViewModifier {
  @Environment(AppModel.self) private var app
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  func body(content: Content) -> some View {
    content.safeAreaInset(edge: .bottom, spacing: 0) {
      // Out of the way while typing: the inset would sit on top of the keyboard.
      if !app.keyboard.isVisible {
        chrome
      }
    }
  }

  private var chrome: some View {
    VStack(spacing: 6) {
      if app.downloads.job != nil {
        DownloadBanner()
          .transition(.move(edge: .bottom).combined(with: .opacity))
      }
      if app.player.isActive {
        MiniPlayerView()
          .transition(.move(edge: .bottom).combined(with: .opacity))
      }
    }
    .padding(.horizontal, 8)
    .padding(.bottom, 6)
    .animation(reduceMotion ? nil : .snappy, value: app.player.isActive)
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
