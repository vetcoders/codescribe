import AppKit

/// The one owner of the app's Dock presence.
///
/// Codescribe launches as an accessory (`LSUIElement`); the tray's
/// "Show Dock icon" toggle promotes it to a regular app. Minimising follows
/// the Dock icon. With the Dock's default "Minimize windows into application
/// icon", a window of an app without a Dock tile has nowhere to go: it leaves
/// the screen, stays in the window list, and App Exposé and Mission Control
/// draw it as an empty, transparent tile until it is restored. So the Settings
/// and Agent windows can be minimised only while the Dock icon is shown;
/// otherwise their yellow button is disabled and ⌘M does nothing.
@MainActor
enum DockPresence {
  private static let adopted = NSHashTable<NSWindow>.weakObjects()

  /// Apply the toggle: the activation policy plus the minimise button of every
  /// adopted window. A window minimised while the icon was shown comes back
  /// before the icon disappears, so none is left orphaned.
  static func apply(showDockIcon: Bool) {
    NSApp.setActivationPolicy(showDockIcon ? .regular : .accessory)
    for window in adopted.allObjects {
      align(window, dockIconShown: showDockIcon)
    }
  }

  /// Register a titled window that minimises only while the Dock icon is shown.
  static func adopt(_ window: NSWindow) {
    adopted.add(window)
    align(window, dockIconShown: NSApp.activationPolicy() == .regular)
  }

  /// The style mask a registered window carries for a given Dock presence.
  static func styleMask(_ mask: NSWindow.StyleMask, dockIconShown: Bool) -> NSWindow.StyleMask {
    var mask = mask
    if dockIconShown {
      mask.insert(.miniaturizable)
    } else {
      mask.remove(.miniaturizable)
    }
    return mask
  }

  private static func align(_ window: NSWindow, dockIconShown: Bool) {
    if !dockIconShown, window.isMiniaturized {
      window.deminiaturize(nil)
    }
    window.styleMask = styleMask(window.styleMask, dockIconShown: dockIconShown)
  }
}
