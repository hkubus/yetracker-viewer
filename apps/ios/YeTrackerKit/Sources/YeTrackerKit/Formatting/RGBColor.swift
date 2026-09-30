import Foundation

/// A gamma-encoded sRGB colour with CSS `color-mix()` semantics, so the app
/// derives the same era tints as the web stylesheets.
public struct RGBColor: Hashable, Sendable {
  /// Components in 0…1 (gamma-encoded sRGB).
  public var red: Double
  public var green: Double
  public var blue: Double

  public init(red: Double, green: Double, blue: Double) {
    self.red = Self.clamp(red)
    self.green = Self.clamp(green)
    self.blue = Self.clamp(blue)
  }

  /// Parses "RRGGBB" or "#RRGGBB" (web `COLOR_PATTERN`); `nil` otherwise.
  public init?(hex: String?) {
    guard var value = hex?.trimmingCharacters(in: .whitespacesAndNewlines) else { return nil }
    if value.hasPrefix("#") { value.removeFirst() }
    guard value.count == 6, value.allSatisfy(\.isHexDigit), let raw = UInt32(value, radix: 16) else { return nil }
    self.init(
      red: Double((raw >> 16) & 0xFF) / 255,
      green: Double((raw >> 8) & 0xFF) / 255,
      blue: Double(raw & 0xFF) / 255)
  }

  /// The web's fallback accent, `#666666`.
  public static let fallbackAccent = RGBColor(red: 0x66 / 255, green: 0x66 / 255, blue: 0x66 / 255)
  /// Page background, `#181818`.
  public static let background = RGBColor(red: 0x18 / 255, green: 0x18 / 255, blue: 0x18 / 255)
  public static let white = RGBColor(red: 1, green: 1, blue: 1)
  public static let black = RGBColor(red: 0, green: 0, blue: 0)

  /// Lowercase "rrggbb".
  public var hex: String {
    let components = [red, green, blue].map { Int(($0 * 255).rounded()) }
    return components.map { String(format: "%02x", $0) }.joined()
  }

  public enum MixSpace: Sendable {
    case srgb
    case oklab
  }

  /// CSS `color-mix(in <space>, self <weight>, other)`: `weight` (0…1) is this colour's share.
  public func mixed(with other: RGBColor, weight: Double, in space: MixSpace = .srgb) -> RGBColor {
    let share = Self.clamp(weight)
    switch space {
    case .srgb:
      return RGBColor(
        red: red * share + other.red * (1 - share),
        green: green * share + other.green * (1 - share),
        blue: blue * share + other.blue * (1 - share))
    case .oklab:
      let a = oklab
      let b = other.oklab
      return RGBColor(
        oklab: (
          a.l * share + b.l * (1 - share),
          a.a * share + b.a * (1 - share),
          a.b * share + b.b * (1 - share)
        ))
    }
  }

  /// WCAG relative luminance (0 black … 1 white).
  public var relativeLuminance: Double {
    0.2126 * Self.linear(red) + 0.7152 * Self.linear(green) + 0.0722 * Self.linear(blue)
  }

  // MARK: - OKLab (Björn Ottosson)

  var oklab: (l: Double, a: Double, b: Double) {
    let r = Self.linear(red)
    let g = Self.linear(green)
    let b = Self.linear(blue)
    let l = cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
    let m = cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
    let s = cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
    return (
      0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
      1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
      0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s
    )
  }

  init(oklab: (l: Double, a: Double, b: Double)) {
    let l = oklab.l + 0.3963377774 * oklab.a + 0.2158037573 * oklab.b
    let m = oklab.l - 0.1055613458 * oklab.a - 0.0638541728 * oklab.b
    let s = oklab.l - 0.0894841775 * oklab.a - 1.2914855480 * oklab.b
    let l3 = l * l * l
    let m3 = m * m * m
    let s3 = s * s * s
    self.init(
      red: Self.gamma(4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3),
      green: Self.gamma(-1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3),
      blue: Self.gamma(-0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3))
  }

  private static func linear(_ value: Double) -> Double {
    value <= 0.04045 ? value / 12.92 : pow((value + 0.055) / 1.055, 2.4)
  }

  private static func gamma(_ value: Double) -> Double {
    let clamped = clamp(value)
    return clamped <= 0.0031308 ? clamped * 12.92 : 1.055 * pow(clamped, 1 / 2.4) - 0.055
  }

  private static func clamp(_ value: Double) -> Double {
    guard value.isFinite else { return 0 }
    return min(1, max(0, value))
  }
}
