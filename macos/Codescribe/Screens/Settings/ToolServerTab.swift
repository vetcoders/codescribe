import SwiftUI

/// One server tab in the tool-overrides browser: name plus tool count, with
/// the selected tab filled — and marked by a chevron too when the user asks
/// for differentiation without color.
struct ToolServerTab: View {
  let server: String
  let count: Int
  let isSelected: Bool
  let action: () -> Void

  @Environment(\.accessibilityDifferentiateWithoutColor) private var differentiateWithoutColor

  var body: some View {
    Button(action: action) {
      HStack(spacing: CSSpace.sm) {
        if differentiateWithoutColor, isSelected {
          Image(systemName: "chevron.right")
            .font(.system(size: 9, weight: .bold))
            .foregroundStyle(CSColor.textHigh)
            .accessibilityHidden(true)
        }
        Text(server)
          .font(CSFont.ui(12, .semibold))
          .foregroundStyle(isSelected ? CSColor.textHigh : CSColor.textBody)
          .lineLimit(1)
          .truncationMode(.middle)
        Spacer(minLength: 0)
        Text("\(count)")
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textFaint)
          .padding(.horizontal, CSSpace.xs)
          .padding(.vertical, 2)
          .background(CSColor.surfaceRaised(0.05), in: .capsule)
      }
      .padding(.horizontal, 10)
      .padding(.vertical, 7)
      .background(
        isSelected ? CSColor.surfaceRaised(0.07) : .clear, in: .rect(cornerRadius: 7)
      )
      .contentShape(.rect)
    }
    .buttonStyle(.plain)
    .csFocusRing()
    .accessibilityLabel("\(server), \(count) tools")
    .accessibilityAddTraits(isSelected ? .isSelected : [])
  }
}
