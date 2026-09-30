import SwiftUI
import YeTrackerKit

/// "Recently leaked": the newest playable tracks, playable in place
/// (`RecentLeaks.astro`). The strip is its own play queue.
struct RecentLeaksSection: View {
  @Environment(AppModel.self) private var app

  private let columns = [GridItem(.adaptive(minimum: 280), spacing: 8)]

  var body: some View {
    let home = app.home
    VStack(alignment: .leading, spacing: 10) {
      VStack(alignment: .leading, spacing: 2) {
        Text("Recently leaked")
          .font(.title3.weight(.bold))
        Text("Newest tracks that can be played right now.")
          .font(.footnote)
          .foregroundStyle(Theme.secondaryText)
      }
      LazyVGrid(columns: columns, spacing: 8) {
        ForEach(home.recentLeaks) { song in
          RecentLeakRow(song: song)
        }
      }
    }
  }
}

private struct RecentLeakRow: View {
  @Environment(AppModel.self) private var app
  let song: SearchSong

  var body: some View {
    let palette = EraPalette(song.color)
    let player = app.player
    let isCurrent = player.current?.id == song.id
    let route = app.home.route(for: song)

    HStack(spacing: 10) {
      Button {
        if isCurrent {
          player.togglePlayPause()
        } else {
          app.home.play(song, with: player)
        }
      } label: {
        HStack(spacing: 10) {
          Image(systemName: isCurrent && player.isPlaying ? "pause.fill" : "play.fill")
            .font(.subheadline)
            .frame(width: 34, height: 34)
            .background(palette.accentColor.opacity(0.3), in: Circle())
          VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
              Text(song.displayTitle)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.white)
                .lineLimit(1)
              if isCurrent {
                NowPlayingGlyph(isPlaying: player.isPlaying, color: palette.cardText)
              }
            }
            HStack(spacing: 5) {
              Text(song.eraDisplayName)
                .lineLimit(1)
              Text("•")
                .accessibilityHidden(true)
              Text(Formatters.mediumDate(song.leakDate))
                .fixedSize()
            }
            .font(.caption)
            .foregroundStyle(palette.mutedText)
          }
          Spacer(minLength: 0)
        }
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .accessibilityLabel("Play \(song.displayTitle), \(song.eraDisplayName)")

      if let route {
        Button {
          app.router.show(route)
        } label: {
          Image(systemName: "chevron.right")
            .font(.footnote.weight(.semibold))
            .foregroundStyle(palette.mutedText)
            .frame(width: 30, height: 34)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Show \(song.displayTitle) in \(song.eraDisplayName)")
      }
    }
    .padding(.vertical, 8)
    .padding(.horizontal, 10)
    .background(
      isCurrent ? palette.listCardPlayingFill : palette.listCardFill,
      in: RoundedRectangle(cornerRadius: 10, style: .continuous)
    )
    .overlay(alignment: .leading) {
      UnevenRoundedRectangle(topLeadingRadius: 10, bottomLeadingRadius: 10, style: .continuous)
        .fill(isCurrent ? Color.white : palette.listCardEdge)
        .frame(width: 4)
    }
    .overlay {
      RoundedRectangle(cornerRadius: 10, style: .continuous)
        .strokeBorder(isCurrent ? Color.white : palette.listCardBorder, lineWidth: 1)
    }
    .contextMenu {
      Button("Play", systemImage: "play.fill") { app.home.play(song, with: player) }
      if let route {
        Button("Show in Era", systemImage: "rectangle.stack") { app.router.show(route) }
      }
    }
  }
}
