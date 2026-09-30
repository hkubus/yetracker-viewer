import SwiftUI
import UIKit
import YeTrackerKit

/// Loads and caches cover art. Covers are AVIF (decoded by ImageIO on iOS 16+).
/// Their URLs carry `?v=` (`Era.coverKey`), so a changed cover gets a new URL,
/// and decoded images are kept in memory on top of the HTTP cache.
final class ImagePipeline: @unchecked Sendable {
  static let shared = ImagePipeline()

  private let memory = NSCache<NSURL, UIImage>()
  private let session: URLSession
  private let urlCache: URLCache

  private init() {
    // Its own directory: API responses must survive "Clear Cover Cache", and
    // the two caches must not evict each other.
    let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first
    urlCache = URLCache(
      memoryCapacity: 16 << 20,
      diskCapacity: 150 << 20,
      directory: caches?.appendingPathComponent("Covers", isDirectory: true))
    let configuration = URLSessionConfiguration.default
    configuration.urlCache = urlCache
    // The API sends `max-age=86400, immutable` for covers. Following it (rather
    // than returning cached data unconditionally) never pins a 404 for a cover
    // the server has not downloaded yet.
    configuration.requestCachePolicy = .useProtocolCachePolicy
    configuration.timeoutIntervalForRequest = 20
    session = URLSession(configuration: configuration)
    memory.countLimit = 150
  }

  func cachedImage(for url: URL) -> UIImage? {
    memory.object(forKey: url as NSURL)
  }

  /// The decoded image, or `nil` when it is missing (404) or unreadable.
  func image(for url: URL) async -> UIImage? {
    if let cached = cachedImage(for: url) { return cached }
    do {
      let (data, response) = try await session.data(from: url)
      guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else { return nil }
      let decoded = await Task.detached(priority: .utility) { () -> UIImage? in
        guard let image = UIImage(data: data) else { return nil }
        return image.preparingForDisplay() ?? image
      }.value
      if let decoded { memory.setObject(decoded, forKey: url as NSURL) }
      return decoded
    } catch {
      return nil
    }
  }

  func removeAll() {
    memory.removeAllObjects()
    urlCache.removeAllCachedResponses()
  }

  var diskUsage: Int { urlCache.currentDiskUsage }
}

/// Square-friendly cover art with an era-tinted placeholder.
struct CoverImage: View {
  let url: URL?
  var accent: RGBColor = .fallbackAccent
  var cornerRadius: CGFloat = 12

  @State private var image: UIImage?
  @State private var failed = false

  init(url: URL?, accent: RGBColor = .fallbackAccent, cornerRadius: CGFloat = 12) {
    self.url = url
    self.accent = accent
    self.cornerRadius = cornerRadius
    _image = State(initialValue: url.flatMap { ImagePipeline.shared.cachedImage(for: $0) })
  }

  var body: some View {
    Color.clear
      .overlay {
        if let image {
          Image(uiImage: image)
            .resizable()
            .scaledToFill()
        } else {
          placeholder
        }
      }
      .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
      .accessibilityHidden(true)
      .task(id: url) { await load() }
  }

  private var placeholder: some View {
    LinearGradient(
      colors: [Color(accent, opacity: 0.6), Color(accent, opacity: 0.22)],
      startPoint: .topLeading,
      endPoint: .bottomTrailing
    )
    .overlay {
      Image(systemName: "music.note")
        .font(.system(size: 22, weight: .semibold))
        .foregroundStyle(.white.opacity(failed ? 0.4 : 0.25))
    }
  }

  private func load() async {
    guard let url else {
      image = nil
      return
    }
    if let cached = ImagePipeline.shared.cachedImage(for: url) {
      image = cached
      failed = false
      return
    }
    // A new URL (e.g. the next track): drop the previous cover meanwhile.
    image = nil
    failed = false
    let loaded = await ImagePipeline.shared.image(for: url)
    guard !Task.isCancelled else { return }
    image = loaded
    failed = loaded == nil
  }
}
