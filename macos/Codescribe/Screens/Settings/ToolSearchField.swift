import SwiftUI

/// Search field for the per-tool permissions browser. It filters the whole
/// catalog — tool name, identifier, source and server — and the source popup
/// beside it carries the sources and their counts, so this field no longer
/// states how many sources there are (Founder brief, round 12, 2026-10-10).
///
/// Since round 16 the two share one row, so the field takes the control rung of
/// the Settings scale: the chip's insets and corner, which is also the corner
/// `CSFocusOutline` draws.
struct ToolSearchField: View {
  @Binding var text: String
  @FocusState private var searchFocused: Bool

  var body: some View {
    HStack(spacing: CSSpace.xs) {
      Image(systemName: "magnifyingglass")
        .font(.system(size: 11, weight: .medium))
        .foregroundStyle(CSColor.secondaryInk)
        .accessibilityHidden(true)
      TextField("Search tools…", text: $text)
        .textFieldStyle(.plain)
        .focused($searchFocused)
        // Input text, not a label: the scale's control rung is semibold and
        // reads wrong under a caret, and the one input rung the pane owns
        // (`settingsInputChrome`) is mono and taller than this row allows.
        .font(CSFont.ui(12, .medium))
        .foregroundStyle(Color.primary)
    }
    .padding(.horizontal, SettingsScale.controlInsetH)
    .padding(.vertical, SettingsScale.controlInsetV)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background {
      RoundedRectangle(cornerRadius: SettingsScale.controlRadius)
        .fill(Color.primary.opacity(CSAlpha.fill))
        .strokeBorder(Color.primary.opacity(CSAlpha.stroke), lineWidth: 1)
    }
    .overlay {
      CSFocusOutline(isFocused: searchFocused, cornerRadius: SettingsScale.controlRadius)
    }
  }
}
