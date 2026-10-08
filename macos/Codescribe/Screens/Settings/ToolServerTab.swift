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
            .foregroundStyle(Color.primary)
            .accessibilityHidden(true)
        }
        Text(ToolPermissionLabels.source(server))
          .font(CSFont.ui(12, .semibold))
          .foregroundStyle(isSelected ? Color.primary : Color.secondary)
          .lineLimit(1)
          .truncationMode(.middle)
        Spacer(minLength: 0)
        Text(verbatim: "\(count)")
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(Color.secondary)
          .padding(.horizontal, CSSpace.xs)
          .padding(.vertical, 2)
          .background(Color.primary.opacity(0.1), in: .capsule)
      }
      .padding(.horizontal, 10)
      .padding(.vertical, 7)
      .background(
        isSelected ? Color.primary.opacity(0.14) : .clear, in: .rect(cornerRadius: 7)
      )
      .contentShape(.rect)
    }
    .buttonStyle(.plain)
    .csFocusRing()
    .accessibilityLabel("\(ToolPermissionLabels.source(server)), \(count) tools")
    .accessibilityAddTraits(isSelected ? .isSelected : [])
  }
}
