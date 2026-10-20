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
/// One semantic for every adopted window: **minimise means hide**. The yellow
/// button and ⌘M stay enabled and order the window out; the tray's "Open
/// chat", the summon shortcut and the passive voice-delivery reveal bring the
/// Agent window back, Open Settings and ⌘, the Settings window (Founder
/// decisions, 2026-10-10 for the Agent window, 2026-10-20 for Settings).
///
/// How each window gets there depends on who owns its class:
/// - The Agent window is a `HidingWindow`, so its own overrides of
///   `performMiniaturize(_:)` and `miniaturize(_:)` turn every path into
///   `orderOut`.
/// - The Settings window is a SwiftUI scene whose window class the app does
///   not own. `adopt(_:)` installs one `MinimiseHider` for it: the yellow
///   button's target/action hide the window, the scene's ⌘M command calls
///   `minimiseKeyWindow()`, and every remaining path (Window ▸ Minimize All,
///   the title-bar double-click preference) ends at `didMiniaturize`, where
///   the hider restores the window and orders it out at once. That last step
///   is the guarantee that no path leaves an Exposé tile, not a second
///   behaviour.
@MainActor
enum DockPresence {
  private static var hiders: [ObjectIdentifier: MinimiseHider] = [:]

  /// Apply the tray toggle to the activation policy.
  static func apply(showDockIcon: Bool) {
    NSApp.setActivationPolicy(showDockIcon ? .regular : .accessory)
  }

  /// Register a titled window. A window left miniaturised by an earlier build
  /// comes back first, so none stays orphaned off screen as an empty Exposé
  /// tile. Adopting the same window again keeps its one hider and re-points
  /// the minimise button at it.
  static func adopt(_ window: NSWindow) {
    if window.isMiniaturized {
      window.deminiaturize(nil)
    }
    window.styleMask = styleMask(window.styleMask)
    guard !(window is HidingWindow) else { return }
    let key = ObjectIdentifier(window)
    if let hider = hiders[key], hider.isAttached(to: window) {
      hider.retargetMinimiseButton()
    } else {
      // A stale entry left by a window that deallocated without closing.
      hiders[key]?.invalidate()
      hiders[key] = MinimiseHider(window: window)
    }
  }

  /// The style mask a registered window carries: the minimise control is
  /// always present, because on every adopted window it hides.
  static func styleMask(_ mask: NSWindow.StyleMask) -> NSWindow.StyleMask {
    var mask = mask
    mask.insert(.miniaturizable)
    return mask
  }

  /// Whether minimising `window` hides it instead of miniaturising it.
  static func hidesOnMinimise(_ window: NSWindow) -> Bool {
    window is HidingWindow || hiders[ObjectIdentifier(window)]?.isAttached(to: window) == true
  }

  /// The Window ▸ Minimize command (⌘M): hide the key window when it is
  /// adopted, and leave any other window its native miniaturise.
  static func minimiseKeyWindow() {
    guard let window = NSApp.keyWindow else { return }
    if hidesOnMinimise(window) {
      window.orderOut(nil)
    } else {
      window.performMiniaturize(nil)
    }
  }

  fileprivate static func release(_ key: ObjectIdentifier) {
    hiders[key] = nil
  }
}

/// Turns minimise into hide for one adopted window whose class the app does
/// not own. `DockPresence` retains it until the window closes.
@MainActor
private final class MinimiseHider: NSObject {
  private weak var window: NSWindow?
  private let key: ObjectIdentifier
  private let center = NotificationCenter.default
  private var tokens: [any NSObjectProtocol] = []
  private var undoingMiniaturise = false

  init(window: NSWindow) {
    self.window = window
    key = ObjectIdentifier(window)
    super.init()
    retargetMinimiseButton()
    tokens = [
      center.addObserver(
        forName: NSWindow.didMiniaturizeNotification,
        object: window,
        queue: .main
      ) { [weak self] _ in
        MainActor.assumeIsolated { self?.undoMiniaturise() }
      },
      center.addObserver(
        forName: NSWindow.didDeminiaturizeNotification,
        object: window,
        queue: .main
      ) { [weak self] _ in
        MainActor.assumeIsolated { self?.finishUndoingMiniaturise() }
      },
      center.addObserver(
        forName: NSWindow.willCloseNotification,
        object: window,
        queue: .main
      ) { [weak self] _ in
        MainActor.assumeIsolated { self?.invalidate() }
      },
    ]
  }

  func isAttached(to window: NSWindow) -> Bool {
    self.window === window
  }

  /// Point the yellow button at `hide(_:)` instead of `performMiniaturize:`.
  func retargetMinimiseButton() {
    guard let button = window?.standardWindowButton(.miniaturizeButton) else { return }
    button.target = self
    button.action = #selector(hide(_:))
    button.isEnabled = true
  }

  @objc func hide(_ sender: Any?) {
    window?.orderOut(nil)
  }

  /// A path that reached AppKit's own miniaturise still ends hidden. The
  /// restore animates, so the window is ordered out once AppKit reports it
  /// back (`didDeminiaturize`), not while it is still on its way.
  private func undoMiniaturise() {
    guard let window else { return }
    undoingMiniaturise = true
    window.deminiaturize(nil)
  }

  private func finishUndoingMiniaturise() {
    guard undoingMiniaturise else { return }
    undoingMiniaturise = false
    window?.orderOut(nil)
  }

  func invalidate() {
    for token in tokens { center.removeObserver(token) }
    tokens.removeAll()
    DockPresence.release(key)
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
