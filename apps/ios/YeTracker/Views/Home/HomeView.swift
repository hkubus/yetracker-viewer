import SwiftUI
import YeTrackerKit

/// The home page: catalog stats, "Recently leaked" and the era grid
/// (`apps/web/src/pages/index.astro`).
struct HomeView: View {
  @Environment(AppModel.self) private var app

  private let columns = [GridItem(.adaptive(minimum: 150, maximum: 240), spacing: 16, alignment: .top)]

  var body: some View {
    @Bindable var home = app.home
    let directory = home.directory

    ScrollView {
      VStack(alignment: .leading, spacing: 28) {
        if case .failed(let error) = directory.state {
          CatalogNotice(message: error.userMessage) {
            Task { await home.load(force: true) }
          }
        }

        if !home.recentLeaks.isEmpty, home.eraFilter.isEmpty {
          RecentLeaksSection()
        }

        erasSection(home: home)
      }
      .padding(.horizontal, 16)
      .padding(.top, 8)
      .padding(.bottom, 24)
      .frame(maxWidth: 1200)
      .frame(maxWidth: .infinity)
    }
    .navigationTitle("Ye Tracker")
    .navigationSubtitle(directory.eras.isEmpty ? "" : home.headerSummary)
    .searchable(text: $home.eraFilter, prompt: "Filter eras")
    .refreshable { [home] in await home.load(force: true) }
    .task { await home.load() }
  }

  @ViewBuilder
  private func erasSection(home: HomeModel) -> some View {
    let directory = home.directory
    VStack(alignment: .leading, spacing: 12) {
      HStack(alignment: .firstTextBaseline) {
        Text("Eras")
          .font(.title2.weight(.bold))
          .accessibilityAddTraits(.isHeader)
        Spacer()
        if home.showsEraFilter {
          Text(home.eraCountLabel)
            .font(.footnote)
            .foregroundStyle(.secondary)
            .contentTransition(.numericText())
        }
      }

      if directory.eras.isEmpty, directory.state == .loading || directory.state == .idle {
        LoadingStateView(label: "Loading eras…")
      } else if let message = home.eraFilterEmptyMessage {
        Text(message)
          .foregroundStyle(.secondary)
          .frame(maxWidth: .infinity)
          .padding(.vertical, 24)
      } else {
        LazyVGrid(columns: columns, spacing: 20) {
          ForEach(home.filteredEras) { era in
            NavigationLink(value: AppRoute.era(EraRoute(eraID: era.id))) {
              EraCard(era: era, coverURL: app.api.coverURL(eraID: era.id, version: era.coverKey))
            }
            .buttonStyle(.plain)
          }
        }
      }
    }
  }
}

/// An era tile: cover, name and song count, like an album in Music (`Era.astro`).
struct EraCard: View {
  let era: Era
  let coverURL: URL

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      CoverImage(url: coverURL, accent: era.color, cornerRadius: 12)
        .aspectRatio(1, contentMode: .fit)
        .overlay {
          RoundedRectangle(cornerRadius: 12, style: .continuous)
            .strokeBorder(Color.primary.opacity(0.08), lineWidth: 0.5)
        }
      VStack(alignment: .leading, spacing: 2) {
        Text(era.displayName)
          .font(.subheadline.weight(.semibold))
          .foregroundStyle(.primary)
          .lineLimit(2)
          .multilineTextAlignment(.leading)
        Text(era.songCountLabel)
          .font(.footnote)
          .foregroundStyle(.secondary)
      }
    }
    .contentShape(.rect)
    .hoverEffect(.lift)
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("\(era.displayName), \(era.songCountLabel)")
    .accessibilityAddTraits(.isButton)
  }
}
