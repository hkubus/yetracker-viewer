import SwiftUI
import YeTrackerKit

/// Full-screen loading indicator.
struct LoadingStateView: View {
  var label = "Loading…"

  var body: some View {
    ProgressView(label)
      .foregroundStyle(.secondary)
      .frame(maxWidth: .infinity, minHeight: 240)
  }
}

/// The web's 500 page: something failed, with a retry.
struct ErrorStateView: View {
  var title = "Something went wrong"
  let message: String
  var retryTitle = "Try again"
  let retry: @MainActor () -> Void

  var body: some View {
    ContentUnavailableView {
      Label(title, systemImage: "exclamationmark.triangle")
    } description: {
      Text(message)
    } actions: {
      Button(retryTitle) { retry() }
        .buttonStyle(.glass)
    }
  }
}

/// The web's 404 page for an era that does not exist.
struct NotFoundStateView: View {
  let back: @MainActor () -> Void

  var body: some View {
    ContentUnavailableView {
      Label("That era does not exist", systemImage: "questionmark.folder")
    } description: {
      Text("The era may have been renamed, or the link is out of date.")
    } actions: {
      Button("Back to all eras") { back() }
        .buttonStyle(.glass)
    }
  }
}

/// The home page's "catalog could not be loaded" notice.
struct CatalogNotice: View {
  let message: String
  let retry: @MainActor () -> Void

  var body: some View {
    HStack(alignment: .center, spacing: 12) {
      Image(systemName: "exclamationmark.triangle.fill")
        .font(.title3)
        .foregroundStyle(.orange)
      VStack(alignment: .leading, spacing: 2) {
        Text("The catalog could not be loaded.")
          .font(.subheadline.weight(.semibold))
        Text(message)
          .font(.footnote)
          .foregroundStyle(.secondary)
      }
      Spacer(minLength: 8)
      Button("Try Again") { retry() }
        .font(.footnote.weight(.semibold))
        .buttonStyle(.glass)
    }
    .padding(14)
    .glassEffect(.regular.tint(Color.orange.opacity(0.2)), in: .rect(cornerRadius: 22))
    .accessibilityElement(children: .combine)
  }
}

/// Text clamped to a few lines, with More/Less only when it actually overflows
/// (the web measures the same way before offering the toggle).
struct ExpandableText: View {
  let text: String
  var lineLimit = 3
  var tint: Color = .accentColor

  @State private var isExpanded = false
  @State private var isTruncated = false

  var body: some View {
    VStack(alignment: .leading, spacing: 2) {
      Text(text)
        .lineLimit(isExpanded ? nil : lineLimit)
        .fixedSize(horizontal: false, vertical: true)
        .background { truncationProbe }
      if isTruncated || isExpanded {
        Button(isExpanded ? "Less" : "More") {
          withAnimation(.snappy) { isExpanded.toggle() }
        }
        .font(.caption.weight(.semibold))
        .foregroundStyle(tint)
        .buttonStyle(.borderless)
        .accessibilityLabel(isExpanded ? "Show less" : "Show more")
      }
    }
  }

  /// Lays the full text out at the same width, invisibly, and compares heights.
  private var truncationProbe: some View {
    GeometryReader { limited in
      Text(text)
        .fixedSize(horizontal: false, vertical: true)
        .frame(width: limited.size.width, alignment: .leading)
        .hidden()
        .background {
          GeometryReader { full in
            Color.clear
              .onAppear { measure(full: full.size.height, limited: limited.size.height) }
              .onChange(of: full.size.height) { _, height in measure(full: height, limited: limited.size.height) }
              .onChange(of: limited.size.height) { _, height in measure(full: full.size.height, limited: height) }
          }
        }
    }
  }

  private func measure(full: CGFloat, limited: CGFloat) {
    guard !isExpanded else { return }
    let truncated = full > limited + 1
    if truncated != isTruncated { isTruncated = truncated }
  }
}

/// Animated bars next to the playing song (the web's "♪").
struct NowPlayingGlyph: View {
  var isPlaying: Bool
  var color: Color

  var body: some View {
    Image(systemName: "waveform")
      .font(.caption.weight(.bold))
      .foregroundStyle(color)
      .symbolEffect(.variableColor.iterative, isActive: isPlaying)
      .accessibilityLabel(isPlaying ? "Now playing" : "Paused")
  }
}
