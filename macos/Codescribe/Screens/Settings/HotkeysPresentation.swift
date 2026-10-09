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

  /// What the mode does with the user's voice, in one short clause.
  ///
  /// None of the three promises a paste: `PASTE_MODE` can be `off`, so the
  /// Automatic paste section below is the only place that promise is made.
  var blurb: String {
    switch self {
    case .dictation:
      return String(
        localized: "hotkeys.mode.dictation.blurb",
        defaultValue: "Turns speech into text.",
        comment: "Work mode description on the Shortcuts screen")
    case .formatting:
      return String(
        localized: "hotkeys.mode.formatting.blurb",
        defaultValue: "Dictation with AI formatting.",
        comment: "Work mode description on the Shortcuts screen")
    case .assistive:
      return String(
        localized: "hotkeys.mode.agent.blurb",
        defaultValue: "Passes the recognized text to the Agent.",
        comment: "Work mode description on the Shortcuts screen")
    }
  }
}

extension CsShortcutBinding {
  /// Human-readable gesture, shown in the binding pill, the gesture menu and
  /// the conflict list. Written the way the gesture is performed ("Hold",
  /// "2×"), with the key cap next to the key's plain name, so left and right
  /// Option stay distinguishable at a glance.
  var visibleName: String {
    switch self {
    case .disabled:
      return String(
        localized: "hotkeys.binding.disabled", defaultValue: "Disabled",
        comment: "Gesture option: the mode has no shortcut")
    case .holdFn:
      return String(
        localized: "hotkeys.binding.holdFn", defaultValue: "Hold Fn",
        comment: "Gesture option")
    case .holdCtrl:
      return String(
        localized: "hotkeys.binding.holdCtrl", defaultValue: "Hold ⌃ (Ctrl)",
        comment: "Gesture option")
    case .holdCtrlAlt:
      return String(
        localized: "hotkeys.binding.holdCtrlOption", defaultValue: "Hold ⌃⌥ (Ctrl+Option)",
        comment: "Gesture option")
    case .holdCtrlShift:
      return String(
        localized: "hotkeys.binding.holdCtrlShift", defaultValue: "Hold ⌃⇧ (Ctrl+Shift)",
        comment: "Gesture option")
    case .holdCtrlCmd:
      return String(
        localized: "hotkeys.binding.holdCtrlCommand", defaultValue: "Hold ⌃⌘ (Ctrl+Command)",
        comment: "Gesture option")
    case .doubleCtrl:
      return String(
        localized: "hotkeys.binding.doubleCtrl", defaultValue: "2× ⌃ (Ctrl)",
        comment: "Gesture option: tap the key twice")
    case .doubleLeftOption:
      return String(
        localized: "hotkeys.binding.doubleLeftOption", defaultValue: "2× Left ⌥ (Option)",
        comment: "Gesture option: tap the left Option key twice")
    case .doubleRightOption:
      return String(
        localized: "hotkeys.binding.doubleRightOption", defaultValue: "2× Right ⌥ (Option)",
        comment: "Gesture option: tap the right Option key twice")
    }
  }

  /// The same gesture spelled out for VoiceOver. `visibleName` is deliberately
  /// terse and carries key caps, which a screen reader skips or reads as
  /// punctuation; this one names every key in words and still keeps left and
  /// right Option apart.
  var spokenName: String {
    switch self {
    case .disabled:
      return String(
        localized: "hotkeys.binding.disabled.spoken", defaultValue: "No shortcut",
        comment: "VoiceOver reading of the gesture option: the mode has no shortcut")
    case .holdFn:
      return String(
        localized: "hotkeys.binding.holdFn.spoken",
        defaultValue: "Hold the Fn key, also called Globe",
        comment: "VoiceOver reading of a gesture option")
    case .holdCtrl:
      return String(
        localized: "hotkeys.binding.holdCtrl.spoken", defaultValue: "Hold the Control key",
        comment: "VoiceOver reading of a gesture option")
    case .holdCtrlAlt:
      return String(
        localized: "hotkeys.binding.holdCtrlOption.spoken",
        defaultValue: "Hold Control and Option",
        comment: "VoiceOver reading of a gesture option")
    case .holdCtrlShift:
      return String(
        localized: "hotkeys.binding.holdCtrlShift.spoken",
        defaultValue: "Hold Control and Shift",
        comment: "VoiceOver reading of a gesture option")
    case .holdCtrlCmd:
      return String(
        localized: "hotkeys.binding.holdCtrlCommand.spoken",
        defaultValue: "Hold Control and Command",
        comment: "VoiceOver reading of a gesture option")
    case .doubleCtrl:
      return String(
        localized: "hotkeys.binding.doubleCtrl.spoken",
        defaultValue: "Tap the Control key twice",
        comment: "VoiceOver reading of a gesture option")
    case .doubleLeftOption:
      return String(
        localized: "hotkeys.binding.doubleLeftOption.spoken",
        defaultValue: "Tap the left Option key twice",
        comment: "VoiceOver reading of a gesture option")
    case .doubleRightOption:
      return String(
        localized: "hotkeys.binding.doubleRightOption.spoken",
        defaultValue: "Tap the right Option key twice",
        comment: "VoiceOver reading of a gesture option")
    }
  }
}

/// One validation result as the Shortcuts panel shows it.
///
/// `blocking` keeps the core's meaning: a blocking entry refuses the save, a
/// non-blocking one is a note about the environment and must never be painted
/// as an error. `technical` carries the wire strings whenever the shown
/// sentence is not the one Rust sent, so the identifier stays recoverable from
/// the screen without the user having to read English prose.
struct HotkeyConflictPresentation: Equatable {
  let gesture: String
  let message: String
  let technical: String?
  let blocking: Bool
}

/// The exact sentences `app/os/shortcut_registry.rs` emits. They are wire
/// values, not copy: Swift switches on them to pick its own sentence and never
/// shows them as the headline. A wording change on the Rust side falls through
/// to the verbatim `message` instead of being mistranslated.
private enum HotkeyConflictWire {
  static let dictationDoubleCtrlBlocksLeftOption =
    "Dictation is set to Double Ctrl, so Left Option toggle is disabled."
  static let dictationDoubleCtrlBlocksRightOption =
    "Dictation is set to Double Ctrl, so Right Option toggle is disabled."
  /// `unreachable_binding_message`: the detector routes this gesture to one
  /// other mode only, so the mode it is bound to would never start.
  static let onlyDictationNotFormatting =
    "This gesture only starts Dictation, so Formatting would never start from it."
  static let onlyDictationNotAssistive =
    "This gesture only starts Dictation, so Assistive would never start from it."
  static let onlyFormattingNotDictation =
    "This gesture only starts Formatting, so Dictation would never start from it."
  static let onlyFormattingNotAssistive =
    "This gesture only starts Formatting, so Assistive would never start from it."
  static let onlyAssistiveNotDictation =
    "This gesture only starts Assistive, so Dictation would never start from it."
  static let onlyAssistiveNotFormatting =
    "This gesture only starts Assistive, so Formatting would never start from it."
  /// `format!("Conflicts with {} (macOS #{}).", …)` — the tail names the system
  /// shortcut and its numeric id, so it belongs in `technical`, not the sentence.
  static let macosSymbolicPrefix = "Conflicts with "
}

extension CsHotkeyConflict {
  /// The conflict names its gesture by the core wire label; show the same
  /// gesture the way the binding pills do. Unknown labels pass through.
  func visibleGesture(options: [CsBindingOption]) -> String {
    options.first { $0.label == gestureLabel }?.binding.visibleName ?? gestureLabel
  }

  /// Localize the sentence, keeping the wire strings available underneath.
  func presentation(options: [CsBindingOption]) -> HotkeyConflictPresentation {
    let localized = localizedMessage()
    return HotkeyConflictPresentation(
      gesture: visibleGesture(options: options),
      message: localized ?? message,
      technical: localized == nil ? nil : "\(gestureLabel) · \(message)",
      blocking: blocking
    )
  }

  /// `nil` when no mapping covers this wire message — the caller then shows it
  /// verbatim rather than inventing a sentence for it.
  private func localizedMessage() -> String? {
    // A non-blocking entry can only be `fn_tap_intercept_note`: every registry
    // conflict crosses the bridge as blocking (bridge/src/hotkeys.rs).
    if !blocking {
      return String(
        localized: "settings.shortcuts.note.fnTapOwnedByMacos",
        defaultValue:
          "The Fn key is also configured in macOS. Codescribe may intercept its short press while dictation is running. This note does not block saving the shortcuts.",
        comment: "Shortcuts screen: informational note about macOS, never an error")
    }
    switch message {
    case HotkeyConflictWire.dictationDoubleCtrlBlocksLeftOption:
      return String(
        localized: "settings.shortcuts.conflict.doubleCtrlBlocksLeftOption",
        defaultValue: "Dictation uses 2× Ctrl, so the left Option gesture cannot start a take.",
        comment: "Shortcuts screen: blocking conflict between two gestures")
    case HotkeyConflictWire.dictationDoubleCtrlBlocksRightOption:
      return String(
        localized: "settings.shortcuts.conflict.doubleCtrlBlocksRightOption",
        defaultValue: "Dictation uses 2× Ctrl, so the right Option gesture cannot start a take.",
        comment: "Shortcuts screen: blocking conflict between two gestures")
    case HotkeyConflictWire.onlyDictationNotFormatting:
      return String(
        localized: "settings.shortcuts.conflict.onlyDictationNotFormatting",
        defaultValue:
          "This gesture starts only Dictation, so Formatting would never start. Formatting starts with 2× Left ⌥.",
        comment: "Shortcuts screen: blocking conflict, the gesture is not available for this mode")
    case HotkeyConflictWire.onlyDictationNotAssistive:
      return String(
        localized: "settings.shortcuts.conflict.onlyDictationNotAgent",
        defaultValue:
          "This gesture starts only Dictation, so the Agent would never start. The Agent starts with 2× Right ⌥.",
        comment: "Shortcuts screen: blocking conflict, the gesture is not available for this mode")
    case HotkeyConflictWire.onlyFormattingNotDictation:
      return String(
        localized: "settings.shortcuts.conflict.onlyFormattingNotDictation",
        defaultValue:
          "2× Left ⌥ starts only Formatting, so Dictation would never start. Dictation starts with a hold or 2× Ctrl.",
        comment: "Shortcuts screen: blocking conflict, the gesture is not available for this mode")
    case HotkeyConflictWire.onlyFormattingNotAssistive:
      return String(
        localized: "settings.shortcuts.conflict.onlyFormattingNotAgent",
        defaultValue:
          "2× Left ⌥ starts only Formatting, so the Agent would never start. The Agent starts with 2× Right ⌥.",
        comment: "Shortcuts screen: blocking conflict, the gesture is not available for this mode")
    case HotkeyConflictWire.onlyAssistiveNotDictation:
      return String(
        localized: "settings.shortcuts.conflict.onlyAgentNotDictation",
        defaultValue:
          "2× Right ⌥ starts only the Agent, so Dictation would never start. Dictation starts with a hold or 2× Ctrl.",
        comment: "Shortcuts screen: blocking conflict, the gesture is not available for this mode")
    case HotkeyConflictWire.onlyAssistiveNotFormatting:
      return String(
        localized: "settings.shortcuts.conflict.onlyAgentNotFormatting",
        defaultValue:
          "2× Right ⌥ starts only the Agent, so Formatting would never start. Formatting starts with 2× Left ⌥.",
        comment: "Shortcuts screen: blocking conflict, the gesture is not available for this mode")
    default:
      guard message.hasPrefix(HotkeyConflictWire.macosSymbolicPrefix) else { return nil }
      return String(
        localized: "settings.shortcuts.conflict.macosSystemShortcut",
        defaultValue: "This gesture is already taken by a macOS system shortcut.",
        comment: "Shortcuts screen: blocking conflict with a macOS keyboard shortcut")
    }
  }
}
