import AppKit

/// What opening the overlay's keyboard gate asks of AppKit.
///
/// Maciej, 2026-11-08 14:25Z: "Composer w oknie Agents nie focusuje się mimo
/// kliknięcia i pozostawia focus na innej aplikacji w niektórych sytuacjach."
/// A non-activating panel's key status is AppKit's record of a grant the
/// window server made, and a click into another app does not reliably reach
/// the panel as resign-key. The panel went on reporting `isKeyWindow` while
/// keystrokes went to the other app, and the next click into the composer
/// skipped `makeKey()` because the panel believed it already had them.
enum OverlayKeyboardRequest: String, Equatable {
  /// The gate is open on a key panel: the keyboard is already this panel's.
  case none
  /// Ask the window server for the keyboard.
  case makeKey
  /// The panel reports key behind a closed gate. A closed gate refuses
  /// `canBecomeKey`, so only a stale record says that: resign it, then ask.
  case reclaimKey

  static func opening(gateOpen: Bool, panelIsKey: Bool) -> Self {
    guard panelIsKey else { return .makeKey }
    return gateOpen ? .none : .reclaimKey
  }
}

/// AppKit's key calls on the overlay panel. Tests record them instead, so a
/// hidden panel never asks the window server for the keyboard on the
/// Founder's desktop.
struct OverlayKeyCalls {
  var isKey: @MainActor (NSWindow) -> Bool
  var makeKey: @MainActor (NSWindow) -> Void
  var resignKey: @MainActor (NSWindow) -> Void

  static var appKit: OverlayKeyCalls {
    OverlayKeyCalls(
      isKey: { $0.isKeyWindow },
      makeKey: { $0.makeKey() },
      resignKey: { $0.resignKey() })
  }
}

/// The user went on somewhere the overlay does not see as its own event.
enum OverlayOutsideInteraction: String {
  /// A mouse button went down in another app's window, menu bar or Dock.
  case click = "click-in-another-app"
  /// Another app became active (⌘Tab, a Dock click, an app opening a window).
  case activation = "another-app-activated"
}

/// Reports the user leaving the overlay for another app while its keyboard
/// gate is open. One handler at a time; `stop()` removes every observer.
@MainActor
protocol OverlayOutsideInteractionSource: AnyObject {
  var isWatching: Bool { get }
  func start(_ handler: @escaping @MainActor (OverlayOutsideInteraction) -> Void)
  func stop()
}

/// Production source: a global monitor for mouse-down events delivered to
/// other apps (mouse events need no accessibility trust) and workspace
/// activation of any app other than Codescribe.
@MainActor
final class OverlayGlobalOutsideInteractions: OverlayOutsideInteractionSource {
  private var monitor: Any?
  private var activationObserver: NSObjectProtocol?
  private var handler: (@MainActor (OverlayOutsideInteraction) -> Void)?

  var isWatching: Bool { monitor != nil || activationObserver != nil }

  func start(_ handler: @escaping @MainActor (OverlayOutsideInteraction) -> Void) {
    self.handler = handler
    if monitor == nil {
      monitor = NSEvent.addGlobalMonitorForEvents(
        matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown]
      ) { [weak self] _ in
        Task { @MainActor in self?.deliver(.click) }
      }
    }
    if activationObserver == nil {
      let own = ProcessInfo.processInfo.processIdentifier
      activationObserver = NSWorkspace.shared.notificationCenter.addObserver(
        forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
      ) { [weak self] notification in
        let app =
          notification.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
        guard app?.processIdentifier != own else { return }
        Task { @MainActor in self?.deliver(.activation) }
      }
    }
  }

  func stop() {
    handler = nil
    if let monitor { NSEvent.removeMonitor(monitor) }
    if let activationObserver {
      NSWorkspace.shared.notificationCenter.removeObserver(activationObserver)
    }
    monitor = nil
    activationObserver = nil
  }

  private func deliver(_ interaction: OverlayOutsideInteraction) {
    handler?(interaction)
  }
}
