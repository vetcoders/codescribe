import AppKit
import SwiftUI

/// The user's chord that shows or hides the collapsed widget (`WIDGET_SHORTCUT`).
/// Mirrors core `WidgetShortcut` (core/config/types.rs): the same wire id
/// (`ctrl+shift+b`), the same glyph label (`⌃⇧B`), the same rule — any of
/// Control, Option, Command and Shift plus one letter or Space, at least one of
/// Control, Option or Command, and never the Agent summon `⌘⇧Space`.
struct WidgetShortcutChord: Equatable, Sendable {
  enum Key: Equatable, Sendable {
    /// ASCII uppercase letter.
    case letter(Character)
    case space

    var wireToken: String {
      switch self {
      case .space: "space"
      case .letter(let letter): String(letter).lowercased()
      }
    }

    var glyph: String {
      switch self {
      case .space: "Space"
      case .letter(let letter): String(letter)
      }
    }
  }

  /// Why a chord cannot be used; the same reasons the core refuses.
  enum Refusal: Equatable, Sendable {
    case needsCommandModifier
    case noKey
    case agentSummon

    var message: String {
      switch self {
      case .needsCommandModifier:
        String(
          localized: "Add Control, Option or Command: a plain or Shift-only key would type.",
          comment: "Widget shortcut recorder refusal")
      case .noKey:
        String(
          localized: "End the chord with a letter or Space.",
          comment: "Widget shortcut recorder refusal")
      case .agentSummon:
        String(
          localized: "Command+Shift+Space already opens the Agent window.",
          comment: "Widget shortcut recorder refusal")
      }
    }
  }

  var ctrl: Bool
  var option: Bool
  var cmd: Bool
  var shift: Bool
  var key: Key

  static let `default` = WidgetShortcutChord(
    ctrl: true, option: false, cmd: false, shift: true, key: .letter("B"))

  /// Canonical wire id: modifiers in the order ctrl, option, cmd, shift, then the key.
  var wireId: String {
    var parts: [String] = []
    if ctrl { parts.append("ctrl") }
    if option { parts.append("option") }
    if cmd { parts.append("cmd") }
    if shift { parts.append("shift") }
    parts.append(key.wireToken)
    return parts.joined(separator: "+")
  }

  /// macOS glyph order: Control, Option, Shift, Command.
  var label: String {
    var label = ""
    if ctrl { label += "⌃" }
    if option { label += "⌥" }
    if shift { label += "⇧" }
    if cmd { label += "⌘" }
    return label + key.glyph
  }

  /// Spelled out for VoiceOver, e.g. "Control Shift B".
  var spokenName: String {
    var words: [String] = []
    if ctrl { words.append(String(localized: "Control", comment: "Modifier key name")) }
    if option { words.append(String(localized: "Option", comment: "Modifier key name")) }
    if shift { words.append(String(localized: "Shift", comment: "Modifier key name")) }
    if cmd { words.append(String(localized: "Command", comment: "Modifier key name")) }
    switch key {
    case .space: words.append(String(localized: "Space", comment: "Key name"))
    case .letter(let letter): words.append(String(letter))
    }
    return words.joined(separator: " ")
  }

  var refusal: Refusal? {
    if !(ctrl || option || cmd) { return .needsCommandModifier }
    if cmd && shift && !ctrl && !option && key == .space { return .agentSummon }
    return nil
  }

  /// Parses a wire id the core wrote; `nil` for anything the core would refuse.
  init?(wireId: String) {
    var chord = WidgetShortcutChord(ctrl: false, option: false, cmd: false, shift: false, key: .space)
    var key: Key?
    for token in wireId.split(separator: "+").map({ $0.trimmingCharacters(in: .whitespaces).lowercased() }) {
      switch token {
      case "ctrl", "control": chord.ctrl = true
      case "option", "alt", "opt": chord.option = true
      case "cmd", "command": chord.cmd = true
      case "shift": chord.shift = true
      case "space": key = .space
      default:
        guard token.count == 1, let scalar = token.unicodeScalars.first,
          scalar.value >= 0x61, scalar.value <= 0x7A
        else { return nil }
        key = .letter(Character(token.uppercased()))
      }
    }
    guard let key else { return nil }
    chord.key = key
    guard chord.refusal == nil else { return nil }
    self = chord
  }

  init(ctrl: Bool, option: Bool, cmd: Bool, shift: Bool, key: Key) {
    self.ctrl = ctrl
    self.option = option
    self.cmd = cmd
    self.shift = shift
    self.key = key
  }

  /// Reads one key press the way the recorder sees it: the held modifiers and
  /// the key's virtual keycode (Space is 49; letters follow the ANSI layout, so
  /// the chord is the physical key, not the typed character). Returns `nil` for
  /// a key the chord may not end with (digits, Return, function keys...).
  static func from(modifierFlags: NSEvent.ModifierFlags, keyCode: UInt16) -> WidgetShortcutChord? {
    guard let key = Key(keyCode: keyCode) else { return nil }
    return WidgetShortcutChord(
      ctrl: modifierFlags.contains(.control),
      option: modifierFlags.contains(.option),
      cmd: modifierFlags.contains(.command),
      shift: modifierFlags.contains(.shift),
      key: key)
  }
}

extension WidgetShortcutChord.Key {
  /// Carbon ANSI keycodes for the letters, lockstep with
  /// `WidgetShortcutKey::keycode` in core/config/types.rs.
  private static let letterKeycodes: [UInt16: Character] = [
    0: "A", 1: "S", 2: "D", 3: "F", 4: "H", 5: "G", 6: "Z", 7: "X", 8: "C", 9: "V",
    11: "B", 12: "Q", 13: "W", 14: "E", 15: "R", 16: "Y", 17: "T", 31: "O", 32: "U",
    34: "I", 35: "P", 37: "L", 38: "J", 40: "K", 45: "N", 46: "M",
  ]

  init?(keyCode: UInt16) {
    if keyCode == 49 {
      self = .space
    } else if let letter = Self.letterKeycodes[keyCode] {
      self = .letter(letter)
    } else {
      return nil
    }
  }
}

/// The chord pill in Settings › Shortcuts: shows the current chord; a click
/// arms it and the next key press becomes the chord. Escape cancels. A press
/// the core would refuse keeps the old chord and says why under the pill.
struct WidgetShortcutRecorderPill: View {
  let chord: WidgetShortcutChord
  let onRecorded: (WidgetShortcutChord) -> Void

  @State private var recording = false
  @State private var refusal: WidgetShortcutChord.Refusal?
  @State private var monitor: Any?

  var body: some View {
    VStack(alignment: .trailing, spacing: 4) {
      Button {
        recording ? stopRecording() : startRecording()
      } label: {
        Text(verbatim: recording ? String(localized: "Press keys…", comment: "Widget shortcut recorder is waiting for a chord") : chord.label)
          .font(SettingsScale.monoValue)
          .foregroundStyle(recording ? CSColor.secondaryInk : CSColor.terracottaText)
          .padding(.horizontal, SettingsScale.controlInsetH)
          .padding(.vertical, SettingsScale.controlInsetV)
          .background(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
              .fill(Color.primary.opacity(recording ? 0.14 : 0.08))
          )
          .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
              .strokeBorder(
                recording ? CSColor.terracotta.opacity(0.6) : Color.primary.opacity(0.12),
                lineWidth: 1)
          )
      }
      .buttonStyle(.plain)
      .fixedSize()
      .accessibilityLabel(Text("Widget shortcut"))
      .accessibilityValue(Text(chord.spokenName))
      .accessibilityHint(Text("Activate, then press the chord you want."))
      .accessibilityIdentifier("settings-widget-shortcut")
      if let refusal {
        Text(refusal.message)
          .font(SettingsScale.helper)
          .foregroundStyle(CSColor.amberLight)
          .multilineTextAlignment(.trailing)
          .fixedSize(horizontal: false, vertical: true)
      }
    }
    .onDisappear(perform: stopRecording)
  }

  private func startRecording() {
    refusal = nil
    recording = true
    monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
      let keyCode = event.keyCode
      let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
      // Local key monitors always deliver on the main thread; assume the
      // main actor only after the non-Sendable NSEvent has been reduced to
      // Sendable value fields (the composer's ⌘V monitor does the same).
      MainActor.assumeIsolated { handle(keyCode: keyCode, flags: flags) }
      return nil
    }
  }

  private func stopRecording() {
    if let monitor { NSEvent.removeMonitor(monitor) }
    monitor = nil
    recording = false
  }

  /// A press ends the recording either way: Escape keeps the old chord, a
  /// refused chord keeps it and explains, an accepted chord is persisted.
  @MainActor
  private func handle(keyCode: UInt16, flags: NSEvent.ModifierFlags) {
    stopRecording()
    if keyCode == 53 { return }
    guard let candidate = WidgetShortcutChord.from(modifierFlags: flags, keyCode: keyCode) else {
      refusal = .noKey
      return
    }
    if let why = candidate.refusal {
      refusal = why
      return
    }
    onRecorded(candidate)
  }
}
