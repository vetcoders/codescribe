import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class ComposerTextViewTests: XCTestCase {
  func testNativeComposerDisablesSystemRewritingButKeepsEditing() throws {
    let host = NSHostingView(
      rootView: ComposerTextView(
        text: .constant(""), height: .constant(40), textScale: 1,
        isFocused: .constant(false), history: [], onSend: {}))
    host.frame = NSRect(x: 0, y: 0, width: 400, height: 80)
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
    XCTAssertEqual(text.inlinePredictionType, .no)
    XCTAssertFalse(text.isAutomaticTextCompletionEnabled)
    XCTAssertFalse(text.isAutomaticTextReplacementEnabled)
    XCTAssertFalse(text.isAutomaticSpellingCorrectionEnabled)
    XCTAssertFalse(text.isAutomaticQuoteSubstitutionEnabled)
    XCTAssertFalse(text.isAutomaticDashSubstitutionEnabled)
    if #available(macOS 15.0, *) {
      XCTAssertEqual(text.writingToolsBehavior, .none)
      XCTAssertEqual(text.mathExpressionCompletionType, .no)
    }
    XCTAssertTrue(text.isEditable)
    XCTAssertTrue(text.isSelectable)
    XCTAssertTrue(text.allowsUndo)
  }

  /// Crash 2026-10-23 (`-[_NSUndoStack popAndInvoke]`): typing undo recorded on
  /// the window's manager outlived the composer, and Cmd+Z reached a freed target.
  func testTypingUndoIsOwnedByTheComposerNotItsWindow() throws {
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 400, height: 80),
      styleMask: [.titled], backing: .buffered, defer: true)
    window.isReleasedWhenClosed = false
    let host = NSHostingView(
      rootView: ComposerTextView(
        text: .constant(""), height: .constant(40), textScale: 1,
        isFocused: .constant(false), history: [], onSend: {}))
    window.contentView = host
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
    let undo = try XCTUnwrap(text.undoManager)
    XCTAssertFalse(undo === window.undoManager)
    text.insertText("hello", replacementRange: NSRange(location: 0, length: 0))
    XCTAssertTrue(undo.canUndo)
    XCTAssertFalse(window.undoManager?.canUndo ?? false)

    // Cmd+Z reaches the composer's own history before any menu item does.
    XCTAssertTrue(window.makeFirstResponder(text))
    let chord = try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0,
        windowNumber: window.windowNumber, context: nil, characters: "z",
        charactersIgnoringModifiers: "z", isARepeat: false, keyCode: 6))
    XCTAssertTrue(text.performKeyEquivalent(with: chord))
    XCTAssertEqual(text.string, "")
  }

  func testHeightGrowsFromOneLineAndClampsAtEight() {
    let lineHeight: CGFloat = 20

    XCTAssertEqual(
      ComposerTextLayout.resolve(contentHeight: 1, lineHeight: lineHeight),
      ComposerTextLayout(height: 30, isVerticallyScrollable: false)
    )
    XCTAssertEqual(
      ComposerTextLayout.resolve(contentHeight: 86, lineHeight: lineHeight),
      ComposerTextLayout(height: 86, isVerticallyScrollable: false)
    )
    XCTAssertEqual(
      ComposerTextLayout.resolve(contentHeight: 300, lineHeight: lineHeight),
      ComposerTextLayout(height: 170, isVerticallyScrollable: true)
    )
  }

  func testLongNativePayloadMeasuresBeyondCapAndEnablesScrolling() {
    let fontSize: CGFloat = 13.5
    let text = (1...12).map { "Zażółć gęślą jaźń — line \($0)" }.joined(separator: "\n")
    let contentHeight = ComposerTextLayout.contentHeight(
      text: text,
      width: 320,
      fontSize: fontSize
    )
    let layout = ComposerTextLayout.resolve(
      contentHeight: contentHeight,
      lineHeight: ComposerTextLayout.lineHeight(fontSize: fontSize)
    )

    XCTAssertTrue(layout.isVerticallyScrollable)
    XCTAssertEqual(
      layout.height,
      ComposerTextLayout.lineHeight(fontSize: fontSize) * 8
        + ComposerTextLayout.verticalPadding
    )
  }

  func testNativeMeasurementRespondsToVisualWrappingWidth() {
    let text = String(repeating: "Zażółć gęślą jaźń payload ", count: 12)
    let narrow = ComposerTextLayout.contentHeight(text: text, width: 180, fontSize: 13.5)
    let wide = ComposerTextLayout.contentHeight(text: text, width: 520, fontSize: 13.5)

    XCTAssertGreaterThan(narrow, wide)
  }

  func testReturnSendsAndShiftReturnInsertsNewline() {
    XCTAssertEqual(
      ComposerTextKeyDisposition.resolve(keyCode: 36, modifiers: [], hasMarkedText: false),
      .send
    )
    XCTAssertEqual(
      ComposerTextKeyDisposition.resolve(keyCode: 36, modifiers: .shift, hasMarkedText: false),
      .insertNewline
    )
    XCTAssertEqual(
      ComposerTextKeyDisposition.resolve(keyCode: 76, modifiers: [], hasMarkedText: false),
      .send
    )
  }

  func testProgrammaticDraftWithNewlineCannotTriggerSend() throws {
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

    draft.text = "first line\nsecond line"
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
    XCTAssertEqual(text.string, draft.text)
    XCTAssertEqual(sends, 0)

    let enter = try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
        windowNumber: 0, context: nil, characters: "\r",
        charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36))
    text.keyDown(with: enter)
    XCTAssertEqual(sends, 1, "only the native Enter key event invokes onSend")
  }

  func testIMEAndModifiedReturnRemainNative() {
    XCTAssertEqual(
      ComposerTextKeyDisposition.resolve(keyCode: 36, modifiers: [], hasMarkedText: true),
      .native
    )
    for modifier in [NSEvent.ModifierFlags.command, .control, .option] {
      XCTAssertEqual(
        ComposerTextKeyDisposition.resolve(
          keyCode: 36,
          modifiers: modifier,
          hasMarkedText: false
        ),
        .native
      )
    }
    XCTAssertEqual(
      ComposerTextKeyDisposition.resolve(keyCode: 0, modifiers: [], hasMarkedText: false),
      .native
    )
  }

  func testArrowKeysNavigateTerminalStyleHistory() {
    XCTAssertEqual(
      ComposerTextKeyDisposition.resolve(keyCode: 126, modifiers: [], hasMarkedText: false),
      .previousHistory
    )
    XCTAssertEqual(
      ComposerTextKeyDisposition.resolve(keyCode: 125, modifiers: [], hasMarkedText: false),
      .nextHistory
    )

    var state = ComposerDraftHistoryState()
    let history = ["first", "second", "third"]
    XCTAssertEqual(state.previous(history: history, current: "unfinished draft"), "third")
    XCTAssertEqual(state.previous(history: history, current: "ignored"), "second")
    XCTAssertEqual(state.previous(history: history, current: "ignored"), "first")
    XCTAssertEqual(state.previous(history: history, current: "ignored"), "first")
    XCTAssertEqual(state.next(history: history), "second")
    XCTAssertEqual(state.next(history: history), "third")
    XCTAssertEqual(state.next(history: history), "unfinished draft")
    XCTAssertNil(state.next(history: history))
  }

  func testTextSurfaceHasStableAccessibilityIdentifier() {
    XCTAssertEqual(ComposerAccessibility.textViewIdentifier, "agent-composer-text")
  }
}
