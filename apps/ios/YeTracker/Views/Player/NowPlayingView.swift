import AVKit
import SwiftUI
import YeTrackerKit

/// The full player: artwork, state line, era link, scrubber, transport,
/// volume, quality, AirPlay and errors (`Player.astro`). Always dark, over a
/// gradient of the era colour, like Music.
struct NowPlayingView: View {
  @Environment(AppModel.self) private var app
  @Environment(\.dismiss) private var dismiss
  /// The scrubber's position while dragging; `nil` follows playback.
  @State private var scrubPosition: Double?
  /// A touch drag is in progress. VoiceOver and keyboard adjustments change the
  /// value without an editing phase, so those seek immediately.
  @State private var isScrubbing = false
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  var body: some View {
    let player = app.player
    if let track = player.current {
      let palette = EraPalette(track.color, colorScheme: .dark)
      ScrollView {
        VStack(spacing: 24) {
          CoverImage(url: player.coverURL(for: track), accent: track.color, cornerRadius: 16)
            .aspectRatio(1, contentMode: .fit)
            .frame(maxWidth: 360)
            .shadow(color: .black.opacity(0.4), radius: 24, y: 12)
            // Music's artwork recedes while paused.
            .scaleEffect(player.isPlaying || reduceMotion ? 1 : 0.86)
            .animation(reduceMotion ? nil : .spring(duration: 0.45, bounce: 0.25), value: player.isPlaying)
            .padding(.top, 28)

          titleBlock(track: track, palette: palette)
          scrubber(palette: palette)
          transport(palette: palette)
          volume(palette: palette)

          GlassEffectContainer(spacing: 12) {
            HStack {
              QualityMenu()
              Spacer()
              RoutePicker(tint: UIColor(palette.playerText))
                .frame(width: 44, height: 44)
                .glassEffect(.regular.interactive(), in: .circle)
                .accessibilityLabel("AirPlay and audio output")
            }
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
      .scrollBounceBehavior(.basedOnSize)
      .foregroundStyle(palette.playerText)
      .tint(palette.playerText)
      .background {
        LinearGradient(
          colors: [palette.playerBackground, Color(RGBColor.background)],
          startPoint: .top,
          endPoint: .bottom
        )
        .ignoresSafeArea()
      }
      .environment(\.colorScheme, .dark)
      .presentationDragIndicator(.visible)
    } else {
      ContentUnavailableView("Nothing playing", systemImage: "music.note")
        .presentationDragIndicator(.visible)
        .onAppear { dismiss() }
    }
  }

  private func titleBlock(track: Track, palette: EraPalette) -> some View {
    let player = app.player
    return VStack(spacing: 4) {
      Text(player.stateLine)
        .font(.caption.weight(.semibold))
        .textCase(.uppercase)
        .tracking(0.8)
        .foregroundStyle(palette.playerText.opacity(0.6))
        .contentTransition(.opacity)
      Text(track.title)
        .font(.title2.weight(.bold))
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
          .font(.body)
          .foregroundStyle(palette.playerText.opacity(0.7))
        }
        .buttonStyle(.plain)
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
      .font(.caption.monospacedDigit().weight(.medium))
      .foregroundStyle(palette.playerText.opacity(0.6))
    }
  }

  private func transport(palette: EraPalette) -> some View {
    let player = app.player
    return HStack(spacing: 48) {
      Button {
        player.previous()
      } label: {
        Image(systemName: "backward.fill")
          .font(.system(size: 34))
          .frame(width: 64, height: 64)
          .contentShape(.circle)
      }
      .disabled(!player.canGoPrevious)
      .opacity(player.canGoPrevious ? 1 : 0.35)
      .accessibilityLabel("Previous track")

      Button {
        player.togglePlayPause()
      } label: {
        Image(systemName: player.isPlaying ? "pause.fill" : "play.fill")
          .font(.system(size: 48))
          .frame(width: 80, height: 80)
          .contentShape(.circle)
          .contentTransition(.symbolEffect(.replace))
      }
      .accessibilityLabel(player.isPlaying ? "Pause" : "Play")

      Button {
        player.next()
      } label: {
        Image(systemName: "forward.fill")
          .font(.system(size: 34))
          .frame(width: 64, height: 64)
          .contentShape(.circle)
      }
      .disabled(!player.canGoNext)
      .opacity(player.canGoNext ? 1 : 0.35)
      .accessibilityLabel("Next track")
    }
    .buttonStyle(.plain)
    .foregroundStyle(palette.playerText)
  }

  private func volume(palette: EraPalette) -> some View {
    @Bindable var player = app.player
    return HStack(spacing: 12) {
      Image(systemName: "speaker.fill")
        .font(.footnote)
      Slider(value: $player.volume, in: 0...1)
        .tint(palette.playerText)
        .accessibilityLabel("Volume")
        .accessibilityValue("\(Int((player.volume * 100).rounded())) percent")
      Image(systemName: "speaker.wave.3.fill")
        .font(.footnote)
    }
    .foregroundStyle(palette.playerText.opacity(0.6))
  }
}

/// Original / 64–320 kbps, remembered across launches.
private struct QualityMenu: View {
  @Environment(AppModel.self) private var app

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
      .font(.footnote.weight(.semibold))
      .padding(.horizontal, 4)
      .frame(minHeight: 32)
    }
    .buttonStyle(.glass)
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
        .font(.footnote.weight(.semibold))
        .buttonStyle(.glass)
    }
    .padding(12)
    .glassEffect(.regular.tint(Theme.error.opacity(0.35)), in: .rect(cornerRadius: 20))
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
