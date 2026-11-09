import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Terminal (readline) chords in both composers. The table is pure; the two
/// behavioural tests drive the real text views with synthesized key events and
/// never present a window on the desktop.
@MainActor
final class ComposerTerminalChordTests: XCTestCase {
  // MARK: - Table

  func testTheSevenReadlineChordsResolve() {
    func chord(_ key: String, _ flags: NSEvent.ModifierFlags) -> ComposerTerminalChord? {
      ComposerTerminalChord.resolve(characters: key, modifiers: flags, hasMarkedText: false)
    }
    XCTAssertEqual(chord("j", .control), .insertNewline)
    XCTAssertEqual(chord("u", .control), .deleteToParagraphStart)
    XCTAssertEqual(chord("w", .control), .deleteWordBackward)
    XCTAssertEqual(chord("d", .option), .deleteWordForward)
    XCTAssertEqual(chord("b", .option), .moveWordBackward)
    XCTAssertEqual(chord("f", .option), .moveWordForward)
    XCTAssertEqual(chord("_", .control), .undo)
    XCTAssertEqual(chord("_", [.control, .shift]), .undo)
    // The key arrives in whatever case the layout reports; the table is case-blind.
    XCTAssertEqual(chord("J", .control), .insertNewline)
  }

  func testEverythingElseStaysNative() {
    func chord(_ key: String?, _ flags: NSEvent.ModifierFlags, marked: Bool = false)
      -> ComposerTerminalChord?
    {
      ComposerTerminalChord.resolve(characters: key, modifiers: flags, hasMarkedText: marked)
    }
    XCTAssertNil(chord("j", []), "a plain letter types")
    XCTAssertNil(chord("j", .command), "Cmd chords belong to the menu")
    XCTAssertNil(chord("j", [.control, .command]))
    XCTAssertNil(chord("j", [.control, .option]))
    XCTAssertNil(chord("j", [.control, .shift]), "only ^_ tolerates Shift")
    XCTAssertNil(chord("j", .control, marked: true), "composing text is never interrupted")
    // Polish Pro diacritics ride on Option; none of them is in the table.
    for key in ["a", "c", "e", "l", "n", "o", "s", "z", "x"] {
      XCTAssertNil(chord(key, .option), "⌥\(key) types a diacritic")
    }
    // Chords AppKit already binds never reach the table.
    for key in ["a", "e", "k", "y", "o", "h", "t", "n", "p"] {
      XCTAssertNil(chord(key, .control), "^\(key) is AppKit's")
    }
    XCTAssertNil(chord(nil, .control))
    XCTAssertNil(chord("", .control))
    XCTAssertNil(chord("jj", .control))
  }

  // MARK: - Agent Chat composer

  func testAgentChatComposerEditsWithReadlineChords() throws {
    final class DraftBox { var text = "" }
    let draft = DraftBox()
    var sends = 0
    func field() -> ComposerTextView {
      ComposerTextView(
        text: Binding(get: { draft.text }, set: { draft.text = $0 }),
        height: .constant(40), textScale: 1, isFocused: .constant(false),
        history: [], onSend: { sends += 1 })
    }
    let host = NSHostingView(rootView: field())
    host.frame = NSRect(x: 0, y: 0, width: 400, height: 80)
    host.layoutSubtreeIfNeeded()
    draft.text = "ala ma kota"
    host.rootView = field()
    host.layoutSubtreeIfNeeded()
    func composer(in view: NSView) -> NSTextView? {
      if let text = view as? NSTextView,
        text.accessibilityIdentifier() == ComposerAccessibility.textViewIdentifier
      {
        return text
      }
      return view.subviews.lazy.compactMap { composer(in: $0) }.first
    }
    let text = try XCTUnwrap(composer(in: host))
    XCTAssertEqual(text.string, "ala ma kota")
    text.setSelectedRange(NSRange(location: 11, length: 0))

    text.keyDown(with: try Self.chord("b", .option, keyCode: 11))
    XCTAssertEqual(text.selectedRange().location, 7, "⌥B lands on the start of the last word")
    text.keyDown(with: try Self.chord("d", .option, keyCode: 2))
    XCTAssertEqual(text.string, "ala ma ", "⌥D removes the word after the caret")
    text.keyDown(with: try Self.chord("w", .control, keyCode: 13))
    XCTAssertEqual(text.string, "ala ", "^W removes the word before the caret")
    text.keyDown(with: try Self.chord("j", .control, keyCode: 38))
    XCTAssertEqual(text.string, "ala \n", "^J breaks the line")
    XCTAssertEqual(sends, 0, "^J never sends")
    text.keyDown(with: try Self.chord("f", .option, keyCode: 3))
    XCTAssertEqual(text.selectedRange().location, 5, "⌥F at the end stays put")
    text.setSelectedRange(NSRange(location: 4, length: 0))
    text.keyDown(with: try Self.chord("u", .control, keyCode: 32))
    XCTAssertEqual(text.string, "\n", "^U clears back to the paragraph start")
    XCTAssertEqual(sends, 0)
  }

  // MARK: - Intercom composer

  func testIntercomComposerEditsWithReadlineChordsWithoutSubmitting() throws {
    var submitted = false
    let composer = OverlayConversationComposer(
      palette: .dark, draft: .constant("ala ma kota"), sending: false,
      onSubmit: { submitted = true }, onCancel: {})
    let host = NSHostingView(rootView: composer)
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 480, height: 120)
    let window = NSWindow(
      contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    host.layoutSubtreeIfNeeded()
    func editor(in root: NSView) -> NSTextView? {
      if let text = root as? NSTextView, text.isEditable { return text }
      return root.subviews.lazy.compactMap { editor(in: $0) }.first
    }
    let text = try XCTUnwrap(editor(in: host))
    window.makeFirstResponder(text)
    XCTAssertEqual(text.string, "ala ma kota")
    text.setSelectedRange(NSRange(location: 11, length: 0))

    text.keyDown(with: try Self.chord("w", .control, keyCode: 13, in: window))
    XCTAssertEqual(text.string, "ala ma ", "^W removes the word before the caret")
    text.keyDown(with: try Self.chord("j", .control, keyCode: 38, in: window))
    XCTAssertEqual(text.string, "ala ma \n", "^J breaks the line")
    XCTAssertFalse(submitted, "^J never submits the Intercom draft")
    text.keyDown(with: try Self.chord("b", .option, keyCode: 11, in: window))
    XCTAssertEqual(
      text.selectedRange().location, 4,
      "⌥B steps back over the line break and the trailing space onto the last word")
    text.keyDown(with: try Self.chord("u", .control, keyCode: 32, in: window))
    XCTAssertEqual(text.string, "ma \n", "^U clears back to the paragraph start")
    XCTAssertFalse(submitted)
  }

  // MARK: - Events

  /// A key event as the window server delivers it: `characters` carries the
  /// control code or the Option glyph, `charactersIgnoringModifiers` the key.
  private static func chord(
    _ key: String, _ flags: NSEvent.ModifierFlags, keyCode: UInt16, in window: NSWindow? = nil
  ) throws -> NSEvent {
    let typed: String
    if flags.contains(.control), let scalar = key.unicodeScalars.first,
      let control = Unicode.Scalar(scalar.value & 0x1f)
    {
      typed = String(Character(control))
    } else if flags.contains(.option) {
      typed = ["b": "∫", "f": "ƒ", "d": "∂"][key] ?? key
    } else {
      typed = key
    }
    return try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
        windowNumber: window?.windowNumber ?? 0, context: nil, characters: typed,
        charactersIgnoringModifiers: key, isARepeat: false, keyCode: keyCode))
  }
}
