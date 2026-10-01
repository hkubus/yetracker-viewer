import Foundation

/// A row from `GET /eras/:id/songs`: the full catalog row plus file enrichment.
public struct EraSong: Codable, Hashable, Identifiable, Sendable {
  public let id: Int
  public let eraId: Int?
  public let catalogId: String?
  public let name: String?
  public let notes: String?
  /// Unix seconds, `0` when unknown.
  public let fileDate: Int?
  /// Unix seconds, `0` when unknown.
  public let leakDate: Int?
  public let availableLength: String?
  /// Catalog length in seconds.
  public let trackLength: Double?
  public let quality: String?
  /// Source link (pillows.su, YouTube, …).
  public let url: String?
  /// `1` stored, `0` failed, `nil` never attempted.
  public let downloaded: Int?
  public let playable: Bool?
  /// Probed file duration in seconds, only when playable.
  public let duration: Double?

  public init(
    id: Int,
    eraId: Int? = nil,
    catalogId: String? = "unreleased",
    name: String?,
    notes: String? = nil,
    fileDate: Int? = nil,
    leakDate: Int? = nil,
    availableLength: String? = nil,
    trackLength: Double? = nil,
    quality: String? = nil,
    url: String? = nil,
    downloaded: Int? = nil,
    playable: Bool? = nil,
    duration: Double? = nil
  ) {
    self.id = id
    self.eraId = eraId
    self.catalogId = catalogId
    self.name = name
    self.notes = notes
    self.fileDate = fileDate
    self.leakDate = leakDate
    self.availableLength = availableLength
    self.trackLength = trackLength
    self.quality = quality
    self.url = url
    self.downloaded = downloaded
    self.playable = playable
    self.duration = duration
  }

  enum CodingKeys: String, CodingKey {
    case id, eraId, catalogId, name, notes, fileDate, leakDate, availableLength, trackLength, quality, url
    case downloaded, playable, duration
  }

  public init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    guard let id = container.lenientInt(.id) else {
      throw DecodingError.dataCorruptedError(forKey: .id, in: container, debugDescription: "Song without a numeric id")
    }
    self.id = id
    eraId = container.lenientInt(.eraId)
    catalogId = container.lenientString(.catalogId)
    name = container.lenientString(.name)
    notes = container.lenientString(.notes)
    fileDate = container.lenientInt(.fileDate)
    leakDate = container.lenientInt(.leakDate)
    availableLength = container.lenientString(.availableLength)
    trackLength = container.lenientDouble(.trackLength)
    quality = container.lenientString(.quality)
    url = container.lenientString(.url)
    downloaded = container.lenientInt(.downloaded)
    playable = container.lenientBool(.playable)
    duration = container.lenientDouble(.duration)
  }
}

extension EraSong {
  public var displayTitle: String { name.nonBlank ?? "Untitled" }
  public var trimmedNotes: String? { notes.nonBlank }
  public var trimmedQuality: String? { quality.nonBlank }

  /// Mirrors the web's `isPlayable`: the API's `playable` flag wins, then `downloaded`.
  public var isPlayable: Bool {
    if let playable { return playable }
    return downloaded == 1
  }

  /// Probed duration first, then the catalog length (web: `duration ?? trackLength`).
  public var bestDuration: Double? {
    for candidate in [duration, trackLength] {
      if let value = candidate, value.isFinite, value > 0 { return value }
    }
    return nil
  }

  /// "Snippet - 1:05", "Full", "1:05" or "" (web `formatLength`).
  public var lengthLabel: String {
    Formatters.lengthLabel(availableLength: availableLength, seconds: bestDuration)
  }

  /// A web link to the original source, when the row has one.
  public var sourceURL: URL? { URL.webLink(url) }

  /// Why the row has no play button (web `unavailableLabel`).
  public var unavailableReason: String {
    if downloaded == 0 { return "Download failed for this track — open the source instead" }
    if url.nonBlank != nil { return "No local file yet — open the source instead" }
    return "No audio file and no source link for this track"
  }

  /// Everything the server-side `q` matches, lowercased, so the instant filter agrees with it.
  public var searchHaystack: String {
    [name, notes, quality, availableLength, lengthLabel]
      .compactMap { $0 }
      .filter { !$0.isEmpty }
      .joined(separator: " ")
      .lowercased()
  }
}

/// A result from `GET /songs` in search/filter mode.
public struct SearchSong: Codable, Hashable, Identifiable, Sendable {
  public let id: Int
  public let eraId: Int?
  public let name: String?
  public let notes: String?
  public let quality: String?
  public let availableLength: String?
  public let eraName: String?
  public let dominantColor: String?
  /// 1-based index of the song inside its era, over all of the era's songs.
  public let eraPosition: Int?
  public let leakDate: Int?
  public let playable: Bool?
  /// Catalog length in seconds.
  public let trackLength: Double?
  /// Probed length of the stored file, in seconds.
  public let duration: Double?
  public let eraCoverVersion: String?

  public init(
    id: Int,
    eraId: Int? = nil,
    name: String?,
    notes: String? = nil,
    quality: String? = nil,
    availableLength: String? = nil,
    eraName: String? = nil,
    dominantColor: String? = nil,
    eraPosition: Int? = nil,
    leakDate: Int? = nil,
    playable: Bool? = nil,
    trackLength: Double? = nil,
    duration: Double? = nil,
    eraCoverVersion: String? = nil
  ) {
    self.id = id
    self.eraId = eraId
    self.name = name
    self.notes = notes
    self.quality = quality
    self.availableLength = availableLength
    self.eraName = eraName
    self.dominantColor = dominantColor
    self.eraPosition = eraPosition
    self.leakDate = leakDate
    self.playable = playable
    self.trackLength = trackLength
    self.duration = duration
    self.eraCoverVersion = eraCoverVersion
  }

  enum CodingKeys: String, CodingKey {
    case id, eraId, name, notes, quality, availableLength, eraName, dominantColor, eraPosition, leakDate, playable
    case trackLength, duration, eraCoverVersion
  }

  public init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    guard let id = container.lenientInt(.id) else {
      throw DecodingError.dataCorruptedError(forKey: .id, in: container, debugDescription: "Song without a numeric id")
    }
    self.id = id
    eraId = container.lenientInt(.eraId)
    name = container.lenientString(.name)
    notes = container.lenientString(.notes)
    quality = container.lenientString(.quality)
    availableLength = container.lenientString(.availableLength)
    eraName = container.lenientString(.eraName)
    dominantColor = container.lenientString(.dominantColor)
    eraPosition = container.lenientInt(.eraPosition)
    leakDate = container.lenientInt(.leakDate)
    playable = container.lenientBool(.playable)
    trackLength = container.lenientDouble(.trackLength)
    duration = container.lenientDouble(.duration)
    eraCoverVersion = container.lenientString(.eraCoverVersion)
  }
}

extension SearchSong {
  /// Probed duration first, then the catalog length (as `EraSong.bestDuration`).
  public var bestDuration: Double? {
    for candidate in [duration, trackLength] {
      if let value = candidate, value.isFinite, value > 0 { return value }
    }
    return nil
  }

  /// Web `GlobalSongSearch`'s description limit.
  public static let notesPreviewLimit = 120

  public var displayTitle: String { name.nonBlank ?? "Untitled" }
  public var eraDisplayName: String { eraName.nonBlank ?? "Unknown era" }
  public var color: RGBColor { RGBColor(hex: dominantColor) ?? .fallbackAccent }
  public var isPlayable: Bool { playable ?? false }

  /// Notes with whitespace collapsed, cut to 120 characters with an ellipsis.
  public var notesPreview: String? {
    guard let notes = notes.nonBlank else { return nil }
    return TextNormalization.truncate(TextNormalization.collapseWhitespace(notes), limit: Self.notesPreviewLimit)
  }

  /// Where this song sits inside its era, for deep links.
  public var focus: SongFocus { SongFocus(songID: id, position: eraPosition.flatMap { $0 > 0 ? $0 : nil }) }
}

/// `GET /songs` search envelope.
public struct SearchResponse: Codable, Hashable, Sendable {
  public let songs: [SearchSong]
  /// Matches before the `limit` slice.
  public let total: Int

  public init(songs: [SearchSong], total: Int) {
    self.songs = songs
    self.total = total
  }

  enum CodingKeys: String, CodingKey { case songs, total }

  public init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    songs = (try? container.decode(LossyArray<SearchSong>.self, forKey: .songs).elements) ?? []
    total = container.lenientInt(.total) ?? songs.count
  }
}

/// Decodes an array, dropping elements that fail to decode instead of failing the whole payload
/// (the web filters list items the same way).
struct LossyArray<Element: Decodable>: Decodable {
  let elements: [Element]

  init(from decoder: Decoder) throws {
    var container = try decoder.unkeyedContainer()
    var elements: [Element] = []
    if let count = container.count { elements.reserveCapacity(count) }
    while !container.isAtEnd {
      let index = container.currentIndex
      if let element = try? container.decode(Element.self) {
        elements.append(element)
      } else if (try? container.decodeNil()) != true {
        // Advance past the undecodable element.
        _ = try? container.decode(Discarded.self)
      }
      // Never spin on an element the container refuses to step over.
      if container.currentIndex == index { break }
    }
    self.elements = elements
  }

  /// Accepts any JSON value without reading it.
  private struct Discarded: Decodable {
    init(from decoder: Decoder) throws {}
  }
}

extension URL {
  /// An absolute http(s) URL from catalog text, or `nil`.
  static func webLink(_ value: String?) -> URL? {
    guard let value = value.nonBlank, let url = URL(string: value),
      let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https", url.host != nil
    else { return nil }
    return url
  }
}
