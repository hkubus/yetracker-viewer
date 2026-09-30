import SwiftUI
import YeTrackerKit

/// The persistent player in the tab bar's bottom accessory (the web's fixed
/// player, condensed). Only shown while a track is loaded. The system draws the glass around it; tap it for the
/// full Now Playing sheet.
struct MiniPlayerView: View {
  @Environment(AppModel.self) private var app
  @Environment(\.tabViewBottomAccessoryPlacement) private var placement

  var body: some View {
    let player = app.player
    let track = player.current
    // Collapsed into the minimized tab bar: title and play/pause only.
    let isInline = placement == .inline

    HStack(spacing: 10) {
      CoverImage(
        url: track.flatMap { player.coverURL(for: $0) }, accent: track?.color ?? .fallbackAccent, cornerRadius: 6)
        .frame(width: 30, height: 30)

      VStack(alignment: .leading, spacing: 0) {
        Text(track?.title ?? "Not Playing")
          .font(.subheadline.weight(.semibold))
          .lineLimit(1)
        if let track, !isInline {
          Text(subtitle(track: track))
            .font(.caption)
            .foregroundStyle(player.errorMessage == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(Theme.error))
            .lineLimit(1)
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)

      Button {
        player.togglePlayPause()
      } label: {
        Image(systemName: player.isPlaying ? "pause.fill" : "play.fill")
          .font(.title3)
          .frame(width: 36, height: 36)
          .contentShape(.rect)
          .contentTransition(.symbolEffect(.replace))
      }
      .disabled(track == nil)
      .accessibilityLabel(player.isPlaying ? "Pause" : "Play")

      if !isInline {
        Button {
          player.next()
        } label: {
          Image(systemName: "forward.fill")
            .font(.body)
            .frame(width: 32, height: 36)
          .contentShape(.rect)
        }
        .disabled(!player.canGoNext)
        .accessibilityLabel("Next track")
      }
    }
    .buttonStyle(.plain)
    .foregroundStyle(.primary)
    .padding(.horizontal, 12)
    .contentShape(Rectangle())
    .onTapGesture {
      if track != nil { app.router.isNowPlayingPresented = true }
    }
    .accessibilityElement(children: .contain)
    .accessibilityAction(named: "Open player") {
      if track != nil { app.router.isNowPlayingPresented = true }
    }
  }

  /// The state line while it says something ("Buffering…"), else the era.
  private func subtitle(track: Track) -> String {
    let player = app.player
    if player.errorMessage != nil { return "Playback error" }
    if player.stateLine != PlayerModel.idleStateLine || track.eraName == nil { return player.stateLine }
    return track.eraName ?? ""
  }
}

/// Download progress and failures, above the tab bar.
struct DownloadBanner: View {
  @Environment(AppModel.self) private var app

  var body: some View {
    if let job = app.downloads.job {
      HStack(spacing: 12) {
        Image(systemName: job.errorMessage == nil ? "arrow.down.circle" : "exclamationmark.triangle")
          .font(.title3)
          .foregroundStyle(job.errorMessage == nil ? AnyShapeStyle(.tint) : AnyShapeStyle(Theme.error))
        VStack(alignment: .leading, spacing: 4) {
          Text(job.errorMessage == nil ? "Downloading \(job.title)" : "Download failed")
            .font(.footnote.weight(.semibold))
            .lineLimit(1)
          if let message = job.errorMessage {
            Text(message)
              .font(.caption)
              .foregroundStyle(.secondary)
              .lineLimit(2)
          } else if let progress = job.progress {
            ProgressView(value: progress)
          } else {
            ProgressView()
              .progressViewStyle(.linear)
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
        .font(.footnote.weight(.semibold))
        .buttonStyle(.glass)
      }
      .padding(.vertical, 10)
      .padding(.horizontal, 14)
      .glassEffect(.regular, in: .rect(cornerRadius: 22))
      .accessibilityElement(children: .combine)
    }
  }
}
