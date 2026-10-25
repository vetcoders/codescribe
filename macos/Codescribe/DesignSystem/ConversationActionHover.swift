import SwiftUI

/// Message actions change only their ink on hover. Applying this to the whole
/// control keeps a label and its symbol in step, without a wash or tooltip.
/// Keyboard focus keeps the product's single-pixel outline: the window's
/// `csFocusPolicy()` hides pointer outlines before delivering the click and
/// releases focus afterward.
private struct ConversationActionHover: ViewModifier {
  let restingColor: Color
  let hoverColor: Color
  @Environment(\.isEnabled) private var isEnabled
  @State private var hovered = false

  func body(content: Content) -> some View {
    content
      .foregroundStyle(hovered && isEnabled ? hoverColor : restingColor)
      .opacity(isEnabled ? 1 : 0.35)
      .csFocusOutline()
      // One mounted region owns hover in both ordinary windows and the
      // nonactivating overlay. A second SwiftUI tracker can deliver an exit
      // after the native region has already entered its replacement geometry.
      .background { OverlayMenuCursorRegion { hovered = $0 } }
  }
}

extension View {
  func conversationActionHover(restingColor: Color, hoverColor: Color) -> some View {
    modifier(ConversationActionHover(restingColor: restingColor, hoverColor: hoverColor))
  }
}
