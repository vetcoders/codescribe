import AppKit
import XCTest

@testable import Codescribe

/// Responder dispatch without ordering a window or using the general clipboard.
@MainActor
final class OverlayPanelCopyChordTests: XCTestCase {
  private final class KeyPanel: NSPanel {
    var hasKeyFocus = true
    override var isKeyWindow: Bool { hasKeyFocus }
  }

  private final class CopyEditor: NSTextView {
    let board = NSPasteboard.withUniqueName()
    override func copy(_ sender: Any?) {
      board.clearContents()
      board.setString((string as NSString).substring(with: selectedRange()), forType: .string)
    }
  }

  private func exercise(flags: NSEvent.ModifierFlags, keyFocus: Bool) throws -> (Bool, String?) {
    let keyBefore = NSApp.keyWindow
    let panel = KeyPanel(
      contentRect: NSRect(x: -10000, y: -10000, width: 240, height: 100),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.hasKeyFocus = keyFocus
    let editor = CopyEditor(frame: panel.contentView?.bounds ?? .zero)
    editor.string = "A selected bubble stays copyable"
    editor.setSelectedRange(NSRange(location: 2, length: 15))
    panel.contentView = editor
    XCTAssertTrue(panel.makeFirstResponder(editor))
    defer {
      editor.board.releaseGlobally()
      panel.close()
    }
    let event = try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
        windowNumber: panel.windowNumber, context: nil, characters: "c",
        charactersIgnoringModifiers: "c", isARepeat: false, keyCode: 8))
    let consumed = panel.performCopyChord(event)
    XCTAssertFalse(panel.isVisible)
    XCTAssertTrue(NSApp.keyWindow === keyBefore)
    return (consumed, editor.board.string(forType: .string))
  }

  func testCommandCCopiesTheSelectedTextThroughTheSharedPanelResponder() throws {
    let result = try exercise(flags: .command, keyFocus: true)
    XCTAssertTrue(result.0)
    XCTAssertEqual(result.1, "selected bubble")
  }

  func testOtherModifiersRemainAvailableToTheirOwnCommands() throws {
    for flags: NSEvent.ModifierFlags in [.control, [.command, .shift], [.command, .option]] {
      let result = try exercise(flags: flags, keyFocus: true)
      XCTAssertFalse(result.0)
      XCTAssertNil(result.1)
    }
  }

  func testPanelWithoutKeyFocusDoesNotCopy() throws {
    let result = try exercise(flags: .command, keyFocus: false)
    XCTAssertFalse(result.0)
    XCTAssertNil(result.1)
  }
}
