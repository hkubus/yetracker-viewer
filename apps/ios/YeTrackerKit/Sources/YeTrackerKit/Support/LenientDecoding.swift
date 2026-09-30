import Foundation

/// Tolerant field readers for API payloads.
///
/// The API is typed (`i64`, `f64`, nullable strings), but a catalog row with one
/// odd value must not make a whole page undecodable. Every reader returns `nil`
/// instead of throwing when the key is missing, `null`, or has an unexpected type.
extension KeyedDecodingContainer {
  func lenientInt(_ key: Key) -> Int? {
    if let value = try? decodeIfPresent(Int.self, forKey: key) { return value }
    if let value = try? decodeIfPresent(Double.self, forKey: key), value.isFinite,
      value >= Double(Int.min), value <= Double(Int.max)
    {
      return Int(value)
    }
    if let value = try? decodeIfPresent(String.self, forKey: key) {
      return Int(value.trimmingCharacters(in: .whitespaces))
    }
    return nil
  }

  func lenientDouble(_ key: Key) -> Double? {
    if let value = try? decodeIfPresent(Double.self, forKey: key), value.isFinite { return value }
    if let value = try? decodeIfPresent(String.self, forKey: key), let parsed = Double(value), parsed.isFinite {
      return parsed
    }
    return nil
  }

  func lenientString(_ key: Key) -> String? {
    if let value = try? decodeIfPresent(String.self, forKey: key) { return value }
    if let value = try? decodeIfPresent(Int.self, forKey: key) { return String(value) }
    if let value = try? decodeIfPresent(Double.self, forKey: key) { return String(value) }
    return nil
  }

  func lenientBool(_ key: Key) -> Bool? {
    if let value = try? decodeIfPresent(Bool.self, forKey: key) { return value }
    if let value = try? decodeIfPresent(Int.self, forKey: key) { return value != 0 }
    if let value = try? decodeIfPresent(String.self, forKey: key) {
      switch value.lowercased() {
      case "true", "1": return true
      case "false", "0": return false
      default: return nil
      }
    }
    return nil
  }
}

extension String {
  /// `nil` when the string is empty after trimming whitespace and newlines.
  var nonBlank: String? {
    let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
    return trimmed.isEmpty ? nil : trimmed
  }
}

extension Optional where Wrapped == String {
  var nonBlank: String? { self?.nonBlank }
}
