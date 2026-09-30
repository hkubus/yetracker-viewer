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
    VStack(alignment: .leading, spacing: 16) {
      if sizeClass == .regular {
        HStack(alignment: .bottom, spacing: 24) {
          cover.frame(width: 240)
          titleBlock(alignment: .leading)
        }
      } else {
        VStack(spacing: 16) {
          cover
            .frame(maxWidth: 260)
          titleBlock(alignment: .center)
        }
        .frame(maxWidth: .infinity)
      }

      if let notes = model.era?.trimmedNotes {
        ExpandableText(text: notes, lineLimit: 4, tint: palette.tint)
          .font(.subheadline)
          .foregroundStyle(.secondary)
      }

      if model.previousEra != nil || model.nextEra != nil {
        GlassEffectContainer(spacing: 10) {
          HStack(spacing: 10) {
            if let previous = model.previousEra {
              neighbourButton(previous, systemImage: "chevron.backward", label: "Previous era")
            }
            Spacer(minLength: 0)
            if let next = model.nextEra {
              neighbourButton(next, systemImage: "chevron.forward", label: "Next era", trailing: true)
            }
          }
        }
      }
    }
  }

  private var cover: some View {
    CoverImage(url: model.coverURL, accent: model.color, cornerRadius: 16)
      .aspectRatio(1, contentMode: .fit)
      .shadow(color: .black.opacity(0.25), radius: 18, y: 10)
  }

  private func titleBlock(alignment: HorizontalAlignment) -> some View {
    let textAlignment: TextAlignment = alignment == .center ? .center : .leading
    return VStack(alignment: alignment, spacing: 6) {
      Text(model.title)
        .font(.title.weight(.bold))
        .foregroundStyle(.primary)
        .multilineTextAlignment(textAlignment)
        .fixedSize(horizontal: false, vertical: true)
        .accessibilityAddTraits(.isHeader)
      if let description = model.era?.trimmedDescription {
        Text(description)
          .font(.body)
          .foregroundStyle(palette.tint)
          .multilineTextAlignment(textAlignment)
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
      .foregroundStyle(.primary)
      .padding(.vertical, 2)
    }
    .buttonStyle(.glass)
    .accessibilityLabel("\(label): \(era.displayName)")
  }
}
