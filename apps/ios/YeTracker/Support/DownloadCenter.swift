import Foundation
import Observation

/// "Download original file" (`GET /songs/:id/download`): one download at a time,
/// with progress, then the share sheet so the file can go to Files, AirDrop, ….
@MainActor
@Observable
final class DownloadCenter {
  struct Job: Equatable {
    /// Distinguishes restarts of the same song from the job they replaced.
    let token = UUID()
    let songID: Int
    let title: String
    /// 0…1, or `nil` while the size is unknown.
    var progress: Double?
    var errorMessage: String?
  }

  private(set) var job: Job?

  @ObservationIgnored private var task: URLSessionDownloadTask?
  @ObservationIgnored private var session: URLSession?

  func start(songID: Int, title: String, from url: URL) {
    cancel()
    let job = Job(songID: songID, title: title)
    let token = job.token
    self.job = job
    let delegate = DownloadDelegate(
      onProgress: { [weak self] fraction in
        Task { @MainActor in self?.updateProgress(fraction, token: token) }
      },
      onFinish: { [weak self] result in
        Task { @MainActor in self?.finish(result, token: token) }
      })
    let session = URLSession(configuration: .default, delegate: delegate, delegateQueue: nil)
    var request = URLRequest(url: url)
    request.setValue("YeTracker-iOS/1.0", forHTTPHeaderField: "User-Agent")
    let task = session.downloadTask(with: request)
    self.session = session
    self.task = task
    task.resume()
  }

  func cancel() {
    task?.cancel()
    session?.invalidateAndCancel()
    task = nil
    session = nil
    job = nil
  }

  func dismissError() {
    if job?.errorMessage != nil { job = nil }
  }

  private func updateProgress(_ fraction: Double, token: UUID) {
    guard job?.token == token, job?.errorMessage == nil else { return }
    job?.progress = fraction
  }

  private func finish(_ result: Result<URL, DownloadError>, token: UUID) {
    guard job?.token == token else { return }
    session?.finishTasksAndInvalidate()
    session = nil
    task = nil
    switch result {
    case .success(let fileURL):
      job = nil
      Presenter.presentShareSheet(for: fileURL) {
        // The share sheet copies what it needs; the temporary file can go.
        try? FileManager.default.removeItem(at: fileURL)
      }
    case .failure(.cancelled):
      job = nil
    case .failure(let error):
      job?.errorMessage = error.message
    }
  }
}

enum DownloadError: Error {
  case cancelled
  case http(Int)
  case transport(String)
  case file(String)

  var message: String {
    switch self {
    case .cancelled: "The download was cancelled."
    case .http(404): "The file is not on the server (yet)."
    case .http(let status): "The server answered with status \(status)."
    case .transport(let message): message
    case .file(let message): "The file could not be saved: \(message)"
    }
  }
}

/// URLSession callbacks arrive on a background queue; results are handed back
/// through the closures.
private final class DownloadDelegate: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
  private let onProgress: @Sendable (Double) -> Void
  private let onFinish: @Sendable (Result<URL, DownloadError>) -> Void
  private var delivered = false
  /// Progress is forwarded in 1 % steps: every update re-renders the player inset.
  private var lastReported: Double = -1

  init(
    onProgress: @escaping @Sendable (Double) -> Void,
    onFinish: @escaping @Sendable (Result<URL, DownloadError>) -> Void
  ) {
    self.onProgress = onProgress
    self.onFinish = onFinish
  }

  func urlSession(
    _ session: URLSession,
    downloadTask: URLSessionDownloadTask,
    didWriteData bytesWritten: Int64,
    totalBytesWritten: Int64,
    totalBytesExpectedToWrite: Int64
  ) {
    guard totalBytesExpectedToWrite > 0 else { return }
    let fraction = min(1, Double(totalBytesWritten) / Double(totalBytesExpectedToWrite))
    guard fraction - lastReported >= 0.01 || (fraction >= 1 && lastReported < 1) else { return }
    lastReported = fraction
    onProgress(fraction)
  }

  func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
    // The temporary file is deleted when this returns: move it now.
    let response = downloadTask.response as? HTTPURLResponse
    guard let response, (200..<300).contains(response.statusCode) else {
      deliver(.failure(.http(response?.statusCode ?? 0)))
      return
    }
    do {
      let folder = FileManager.default.temporaryDirectory.appendingPathComponent("Downloads", isDirectory: true)
      try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
      let name = Self.safeFilename(response.suggestedFilename ?? "song")
      let destination = folder.appendingPathComponent(name)
      if FileManager.default.fileExists(atPath: destination.path) {
        try FileManager.default.removeItem(at: destination)
      }
      try FileManager.default.moveItem(at: location, to: destination)
      deliver(.success(destination))
    } catch {
      deliver(.failure(.file(error.localizedDescription)))
    }
  }

  func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
    guard let error else { return }
    if (error as? URLError)?.code == .cancelled {
      deliver(.failure(.cancelled))
    } else {
      deliver(.failure(.transport(error.localizedDescription)))
    }
  }

  private func deliver(_ result: Result<URL, DownloadError>) {
    guard !delivered else { return }
    delivered = true
    onFinish(result)
  }

  /// The API already sanitises names; this only guards against path tricks.
  private static func safeFilename(_ name: String) -> String {
    let cleaned = name.replacingOccurrences(of: "/", with: " ").replacingOccurrences(of: ":", with: " ")
      .trimmingCharacters(in: .whitespacesAndNewlines)
    return cleaned.isEmpty || cleaned.hasPrefix(".") ? "song\(cleaned)" : cleaned
  }
}
