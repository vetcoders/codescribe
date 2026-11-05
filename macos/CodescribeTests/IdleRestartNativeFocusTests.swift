import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class IdleRestartNativeFocusTests: XCTestCase {
  private final class Window: NSWindow {
    var key = true
    override var isKeyWindow: Bool { key }
    override var canBecomeKey: Bool { true }
  }

  @MainActor
  private final class Fixture {
    let state: OverlayState
    let host: NSHostingView<OverlayConversationComposer>
    let window: Window
    let editor: NSTextView
    let keyBefore: NSWindow?

    init(state supplied: OverlayState? = nil) throws {
      keyBefore = NSApp.keyWindow
      let state = supplied ?? OverlayState(autoSendEnabled: { false })
      self.state = state
      host = NSHostingView(
        rootView: OverlayConversationComposer(
          palette: .dark,
          draft: Binding(
            get: { state.conversationDrafts["fixture"] ?? "" },
            set: { state.conversationDrafts["fixture"] = $0 }),
          sending: false, onSubmit: { XCTFail("Focus must not send") },
          onEditorActive: { state.noteComposerEditorActive($0) },
          onTypingActivity: { state.noteComposerTypingActivity() }))
      host.frame = CGRect(x: 0, y: 0, width: 420, height: 90)
      window = Window(
        contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      BackgroundTestWindow.present(window)
      host.layoutSubtreeIfNeeded()
      func find(_ view: NSView) -> NSTextView? {
        if let text = view as? NSTextView, text.isEditable { return text }
        return view.subviews.lazy.compactMap { find($0) }.first
      }
      editor = try XCTUnwrap(find(host))
      XCTAssertTrue(window.isVisible)
      XCTAssertTrue(
        NSApp.keyWindow === keyBefore, "Background fixture must not take keyboard focus")
    }

    func focus() throws {
      XCTAssertTrue(window.makeFirstResponder(editor))
      XCTAssertTrue(editor.window === window)
    }

    func cleanup() {
      window.close()
      XCTAssertTrue(NSApp.keyWindow === keyBefore)
    }
  }

  func testBlankNativeEditorIsIdleWhileNonEmptyEditorRequiresVisibleKeyWindow() throws {
    let fixture = try Fixture()
    defer { fixture.cleanup() }
    XCTAssertFalse(OverlayConversationComposer.hasFocusedEditor(in: nil))
    try fixture.focus()
    XCTAssertTrue(fixture.editor.string.isEmpty)
    XCTAssertFalse(
      OverlayConversationComposer.hasFocusedEditor(in: fixture.window),
      "Resting caret in an empty editor must not block idle restart")
    XCTAssertFalse(fixture.state.founderComposing)
    fixture.editor.string = " \n\t "
    XCTAssertFalse(
      OverlayConversationComposer.hasFocusedEditor(in: fixture.window),
      "Whitespace-only editor content must not block idle restart")
    fixture.editor.string = "Writing"
    XCTAssertTrue(OverlayConversationComposer.hasFocusedEditor(in: fixture.window))
    fixture.window.key = false
    NotificationCenter.default.post(name: NSWindow.didResignKeyNotification, object: fixture.window)
    XCTAssertTrue(fixture.window.firstResponder === fixture.editor, "Blur retains native ownership")
    XCTAssertFalse(OverlayConversationComposer.hasFocusedEditor(in: fixture.window))
    fixture.window.key = true
    XCTAssertTrue(OverlayConversationComposer.hasFocusedEditor(in: fixture.window))
    fixture.editor.string = ""
    XCTAssertFalse(OverlayConversationComposer.hasFocusedEditor(in: fixture.window))
  }

  func testHiddenClosedAndDismountedEditorsDoNotLeaveRestartFocus() throws {
    for transition in ["hidden-editor", "hidden-ancestor", "ordered-out", "closed", "dismounted"] {
      let fixture = try Fixture()
      defer { fixture.cleanup() }
      try fixture.focus()
      fixture.editor.string = "Writing"
      XCTAssertTrue(OverlayConversationComposer.hasFocusedEditor(in: fixture.window))
      switch transition {
      case "hidden-editor": fixture.editor.isHidden = true
      case "hidden-ancestor": fixture.host.isHidden = true
      case "ordered-out": fixture.window.orderOut(nil)
      case "closed": fixture.window.close()
      default: fixture.host.removeFromSuperview()
      }
      XCTAssertFalse(OverlayConversationComposer.hasFocusedEditor(in: fixture.window), transition)
    }
  }

  func testForeignTextViewDoesNotCountAndTwoPanelsUseTheirOwnResponder() throws {
    let state = OverlayState(autoSendEnabled: { false })
    let first = try Fixture(state: state)
    let second = try Fixture(state: state)
    defer {
      first.cleanup()
      second.cleanup()
    }
    try first.focus()
    try second.focus()
    first.editor.string = "First"
    second.editor.string = "Second"
    first.window.key = false
    XCTAssertFalse(OverlayConversationComposer.hasFocusedEditor(in: first.window))
    XCTAssertTrue(OverlayConversationComposer.hasFocusedEditor(in: second.window))
    first.host.removeFromSuperview()
    XCTAssertTrue(
      OverlayConversationComposer.hasFocusedEditor(in: second.window),
      "Removing another panel must not revoke the current editor")
    let foreign = NSTextView(frame: second.host.bounds)
    foreign.string = "Foreign"
    second.window.contentView = foreign
    XCTAssertTrue(second.window.makeFirstResponder(foreign))
    XCTAssertFalse(OverlayConversationComposer.hasFocusedEditor(in: second.window))
    XCTAssertTrue(state.pendingTextMessages.isEmpty)
  }
}
