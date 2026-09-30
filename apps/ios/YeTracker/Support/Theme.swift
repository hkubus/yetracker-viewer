import SwiftUI
import YeTrackerKit

extension Color {
  init(_ rgb: RGBColor, opacity: Double = 1) {
    self.init(.sRGB, red: rgb.red, green: rgb.green, blue: rgb.blue, opacity: opacity)
  }

  init(hex: String) {
    self.init(RGBColor(hex: hex) ?? .fallbackAccent)
  }
}

/// App-wide colours from the web stylesheets (dark only).
enum Theme {
  /// `--bg-color`.
  static let background = Color(RGBColor.background)
  /// Search panel / card surfaces.
  static let surface = Color(hex: "202020")
  static let surfaceRaised = Color(hex: "292929")
  static let border = Color(hex: "414141")
  static let secondaryText = Color(hex: "9a9a9a")
  static let tertiaryText = Color(hex: "777777")
  /// The search accent (`--search-accent`), also the app tint.
  static let accent = Color(hex: "d7d7d7")
  /// Player error text.
  static let errorText = Color(hex: "ffb4a9")
  /// Catalog failure notice.
  static let noticeBackground = Color(hex: "2a1d19")
  static let noticeBorder = Color(hex: "7c4a3a")
  static let noticeText = Color(hex: "ffded4")
}

/// An era's tints, using the same `color-mix` recipes as the web.
struct EraPalette {
  let accent: RGBColor

  init(_ accent: RGBColor) {
    self.accent = accent
  }

  var accentColor: Color { Color(accent) }
  /// Card text: `color-mix(in oklab, accent 30%, white)`.
  var cardText: Color { Color(accent.mixed(with: .white, weight: 0.3, in: .oklab)) }
  /// Header text: `color-mix(in oklab, accent 20%, white)`.
  var headerText: Color { Color(accent.mixed(with: .white, weight: 0.2, in: .oklab)) }
  /// Body text on era pages: `color-mix(in oklab, accent 10%, white)`.
  var bodyText: Color { Color(accent.mixed(with: .white, weight: 0.1, in: .oklab)) }
  /// Muted labels: `color-mix(in srgb, accent 25%, #ccc)`.
  var mutedText: Color { Color(accent.mixed(with: RGBColor(hex: "cccccc")!, weight: 0.25)) }
  /// Card fill: accent at 25 %.
  var cardFill: Color { Color(accent, opacity: 0.25) }
  /// Card border: accent at 70 %.
  var border: Color { Color(accent, opacity: 0.7) }
  /// Song card fill on phones: accent at 12 %.
  var rowFill: Color { Color(accent, opacity: 0.12) }
  var rowBorder: Color { Color(accent, opacity: 0.45) }
  /// Deep-link target: `color-mix(in srgb, accent 35%, #242424)`.
  var highlightFill: Color { Color(accent.mixed(with: RGBColor(hex: "242424")!, weight: 0.35)) }
  /// Playing row: `color-mix(in srgb, accent 32%, #1f1f1f)`.
  var playingFill: Color { Color(accent.mixed(with: RGBColor(hex: "1f1f1f")!, weight: 0.32)) }
  /// Player background: `color-mix(in oklab, accent 40%, #181818 60%)`.
  var playerBackground: Color { Color(accent.mixed(with: .background, weight: 0.4, in: .oklab)) }
  /// Player text: `color-mix(in oklab, accent 16%, white)`.
  var playerText: Color { Color(accent.mixed(with: .white, weight: 0.16, in: .oklab)) }
  /// Page wash behind era screens: accent at 20 % over the background.
  var pageWash: Color { Color(accent.mixed(with: .background, weight: 0.2)) }
  /// Recent leak / search result card: `color-mix(in srgb, accent 18%, #202020)`.
  var listCardFill: Color { Color(accent.mixed(with: RGBColor(hex: "202020")!, weight: 0.18)) }
  /// The playing recent leak: `color-mix(in srgb, accent 30%, #2a2a2a)`.
  var listCardPlayingFill: Color { Color(accent.mixed(with: RGBColor(hex: "2a2a2a")!, weight: 0.3)) }
  /// `color-mix(in srgb, accent 55%, #3a3a3a)`.
  var listCardBorder: Color { Color(accent.mixed(with: RGBColor(hex: "3a3a3a")!, weight: 0.55)) }
  /// Left edge: `color-mix(in srgb, accent 80%, white)`.
  var listCardEdge: Color { Color(accent.mixed(with: .white, weight: 0.8)) }
}
