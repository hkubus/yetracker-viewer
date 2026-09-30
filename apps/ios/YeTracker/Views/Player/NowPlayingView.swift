import AVKit
import SwiftUI
import YeTrackerKit

/// The full player: artwork, state line, era link, scrubber, transport,
/// volume, quality, AirPlay and errors (`Player.astro`).
struct NowPlayingView: View {
  @Environment(AppModel.self) private var app
  @Environment(\.dismiss) private var dismiss
  /// The scrubber's position while dragging; `nil` follows playback.
  @State private var scrubPosition: Double?
  /// A touch drag is in progress. VoiceOver and keyboard adjustments change the
  /// value without an editing phase, so those seek immediately.
  @State private var isScrubbing = false

  var body: some View {
    let player = app.player
    if let track = player.current {
      let palette = EraPalette(track.color)
      ScrollView {
        VStack(spacing: 22) {
          CoverImage(url: player.coverURL(for: track), accent: track.color, cornerRadius: 20)
            .aspectRatio(1, contentMode: .fit)
            .frame(maxWidth: 360)
            .shadow(color: .black.opacity(0.45), radius: 24, y: 12)
            .padding(.top, 8)

          titleBlock(track: track, palette: palette)
          scrubber(palette: palette)
          transport(palette: palette)
          volume(palette: palette)

          HStack {
            QualityMenu(palette: palette)
            Spacer()
            RoutePicker(tint: UIColor(palette.playerText))
              .frame(width: 44, height: 44)
              .accessibilityLabel("AirPlay and audio output")
          }

          if let message = player.errorMessage {
            ErrorBanner(message: message) { player.retry() }
          }
        }
        .padding(.horizontal, 24)
        .padding(.bottom, 24)
        .frame(maxWidth: 520)
        .frame(maxWidth: .infinity)
      }
      .foregroundStyle(palette.playerText)
      .background {
        LinearGradient(
          colors: [palette.playerBackground, Theme.background],
          startPoint: .top,
          endPoint: .bottom
        )
        .ignoresSafeArea()
      }
      .presentationDragIndicator(.visible)
    } else {
      ContentUnavailableView("Nothing playing", systemImage: "music.note")
        .presentationDragIndicator(.visible)
        .onAppear { dismiss() }
    }
  }

  private func titleBlock(track: Track, palette: EraPalette) -> some View {
    let player = app.player
    return VStack(spacing: 6) {
      Text(player.stateLine)
        .font(.caption.weight(.bold))
        .textCase(.uppercase)
        .tracking(1)
        .foregroundStyle(palette.playerText.opacity(0.65))
        .contentTransition(.opacity)
      Text(track.title)
        .font(.title3.weight(.bold))
        .multilineTextAlignment(.center)
        .fixedSize(horizontal: false, vertical: true)
      if let eraID = track.eraID, let eraName = track.eraName {
        Button {
          app.router.showFromPlayer(EraRoute(eraID: eraID, focus: SongFocus(songID: track.id, position: nil)))
        } label: {
          HStack(spacing: 4) {
            Text(eraName)
            Image(systemName: "chevron.right")
              .font(.caption2.weight(.bold))
          }
          .font(.subheadline)
          .foregroundStyle(palette.playerText.opacity(0.75))
        }
        .buttonStyle(.borderless)
        .accessibilityLabel("Show \(eraName)")
      }
    }
  }

  private func scrubber(palette: EraPalette) -> some View {
    let player = app.player
    let duration = player.duration
    let shown = scrubPosition ?? player.elapsed
    return VStack(spacing: 4) {
      Slider(
        value: Binding(
          get: { min(scrubPosition ?? player.elapsed, max(duration, 0)) },
          set: { value in
            if isScrubbing {
              scrubPosition = value
            } else {
              player.seek(to: value)
            }
          }),
        in: 0...max(duration, 1),
        onEditingChanged: { editing in
          if editing {
            isScrubbing = true
            scrubPosition = player.elapsed
          } else {
            isScrubbing = false
            if let target = scrubPosition { player.seek(to: target) }
            scrubPosition = nil
          }
        }
      )
      .tint(palette.playerText)
      .disabled(duration <= 0)
      .accessibilityLabel("Track progress")
      .accessibilityValue(Formatters.elapsedDescription(shown))

      HStack {
        Text(Formatters.duration(shown))
        Spacer()
        Text(duration > 0 ? Formatters.duration(duration) : "—")
      }
      .font(.caption.monospacedDigit())
      .foregroundStyle(palette.playerText.opacity(0.75))
    }
  }

  private func transport(palette: EraPalette) -> some View {
    let player = app.player
    return HStack(spacing: 44) {
      Button {
        player.previous()
      } label: {
        Image(systemName: "backward.end.fill")
          .font(.title)
      }
      .disabled(!player.canGoPrevious)
      .accessibilityLabel("Previous track")

      Button {
        player.togglePlayPause()
      } label: {
        Image(systemName: player.isPlaying ? "pause.circle.fill" : "play.circle.fill")
          .font(.system(size: 72))
          .contentTransition(.symbolEffect(.replace))
      }
      .accessibilityLabel(player.isPlaying ? "Pause" : "Play")

      Button {
        player.next()
      } label: {
        Image(systemName: "forward.end.fill")
          .font(.title)
      }
      .disabled(!player.canGoNext)
      .accessibilityLabel("Next track")
    }
    .buttonStyle(.borderless)
    .foregroundStyle(palette.playerText)
  }

  private func volume(palette: EraPalette) -> some View {
    @Bindable var player = app.player
    return HStack(spacing: 12) {
      Image(systemName: "speaker.fill")
        .font(.caption)
      Slider(value: $player.volume, in: 0...1)
        .tint(palette.playerText)
        .accessibilityLabel("Volume")
        .accessibilityValue("\(Int((player.volume * 100).rounded())) percent")
      Image(systemName: "speaker.wave.3.fill")
        .font(.caption)
    }
    .foregroundStyle(palette.playerText.opacity(0.75))
  }
}

/// Original / 64–320 kbps, remembered across launches.
private struct QualityMenu: View {
  @Environment(AppModel.self) private var app
  let palette: EraPalette

  var body: some View {
    let player = app.player
    Menu {
      Picker(
        "Quality",
        selection: Binding(get: { player.quality }, set: { player.setQuality($0) })
      ) {
        ForEach(PlaybackQuality.allCases) { quality in
          Text(quality.label).tag(quality)
        }
      }
    } label: {
      HStack(spacing: 6) {
        if player.isSwitchingQuality {
          ProgressView()
            .controlSize(.small)
        } else {
          Image(systemName: "waveform")
        }
        Text(player.quality.label)
      }
      .font(.footnote.weight(.bold))
      .padding(.horizontal, 12)
      .padding(.vertical, 8)
      .background(palette.playerText.opacity(0.08), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
      .overlay {
        RoundedRectangle(cornerRadius: 10, style: .continuous)
          .strokeBorder(palette.playerText.opacity(0.2))
      }
    }
    .disabled(player.isSwitchingQuality)
    .accessibilityLabel("Playback quality: \(player.quality.label)")
  }
}

/// The player's error line with Retry.
private struct ErrorBanner: View {
  let message: String
  let retry: @MainActor () -> Void

  var body: some View {
    HStack(spacing: 10) {
      Text(message)
        .font(.footnote)
        .frame(maxWidth: .infinity, alignment: .leading)
      Button("Retry") { retry() }
        .font(.footnote.weight(.bold))
        .buttonStyle(.bordered)
    }
    .foregroundStyle(Theme.errorText)
    .padding(12)
    .background(Theme.errorText.opacity(0.1), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    .accessibilityElement(children: .combine)
  }
}

/// The system AirPlay / output picker.
private struct RoutePicker: UIViewRepresentable {
  let tint: UIColor

  func makeUIView(context: Context) -> AVRoutePickerView {
    let view = AVRoutePickerView()
    view.prioritizesVideoDevices = false
    view.tintColor = tint
    view.activeTintColor = .white
    return view
  }

  func updateUIView(_ view: AVRoutePickerView, context: Context) {
    view.tintColor = tint
  }
}
