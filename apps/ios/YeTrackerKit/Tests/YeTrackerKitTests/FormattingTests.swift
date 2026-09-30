import Foundation
import Testing

@testable import YeTrackerKit

@Suite("Formatting")
struct FormattingTests {
  let enUS = Locale(identifier: "en_US")

  @Test func durations() {
    #expect(Formatters.duration(0) == "0:00")
    #expect(Formatters.duration(65.9) == "1:05")
    #expect(Formatters.duration(3600) == "60:00")
    #expect(Formatters.duration(nil) == "—")
    #expect(Formatters.duration(-1) == "—")
    #expect(Formatters.duration(.infinity) == "—")
    #expect(Formatters.duration(.nan) == "—")
  }

  @Test func lengthLabels() {
    #expect(Formatters.lengthLabel(availableLength: "Snippet", seconds: 65) == "Snippet - 1:05")
    #expect(Formatters.lengthLabel(availableLength: "Full", seconds: nil) == "Full")
    #expect(Formatters.lengthLabel(availableLength: nil, seconds: 65) == "1:05")
    #expect(Formatters.lengthLabel(availableLength: "", seconds: 0) == "")
  }

  @Test func datesAreFormattedInUTC() {
    // 2009-04-22T00:00:00Z: local time zones west of UTC must not show the 21st.
    #expect(Formatters.shortDate(1_240_358_400, locale: enUS) == "4/22/09")
    #expect(Formatters.mediumDate(1_240_358_400, locale: enUS) == "Apr 22, 2009")
    #expect(Formatters.shortDate(0, locale: enUS) == nil)
    #expect(Formatters.shortDate(nil, locale: enUS) == nil)
    #expect(Formatters.mediumDate(0, locale: enUS) == "Unknown date")
  }

  @Test func elapsedDescriptions() {
    #expect(Formatters.elapsedDescription(65) == "1:05 elapsed")
    #expect(Formatters.elapsedDescription(-1) == "unknown position")
  }

  @Test func normalization() {
    #expect(TextNormalization.normalize("  Love\t\nLOCKDOWN  ") == "love lockdown")
    #expect(TextNormalization.collapseWhitespace(" a  b ") == "a b")
    let long = String(repeating: "ab ", count: 60)
    let query = TextNormalization.apiQuery(long)
    #expect(query.count <= TextNormalization.maxQueryLength)
    #expect(!query.hasSuffix(" "))
  }

  @Test func substringMatchingIsByteLevelLikeTheAPI() {
    // Swift's `contains` misses both of these; SQL `instr`/`LIKE` and JS `includes` find them.
    #expect(TextNormalization.contains("⭐\u{FE0F} love", "⭐"))
    #expect(TextNormalization.contains("love\u{301}", "love"))
    #expect(!("⭐\u{FE0F} love".contains("⭐")), "documents why the helper exists")
    #expect(TextNormalization.contains("anything", ""))
    #expect(!TextNormalization.contains("abc", "abd"))
  }

  @Test func truncation() {
    #expect(TextNormalization.truncate("short", limit: 120) == "short")
    #expect(TextNormalization.truncate("abcdef", limit: 4) == "abc…")
    #expect(TextNormalization.truncate("ab cdef", limit: 4) == "ab…")
  }

  @Test func counts() {
    #expect(TextNormalization.count(1, "song", "songs") == "1 song")
    #expect(TextNormalization.count(0, "song", "songs") == "0 songs")
  }
}

@Suite("Colour")
struct ColorTests {
  @Test func hexParsing() {
    #expect(RGBColor(hex: "#FF8000")?.hex == "ff8000")
    #expect(RGBColor(hex: " 336699 ")?.hex == "336699")
    #expect(RGBColor(hex: "zzzzzz") == nil)
    #expect(RGBColor(hex: "fff") == nil)
    #expect(RGBColor(hex: nil) == nil)
    #expect(RGBColor.fallbackAccent.hex == "666666")
    #expect(RGBColor.background.hex == "181818")
  }

  @Test func srgbMixIsLinearInterpolation() throws {
    let red = try #require(RGBColor(hex: "ff0000"))
    #expect(red.mixed(with: .white, weight: 1).hex == "ff0000")
    #expect(red.mixed(with: .white, weight: 0).hex == "ffffff")
    #expect(red.mixed(with: .black, weight: 0.5).hex == "800000")
  }

  @Test func oklabRoundTripsAndMixes() throws {
    for hex in ["000000", "ffffff", "666666", "ff8000", "123456", "abcdef"] {
      let color = try #require(RGBColor(hex: hex))
      #expect(RGBColor(oklab: color.oklab).hex == hex)
    }
    let accent = try #require(RGBColor(hex: "3366cc"))
    let tinted = accent.mixed(with: .white, weight: 0.3, in: .oklab)
    #expect(tinted.relativeLuminance > accent.relativeLuminance)
    // CSS color-mix(in oklab, #666666 50%, white): L = (0.5103 + 1) / 2 → a neutral #afafaf.
    let grey = RGBColor.fallbackAccent.mixed(with: .white, weight: 0.5, in: .oklab)
    #expect(grey.hex == "afafaf")
  }
}
