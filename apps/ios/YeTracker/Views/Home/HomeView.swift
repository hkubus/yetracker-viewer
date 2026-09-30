import SwiftUI
import YeTrackerKit

/// The home page: catalog stats, "Recently leaked" and the era grid
/// (`apps/web/src/pages/index.astro`).
struct HomeView: View {
  @Environment(AppModel.self) private var app

  private let columns = [GridItem(.adaptive(minimum: 150, maximum: 260), spacing: 12, alignment: .top)]

  var body: some View {
    @Bindable var home = app.home
    let directory = home.directory

    ScrollView {
      VStack(alignment: .leading, spacing: 24) {
        Text(home.headerSummary)
          .font(.subheadline)
          .foregroundStyle(Theme.secondaryText)
          .opacity(directory.eras.isEmpty ? 0 : 1)

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
      .padding(.bottom, 24)
      .frame(maxWidth: 1200)
      .frame(maxWidth: .infinity)
    }
    .background(Theme.background)
    .navigationTitle("Ye Tracker")
    .searchable(text: $home.eraFilter, prompt: "Filter eras…")
    .refreshable { [home] in await home.load(force: true) }
    .task { await home.load() }
  }

  @ViewBuilder
  private func erasSection(home: HomeModel) -> some View {
    let directory = home.directory
    VStack(alignment: .leading, spacing: 12) {
      HStack(alignment: .firstTextBaseline) {
        Text("Eras")
          .font(.title3.weight(.bold))
        Spacer()
        if home.showsEraFilter {
          Text(home.eraCountLabel)
            .font(.footnote)
            .foregroundStyle(Theme.secondaryText)
            .contentTransition(.numericText())
        }
      }

      if directory.eras.isEmpty, directory.state == .loading || directory.state == .idle {
        LoadingStateView(label: "Loading eras…")
      } else if let message = home.eraFilterEmptyMessage {
        Text(message)
          .foregroundStyle(Theme.secondaryText)
          .frame(maxWidth: .infinity)
          .padding(.vertical, 24)
      } else {
        LazyVGrid(columns: columns, spacing: 12) {
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

/// An era tile: cover, name and song count in the era's colours (`Era.astro`).
struct EraCard: View {
  let era: Era
  let coverURL: URL

  var body: some View {
    let palette = EraPalette(era.color)
    VStack(alignment: .leading, spacing: 8) {
      CoverImage(url: coverURL, accent: era.color, cornerRadius: 10)
        .aspectRatio(1, contentMode: .fit)
      Text(era.displayName)
        .font(.headline.weight(.bold))
        .foregroundStyle(palette.cardText)
        .lineLimit(2, reservesSpace: true)
        .multilineTextAlignment(.leading)
      Text(era.songCountLabel)
        .font(.footnote.weight(.light))
        .foregroundStyle(palette.cardText)
        .frame(maxWidth: .infinity, alignment: .trailing)
    }
    .padding(10)
    .background(palette.cardFill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    .overlay {
      RoundedRectangle(cornerRadius: 14, style: .continuous)
        .strokeBorder(palette.border, lineWidth: 3)
    }
    .contentShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("\(era.displayName), \(era.songCountLabel)")
    .accessibilityAddTraits(.isButton)
  }
}
