import AppKit

/// Readline chords that macOS leaves unbound in `NSTextView`, resolved the
/// same way in both composers (Agent Chat and the Intercom card).
///
/// AppKit's `StandardKeyBinding.dict` already answers `^A ^E ^F ^B ^N ^P ^D ^H
/// ^K ^Y ^T ^O` and `⌥⌫` in every text view, so those never reach this table.
/// What a terminal user still misses is exactly this list: `^J` (newline — the
/// line feed itself), `^U`, `^W`, `⌥B`, `⌥F`, `⌥D` and `^_`. Each chord maps to
/// an `NSTextView` editing selector, never to hand-written string surgery, so
/// undo, selection and the input method behave as they do everywhere else.
///
/// Option chords are matched on `charactersIgnoringModifiers`: on Polish Pro
/// `⌥A ⌥C ⌥E ⌥L ⌥N ⌥O ⌥S ⌥Z ⌥X` type the diacritics and are not in this
/// table, so `⌥B ⌥F ⌥D` (∫ ƒ ∂ otherwise) are the only keys taken over.
enum ComposerTerminalChord: Equatable, CaseIterable {
  /// `^J` — insert a line break without sending, like Shift+Return.
  case insertNewline
  /// `^U` — delete back to the start of the current paragraph.
  case deleteToParagraphStart
  /// `^W` — delete the word before the caret.
  case deleteWordBackward
  /// `⌥D` — delete the word after the caret.
  case deleteWordForward
  /// `⌥B` — move the caret one word back.
  case moveWordBackward
  /// `⌥F` — move the caret one word forward.
  case moveWordForward
  /// `^_` — undo the last typing.
  case undo

  /// `characters` is the event's `charactersIgnoringModifiers`. Composing text
  /// (IME) is never interrupted; Command anywhere in the chord stays native.
  static func resolve(
    characters: String?,
    modifiers: NSEvent.ModifierFlags,
    hasMarkedText: Bool
  ) -> ComposerTerminalChord? {
    guard !hasMarkedText, let key = characters?.lowercased(), key.count == 1 else { return nil }
    let flags = modifiers.intersection([.command, .control, .option, .shift])
    switch (flags, key) {
    case (.control, "j"): return .insertNewline
    case (.control, "u"): return .deleteToParagraphStart
    case (.control, "w"): return .deleteWordBackward
    case (.option, "d"): return .deleteWordForward
    case (.option, "b"): return .moveWordBackward
    case (.option, "f"): return .moveWordForward
    // `_` arrives with Shift on most layouts; both spellings are the readline undo.
    case (.control, "_"), ([.control, .shift], "_"): return .undo
    default: return nil
    }
  }

  @MainActor
  func perform(on textView: NSTextView) {
    switch self {
    case .insertNewline: textView.insertNewline(nil)
    case .deleteToParagraphStart: textView.deleteToBeginningOfParagraph(nil)
    case .deleteWordBackward: textView.deleteWordBackward(nil)
    case .deleteWordForward: textView.deleteWordForward(nil)
    case .moveWordBackward: textView.moveWordBackward(nil)
    case .moveWordForward: textView.moveWordForward(nil)
    case .undo: textView.undoManager?.undo()
    }
  }
}
