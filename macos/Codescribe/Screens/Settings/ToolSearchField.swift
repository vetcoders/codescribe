import SwiftUI

/// Search field for the per-tool permissions browser, with the count of
/// visible tool sources (native plus every MCP server).
struct ToolSearchField: View {
  @Binding var text: String
  let serverCount: Int
  @FocusState private var searchFocused: Bool

  var body: some View {
    HStack(spacing: CSSpace.md) {
      HStack(spacing: CSSpace.sm) {
        Image(systemName: "magnifyingglass")
          .font(.system(size: 11, weight: .medium))
          .foregroundStyle(Color.secondary)
          .accessibilityHidden(true)
        TextField("Search server or tool", text: $text)
          .textFieldStyle(.plain)
          .focused($searchFocused)
          .font(CSFont.mono(11.5, .medium))
          .foregroundStyle(Color.primary)
      }
      .padding(.horizontal, 10)
      .padding(.vertical, 7)
      .background {
        RoundedRectangle(cornerRadius: CSSpace.sm)
          .fill(Color.primary.opacity(0.06))
          .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
      }
      .overlay {
        CSFocusOutline(isFocused: searchFocused, cornerRadius: CSSpace.sm)
      }

      Text("\(serverCount) tool sources", comment: "Plural: native plus MCP servers")
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(Color.secondary)
    }
  }
}
