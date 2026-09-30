import SwiftUI
import YeTrackerKit

/// Cover, name, description and notes in the era's colours, with links to the
/// neighbouring eras.
struct EraHeaderView: View {
  let model: EraDetailModel
  let palette: EraPalette
  let onSelectEra: @MainActor (Era) -> Void

  @Environment(\.horizontalSizeClass) private var sizeClass

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      if sizeClass == .regular {
        HStack(alignment: .top, spacing: 18) {
          cover.frame(width: 220)
          titleBlock
        }
      } else {
        VStack(alignment: .leading, spacing: 14) {
          cover
            .frame(maxWidth: 320)
            .frame(maxWidth: .infinity)
          titleBlock
        }
      }

      if let notes = model.era?.trimmedNotes {
        Divider()
          .overlay(palette.accentColor.opacity(0.3))
        ExpandableText(text: notes, lineLimit: 4, tint: palette.headerText)
          .font(.subheadline)
          .foregroundStyle(palette.headerText)
      }

      if model.previousEra != nil || model.nextEra != nil {
        HStack(spacing: 10) {
          if let previous = model.previousEra {
            neighbourButton(previous, systemImage: "chevron.left", label: "Previous era")
          }
          Spacer(minLength: 0)
          if let next = model.nextEra {
            neighbourButton(next, systemImage: "chevron.right", label: "Next era", trailing: true)
          }
        }
      }
    }
    .padding(14)
    .background(palette.cardFill, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
    .overlay {
      RoundedRectangle(cornerRadius: 16, style: .continuous)
        .strokeBorder(palette.border, lineWidth: 3)
    }
  }

  private var cover: some View {
    CoverImage(url: model.coverURL, accent: model.color, cornerRadius: 14)
      .aspectRatio(1, contentMode: .fit)
  }

  private var titleBlock: some View {
    VStack(alignment: .leading, spacing: 8) {
      Text(model.title)
        .font(.largeTitle.weight(.bold))
        .foregroundStyle(palette.headerText)
        .fixedSize(horizontal: false, vertical: true)
        .accessibilityAddTraits(.isHeader)
      if let description = model.era?.trimmedDescription {
        Text(description)
          .font(.body)
          .foregroundStyle(palette.headerText)
          .fixedSize(horizontal: false, vertical: true)
      }
    }
  }

  private func neighbourButton(_ era: Era, systemImage: String, label: String, trailing: Bool = false) -> some View {
    Button {
      onSelectEra(era)
    } label: {
      HStack(spacing: 6) {
        if !trailing { Image(systemName: systemImage) }
        Text(era.displayName)
          .lineLimit(1)
        if trailing { Image(systemName: systemImage) }
      }
      .font(.footnote.weight(.semibold))
      .foregroundStyle(palette.bodyText)
      .padding(.horizontal, 12)
      .padding(.vertical, 8)
      .background(palette.cardFill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
      .overlay {
        RoundedRectangle(cornerRadius: 10, style: .continuous)
          .strokeBorder(palette.accentColor.opacity(0.45), lineWidth: 2)
      }
    }
    .buttonStyle(.borderless)
    .accessibilityLabel("\(label): \(era.displayName)")
  }
}
