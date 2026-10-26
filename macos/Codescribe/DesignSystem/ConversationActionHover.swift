import SwiftUI

/// Message actions change only their ink on hover. Applying this to the whole
/// control keeps a label and its symbol in step, without a wash or tooltip.
/// Keyboard focus keeps the product's single-pixel outline: the window's
/// `csFocusPolicy()` hides pointer outlines before delivering the click and
/// releases focus afterward.
private struct ConversationActionHover: ViewModifier {
  let restingColor: Color
  let hoverColor: Color
  var hoverBackground: Color = .clear
  var restingOpacity: Double = 1
  @Environment(\.isEnabled) private var isEnabled
  @State private var hovered = false

  func body(content: Content) -> some View {
    content
      .foregroundStyle(hovered && isEnabled ? hoverColor : restingColor)
      .opacity(isEnabled ? (hovered ? 1 : restingOpacity) : 0.35)
      .background {
        RoundedRectangle(cornerRadius: 6, style: .continuous)
          .fill(hovered && isEnabled ? hoverBackground : .clear)
          .allowsHitTesting(false)
          .accessibilityHidden(true)
      }
      .csFocusOutline()
      // One mounted region owns hover in both ordinary windows and the
      // nonactivating overlay. A second SwiftUI tracker can deliver an exit
      // after the native region has already entered its replacement geometry.
      .background { OverlayMenuCursorRegion { hovered = $0 } }
      .onChange(of: isEnabled) { _, enabled in
        if !enabled { hovered = false }
      }
  }
}

extension View {
  func conversationActionHover(
    restingColor: Color, hoverColor: Color, hoverBackground: Color = .clear,
    restingOpacity: Double = 1
  ) -> some View {
    modifier(ConversationActionHover(
      restingColor: restingColor, hoverColor: hoverColor,
      hoverBackground: hoverBackground, restingOpacity: restingOpacity))
  }
}
