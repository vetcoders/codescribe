import Foundation

// Receipt for the one Shortcuts control that is NOT autosaved: the three
// per-mode gestures are a draft and need an explicit save.
//
// The receipt is built from disk, never from the draft. `set_mode_binding`
// persists through `UserSettings::save_if_changed`, which only *warns* when the
// write fails (core/config/settings.rs), and the bridge rejects some
// mode/gesture pairs outright (bridge/src/hotkeys.rs) — so neither a returned
// `Ok` nor the absence of a thrown error proves anything landed. The only
// honest confirmation is a re-read, compared mode by mode against what was
// requested.

/// What the last save actually achieved, as read back from persisted settings.
struct HotkeyBindingSaveReceipt: Equatable {
  /// Modes whose requested gesture is now the persisted gesture.
  let saved: [CsWorkMode]
  /// Modes that were requested and are still not persisted.
  let rejected: [CsWorkMode]
  /// First rejection reported by the engine, verbatim. Kept as the technical
  /// detail line; it is wire text, not copy.
  let failureDetail: String?

  var isEmpty: Bool { saved.isEmpty && rejected.isEmpty }
  var hasRejection: Bool { !rejected.isEmpty }

  /// The persisted outcome for every requested mode. `nil` when there is
  /// nothing to report.
  ///
  /// A partial save is the two halves side by side rather than a third
  /// sentence: one list per string keeps every localization to a single
  /// placeholder, and a translator never has to keep two of them in order.
  var sentence: String? {
    let parts = [savedSentence, rejectedSentence].compactMap { $0 }
    return parts.isEmpty ? nil : parts.joined(separator: " ")
  }

  private var savedSentence: String? {
    guard !saved.isEmpty else { return nil }
    let names = saved.map(\.visibleName).formatted(.list(type: .and))
    return String(
      localized: "settings.shortcuts.save.saved",
      defaultValue: "Saved and in effect: \(names).",
      comment: "Shortcuts screen: the placeholder lists work mode names, e.g. Dictation and Agent"
    )
  }

  private var rejectedSentence: String? {
    guard !rejected.isEmpty else { return nil }
    let names = rejected.map(\.visibleName).formatted(.list(type: .and))
    return String(
      localized: "settings.shortcuts.save.rejected",
      defaultValue: "Not saved: \(names). The gesture in effect is unchanged.",
      comment: "Shortcuts screen: the placeholder lists work mode names, e.g. Dictation and Agent"
    )
  }
}
