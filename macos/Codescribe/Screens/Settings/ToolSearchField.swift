import SwiftUI

/// Search field for the tool-overrides browser, with the visible server count.
struct ToolSearchField: View {
  @Binding var text: String
  let serverCount: Int

  var body: some View {
    HStack(spacing: CSSpace.md) {
      HStack(spacing: CSSpace.sm) {
        Image(systemName: "magnifyingglass")
          .font(.system(size: 11, weight: .medium))
          .foregroundStyle(CSColor.textFaint)
          .accessibilityHidden(true)
        TextField("Search server or tool", text: $text)
          .textFieldStyle(.plain)
          .font(CSFont.mono(11.5, .medium))
          .foregroundStyle(CSColor.textBody)
      }
      .padding(.horizontal, 10)
      .padding(.vertical, 7)
      .background {
        RoundedRectangle(cornerRadius: CSSpace.sm)
          .fill(CSColor.surfaceRaised(0.03))
          .strokeBorder(CSColor.hairline(0.06), lineWidth: 1)
      }

      Text("^[\(serverCount) server](inflect: true)")
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(CSColor.textFaint)
    }
  }
}
