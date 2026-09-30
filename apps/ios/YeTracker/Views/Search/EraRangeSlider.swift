import SwiftUI
import YeTrackerKit

/// Two-thumb slider over the ordered era list. The track between the thumbs is
/// a gradient from the first era's colour to the last one's, with the era names
/// underneath (the web's "Era range" control).
struct EraRangeSlider: View {
  let eras: [Era]
  let lower: Int
  let upper: Int
  let status: String
  let onLowerChange: @MainActor (Int) -> Void
  let onUpperChange: @MainActor (Int) -> Void

  private let thumbSize: CGFloat = 24
  private let space = "eraRange"

  private var lastIndex: Int { max(eras.count - 1, 0) }
  private var lowerEra: Era? { eras.indices.contains(lower) ? eras[lower] : nil }
  private var upperEra: Era? { eras.indices.contains(upper) ? eras[upper] : nil }
  private var lowerColor: Color { Color(lowerEra?.color ?? RGBColor(hex: "cfcfcf")!) }
  private var upperColor: Color { Color(upperEra?.color ?? RGBColor(hex: "cfcfcf")!) }

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      HStack {
        Text("ERA RANGE")
          .font(.caption2.weight(.bold))
          .tracking(1)
          .foregroundStyle(Theme.secondaryText)
        Spacer()
        Text(status)
          .font(.caption.weight(.semibold))
          .foregroundStyle(Color(hex: "b8b8b8"))
      }

      GeometryReader { geometry in
        let usable = max(1, geometry.size.width - thumbSize)
        let lowerX = position(of: lower, usable: usable)
        let upperX = position(of: upper, usable: usable)
        let collapsed = lower == upper

        ZStack(alignment: .leading) {
          Capsule()
            .fill(Color(hex: "3f3f3f"))
            .frame(height: 4)
            .padding(.horizontal, thumbSize / 2)
          Capsule()
            .fill(LinearGradient(colors: [lowerColor, upperColor], startPoint: .leading, endPoint: .trailing))
            .frame(width: max(4, upperX - lowerX), height: 4)
            .offset(x: lowerX + thumbSize / 2)
          thumb(color: lowerColor)
            .offset(x: lowerX - (collapsed ? 5 : 0))
            .gesture(drag(usable: usable, onChange: onLowerChange))
            .accessibilityElement()
            .accessibilityLabel("Earliest era")
            .accessibilityValue(lowerEra?.displayName ?? "First era")
            .accessibilityAdjustableAction { direction in
              adjust(direction, from: lower, onChange: onLowerChange)
            }
          thumb(color: upperColor)
            .offset(x: upperX + (collapsed ? 5 : 0))
            .gesture(drag(usable: usable, onChange: onUpperChange))
            .accessibilityElement()
            .accessibilityLabel("Latest era")
            .accessibilityValue(upperEra?.displayName ?? "Last era")
            .accessibilityAdjustableAction { direction in
              adjust(direction, from: upper, onChange: onUpperChange)
            }
        }
        .frame(height: thumbSize)
        .coordinateSpace(.named(space))
      }
      .frame(height: thumbSize)

      HStack(alignment: .top) {
        Text(lowerEra?.displayName ?? "First era")
          .frame(maxWidth: .infinity, alignment: .leading)
        Text(upperEra?.displayName ?? "Last era")
          .frame(maxWidth: .infinity, alignment: .trailing)
          .multilineTextAlignment(.trailing)
      }
      .font(.caption.weight(.semibold))
      .foregroundStyle(Color(hex: "d5d5d5"))
      .lineLimit(2)
      .accessibilityHidden(true)
    }
    .padding(.vertical, 4)
    .sensoryFeedback(.selection, trigger: lower)
    .sensoryFeedback(.selection, trigger: upper)
  }

  private func thumb(color: Color) -> some View {
    Circle()
      .fill(color)
      .overlay(Circle().strokeBorder(Theme.surface, lineWidth: 3))
      .overlay(Circle().strokeBorder(Color(hex: "d8d8d8"), lineWidth: 1))
      .frame(width: thumbSize, height: thumbSize)
      .contentShape(Rectangle().inset(by: -10))
  }

  private func position(of index: Int, usable: CGFloat) -> CGFloat {
    guard lastIndex > 0 else { return 0 }
    return usable * CGFloat(index) / CGFloat(lastIndex)
  }

  private func drag(usable: CGFloat, onChange: @escaping @MainActor (Int) -> Void) -> some Gesture {
    DragGesture(minimumDistance: 0, coordinateSpace: .named(space))
      .onChanged { value in
        let fraction = (value.location.x - thumbSize / 2) / usable
        let index = Int((min(1, max(0, fraction)) * CGFloat(lastIndex)).rounded())
        onChange(index)
      }
  }

  private func adjust(
    _ direction: AccessibilityAdjustmentDirection, from index: Int, onChange: @MainActor (Int) -> Void
  ) {
    switch direction {
    case .increment: onChange(index + 1)
    case .decrement: onChange(index - 1)
    @unknown default: break
    }
  }
}
