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

  /// The preference is in unscaled points. Shrinking the window clamps the
  /// displayed width without changing that preference or squeezing the chat.
  static func drawerWidth(
    textScale: CGFloat, preferredWidth: CGFloat = 300, windowWidth: CGFloat = .infinity
  ) -> CGFloat {
    let preferred = preferredWidth.isFinite ? preferredWidth : 300
    let maximum = min(560 * textScale, max(0, windowWidth - 520 * textScale))
    let minimum = min(240 * textScale, maximum)
    return min(maximum, max(minimum, preferred * textScale))
  }

  /// The name pill and every pill in the stack share this width: 224 at 1.0.
  static func pillWidth(textScale: CGFloat) -> CGFloat { 224 * textScale }
}
