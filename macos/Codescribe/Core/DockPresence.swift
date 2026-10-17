import AppKit

/// The one owner of the app's Dock presence and of what the minimise control
/// does on the app's titled windows.
///
/// Codescribe launches as an accessory (`LSUIElement`); the tray's
/// "Show Dock icon" toggle promotes it to a regular app. The Settings and
/// Agent windows must never actually miniaturise, with or without the icon:
/// with the Dock's default "Minimize windows into application icon", a
/// miniaturised Codescribe window leaves the screen, stays in the window list,
/// and App Exposé and Mission Control draw it as an empty, transparent tile
/// until it is restored. Reproduced on Silver with the Dock tile present, so
/// the icon is no cure.
///
/// The Agent window is a `HidingWindow`: its yellow button and ⌘M stay enabled
/// and **hide** it instead (Founder decision, 2026-10-10); the tray's "Open
/// chat", the summon shortcut and the passive voice-delivery reveal bring it
/// back. The Settings window is a SwiftUI scene whose window class the app does
/// not own, so no override can catch every miniaturise path there (Window ▸
/// Minimize All, the title-bar double-click preference); it keeps no minimise
/// control at all, and closing it is the way out.
@MainActor
enum DockPresence {
  /// Apply the tray toggle to the activation policy.
  static func apply(showDockIcon: Bool) {
    NSApp.setActivationPolicy(showDockIcon ? .regular : .accessory)
  }

  /// Register a titled window. A window left miniaturised by an earlier build
  /// comes back first, so none stays orphaned off screen as an empty Exposé
  /// tile.
  static func adopt(_ window: NSWindow) {
    if window.isMiniaturized {
      window.deminiaturize(nil)
    }
    window.styleMask = styleMask(window.styleMask, hidesOnMinimise: window is HidingWindow)
  }

  /// The style mask a registered window carries: the minimise control only
  /// where the window's own class turns it into "hide".
  static func styleMask(
    _ mask: NSWindow.StyleMask, hidesOnMinimise: Bool
  ) -> NSWindow.StyleMask {
    var mask = mask
    if hidesOnMinimise {
      mask.insert(.miniaturizable)
    } else {
      mask.remove(.miniaturizable)
    }
    return mask
  }
}

/// A window the app itself creates and keeps a handle to, so hiding it is a
/// plain `orderOut` and every reopen path re-orders the same instance.
///
/// Both miniaturise entry points are overridden: `performMiniaturize(_:)` is
/// what the titlebar button and the Window ▸ Minimize item (⌘M, sent down the
/// responder chain to the key window) deliver, and `miniaturize(_:)` is the
/// programmatic one that the "double-click a window's title bar to minimise"
/// preference and Minimize All end at. Overriding both means no path can put
/// this window into the window list as an empty Exposé tile.
final class HidingWindow: NSWindow {
  override func performMiniaturize(_ sender: Any?) {
    orderOut(nil)
  }

  override func miniaturize(_ sender: Any?) {
    orderOut(nil)
  }
}
