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
@MainActor
final class OverlayAppWindowYield {
  private weak var panel: NSPanel?
  private weak var state: OverlayState?
  private var keyObserver: NSObjectProtocol?

  init(panel: NSPanel, state: OverlayState) {
    self.panel = panel
    self.state = state
  }

  /// Listen for every window in this process becoming key. Delivery stays on
  /// the main queue, where the panel and the presentation live.
  func start() {
    guard keyObserver == nil else { return }
    let handler: @Sendable (Notification) -> Void = { [weak self] notification in
      MainActor.assumeIsolated {
        self?.noteBecameKey(notification)
      }
    }
    keyObserver = NotificationCenter.default.addObserver(
      forName: NSWindow.didBecomeKeyNotification,
      object: nil,
      queue: .main,
      using: handler
    )
  }

  /// Drop the key-window observer. The panel keeps this object, so only a
  /// later `start` brings the watch back.
  func invalidate() {
    if let keyObserver {
      NotificationCenter.default.removeObserver(keyObserver)
    }
    keyObserver = nil
  }

  /// The overlay must not fold itself, and a hidden panel has nothing to fold.
  /// A window that fails the regular-window rules is ignored.
  private func noteBecameKey(_ notification: Notification) {
    guard let window = notification.object as? NSWindow else { return }
    guard let panel, window !== (panel as NSWindow) else { return }
    guard panel.isVisible else { return }
    guard OverlayAppWindowPolicy.shouldYield(to: window) else { return }
    state?.yieldToAppWindow()
  }
}
