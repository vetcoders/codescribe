import SwiftUI

/// Message actions change only their ink on hover. Applying this to the whole
/// control keeps a label and its symbol in step, without a wash or tooltip.
/// Keyboard focus keeps the product's single-pixel outline: the window's
/// `csFocusPolicy()` releases focus after a pointer click, so it never shows
/// for the mouse.
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
      .onHover { hovered = $0 }
      // The nonactivating overlay routes pointer motion through this region;
      // ordinary windows use SwiftUI's onHover. Neither path intercepts clicks.
      .background { OverlayMenuCursorRegion { hovered = $0 } }
  }
}

extension View {
  func conversationActionHover(restingColor: Color, hoverColor: Color) -> some View {
    modifier(ConversationActionHover(restingColor: restingColor, hoverColor: hoverColor))
  }
}
