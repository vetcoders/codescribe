import Foundation

// Presentation seam for the Shortcuts screen. Core sends the work mode and the
// gesture as enums (`CsWorkMode`, `CsShortcutBinding`); every sentence the user
// reads is composed here so it localizes with the rest of the app. The
// `modeLabel` / `modeDescription` / `bindingLabel` strings carried by
// `CsModeBinding` are the English wire presentation and are not shown.

extension CsWorkMode {
  /// Product name of the mode. The agent lane is "Agent" in product language.
  var visibleName: String {
    switch self {
    case .dictation:
      return String(
        localized: "hotkeys.mode.dictation", defaultValue: "Dictation",
        comment: "Work mode name on the Shortcuts screen")
    case .formatting:
      return String(
        localized: "hotkeys.mode.formatting", defaultValue: "Formatting",
        comment: "Work mode name on the Shortcuts screen")
    case .assistive:
      return String(
        localized: "hotkeys.mode.agent", defaultValue: "Agent",
        comment: "Work mode name on the Shortcuts screen: voice goes to the agent")
    }
  }

  /// One sentence on what the mode does with the user's voice.
  var blurb: String {
    switch self {
    case .dictation:
      return String(
        localized: "hotkeys.mode.dictation.blurb",
        defaultValue: "Transcribes your voice and pastes the text.",
        comment: "Work mode description on the Shortcuts screen")
    case .formatting:
      return String(
        localized: "hotkeys.mode.formatting.blurb",
        defaultValue: "Records dictation, then formats it before pasting.",
        comment: "Work mode description on the Shortcuts screen")
    case .assistive:
      return String(
        localized: "hotkeys.mode.agent.blurb",
        defaultValue: "Sends your voice to the Agent instead of pasting.",
        comment: "Work mode description on the Shortcuts screen")
    }
  }
}

extension CsShortcutBinding {
  /// Human-readable gesture, shown in the binding pill, the gesture menu and
  /// the conflict list.
  var visibleName: String {
    switch self {
    case .disabled:
      return String(
        localized: "hotkeys.binding.disabled", defaultValue: "Disabled",
        comment: "Gesture option: the mode has no shortcut")
    case .holdFn:
      return String(
        localized: "hotkeys.binding.holdFn", defaultValue: "Hold Fn/Globe",
        comment: "Gesture option")
    case .holdCtrl:
      return String(
        localized: "hotkeys.binding.holdCtrl", defaultValue: "Hold Ctrl",
        comment: "Gesture option")
    case .holdCtrlAlt:
      return String(
        localized: "hotkeys.binding.holdCtrlOption", defaultValue: "Hold Ctrl+Option",
        comment: "Gesture option")
    case .holdCtrlShift:
      return String(
        localized: "hotkeys.binding.holdCtrlShift", defaultValue: "Hold Ctrl+Shift",
        comment: "Gesture option")
    case .holdCtrlCmd:
      return String(
        localized: "hotkeys.binding.holdCtrlCommand", defaultValue: "Hold Ctrl+Command",
        comment: "Gesture option")
    case .doubleCtrl:
      return String(
        localized: "hotkeys.binding.doubleCtrl", defaultValue: "Double-tap Ctrl",
        comment: "Gesture option")
    case .doubleLeftOption:
      return String(
        localized: "hotkeys.binding.doubleLeftOption", defaultValue: "Double-tap Left Option",
        comment: "Gesture option")
    case .doubleRightOption:
      return String(
        localized: "hotkeys.binding.doubleRightOption", defaultValue: "Double-tap Right Option",
        comment: "Gesture option")
    }
  }
}

extension CsHotkeyConflict {
  /// The conflict names its gesture by the core wire label; show the same
  /// gesture the way the binding pills do. Unknown labels pass through.
  func visibleGesture(options: [CsBindingOption]) -> String {
    options.first { $0.label == gestureLabel }?.binding.visibleName ?? gestureLabel
  }
}
