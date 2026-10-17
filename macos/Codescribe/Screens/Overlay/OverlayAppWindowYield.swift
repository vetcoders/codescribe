import AppKit

/// Whether a window of this app is a regular window the overlay should step
/// aside for. Borderless chrome and modal alerts never qualify.
enum OverlayAppWindowPolicy {
  /// Titled, not a sheet, and at the normal level. The overlay, tray, header
  /// chrome and popovers are borderless, so they cannot fold the overlay; a
  /// confirmation raised from it sits above the normal level.
  static func shouldYield(to window: NSWindow) -> Bool {
    window.styleMask.contains(.titled) && window.sheetParent == nil && window.level == .normal
  }
}

/// Folds the expanded overlay when another window of this app becomes key.
/// Closing that window does not open the overlay back up.
///
/// The overlay steps aside once per appearance of a window, not on every
/// click into it: with the agent chat open, an overlay expanded by hand would
/// otherwise fold again each time focus returned to the chat. A window counts
/// as appearing again after it closes or leaves the screen; the agent chat
/// only orders out when minimised, so closing alone would miss its return.
@MainActor
final class OverlayAppWindowYield {
  private weak var panel: NSPanel?
  private weak var state: OverlayState?
  private var observers: [NSObjectProtocol] = []
  /// `windowNumber`s of the windows already yielded to in their current appearance.
  private var yieldedWindows: Set<Int> = []

  init(panel: NSPanel, state: OverlayState) {
    self.panel = panel
    self.state = state
  }

  /// Listen for every window in this process becoming key. Delivery stays on
  /// the main queue, where the panel and the presentation live.
  func start() {
    guard observers.isEmpty else { return }
    observers = [
      observe(NSWindow.didBecomeKeyNotification) { $0.noteBecameKey($1) },
      observe(NSWindow.willCloseNotification) { $0.noteGone($1) },
      observe(NSWindow.didChangeOcclusionStateNotification) { $0.noteOcclusion($1) },
    ]
  }

  /// Drop the key-window observer. The panel keeps this object, so only a
  /// later `start` brings the watch back.
  func invalidate() {
    for observer in observers {
      NotificationCenter.default.removeObserver(observer)
    }
    observers = []
    yieldedWindows = []
  }

  private func observe(
    _ name: Notification.Name,
    _ action: @escaping @MainActor (OverlayAppWindowYield, Notification) -> Void
  ) -> NSObjectProtocol {
    let handler: @Sendable (Notification) -> Void = { [weak self] notification in
      MainActor.assumeIsolated {
        guard let self else { return }
        action(self, notification)
      }
    }
    return NotificationCenter.default.addObserver(
      forName: name, object: nil, queue: .main, using: handler)
  }

  /// The overlay must not fold itself, and a window that fails the
  /// regular-window rules is ignored. A regular window is marked as seen even
  /// while the panel is hidden or compact, so an overlay brought up by hand
  /// later does not fold on the next click into that window.
  private func noteBecameKey(_ notification: Notification) {
    guard let window = notification.object as? NSWindow else { return }
    guard let panel, window !== (panel as NSWindow) else { return }
    guard OverlayAppWindowPolicy.shouldYield(to: window) else { return }
    guard yieldedWindows.insert(window.windowNumber).inserted else { return }
    guard panel.isVisible else { return }
    state?.yieldToAppWindow()
  }

  /// A closed window that opens again is a new appearance.
  private func noteGone(_ notification: Notification) {
    guard let window = notification.object as? NSWindow else { return }
    yieldedWindows.remove(window.windowNumber)
  }

  /// An ordered-out window (the agent chat when minimised) is a new appearance
  /// once it is back on screen.
  private func noteOcclusion(_ notification: Notification) {
    guard let window = notification.object as? NSWindow,
      !window.occlusionState.contains(.visible)
    else { return }
    yieldedWindows.remove(window.windowNumber)
  }
}
