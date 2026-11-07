import SwiftUI

/// The user's explicit choice of visible panes. Geometry never changes it.
enum OverlayAgentsLayout: String {
  /// Conversation and roster, side by side at every window width.
  case split
  /// Roster with the conversation folded to its own reveal edge.
  case agents
  /// Conversation with the roster folded to its rail of agent pills.
  case rail
}

/// Geometry for the roster column. An ordinary reveal always leaves the
/// conversation beside it; only an explicit pane fold chooses the agents view.
enum OverlayAgentDrawerLayout {
  static let widthDefaultsKey = "Overlay.agentDrawerWidth.v1"

  /// The host can grow to this floor when a side column is requested. During
  /// a smaller frame's transition both panes still receive a positive width.
  static func minimumSplitWindowWidth(textScale: CGFloat) -> CGFloat {
    contentMinimumWidth(textScale: textScale) + minimumConversationWidth(textScale: textScale)
  }

  static func isDocked(windowWidth: CGFloat, textScale: CGFloat) -> Bool {
    windowWidth > 0
  }

  /// A selected conversation keeps the explicit pane choice, independently
  /// of the current width. Transcription reveals the roster only on request.
  static func listPinned(
    layout: OverlayAgentsLayout, docked: Bool, conversationSelected: Bool
  ) -> Bool {
    guard conversationSelected else { return false }
    return layout != .rail
  }

  /// Clamp a remembered width to the current split instead of replacing the
  /// conversation with a full-width roster when the window becomes narrower.
  static func drawerWidth(
    textScale: CGFloat, preferredWidth: CGFloat = 300, windowWidth: CGFloat = .infinity
  ) -> CGFloat {
    let preferred = preferredWidth.isFinite ? preferredWidth : 300
    let width = max(contentMinimumWidth(textScale: textScale), preferred * textScale)
    guard windowWidth.isFinite else { return width }
    let available = max(0, windowWidth)
    let conversation =
      available >= minimumSplitWindowWidth(textScale: textScale)
      ? minimumConversationWidth(textScale: textScale) : available * 0.55
    return min(width, max(0, available - conversation))
  }

  static func minimumConversationWidth(textScale: CGFloat) -> CGFloat { 320 * textScale }

  /// The narrowest width the list's rows are laid out at. A window too narrow
  /// for them clips them from the trailing side, leaving their pills.
  static func contentMinimumWidth(textScale: CGFloat) -> CGFloat { 240 * textScale }

  /// The divider folds the list too: released narrower than this, the
  /// list folds to its rail of pills.
  static func railFoldWidth(textScale: CGFloat) -> CGFloat { 160 * textScale }

  /// A rail's divider dragged this far toward the conversation unfolds the list.
  static let railUnfoldDrag: CGFloat = 40

  /// A folded conversation keeps a real edge column beside the divider.
  static func conversationEdgeWidth(textScale: CGFloat) -> CGFloat { 38 * textScale }

  /// The roster covers the window: the conversation beside it is hidden.
  static func fillsWindow(drawerWidth: CGFloat, windowWidth: CGFloat) -> Bool {
    windowWidth.isFinite && drawerWidth >= windowWidth - 0.5
  }

  /// Before the corner buttons the agents view was a roster dragged across
  /// the window, stored as this oversized width. Read once, to migrate it.
  static let legacyFillsWindowWidth: CGFloat = 100_000

  /// Only an explicit resize crossing the conversation floor chooses the
  /// agents view. A narrow window or an oversized saved width never does.
  static func rememberedWidth(
    textScale: CGFloat, preferredWidth: CGFloat, windowWidth: CGFloat
  ) -> CGFloat? {
    guard preferredWidth.isFinite, textScale > 0 else { return nil }
    if windowWidth >= minimumSplitWindowWidth(textScale: textScale),
      preferredWidth * textScale > windowWidth - minimumConversationWidth(textScale: textScale)
    {
      return nil
    }
    return drawerWidth(
      textScale: textScale, preferredWidth: preferredWidth, windowWidth: windowWidth) / textScale
  }

  static func widestSplitWidth(textScale: CGFloat, windowWidth: CGFloat) -> CGFloat {
    max(0, windowWidth - minimumConversationWidth(textScale: textScale)) / textScale
  }

  /// The widest pill sets the column: the broadcast glyph (12), one
  /// monospaced digit (about 7.2 at 12-point semibold) and the pill's
  /// horizontal padding (2 × 6), all scaled, plus the fixed 3-point
  /// glyph-to-digit spacing from `railPill`.
  static func railPillWidth(textScale: CGFloat) -> CGFloat {
    (12 + 7.2 + 12) * textScale + 3
  }

  /// A 2-point inset on each side of the widest pill, the room its unread
  /// capsule takes beside it (`OverlayChannelPill.railBadgeReach`), plus
  /// equal gutters matching the panel's native resize band. Pill and capsule
  /// stay centred together in the column and end before the trailing resize
  /// target starts.
  static func railWidth(textScale: CGFloat) -> CGFloat {
    railPillWidth(textScale: textScale) + 4
      + OverlayChannelPill.railBadgeReach(scale: textScale) + 2 * OverlayResizeHit.band
  }

  /// The name pill's width: 224 at 1.0.
  static func pillWidth(textScale: CGFloat) -> CGFloat { 224 * textScale }
}
