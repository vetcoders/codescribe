import SwiftUI

/// Search field for the per-tool permissions browser. It filters the whole
/// catalog — tool name, identifier, source and server — and the source popup
/// next to it carries the sources and their counts, so this field no longer
/// states how many sources there are (Founder brief, round 12, 2026-10-10).
struct ToolSearchField: View {
  @Binding var text: String
  @FocusState private var searchFocused: Bool

  var body: some View {
    HStack(spacing: CSSpace.sm) {
      Image(systemName: "magnifyingglass")
        .font(.system(size: 11, weight: .medium))
        .foregroundStyle(Color.secondary)
        .accessibilityHidden(true)
      TextField("Search tools…", text: $text)
        .textFieldStyle(.plain)
        .focused($searchFocused)
        .font(CSFont.mono(11.5, .medium))
        .foregroundStyle(Color.primary)
    }
    .padding(.horizontal, 10)
    .padding(.vertical, 7)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background {
      RoundedRectangle(cornerRadius: CSSpace.sm)
        .fill(Color.primary.opacity(0.06))
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    }
    .overlay {
      CSFocusOutline(isFocused: searchFocused, cornerRadius: CSSpace.sm)
    }
  }
}
