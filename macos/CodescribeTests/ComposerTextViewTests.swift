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

  // MARK: Long Fn while a composer owns the caret

  /// Falsifier 2: with the caret in the Agent composer, the Rust hold-start
  /// gate reads "owned", and a refused long Fn reaches that composer's hint.
  func testAgentComposerCaretOwnershipGatesLongFnAndReceivesTheRedirect() throws {
    var redirects = 0
    try assertCaretOwnership(
      rootView: ComposerTextView(
        text: .constant("Draft"), height: .constant(40), textScale: 1,
        isFocused: .constant(false), history: [], onSend: {},
        onMicrophoneRedirect: { redirects += 1 }),
      redirects: { redirects })
  }

  /// The Intercom field's redirect drives its own hint state; ownership is
  /// what this test pins.
  func testIntercomComposerCaretOwnershipGatesLongFn() throws {
    try assertCaretOwnership(
      rootView: OverlayConversationComposer(
        palette: .dark, draft: .constant("Bus draft"), sending: false, onSubmit: {},
        onToggleDraftMicrophone: {}),
      redirects: { 0 }, onRedirect: {})
  }

  /// A field without its own microphone keeps long Fn as global dictation.
  func testComposerWithoutMicrophoneNeverOwnsTheCaretForLongFn() throws {
    let registry = ComposerCaretOwnership.shared
    var published: [Bool] = []
    let previous = registry.publish
    registry.publish = { published.append($0) }
    defer { registry.publish = previous }
    let host = NSHostingView(
      rootView: OverlayConversationComposer(
        palette: .dark, draft: .constant("Inbox"), sending: false, onSubmit: {}))
    let (window, text) = try present(host)
    defer { window.close() }
    XCTAssertTrue(window.makeFirstResponder(text))
    settle(host)
    XCTAssertFalse(registry.ownsCaret)
    XCTAssertFalse(published.contains(true))
  }

  private func assertCaretOwnership<Root: View>(
    rootView: Root, redirects: () -> Int, onRedirect: (() -> Void)? = nil,
    file: StaticString = #filePath, line: UInt = #line
  ) throws {
    let registry = ComposerCaretOwnership.shared
    var published: [Bool] = []
    let previous = registry.publish
    registry.publish = { published.append($0) }
    defer { registry.publish = previous }
    let host = NSHostingView(rootView: rootView)
    let (window, text) = try present(host)
    defer { window.close() }
    let other = NSTextField(frame: NSRect(x: 0, y: 0, width: 40, height: 20))
    window.contentView?.addSubview(other)

    XCTAssertTrue(window.makeFirstResponder(text), file: file, line: line)
    settle(host)
    XCTAssertTrue(registry.ownsCaret, "caret in the field must gate long Fn", file: file, line: line)
    XCTAssertEqual(published.last, true, file: file, line: line)

    if onRedirect == nil {
      registry.redirectToComposerMicrophone()
      XCTAssertEqual(redirects(), 1, "the refused hold reaches this field", file: file, line: line)
    }

    window.forcedIsKey = false
    NotificationCenter.default.post(name: NSWindow.didResignKeyNotification, object: window)
    settle(host)
    XCTAssertFalse(registry.ownsCaret, "another app's caret is not ours", file: file, line: line)
    XCTAssertEqual(published.last, false, file: file, line: line)

    window.forcedIsKey = true
    NotificationCenter.default.post(name: NSWindow.didBecomeKeyNotification, object: window)
    settle(host)
    XCTAssertTrue(registry.ownsCaret, file: file, line: line)

    XCTAssertTrue(window.makeFirstResponder(other), file: file, line: line)
    settle(host)
    XCTAssertFalse(registry.ownsCaret, "another field releases the gate", file: file, line: line)

    XCTAssertTrue(window.makeFirstResponder(text), file: file, line: line)
    settle(host)
    host.removeFromSuperview()
    settle(host)
    XCTAssertFalse(registry.ownsCaret, "an unmounted field releases the gate", file: file, line: line)
    XCTAssertEqual(published.last, false, file: file, line: line)
  }

  private func present(_ host: NSView) throws -> (CaretTestWindow, NSTextView) {
    host.frame = NSRect(x: 0, y: 0, width: 420, height: 90)
    let window = CaretTestWindow(frame: host.frame)
    let container = NSView(frame: host.frame)
    container.addSubview(host)
    window.contentView = container
    BackgroundTestWindow.present(window)
    settle(host)
    return (window, try XCTUnwrap(editable(in: host)))
  }

  private func editable(in view: NSView) -> NSTextView? {
    if let text = view as? NSTextView, text.isEditable { return text }
    return view.subviews.lazy.compactMap { self.editable(in: $0) }.first
  }

  private func settle(_ host: NSView) {
    host.layoutSubtreeIfNeeded()
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
    host.layoutSubtreeIfNeeded()
  }
}

private final class CaretTestWindow: NSWindow {
  var forcedIsKey = true
  override var canBecomeKey: Bool { true }
  override var isKeyWindow: Bool { forcedIsKey }

  init(frame: NSRect) {
    super.init(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
    isReleasedWhenClosed = false
  }
}
