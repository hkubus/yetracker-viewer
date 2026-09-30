// swift-tools-version: 6.0
import PackageDescription

// Platform-neutral core of the iOS app: models, API client, formatting, routing
// and the observable feature models. Foundation + Observation only, so it
// builds and tests on Linux as well as Apple platforms.
let package = Package(
  name: "YeTrackerKit",
  platforms: [.iOS(.v17), .macOS(.v14)],
  products: [
    .library(name: "YeTrackerKit", targets: ["YeTrackerKit"])
  ],
  targets: [
    .target(name: "YeTrackerKit"),
    .testTarget(
      name: "YeTrackerKitTests",
      dependencies: ["YeTrackerKit"],
      resources: [.copy("Fixtures")]
    ),
  ]
)
