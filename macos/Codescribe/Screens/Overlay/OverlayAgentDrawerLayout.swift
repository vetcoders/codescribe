import SwiftUI

/// One Agents panel with three presentations. The conversation is never folded.
enum OverlayAgentPanelPresentation: String, CaseIterable {
  case expanded, compact, hidden
}

/// Geometry for one Agents panel: docked beside a readable conversation or
/// covering it on a narrow window. Visibility is owned by OverlayState.
enum OverlayAgentDrawerLayout {
  static let widthDefaultsKey = "Overlay.agentDrawerWidth.v1"

  static func minimumSplitWindowWidth(textScale: CGFloat) -> CGFloat {
    (240 + 320) * textScale
  }

  static func isDocked(windowWidth: CGFloat, textScale: CGFloat) -> Bool {
    windowWidth >= minimumSplitWindowWidth(textScale: textScale)
  }

  static func drawerWidth(
    textScale: CGFloat, preferredWidth: CGFloat = 300, windowWidth: CGFloat = .infinity
  ) -> CGFloat {
    let preferred = preferredWidth.isFinite ? preferredWidth : 300
    let width = max(240 * textScale, preferred * textScale)
    guard windowWidth.isFinite else { return width }
    let available = max(0, windowWidth)
    guard isDocked(windowWidth: available, textScale: textScale) else { return available }
    return min(width, max(0, available - minimumConversationWidth(textScale: textScale)))
  }

  static func minimumConversationWidth(textScale: CGFloat) -> CGFloat { 320 * textScale }
  static func paneHeaderHeight(textScale: CGFloat) -> CGFloat { 36 * textScale }
  static func paneToggleSize(textScale: CGFloat) -> CGFloat { 24 * textScale }
  static let paneHeaderTopInset: CGFloat = 16
  static let paneControlLeadingInset: CGFloat = 20

  static func paneHeaderControlSpace(textScale: CGFloat) -> CGFloat {
    max(
      0,
      paneControlSlotWidth(textScale: textScale) / 2
        + paneToggleSize(textScale: textScale) / 2 + 8 * textScale - paneControlLeadingInset)
  }

  static func paneControlSlotWidth(textScale: CGFloat) -> CGFloat {
    compactWidth(textScale: textScale)
  }

  /// Symmetric edge clearance keeps the buttons centered and their corner
  /// badges inside the viewport, without a dedicated count column.
  static func compactHorizontalInset(textScale: CGFloat) -> CGFloat {
    max(OverlayResizeHit.band, (OverlayAgentUnreadBadge.maximumRailDiameter / 2 + 4) * textScale)
  }

  static func compactWidth(textScale: CGFloat) -> CGFloat {
    OverlayChannelPill.compactButtonWidth(textScale: textScale)
      + 2 * compactHorizontalInset(textScale: textScale)
  }

  /// Divider resizing changes the column width only. It cannot hide the chat.
  static func rememberedWidth(
    textScale: CGFloat, preferredWidth: CGFloat, windowWidth: CGFloat
  ) -> CGFloat? {
    guard preferredWidth.isFinite, textScale > 0,
      isDocked(windowWidth: windowWidth, textScale: textScale)
    else { return nil }
    return drawerWidth(
      textScale: textScale, preferredWidth: preferredWidth, windowWidth: windowWidth) / textScale
  }

  /// Shared with floating agent cards, independent of the horizontal panel.
  static func pillWidth(textScale: CGFloat) -> CGFloat { 224 * textScale }
}
