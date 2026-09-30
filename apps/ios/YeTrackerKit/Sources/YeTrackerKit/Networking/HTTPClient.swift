import Foundation

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

/// A received HTTP response. Header names are lowercased.
public struct HTTPResponse: Sendable {
  public let status: Int
  public let headers: [String: String]
  public let body: Data

  public init(status: Int, headers: [String: String] = [:], body: Data = Data()) {
    self.status = status
    self.headers = Dictionary(headers.map { ($0.key.lowercased(), $0.value) }, uniquingKeysWith: { _, last in last })
    self.body = body
  }

  public func header(_ name: String) -> String? { headers[name.lowercased()] }
}

/// The transport under `APIClient`, so tests can substitute canned responses.
public protocol HTTPClient: Sendable {
  /// Performs the request. Throws `APIError` for transport failures; any HTTP
  /// status (including 4xx/5xx) is returned, not thrown.
  func send(_ request: URLRequest) async throws -> HTTPResponse
}

/// `URLSession`-backed transport.
public struct URLSessionHTTPClient: HTTPClient {
  private let session: URLSession

  public init(session: URLSession = .shared) {
    self.session = session
  }

  public func send(_ request: URLRequest) async throws -> HTTPResponse {
    let data: Data
    let response: URLResponse
    do {
      (data, response) = try await perform(request)
    } catch {
      throw APIError(transportError: error)
    }
    guard let http = response as? HTTPURLResponse else {
      throw APIError.transport("The server sent a response that is not HTTP.")
    }
    var headers: [String: String] = [:]
    for (key, value) in http.allHeaderFields {
      headers[String(describing: key).lowercased()] = String(describing: value)
    }
    return HTTPResponse(status: http.statusCode, headers: headers, body: data)
  }

  private func perform(_ request: URLRequest) async throws -> (Data, URLResponse) {
    #if canImport(FoundationNetworking)
      // corelibs Foundation: bridge the completion-handler API, keeping cancellation.
      let box = TaskBox()
      return try await withTaskCancellationHandler {
        try await withCheckedThrowingContinuation { continuation in
          let task = session.dataTask(with: request) { data, response, error in
            if let error {
              continuation.resume(throwing: error)
            } else if let data, let response {
              continuation.resume(returning: (data, response))
            } else {
              continuation.resume(throwing: URLError(.badServerResponse))
            }
          }
          box.task = task
          task.resume()
        }
      } onCancel: {
        box.task?.cancel()
      }
    #else
      return try await session.data(for: request)
    #endif
  }
}

#if canImport(FoundationNetworking)
  private final class TaskBox: @unchecked Sendable {
    private let lock = NSLock()
    private var _task: URLSessionDataTask?
    var task: URLSessionDataTask? {
      get { lock.withLock { _task } }
      set { lock.withLock { _task = newValue } }
    }
  }
#endif
