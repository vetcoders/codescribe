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

  /// The window may shrink, but it must not crush the user's explicit pane
  /// choice. Folding remains a divider action, never a width side effect.
  static func minimumWindowWidth(layout: OverlayAgentsLayout, textScale: CGFloat) -> CGFloat {
    switch layout {
    case .split: minimumSplitWindowWidth(textScale: textScale)
    case .agents:
      contentMinimumWidth(textScale: textScale) + conversationEdgeWidth(textScale: textScale)
    case .rail:
      minimumConversationWidth(textScale: textScale) + railWidth(textScale: textScale)
    }
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

  /// While the divider is dragged both panes keep their old widths and only
  /// the guide moves; they fade to this so the stale split and the reflow at
  /// release read as one intended transition (Maciej, 2026-11-07). Set once
  /// at the gesture's start, cleared once after the committed width lays out.
  static let dragDimOpacity: Double = 0.55
  /// The light blur under that fade. A compositor filter runs each frame the
  /// guide moves, so it stays small; 0 turns it off.
  static let dragBlurRadius: CGFloat = 1.5

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

  /// Room from the widest pill's trailing edge to the rail's: the resize
  /// band, or the unread capsule's reach when that is wider, so a two-digit
  /// count never leaves the rail. The unfold button ends on the pills'
  /// trailing edge, this far from the rail's.
  static func railTrailingInset(textScale: CGFloat) -> CGFloat {
    max(OverlayResizeHit.band, OverlayChannelPill.railBadgeReach(scale: textScale))
  }

  /// The folded rail: the resize band's inset before the widest pill and
  /// `railTrailingInset` after it. Monika, 2026-11-07: narrower rail
  /// (the pill and two insets, no longer the pill plus a 4 pt margin, the
  /// capsule's reach and two bands).
  static func railWidth(textScale: CGFloat) -> CGFloat {
    OverlayResizeHit.band + railPillWidth(textScale: textScale)
      + railTrailingInset(textScale: textScale)
  }

  /// The name pill's width: 224 at 1.0.
  static func pillWidth(textScale: CGFloat) -> CGFloat { 224 * textScale }
}
