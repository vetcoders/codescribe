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

  /// The docked column's width: 300 points at scale 1.0.
  static func drawerWidth(textScale: CGFloat) -> CGFloat { 300 * textScale }

  /// The name pill and every pill in the stack share this width: 224 at 1.0.
  static func pillWidth(textScale: CGFloat) -> CGFloat { 224 * textScale }
}
