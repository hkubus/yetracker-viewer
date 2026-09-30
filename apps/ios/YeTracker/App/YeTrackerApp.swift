import SwiftUI

@main
struct YeTrackerApp: App {
  @State private var app = AppModel()

  var body: some Scene {
    WindowGroup {
      RootView()
        .environment(app)
        .onOpenURL { url in app.router.open(url) }
    }
  }
}
