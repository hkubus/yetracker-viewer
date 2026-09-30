import SwiftUI
import YeTrackerKit

/// "Recently leaked": the newest playable tracks, playable in place
/// (`RecentLeaks.astro`). The strip is its own play queue.
struct RecentLeaksSection: View {
  @Environment(AppModel.self) private var app

  var body: some View {
    let songs = app.home.recentLeaks
    VStack(alignment: .leading, spacing: 10) {
      VStack(alignment: .leading, spacing: 2) {
        Text("Recently Leaked")
          .font(.title2.weight(.bold))
          .accessibilityAddTraits(.isHeader)
        Text("Newest tracks that can be played right now.")
          .font(.footnote)
          .foregroundStyle(.secondary)
      }
      VStack(spacing: 0) {
        ForEach(songs) { song in
          RecentLeakRow(song: song)
          if song.id != songs.last?.id {
            Divider()
              .padding(.leading, 68)
          }
        }
      }
      .padding(.vertical, 4)
      .background(Color(.secondarySystemBackground), in: .rect(cornerRadius: 22))
    }
  }
}

private struct RecentLeakRow: View {
  @Environment(AppModel.self) private var app
  @Environment(\.colorScheme) private var colorScheme
  let song: SearchSong

  var body: some View {
    let palette = EraPalette(song.color, colorScheme: colorScheme)
    let player = app.player
    let isCurrent = player.current?.id == song.id
    let route = app.home.route(for: song)

    HStack(spacing: 12) {
      Button {
        if isCurrent {
          player.togglePlayPause()
        } else {
          app.home.play(song, with: player)
        }
      } label: {
        HStack(spacing: 12) {
          CoverImage(url: app.coverURL(for: song), accent: song.color, cornerRadius: 8)
            .frame(width: 44, height: 44)
            .overlay {
              if isCurrent {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                  .fill(.black.opacity(0.35))
                  .overlay { NowPlayingGlyph(isPlaying: player.isPlaying, color: .white) }
              }
            }
          VStack(alignment: .leading, spacing: 2) {
            Text(song.displayTitle)
              .font(.body)
              .foregroundStyle(isCurrent ? palette.tint : .primary)
              .lineLimit(1)
            HStack(spacing: 4) {
              Text(song.eraDisplayName)
                .lineLimit(1)
              Text("·")
                .accessibilityHidden(true)
              Text(Formatters.mediumDate(song.leakDate))
                .fixedSize()
            }
            .font(.subheadline)
            .foregroundStyle(.secondary)
          }
          Spacer(minLength: 0)
        }
        .contentShape(.rect)
      }
      .buttonStyle(.plain)
      .accessibilityLabel("Play \(song.displayTitle), \(song.eraDisplayName)")

      if let route {
        Button {
          app.router.show(route)
        } label: {
          Image(systemName: "chevron.forward")
            .font(.footnote.weight(.semibold))
            .foregroundStyle(.tertiary)
            .frame(width: 32, height: 44)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Show \(song.displayTitle) in \(song.eraDisplayName)")
      }
    }
    .padding(.vertical, 6)
    .padding(.leading, 12)
    .padding(.trailing, 6)
    .contextMenu {
      Button("Play", systemImage: "play.fill") { app.home.play(song, with: player) }
      if let route {
        Button("Show in Era", systemImage: "rectangle.stack") { app.router.show(route) }
      }
    }
  }
}
