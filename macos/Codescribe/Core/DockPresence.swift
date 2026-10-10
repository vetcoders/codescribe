import AppKit

/// The one owner of the app's Dock presence.
///
/// Codescribe launches as an accessory (`LSUIElement`); the tray's
/// "Show Dock icon" toggle promotes it to a regular app. The Settings and
/// Agent windows never minimise, with or without the icon: with the Dock's
/// default "Minimize windows into application icon", a minimised Codescribe
/// window leaves the screen, stays in the window list, and App Exposé and
/// Mission Control draw it as an empty, transparent tile until it is restored.
/// Reproduced on Silver with the Dock tile present, so the icon is no cure.
/// The yellow button is therefore disabled and ⌘M does nothing; close and
/// reopen the window instead.
@MainActor
enum DockPresence {
  /// Apply the tray toggle to the activation policy.
  static func apply(showDockIcon: Bool) {
    NSApp.setActivationPolicy(showDockIcon ? .regular : .accessory)
  }

  /// Register a titled window that never minimises. A window already
  /// minimised comes back first, so none is left orphaned off screen.
  static func adopt(_ window: NSWindow) {
    if window.isMiniaturized {
      window.deminiaturize(nil)
    }
    window.styleMask = styleMask(window.styleMask)
  }

  /// The style mask a registered window carries: everything but minimise.
  static func styleMask(_ mask: NSWindow.StyleMask) -> NSWindow.StyleMask {
    var mask = mask
    mask.remove(.miniaturizable)
    return mask
  }
}
