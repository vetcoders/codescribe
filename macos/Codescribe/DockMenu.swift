import AppKit

/// What the Dock menu shows above the Show All Windows / Hide / Quit items
/// AppKit appends on its own: the tray panel's primary actions, in its order
/// (Human Interface Guidelines, Dock menus: app-specific items first, then a
/// separator the system draws).
struct DockMenuState: Equatable {
  var agentAvailable: Bool
  var isStartingDictation: Bool
  var isRecording: Bool
  var assistiveRecording: Bool
  var startInAssistive: Bool
}

enum DockMenuEntry: Int, CaseIterable {
  case agent
  case widget
  case dictation
  case settings

  var title: String {
    switch self {
    case .agent: String(localized: "Agent")
    case .widget: String(localized: "Show widget")
    case .dictation: ""  // state-dependent; see `DockMenu.menu(for:handlers:)`
    case .settings: String(localized: "Settings…")
    }
  }
}

/// Builds the Dock menu for one `applicationDockMenu(_:)` call. AppKit asks
/// again on every Dock click, so the titles and enabled bits always reflect
/// the state at that moment; the handlers are the delegate's existing actions.
@MainActor
final class DockMenu: NSObject {
  private var handlers: [DockMenuEntry: @MainActor () -> Void] = [:]

  func menu(
    for state: DockMenuState,
    handlers: [DockMenuEntry: @MainActor () -> Void]
  ) -> NSMenu {
    self.handlers = handlers
    let menu = NSMenu()
    for entry in DockMenuEntry.allCases {
      let title =
        entry == .dictation
        ? DictationActionTitle.title(
          isStarting: state.isStartingDictation,
          isRecording: state.isRecording,
          assistiveRecording: state.assistiveRecording,
          startInAssistive: state.startInAssistive)
        : entry.title
      let item = NSMenuItem(title: title, action: #selector(performEntry(_:)), keyEquivalent: "")
      item.target = self
      item.tag = entry.rawValue
      switch entry {
      case .agent: item.isEnabled = state.agentAvailable
      case .dictation: item.isEnabled = !state.isStartingDictation
      case .widget, .settings: item.isEnabled = true
      }
      menu.addItem(item)
    }
    // Explicit: `isEnabled` is what the items say, not what `validateMenuItem`
    // would infer from the responder chain.
    menu.autoenablesItems = false
    return menu
  }

  @objc private func performEntry(_ sender: NSMenuItem) {
    guard let entry = DockMenuEntry(rawValue: sender.tag) else { return }
    handlers[entry]?()
  }
}
