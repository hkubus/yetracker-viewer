import Foundation

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

/// Everything that can go wrong talking to the API, with user-facing copy.
public enum APIError: Error, Equatable, Sendable {
  /// The configured server URL is not a usable http(s) URL.
  case invalidBaseURL
  case offline
  case timedOut
  /// DNS failure, refused connection, unknown host.
  case unreachable
  /// App Transport Security refused a plain-HTTP connection.
  case insecureConnectionBlocked
  /// TLS handshake or certificate failure.
  case secureConnectionFailed
  case transport(String)
  /// Non-2xx status. Route errors carry their `text/plain` message
  /// (e.g. `404 Era does not exist`); unknown routes carry `{"error":…}`.
  case http(status: Int, message: String)
  case decoding(String)
  case cancelled

  /// Classifies a `URLSession`/task error.
  public init(transportError error: Error) {
    if let apiError = error as? APIError {
      self = apiError
      return
    }
    if error is CancellationError {
      self = .cancelled
      return
    }
    guard let urlError = error as? URLError else {
      self = .transport(error.localizedDescription)
      return
    }
    // Raw NSURLError codes, identical on every platform.
    switch urlError.code.rawValue {
    case -999:  // cancelled
      self = .cancelled
    case -1001:  // timedOut
      self = .timedOut
    case -1009, -1005, -1020, -1018:  // notConnectedToInternet, networkConnectionLost, dataNotAllowed, roaming off
      self = .offline
    case -1003, -1004, -1006:  // cannotFindHost, cannotConnectToHost, dnsLookupFailed
      self = .unreachable
    case -1022:  // appTransportSecurityRequiresSecureConnection
      self = .insecureConnectionBlocked
    case -1206 ... -1200:  // secureConnectionFailed … clientCertificateRequired
      self = .secureConnectionFailed
    default:
      self = .transport(urlError.localizedDescription)
    }
  }

  public var status: Int? {
    if case .http(let status, _) = self { return status }
    return nil
  }

  public var isNotFound: Bool { status == 404 }
  public var isCancellation: Bool { self == .cancelled }

  /// One-line explanation for error states and alerts.
  public var userMessage: String {
    switch self {
    case .invalidBaseURL:
      "The server URL in Settings is not a valid http(s) address."
    case .offline:
      "You appear to be offline."
    case .timedOut:
      "The server did not answer in time."
    case .unreachable:
      "The server could not be reached. Check the server URL in Settings."
    case .insecureConnectionBlocked:
      "iOS blocks plain HTTP to this server. Use an https:// URL (see apps/ios/README.md for local servers)."
    case .secureConnectionFailed:
      "A secure connection to the server could not be established."
    case .transport(let message):
      message.isEmpty ? "The request failed." : message
    case .http(let status, let message):
      message.isEmpty ? "The server answered with status \(status)." : message
    case .decoding:
      "The server sent something this app could not read."
    case .cancelled:
      "The request was cancelled."
    }
  }
}

extension APIError: LocalizedError {
  public var errorDescription: String? { userMessage }
}
