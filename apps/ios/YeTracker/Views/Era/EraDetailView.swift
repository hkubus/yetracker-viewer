import SwiftUI
import YeTrackerKit

/// One era: header, previous/next era, and its songs with search, category,
/// sort and infinite paging (`pages/eras/[id].astro`, `SongList.astro`).
struct EraDetailView: View {
  @Environment(AppModel.self) private var app
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.colorScheme) private var colorScheme
  @State private var model: EraDetailModel
  let tab: AppTab

  private static let headerID = "era-header"

  init(route: EraRoute, tab: AppTab, directory: EraDirectory, api: @escaping APIProvider) {
    _model = State(initialValue: EraDetailModel(route: route, directory: directory, api: api))
    self.tab = tab
  }

  var body: some View {
    @Bindable var model = model
    let palette = EraPalette(model.color, colorScheme: colorScheme)

    content(palette: palette)
      .background {
        // The era colour washes in from the top, under the glass bars.
        LinearGradient(
          stops: [.init(color: palette.pageWash, location: 0), .init(color: .clear, location: 0.55)],
          startPoint: .top,
          endPoint: .bottom
        )
        .background(Color(.systemBackground))
        .ignoresSafeArea()
      }
      .navigationTitle(model.title)
      .navigationBarTitleDisplayMode(.inline)
      .searchable(
        text: $model.searchText,
        placement: .navigationBarDrawer(displayMode: .always),
        prompt: "Search songs in this era"
      )
      .toolbar {
        ToolbarItem(placement: .topBarTrailing) {
          EraFilterMenu(model: model)
        }
      }
      .task {
        model.player = app.player
        await model.load()
      }
  }

  @ViewBuilder
  private func content(palette: EraPalette) -> some View {
    switch model.phase {
    case .notFound:
      NotFoundStateView { app.router.popToRoot(tab) }
    case .failed(let error):
      ErrorStateView(message: error.userMessage) {
        Task { await model.refresh() }
      }
    case .loading, .loaded:
      songList(palette: palette)
    }
  }

  private func songList(palette: EraPalette) -> some View {
    ScrollViewReader { proxy in
      List {
        EraHeaderView(model: model, palette: palette) { era in
          app.router.replaceTop(with: EraRoute(eraID: era.id), in: tab)
        }
        .id(Self.headerID)
        .listRowInsets(EdgeInsets(top: 8, leading: 16, bottom: 16, trailing: 16))
        .listRowBackground(Color.clear)
        .listRowSeparator(.hidden)

        Section {
          if model.phase == .loading, model.songs.isEmpty {
            ProgressView()
              .frame(maxWidth: .infinity)
              .padding(.vertical, 40)
              .listRowBackground(Color.clear)
              .listRowSeparator(.hidden)
          }
          if let message = model.emptyMessage {
            Text(message)
              .font(.subheadline)
              .foregroundStyle(.secondary)
              .frame(maxWidth: .infinity)
              .padding(.vertical, 24)
              .listRowBackground(Color.clear)
              .listRowSeparator(.hidden)
          }
          ForEach(model.visibleSongs) { song in
            row(for: song, palette: palette)
              .listRowInsets(EdgeInsets(top: 10, leading: 16, bottom: 10, trailing: 12))
              .listRowBackground(rowBackground(for: song, palette: palette))
              .onAppear { model.loadMoreIfNeeded(currentSongID: song.id) }
          }
          footer(palette: palette)
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
        } header: {
          summaryBar(palette: palette)
        }
      }
      .listStyle(.plain)
      .scrollContentBackground(.hidden)
      .scrollDismissesKeyboard(.immediately)
      .refreshable { [model] in await model.refresh() }
      .onChange(of: model.scrollRequest) { _, request in
        scroll(to: request, proxy: proxy)
      }
    }
  }

  private func row(for song: EraSong, palette: EraPalette) -> some View {
    let player = app.player
    let isCurrent = player.current?.id == song.id
    return SongRow(
      song: song,
      palette: palette,
      isCurrent: isCurrent,
      isPlaying: isCurrent && player.isPlaying,
      onPlay: {
        if isCurrent {
          player.togglePlayPause()
        } else {
          model.play(song)
        }
      },
      onDownload: { app.download(songID: song.id, title: song.displayTitle) },
      onOpenSource: { url in app.openWebLink(url) })
  }

  /// The playing row and a deep-link target get a rounded fill in the era colour.
  private func rowBackground(for song: EraSong, palette: EraPalette) -> some View {
    let fill: Color =
      if model.highlightedSongID == song.id {
        palette.highlightFill
      } else if app.player.current?.id == song.id {
        palette.playingFill
      } else {
        .clear
      }
    return RoundedRectangle(cornerRadius: 16, style: .continuous)
      .fill(fill)
      .padding(.horizontal, 6)
      .padding(.vertical, 1)
  }

  /// Pinned under the search field: result count and the active filters.
  private func summaryBar(palette: EraPalette) -> some View {
    HStack(spacing: 8) {
      Text(model.countLabel)
        .font(.footnote)
        .foregroundStyle(.secondary)
        .lineLimit(2)
      if model.isApplyingFilters {
        ProgressView()
          .controlSize(.mini)
      }
      Spacer(minLength: 4)
      GlassEffectContainer(spacing: 6) {
        HStack(spacing: 6) {
          if let category = model.category {
            FilterChip(title: category.title, tint: palette.tint) { model.category = nil }
          }
          if model.sort != .default {
            FilterChip(title: model.sort.label, tint: palette.tint) { model.sort = .default }
          }
        }
      }
    }
    .padding(.horizontal, 4)
    .padding(.vertical, 6)
    .textCase(nil)
  }

  @ViewBuilder
  private func footer(palette: EraPalette) -> some View {
    VStack(spacing: 8) {
      switch model.paging {
      case .loading:
        ProgressView()
      case .failed(let error):
        Text("More songs could not be loaded. \(error.userMessage)")
          .font(.footnote)
          .foregroundStyle(Theme.error)
          .multilineTextAlignment(.center)
        Button("Retry") { model.retryPaging() }
          .buttonStyle(.glass)
      case .idle, .complete:
        EmptyView()
      }
      if let label = model.footerLabel {
        Text(label)
          .font(.footnote)
          .foregroundStyle(.secondary)
      }
    }
    .frame(maxWidth: .infinity)
    .padding(.vertical, 12)
    .onAppear { model.loadNextPage() }
  }

  /// Deep links scroll the row into the middle of the screen, then re-centre
  /// once the rows have settled (the web does the same).
  private func scroll(to request: EraDetailModel.ScrollRequest?, proxy: ScrollViewProxy) {
    guard let request else { return }
    let perform = {
      switch request.target {
      case .song(let id): proxy.scrollTo(id, anchor: .center)
      case .top: proxy.scrollTo(Self.headerID, anchor: .top)
      }
    }
    Task { @MainActor in
      try? await Task.sleep(for: .milliseconds(80))
      if reduceMotion {
        perform()
      } else {
        withAnimation(.easeInOut(duration: 0.35)) { perform() }
      }
      try? await Task.sleep(for: .milliseconds(450))
      if model.scrollRequest == request { perform() }
    }
  }
}

/// Category and sort pickers plus "Clear all filters".
private struct EraFilterMenu: View {
  @Bindable var model: EraDetailModel

  var body: some View {
    let filtered = model.category != nil || model.sort != .default
    Menu {
      Section("Category") {
        Picker("Category", selection: $model.category) {
          Text("All categories").tag(SongCategory?.none)
          ForEach(SongCategory.allCases) { category in
            Text(category.title).tag(SongCategory?.some(category))
          }
        }
      }
      Section("Sort") {
        Picker("Sort", selection: $model.sort) {
          ForEach(SongSort.allCases) { sort in
            Text(sort.label).tag(sort)
          }
        }
      }
      if model.hasActiveFilters {
        Section {
          Button("Clear all filters", systemImage: "xmark.circle", role: .destructive) {
            model.clearFilters()
          }
        }
      }
    } label: {
      Label(
        "Filter and sort",
        systemImage: filtered ? "line.3.horizontal.decrease.circle.fill" : "line.3.horizontal.decrease.circle")
    }
    .disabled(model.phase == .notFound)
  }
}

/// An active filter with a remove button.
private struct FilterChip: View {
  let title: String
  let tint: Color
  let remove: @MainActor () -> Void

  var body: some View {
    Button {
      remove()
    } label: {
      HStack(spacing: 4) {
        Text(title)
          .lineLimit(1)
        Image(systemName: "xmark")
          .font(.caption2.weight(.bold))
      }
      .font(.caption.weight(.semibold))
      .foregroundStyle(tint)
      .padding(.horizontal, 10)
      .padding(.vertical, 5)
      .contentShape(.capsule)
    }
    .buttonStyle(.plain)
    .glassEffect(.regular.interactive(), in: .capsule)
    .accessibilityLabel("Remove filter \(title)")
  }
}
