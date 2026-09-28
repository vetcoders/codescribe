import Foundation

// Seam between the Settings screen and the REAL codescribe hotkey mode-binding
// surface through the UniFFI bridge (CodescribeHotkeys). Mirrors the
// SettingsEngine / AgentStatusEngine seams so #Preview can inject deterministic
// data while the live app injects `RealHotkeysEngine`.
//
// The hotkey ENGINE (seed-at-launch + live-reload) already exists — this seam is
// only the Settings EDITOR: read per-mode bindings, validate a candidate for
// conflicts, and persist. Writes route through the core's canonical
// `set_mode_binding` and re-apply the detector atomics, so a change takes effect
// on the running CGEventTap without an app restart. All bridge calls here are
// synchronous, cheap on-disk reads/writes.

/// Read/write mode-binding surface the Shortcuts panel consumes.
protocol HotkeysEngine {
  /// Current per-mode bindings (Dictation / Formatting / Assistive), normalized.
  func modeBindings() -> [CsModeBinding]
  /// The closed set of selectable gestures (with labels) for the picker.
  func availableBindings() -> [CsBindingOption]
  /// Persist one mode's binding and live-reload the detector.
  func setModeBinding(mode: CsWorkMode, binding: CsShortcutBinding) throws
  /// Clear all custom bindings back to the built-in defaults.
  func resetToDefaults() throws
  /// Validate a candidate set WITHOUT persisting; returns detected conflicts.
  func validate(candidate: [CsModeBinding]) -> [CsHotkeyConflict]
  /// Re-arm the global CGEventTap after a first-run permission grant so hotkeys
  /// go live without an app restart. Idempotent — safe on every Refresh.
  func rearmAfterPermissionGrant()
  /// Agent-channel modifier: `"ctrl"` (default) or `"fn"`. Command is not a value.
  func channelModifier() -> String
  /// Persist the channel modifier and live-reload the detector. `"cmd"` is rejected.
  func setChannelModifier(_ value: String) throws
  /// Whether a quick Fn press below the hold delay toggles dictation. Default off.
  func fnTapTogglesDictation() -> Bool
  /// Persist the Fn-tap dictation toggle and live-reload the detector.
  func setFnTapTogglesDictation(_ enabled: Bool) throws
  /// Whether mouse button 2 follows Fn press and release. Default off.
  func middleMouseActsAsFn() -> Bool
  /// Persist the middle-mouse Fn option and live-reload the detector.
  func setMiddleMouseActsAsFn(_ enabled: Bool) throws
}

// MARK: - Real engine (UniFFI bridge adapter)

/// Concrete adapter over the `CodescribeHotkeys` bridge object. Stateless: every
/// call reads or writes live on-disk truth. A fresh handle is safe — the runtime
/// listener state lives in process-global statics, and binding reads/writes go
/// through settings.json.
final class RealHotkeysEngine: HotkeysEngine {
  private let hotkeys = CodescribeHotkeys()
  private let config = CodescribeConfig()

  func modeBindings() -> [CsModeBinding] { hotkeys.getModeBindings() }
  func availableBindings() -> [CsBindingOption] { hotkeys.availableBindings() }
  func setModeBinding(mode: CsWorkMode, binding: CsShortcutBinding) throws {
    try hotkeys.setModeBinding(mode: mode, binding: binding)
  }
  func resetToDefaults() throws { try hotkeys.resetBindingsToDefaults() }
  func validate(candidate: [CsModeBinding]) -> [CsHotkeyConflict] {
    hotkeys.validateBindings(candidate: candidate)
  }

  func rearmAfterPermissionGrant() {
    // Bridge call is idempotent and returns whether hotkeys are live; the UI
    // reflects live status through the native permission probe, so the result
    // is intentionally discarded here.
    _ = hotkeys.rearmAfterPermissionGrant()
  }

  func channelModifier() -> String {
    normalizeChannelModifier(config.loadSettings().channelModifier)
  }

  func setChannelModifier(_ value: String) throws {
    try config.updateConfig(
      key: "AGENT_CHANNEL_MODIFIER", value: normalizeChannelModifier(value))
  }

  func fnTapTogglesDictation() -> Bool { config.loadSettings().fnTapTogglesDictation }

  func setFnTapTogglesDictation(_ enabled: Bool) throws {
    try config.updateConfig(key: "FN_TAP_TOGGLES_DICTATION", value: enabled ? "1" : "0")
  }

  func middleMouseActsAsFn() -> Bool { config.loadSettings().middleMouseActsAsFn }

  func setMiddleMouseActsAsFn(_ enabled: Bool) throws {
    try config.updateConfig(key: "MIDDLE_MOUSE_ACTS_AS_FN", value: enabled ? "1" : "0")
  }
}

/// `"fn"` stays Fn. Every other token, including Command, is Ctrl.
private func normalizeChannelModifier(_ value: String) -> String {
  value.lowercased() == "fn" ? "fn" : "ctrl"
}

// MARK: - Mock engine (previews)

/// In-memory stand-in for #Preview. Writes are no-ops; the view-model updates its
/// own draft optimistically so the picker still feels live in previews.
struct MockHotkeysEngine: HotkeysEngine {
  var bindings: [CsModeBinding] = CsModeBinding.sampleBindings

  func modeBindings() -> [CsModeBinding] { bindings }
  func availableBindings() -> [CsBindingOption] { CsBindingOption.sampleOptions }
  func setModeBinding(mode: CsWorkMode, binding: CsShortcutBinding) throws {}
  func resetToDefaults() throws {}
  func rearmAfterPermissionGrant() {}
  func channelModifier() -> String { "ctrl" }
  func setChannelModifier(_ value: String) throws {}
  func fnTapTogglesDictation() -> Bool { false }
  func setFnTapTogglesDictation(_ enabled: Bool) throws {}
  func middleMouseActsAsFn() -> Bool { false }
  func setMiddleMouseActsAsFn(_ enabled: Bool) throws {}
  func validate(candidate: [CsModeBinding]) -> [CsHotkeyConflict] {
    // Surface a representative blocking conflict when dictation double-taps Ctrl
    // while a toggle mode is also active — matches the core reachability rule.
    let dictation = candidate.first { $0.mode == .dictation }?.binding
    let formatting = candidate.first { $0.mode == .formatting }?.binding
    guard dictation == .doubleCtrl, formatting == .doubleLeftOption else { return [] }
    return [
      CsHotkeyConflict(
        gestureLabel: "Double-tap Left Option",
        message: "Dictation is set to Double Ctrl, so Left Option toggle is disabled.",
        blocking: true
      )
    ]
  }
}

// MARK: - Bridge value helpers (preview seeds)

extension CsModeBinding {
  /// Default binding set (Dictation=Hold Fn, Formatting=Double Left Option,
  /// Assistive=Double Right Option) — preview seed matching the core defaults.
  static let sampleBindings: [CsModeBinding] = [
    CsModeBinding(
      mode: .dictation,
      modeLabel: "Dictation",
      modeDescription: "Transcribes your voice and pastes the text.",
      binding: .holdFn,
      bindingLabel: "Hold Fn/Globe"
    ),
    CsModeBinding(
      mode: .formatting,
      modeLabel: "Formatting",
      modeDescription: "Records dictation, then formats it before pasting.",
      binding: .doubleLeftOption,
      bindingLabel: "Double-tap Left Option"
    ),
    CsModeBinding(
      mode: .assistive,
      modeLabel: "Assistive",
      modeDescription: "Sends your voice to the agent instead of pasting.",
      binding: .doubleRightOption,
      bindingLabel: "Double-tap Right Option"
    ),
  ]
}

extension CsBindingOption {
  /// The closed gesture set (mirrors `ShortcutBinding`), preview seed.
  static let sampleOptions: [CsBindingOption] = [
    CsBindingOption(binding: .disabled, label: "Disabled"),
    CsBindingOption(binding: .holdFn, label: "Hold Fn/Globe"),
    CsBindingOption(binding: .holdCtrl, label: "Hold Ctrl"),
    CsBindingOption(binding: .holdCtrlAlt, label: "Hold Ctrl+Option"),
    CsBindingOption(binding: .holdCtrlShift, label: "Hold Ctrl+Shift"),
    CsBindingOption(binding: .holdCtrlCmd, label: "Hold Ctrl+Command"),
    CsBindingOption(binding: .doubleCtrl, label: "Double-tap Ctrl"),
    CsBindingOption(binding: .doubleLeftOption, label: "Double-tap Left Option"),
    CsBindingOption(binding: .doubleRightOption, label: "Double-tap Right Option"),
  ]
}
