import Foundation
import Testing

@testable import YeTrackerKit

@Suite("Decoding real API payloads")
struct DecodingTests {
  @Test func erasListDecodes() throws {
    let eras = try Fixture.decode(LossyArray<Era>.self, "eras").elements
    #expect(eras.count == 43)
    let first = try #require(eras.first)
    #expect(first.id == 1)
    #expect(first.name == "Before The College Dropout")
    #expect(first.songsCount == 167)
    #expect(first.coverVersion == "938ba5ccb3be")
    #expect(first.dominantColor == "666666")
    #expect(first.songCountLabel == "167 songs")
    #expect(eras.reduce(0) { $0 + ($1.songsCount ?? 0) } == 9649)
  }

  @Test func coverKeysPreferTheVersionThenTheColour() {
    #expect(Era(id: 1, name: "a", dominantColor: "#AABBCC", coverVersion: "abc").coverKey == "abc")
    #expect(Era(id: 1, name: "a", dominantColor: "#AABBCC", coverVersion: " ").coverKey == "aabbcc")
    #expect(Era(id: 1, name: "a").coverKey == "666666")
    #expect(Track(id: 1, title: "t", eraID: 1, eraName: nil, colorHex: "112233").coverKey == "112233")
  }

  @Test func singleEraHasNoSongsCount() throws {
    let era = try Fixture.decode(Era.self, "era-31")
    #expect(era.id == 31)
    #expect(era.displayName == "DONDA 2 [V1]")
    #expect(era.songsCount == nil)
    #expect(era.trimmedDescription != nil)
  }

  @Test func eraSongsDecodeWithFileEnrichment() throws {
    let songs = try Fixture.decode(LossyArray<EraSong>.self, "era-31-songs-nebraska").elements
    #expect(songs.count == 6)
    let playable = songs.filter(\.isPlayable)
    #expect(playable.map(\.id) == [6751, 6752])
    let song = try #require(playable.first)
    #expect(song.downloaded == 1)
    #expect(song.duration == 12)
    #expect(song.bestDuration == 12)
    #expect(song.lengthLabel == "OG File - 0:12")

    let silent = try #require(songs.first { $0.id == 6748 })
    #expect(!silent.isPlayable)
    #expect(silent.downloaded == nil)
    #expect(silent.duration == nil)
    #expect(silent.bestDuration == 98)
    #expect(silent.fileDate == 1_643_587_200)
    #expect(silent.sourceURL?.host == "imgur.gg")
    #expect(silent.unavailableReason == "No local file yet — open the source instead")
  }

  @Test func searchEnvelopeDecodes() throws {
    let response = try Fixture.decode(SearchResponse.self, "search-love-lockdown")
    #expect(response.songs.count == 5)
    #expect(response.total >= 5)
    let first = try #require(response.songs.first)
    #expect(first.eraName == "SWISH")
    #expect(first.eraPosition == 170)
    #expect(first.focus == SongFocus(songID: first.id, position: 170))
    #expect(first.name?.hasPrefix("✨") == true)
  }

  @Test func recentLeaksDecode() throws {
    let response = try Fixture.decode(SearchResponse.self, "recent-leaks")
    #expect(response.songs.allSatisfy { $0.isPlayable })
    #expect(response.songs.first?.leakDate == 1_771_372_800)
  }

  @Test func lenientFieldsTolerateOddTypes() throws {
    let json = #"""
      [{"id":"7","eraId":2.0,"name":null,"trackLength":"65","leakDate":1.7e9,"playable":1,"downloaded":"1"},
       {"name":"no id"},
       null,
       42,
       {"id":8,"name":"Fine","playable":"false"}]
      """#
    let songs = try JSONDecoder().decode(LossyArray<EraSong>.self, from: Data(json.utf8)).elements
    #expect(songs.map(\.id) == [7, 8])
    #expect(songs[0].eraId == 2)
    #expect(songs[0].displayTitle == "Untitled")
    #expect(songs[0].trackLength == 65)
    #expect(songs[0].leakDate == 1_700_000_000)
    #expect(songs[0].playable == true)
    #expect(songs[0].downloaded == 1)
    #expect(songs[1].isPlayable == false)
  }

  @Test func playableFallsBackToDownloaded() {
    #expect(EraSong(id: 1, name: "a", downloaded: 1).isPlayable)
    #expect(!EraSong(id: 1, name: "a", downloaded: 0).isPlayable)
    #expect(!EraSong(id: 1, name: "a", downloaded: 1, playable: false).isPlayable)
  }

  @Test func unavailableReasons() {
    #expect(
      EraSong(id: 1, name: "a", url: "https://x.y", downloaded: 0).unavailableReason
        == "Download failed for this track — open the source instead")
    #expect(EraSong(id: 1, name: "a").unavailableReason == "No audio file and no source link for this track")
  }

  @Test func sourceURLOnlyAcceptsWebLinks() {
    #expect(EraSong(id: 1, name: "a", url: "javascript:alert(1)").sourceURL == nil)
    #expect(EraSong(id: 1, name: "a", url: "  ").sourceURL == nil)
    #expect(
      EraSong(id: 1, name: "a", url: "https://pillows.su/f/abc").sourceURL?.absoluteString == "https://pillows.su/f/abc"
    )
  }

  @Test func searchHaystackCoversServerFields() {
    let song = EraSong(
      id: 1, name: "Love Lockdown", notes: "Stem BOUNCE", availableLength: "Snippet", trackLength: 65,
      quality: "CD Quality")
    #expect(song.searchHaystack == "love lockdown stem bounce cd quality snippet snippet - 1:05")
  }

  @Test func notesPreviewTruncatesAt120() throws {
    let notes = String(repeating: "word ", count: 60)
    let song = SearchSong(id: 1, name: "x", notes: notes)
    let preview = try #require(song.notesPreview as String?)
    #expect(preview.count == 120)
    #expect(preview.hasSuffix("…"))
    #expect(SearchSong(id: 1, name: "x", notes: "  a\n\n b  ").notesPreview == "a b")
    #expect(SearchSong(id: 1, name: "x", notes: " ").notesPreview == nil)
  }
}
