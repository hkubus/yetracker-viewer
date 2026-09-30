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

/// App-wide colours. Everything else uses the system's semantic colours, so the
/// app follows the light/dark appearance like any other iOS app.
enum Theme {
  /// Error text (player, downloads, paging).
  static let error = Color.red
}

/// An era's tints for the current appearance. Sheet colours range from near
/// black to near white, so text and controls use the accent pulled towards
/// white (dark mode) or black (light mode) to stay readable on the system
/// background.
struct EraPalette {
  let accent: RGBColor
  let colorScheme: ColorScheme

  init(_ accent: RGBColor, colorScheme: ColorScheme) {
    self.accent = accent
    self.colorScheme = colorScheme
  }

  private var isDark: Bool { colorScheme == .dark }

  var accentColor: Color { Color(accent) }

  /// Text, symbols and control tints in the era's colour.
  var tint: Color {
    isDark
      ? Color(accent.mixed(with: .white, weight: 0.55, in: .oklab))
      : Color(accent.mixed(with: .black, weight: 0.6, in: .oklab))
  }

  /// The wash at the top of an era page, fading into the system background.
  var pageWash: Color { Color(accent, opacity: isDark ? 0.45 : 0.3) }
  /// Fill of the playing row.
  var playingFill: Color { Color(accent, opacity: isDark ? 0.2 : 0.14) }
  /// Fill of a deep-link target row.
  var highlightFill: Color { Color(accent, opacity: isDark ? 0.32 : 0.24) }
  /// Circle behind a row's play symbol.
  var controlFill: Color { Color(accent, opacity: isDark ? 0.3 : 0.18) }

  // Now Playing is always dark, over a gradient of the era colour.

  /// `color-mix(in oklab, accent 40%, #181818 60%)`, the web player's background.
  var playerBackground: Color { Color(accent.mixed(with: .background, weight: 0.4, in: .oklab)) }
  /// `color-mix(in oklab, accent 16%, white)`, the web player's text.
  var playerText: Color { Color(accent.mixed(with: .white, weight: 0.16, in: .oklab)) }
}

