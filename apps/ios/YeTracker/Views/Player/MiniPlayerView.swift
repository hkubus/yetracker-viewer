import SwiftUI
import YeTrackerKit

/// The persistent player bar above the tab bar (the web's fixed player,
/// condensed). Tap it for the full Now Playing sheet.
struct MiniPlayerView: View {
  @Environment(AppModel.self) private var app

  var body: some View {
    let player = app.player
    if let track = player.current {
      let palette = EraPalette(track.color)
      HStack(spacing: 12) {
        CoverImage(url: player.coverURL(for: track), accent: track.color, cornerRadius: 8)
          .frame(width: 44, height: 44)

        VStack(alignment: .leading, spacing: 2) {
          Text(player.errorMessage == nil ? player.stateLine : "Playback error")
            .font(.caption2.weight(.bold))
            .textCase(.uppercase)
            .foregroundStyle(player.errorMessage == nil ? palette.playerText.opacity(0.65) : Theme.errorText)
            .lineLimit(1)
          Text(track.title)
            .font(.subheadline.weight(.semibold))
            .foregroundStyle(palette.playerText)
            .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)

        Button {
          player.togglePlayPause()
        } label: {
          Image(systemName: player.isPlaying ? "pause.fill" : "play.fill")
            .font(.title3)
            .frame(width: 40, height: 40)
            .contentTransition(.symbolEffect(.replace))
        }
        .accessibilityLabel(player.isPlaying ? "Pause" : "Play")

        Button {
          player.next()
        } label: {
          Image(systemName: "forward.end.fill")
            .font(.body)
            .frame(width: 34, height: 40)
        }
        .disabled(!player.canGoNext)
        .accessibilityLabel("Next track")
      }
      .buttonStyle(.borderless)
      .foregroundStyle(palette.playerText)
      .padding(.leading, 8)
      .padding(.trailing, 10)
      .padding(.vertical, 8)
      .background {
        RoundedRectangle(cornerRadius: 16, style: .continuous)
          .fill(palette.playerBackground)
          .shadow(color: .black.opacity(0.35), radius: 16, y: 8)
      }
      .overlay {
        RoundedRectangle(cornerRadius: 16, style: .continuous)
          .strokeBorder(palette.accentColor.opacity(0.6), lineWidth: 1)
      }
      .overlay(alignment: .bottom) {
        ProgressBar(progress: player.progress, color: palette.playerText)
          .padding(.horizontal, 14)
          .padding(.bottom, 3)
      }
      .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
      .onTapGesture { app.router.isNowPlayingPresented = true }
      .accessibilityElement(children: .contain)
      .accessibilityAction(named: "Open player") { app.router.isNowPlayingPresented = true }
    }
  }
}

/// A thin capsule progress line.
private struct ProgressBar: View {
  let progress: Double
  let color: Color

  var body: some View {
    GeometryReader { geometry in
      Capsule()
        .fill(color.opacity(0.18))
        .overlay(alignment: .leading) {
          Capsule()
            .fill(color.opacity(0.85))
            .frame(width: geometry.size.width * min(1, max(0, progress)))
        }
    }
    .frame(height: 2)
    .accessibilityHidden(true)
  }
}

/// Download progress and failures, above the mini player.
struct DownloadBanner: View {
  @Environment(AppModel.self) private var app

  var body: some View {
    if let job = app.downloads.job {
      HStack(spacing: 12) {
        Image(systemName: job.errorMessage == nil ? "arrow.down.circle" : "exclamationmark.triangle")
          .font(.title3)
          .foregroundStyle(job.errorMessage == nil ? Theme.accent : Theme.errorText)
        VStack(alignment: .leading, spacing: 4) {
          Text(job.errorMessage == nil ? "Downloading \(job.title)" : "Download failed")
            .font(.footnote.weight(.semibold))
            .lineLimit(1)
          if let message = job.errorMessage {
            Text(message)
              .font(.caption)
              .foregroundStyle(Theme.errorText)
              .lineLimit(2)
          } else if let progress = job.progress {
            ProgressView(value: progress)
              .tint(Theme.accent)
          } else {
            ProgressView()
              .progressViewStyle(.linear)
              .tint(Theme.accent)
          }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        Button(job.errorMessage == nil ? "Cancel" : "Dismiss") {
          if job.errorMessage == nil {
            app.downloads.cancel()
          } else {
            app.downloads.dismissError()
          }
        }
        .font(.footnote.weight(.bold))
        .buttonStyle(.borderless)
      }
      .padding(12)
      .background(Theme.surfaceRaised, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
      .overlay {
        RoundedRectangle(cornerRadius: 14, style: .continuous)
          .strokeBorder(Theme.border)
      }
      .accessibilityElement(children: .combine)
    }
  }
}
