import SwiftUI
import YeTrackerKit

/// "Find a track": catalog-wide search with an era range and a playable filter
/// (`GlobalSongSearch.astro`). Results open their era scrolled to the song.
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
        Toggle("Playable only", isOn: $search.playableOnly)
          .font(.subheadline.weight(.semibold))
        Button("Clear filters", systemImage: "xmark", role: .destructive) { search.clear() }
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
      .listRowBackground(Theme.surface)

      results(search: search)
    }
    .listStyle(.insetGrouped)
    .scrollContentBackground(.hidden)
    .background(Theme.background)
    .scrollDismissesKeyboard(.immediately)
    .navigationTitle("Find a track")
    .searchable(
      text: $search.query,
      placement: .navigationBarDrawer(displayMode: .always),
      prompt: "Search songs, artists, notes…"
    )
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
        .foregroundStyle(Theme.secondaryText)
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
            .foregroundStyle(Theme.secondaryText)
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

/// A result card: title, era and a notes preview in the era's colours.
private struct SearchResultRow: View {
  @Environment(AppModel.self) private var app
  let song: SearchSong

  var body: some View {
    let palette = EraPalette(song.color)
    let route = app.search.route(for: song)
    let isCurrent = app.player.current?.id == song.id

    Button {
      if let route { app.router.show(route, in: .search) }
    } label: {
      VStack(alignment: .leading, spacing: 4) {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
          Text(song.displayTitle)
            .font(.subheadline.weight(.bold))
            .foregroundStyle(.white)
            .multilineTextAlignment(.leading)
          if isCurrent {
            NowPlayingGlyph(isPlaying: app.player.isPlaying, color: palette.cardText)
          }
          Spacer(minLength: 8)
          Text(song.eraDisplayName)
            .font(.caption)
            .foregroundStyle(palette.cardText)
            .multilineTextAlignment(.trailing)
        }
        if let notes = song.notesPreview {
          Text(notes)
            .font(.footnote)
            .foregroundStyle(Color(hex: "aaaaaa"))
            .lineLimit(2)
            .multilineTextAlignment(.leading)
        }
      }
      .padding(.vertical, 4)
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .listRowBackground(
      palette.listCardFill
        .overlay(alignment: .leading) {
          Rectangle().fill(palette.listCardEdge).frame(width: 3)
        }
    )
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
