import SwiftUI

/// What the expanded agents surface shows, chosen with the corner buttons of
/// the list and remembered between reveals (Maciej, 2026-10-19: the agents
/// list as the main view with the messages sliding out on the left; folded,
/// the list leaves its agents as pills). The window width decides only
/// whether the list fits beside the conversation.
enum OverlayAgentsLayout: String {
  /// The conversation on the left and the list beside it where the window is
  /// wide enough; below the threshold the list comes when asked for.
  case split
  /// The list alone, at any width: the agents view. A row chosen in it
  /// slides the conversation out.
  case agents
  /// The conversation, with the list folded to a rail of agent pills.
  case rail
}

/// Wide/narrow hosting decision for the agents roster: a docked column beside
/// the conversation, or the same list over the whole window below the
/// threshold. One threshold in one text scale, so ⌘+ moves both surfaces
/// together (mockup `szuflada-agentow.html`, Founder 2026-10-16).
enum OverlayAgentDrawerLayout {
  static let widthDefaultsKey = "Overlay.agentDrawerWidth.v1"

  /// Docking needs the drawer and a conversation that still reads well side by
  /// side: 300 + 420 points in the overlay's text scale. 520 for the
  /// conversation kept a 900-point window at text 1.2 on pills with room to
  /// spare, while the same column docked on dragon at text 0.8 (Maciej,
  /// 2026-10-19: "Breaking point dla trybu rozwiniętego jest zbyt szeroko").
  /// At very large text a medium window still falls back to the full-width list.
  static func isDocked(windowWidth: CGFloat, textScale: CGFloat) -> Bool {
    windowWidth >= (300 + 420) * textScale
  }

  /// The list stays on screen beside, or over, a conversation without being
  /// asked for. The transcription keeps its full width (OverlayChromeV3Tests)
  /// and shows the list only while it is asked for.
  static func listPinned(
    layout: OverlayAgentsLayout, docked: Bool, conversationSelected: Bool
  ) -> Bool {
    guard conversationSelected else { return false }
    switch layout {
    case .split: return docked
    case .agents: return true
    case .rail: return false
    }
  }

  /// The preference is in unscaled points and has no upper bound (Maciej,
  /// 2026-10-18: "Jeśli przesunę sobie resizing całkiem do lewej, to okno
  /// stanowi tylko i wyłącznie widok agentów"). A conversation narrower than
  /// `minimumConversationWidth` is unusable, so the roster then takes the
  /// whole window; a drag that ends there chooses the agents view.
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

  /// Before the corner buttons the agents view was a roster dragged across
  /// the window, stored as this oversized width. Read once, to migrate it.
  static let legacyFillsWindowWidth: CGFloat = 100_000

  /// The preference to store for a width the user drew. Nil when the drawn
  /// roster covers the window: that chooses the agents view, and the width
  /// remembered before the drag stays for the way back.
  static func rememberedWidth(
    textScale: CGFloat, preferredWidth: CGFloat, windowWidth: CGFloat
  ) -> CGFloat? {
    let shown = drawerWidth(
      textScale: textScale, preferredWidth: preferredWidth, windowWidth: windowWidth)
    return fillsWindow(drawerWidth: shown, windowWidth: windowWidth) ? nil : shown / textScale
  }

  /// The widest roster, unscaled, that still leaves the conversation its
  /// minimum: where the conversation slides out to when the remembered width
  /// leaves it no room in this window.
  static func widestSplitWidth(textScale: CGFloat, windowWidth: CGFloat) -> CGFloat {
    (windowWidth - minimumConversationWidth(textScale: textScale)) / textScale
  }

  /// The folded list's rail of agent pills: 44 at 1.0.
  static func railWidth(textScale: CGFloat) -> CGFloat { 44 * textScale }

  /// The name pill's width: 224 at 1.0.
  static func pillWidth(textScale: CGFloat) -> CGFloat { 224 * textScale }
}
