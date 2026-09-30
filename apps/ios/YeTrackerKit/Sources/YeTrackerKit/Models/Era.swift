import Foundation

/// An era from `GET /eras` (with `songsCount`) or `GET /eras/:id` (without).
public struct Era: Codable, Hashable, Identifiable, Sendable {
  public let id: Int
  public let name: String?
  public let notes: String?
  public let description: String?
  public let dominantColor: String?
  /// `sha1(imageUrl)[0:12]`, used to version cover URLs (`?v=`).
  public let coverVersion: String?
  /// Only present in the list endpoint.
  public let songsCount: Int?

  public init(
    id: Int,
    name: String?,
    notes: String? = nil,
    description: String? = nil,
    dominantColor: String? = nil,
    coverVersion: String? = nil,
    songsCount: Int? = nil
  ) {
    self.id = id
    self.name = name
    self.notes = notes
    self.description = description
    self.dominantColor = dominantColor
    self.coverVersion = coverVersion
    self.songsCount = songsCount
  }

  enum CodingKeys: String, CodingKey {
    case id, name, notes, description, dominantColor, coverVersion, songsCount
  }

  public init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    guard let id = container.lenientInt(.id) else {
      throw DecodingError.dataCorruptedError(forKey: .id, in: container, debugDescription: "Era without a numeric id")
    }
    self.id = id
    name = container.lenientString(.name)
    notes = container.lenientString(.notes)
    description = container.lenientString(.description)
    dominantColor = container.lenientString(.dominantColor)
    coverVersion = container.lenientString(.coverVersion)
    songsCount = container.lenientInt(.songsCount)
  }
}

extension Era {
  public var displayName: String { name.nonBlank ?? "Untitled era" }

  /// The era accent, falling back to the web's `#666666`.
  public var color: RGBColor { RGBColor(hex: dominantColor) ?? .fallbackAccent }

  public var trimmedDescription: String? { description.nonBlank }
  public var trimmedNotes: String? { notes.nonBlank }

  /// Cover cache key (`?v=`): the cover version, else the colour, as on the web
  /// era page. Every cover URL for an era must use it, so each is cached once.
  public var coverKey: String { coverVersion.nonBlank ?? color.hex }

  /// "1 song" / "1,234 songs".
  public var songCountLabel: String {
    let count = max(0, songsCount ?? 0)
    return "\(count.formatted()) \(count == 1 ? "song" : "songs")"
  }
}
