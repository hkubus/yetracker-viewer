import SwiftUI
import YeTrackerKit

/// "Find a track": catalog-wide search with an era range and a playable filter
/// (`GlobalSongSearch.astro`). Results open their era scrolled to the song.
/// The query field is the tab bar's search field (`RootView`).
struct SearchView: View {
  @Environment(AppModel.self) private var app

  var body: some View {
    @Bindable var search = app.search
    let eras = search.directory.eras

    List {
      Section {
        if eras.count > 1 {
          EraRangeSlider(
            eras: eras,
            lower: search.effectiveLowerIndex,
            upper: search.upperIndex,
            status: search.rangeStatus,
            onLowerChange: search.setLowerIndex,
            onUpperChange: search.setUpperIndex)
        }
        Toggle("Playable Only", isOn: $search.playableOnly)
        Button("Clear Filters", role: .destructive) { search.clear() }
          .disabled(!search.hasFilters)
      } header: {
        HStack {
          Text("Filters")
          Spacer()
          Text(search.countLabel)
            .textCase(nil)
            .contentTransition(.numericText())
        }
      }

      results(search: search)
    }
    .listStyle(.insetGrouped)
    .scrollDismissesKeyboard(.immediately)
    .navigationTitle("Search")
    .task { await search.directory.load() }
  }

  @ViewBuilder
  private func results(search: GlobalSearchModel) -> some View {
    switch search.phase {
    case .idle:
      Section {
        Text(
          "Type to search every song in the catalog. Narrow the era range or keep only playable tracks to refine it."
        )
        .font(.footnote)
        .foregroundStyle(.secondary)
      }
      .listRowBackground(Color.clear)
    case .searching, .loaded, .failed:
      Section {
        if search.phase == .searching, search.results.isEmpty {
          HStack {
            Spacer()
            ProgressView()
            Spacer()
          }
          .listRowBackground(Color.clear)
        }
        if let message = search.emptyMessage {
          Text(message)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity)
            .listRowBackground(Color.clear)
        }
        ForEach(search.results) { song in
          SearchResultRow(song: song)
            .opacity(search.phase == .searching ? 0.6 : 1)
        }
      }
    }
  }
}

/// A result: the era's cover, title, era and a notes preview.
private struct SearchResultRow: View {
  @Environment(AppModel.self) private var app
  @Environment(\.colorScheme) private var colorScheme
  let song: SearchSong

  var body: some View {
    let palette = EraPalette(song.color, colorScheme: colorScheme)
    let route = app.search.route(for: song)
    let isCurrent = app.player.current?.id == song.id

    Button {
      if let route { app.router.show(route, in: .search) }
    } label: {
      HStack(alignment: .top, spacing: 12) {
        CoverImage(url: app.coverURL(for: song), accent: song.color, cornerRadius: 8)
          .frame(width: 44, height: 44)
        VStack(alignment: .leading, spacing: 2) {
          HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(song.displayTitle)
              .font(.body)
              .foregroundStyle(isCurrent ? palette.tint : .primary)
              .multilineTextAlignment(.leading)
            if isCurrent {
              NowPlayingGlyph(isPlaying: app.player.isPlaying, color: palette.tint)
            }
          }
          Text(song.eraDisplayName)
            .font(.subheadline)
            .foregroundStyle(palette.tint)
          if let notes = song.notesPreview {
            Text(notes)
              .font(.footnote)
              .foregroundStyle(.secondary)
              .lineLimit(2)
              .multilineTextAlignment(.leading)
          }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
      }
      .contentShape(.rect)
    }
    .buttonStyle(.plain)
    .contextMenu {
      if app.search.track(for: song) != nil {
        Button("Play", systemImage: "play.fill") { app.search.play(song, with: app.player) }
      }
      if let route {
        Button("Show in Era", systemImage: "rectangle.stack") { app.router.show(route, in: .search) }
      }
    }
    .accessibilityHint(song.isPlayable ? "Opens the era. Long-press to play." : "Opens the era at this song.")
  }
}
