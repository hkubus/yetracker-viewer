import Foundation

/// Suspends for a duration. Injected so debounce and delay logic is testable.
public typealias Sleeper = @Sendable (Duration) async throws -> Void

public enum Sleepers {
  public static let live: Sleeper = { duration in try await Task.sleep(for: duration) }
}

/// Supplies the current `APIClient` (the server URL can change at runtime).
public typealias APIProvider = @MainActor () -> APIClient
