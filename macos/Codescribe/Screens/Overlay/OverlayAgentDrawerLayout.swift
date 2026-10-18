import SwiftUI

/// Wide/narrow hosting decision for the agents roster, shared by the overlay
/// canvas (docked column) and the conversation surface (pill stack and the
/// name pill's width). One threshold in one text scale, so ⌘+ moves both
/// surfaces together (mockup `szuflada-agentow.html`, Founder 2026-10-16).
enum OverlayAgentDrawerLayout {
  /// Docking needs the drawer and an unsqueezed conversation side by side.
  /// At very large text a medium window falls back to pills instead.
  static func isDocked(windowWidth: CGFloat, textScale: CGFloat) -> Bool {
    windowWidth >= (300 + 520) * textScale
  }

  /// The preference is in unscaled points and has no upper bound (Maciej,
  /// 2026-10-18: "Jeśli przesunę sobie resizing całkiem do lewej, to okno
  /// stanowi tylko i wyłącznie widok agentów"). A conversation narrower than
  /// `minimumConversationWidth` is unusable, so the roster then takes the
  /// whole window; dragging the edge back out of that zone slides it out.
  static func drawerWidth(
    textScale: CGFloat, preferredWidth: CGFloat = 300, windowWidth: CGFloat = .infinity
  ) -> CGFloat {
    let preferred = preferredWidth.isFinite ? preferredWidth : 300
    let width = max(min(240 * textScale, max(0, windowWidth)), preferred * textScale)
    guard windowWidth.isFinite else { return width }
    return windowWidth - width < minimumConversationWidth(textScale: textScale)
      ? max(0, windowWidth) : width
  }

  static func minimumConversationWidth(textScale: CGFloat) -> CGFloat { 320 * textScale }

  /// The roster covers the window: the conversation beside it is hidden.
  static func fillsWindow(drawerWidth: CGFloat, windowWidth: CGFloat) -> Bool {
    windowWidth.isFinite && drawerWidth >= windowWidth - 0.5
  }

  /// Remembered for a roster that covered the window, so a window grown
  /// later stays the agents view instead of showing a sliver of chat.
  static let fillsWindowPreference: CGFloat = 100_000

  /// The preference to store for a width the user drew.
  static func rememberedWidth(
    textScale: CGFloat, preferredWidth: CGFloat, windowWidth: CGFloat
  ) -> CGFloat {
    let shown = drawerWidth(
      textScale: textScale, preferredWidth: preferredWidth, windowWidth: windowWidth)
    return fillsWindow(drawerWidth: shown, windowWidth: windowWidth)
      ? fillsWindowPreference : shown / textScale
  }

  /// The name pill and every pill in the stack share this width: 224 at 1.0.
  static func pillWidth(textScale: CGFloat) -> CGFloat { 224 * textScale }
}
