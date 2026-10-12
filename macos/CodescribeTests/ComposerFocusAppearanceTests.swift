import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class ComposerFocusAppearanceTests: XCTestCase {
  func testAgentComposerTextViewTracksMountedResponderAndKeyWindowLifecycle() throws {
    final class StateBox { var isFocused = false }
    let state = StateBox()

    let host = NSHostingView(
      rootView: ComposerTextView(
        text: .constant("Draft payload"),
        height: .constant(40),
        textScale: 1,
        isFocused: Binding(get: { state.isFocused }, set: { state.isFocused = $0 }),
        history: [],
        onSend: {}
      )
    )
    host.frame = NSRect(x: 0, y: 0, width: 420, height: 80)

    let window = TestWindow(frame: host.frame)
    let otherField = NSTextField(frame: NSRect(x: 0, y: 0, width: 50, height: 20))
    let container = NSView(frame: host.frame)
    container.addSubview(host)
    container.addSubview(otherField)
    window.contentView = container
    window.orderFrontRegardless()
    defer { window.close() }

    settle(host)

    let textView = try XCTUnwrap(findEditableTextView(in: host))
    XCTAssertFalse(state.isFocused, "Initially unselected editor must report isFocused = false")

    // 1. Gain focus in key window
    XCTAssertTrue(window.makeFirstResponder(textView))
    settle(host)
    XCTAssertTrue(state.isFocused, "Active text view in key window must report isFocused = true")

    // 2. Click another field in the same window
    XCTAssertTrue(window.makeFirstResponder(otherField))
    settle(host)
    XCTAssertFalse(state.isFocused, "Focusing another field must clear active outline")

    // 3. Refocus editor
    XCTAssertTrue(window.makeFirstResponder(textView))
    settle(host)
    XCTAssertTrue(state.isFocused, "Refocusing editor must restore isFocused = true")

    // 4. Window loses key status
    window.forcedIsKey = false
    NotificationCenter.default.post(
      name: NSWindow.didResignKeyNotification,
      object: window
    )
    settle(host)
    XCTAssertFalse(
      state.isFocused,
      "Key window loss must clear active outline"
    )
    XCTAssertTrue(
      window.firstResponder === textView,
      "Key window loss must not steal native first responder"
    )

    // 5. Window becomes key again
    window.forcedIsKey = true
    NotificationCenter.default.post(
      name: NSWindow.didBecomeKeyNotification,
      object: window
    )
    settle(host)
    XCTAssertTrue(
      state.isFocused,
      "Returning to key window must automatically restore active outline"
    )

    // 6. View removed from window
    host.removeFromSuperview()
    settle(host)
    XCTAssertFalse(
      state.isFocused,
      "Removing editor from window must report isFocused = false"
    )
  }

  func testOverlayComposerTracksMountedResponderAndKeyWindowLifecycle() throws {
    final class StateBox {
      var isFocused = false
      var isEditorActive = false
    }
    let state = StateBox()

    let host = NSHostingView(
      rootView: OverlayConversationComposer(
        palette: OverlayAppearancePalette.dark,
        draft: .constant("Overlay draft"),
        sending: false,
        onSubmit: {},
        onEditorActive: { state.isEditorActive = $0 },
        onFocusChange: { state.isFocused = $0 }
      )
    )
    host.frame = NSRect(x: 0, y: 0, width: 420, height: 80)

    let window = TestWindow(frame: host.frame)
    let otherField = NSTextField(frame: NSRect(x: 0, y: 0, width: 50, height: 20))
    let container = NSView(frame: host.frame)
    container.addSubview(host)
    container.addSubview(otherField)
    window.contentView = container
    window.orderFrontRegardless()
    defer { window.close() }

    settle(host)

    let textView = try XCTUnwrap(findEditableTextView(in: host))
    XCTAssertFalse(
      state.isFocused, "Initially unselected overlay editor must report isFocused = false")
    XCTAssertFalse(
      state.isEditorActive, "Initially unselected overlay editor must report isEditorActive = false"
    )

    // 1. Gain focus in key window
    XCTAssertTrue(window.makeFirstResponder(textView))
    settle(host)
    XCTAssertTrue(
      state.isFocused, "Active overlay editor in key window must report isFocused = true")
    XCTAssertTrue(
      state.isEditorActive, "onEditorActive must be true when first responder is acquired")

    // 2. Click another field in the same window
    XCTAssertTrue(window.makeFirstResponder(otherField))
    settle(host)
    XCTAssertFalse(state.isFocused, "Focusing another field must clear active outline")
    XCTAssertFalse(state.isEditorActive, "onEditorActive must be false on resign")

    // 3. Refocus editor
    XCTAssertTrue(window.makeFirstResponder(textView))
    settle(host)
    XCTAssertTrue(state.isFocused, "Refocusing overlay editor must restore isFocused = true")
    XCTAssertTrue(state.isEditorActive, "onEditorActive must restore on refocus")

    // 4. Window loses key status
    window.forcedIsKey = false
    NotificationCenter.default.post(
      name: NSWindow.didResignKeyNotification,
      object: window
    )
    settle(host)
    XCTAssertFalse(
      state.isFocused,
      "Key window loss must clear active outline"
    )
    XCTAssertTrue(
      window.firstResponder === textView,
      "Key window loss must not strip first responder from overlay editor"
    )
    XCTAssertTrue(
      state.isEditorActive,
      "onEditorActive must be preserved during window blur so auto-collapse stays suspended"
    )

    // 5. Window becomes key again
    window.forcedIsKey = true
    NotificationCenter.default.post(
      name: NSWindow.didBecomeKeyNotification,
      object: window
    )
    settle(host)
    XCTAssertTrue(
      state.isFocused,
      "Returning to key window must restore active outline"
    )

    // 6. View removed from window
    host.removeFromSuperview()
    settle(host)
    XCTAssertFalse(
      state.isFocused,
      "Removing overlay editor from window must report isFocused = false"
    )
  }

  func testComposerDecorationsDoNotMutateDraftOrAddTextCharacters() throws {
    final class TextHolder { var draft = "Initial typed words with 123 symbols." }
    let holder = TextHolder()

    let host = NSHostingView(
      rootView: ComposerTextView(
        text: Binding(get: { holder.draft }, set: { holder.draft = $0 }),
        height: .constant(40),
        textScale: 1,
        isFocused: .constant(false),
        history: [],
        onSend: {}
      )
    )
    host.frame = NSRect(x: 0, y: 0, width: 420, height: 80)

    let window = TestWindow(frame: host.frame)
    window.contentView = host
    window.orderFrontRegardless()
    defer { window.close() }

    settle(host)
    let textView = try XCTUnwrap(findEditableTextView(in: host))
    XCTAssertEqual(textView.string, "Initial typed words with 123 symbols.")

    // Focus cycle
    _ = window.makeFirstResponder(textView)
    settle(host)
    NotificationCenter.default.post(name: NSWindow.didResignKeyNotification, object: window)
    settle(host)
    NotificationCenter.default.post(name: NSWindow.didBecomeKeyNotification, object: window)
    settle(host)

    XCTAssertEqual(
      textView.string,
      "Initial typed words with 123 symbols.",
      "Input decoration must never add glyphs or mutate draft text"
    )
  }

  // MARK: - Helpers

  private func settle(_ host: NSView) {
    host.layoutSubtreeIfNeeded()
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
    host.layoutSubtreeIfNeeded()
  }

  private func findEditableTextView(in root: NSView) -> NSTextView? {
    if let tv = root as? NSTextView, tv.isEditable {
      return tv
    }
    for subview in root.subviews {
      if let tv = findEditableTextView(in: subview) {
        return tv
      }
    }
    return nil
  }

}

private final class TestWindow: NSWindow {
  var forcedIsKey = true
  override var canBecomeKey: Bool { true }
  override var isKeyWindow: Bool { forcedIsKey }

  init(frame: NSRect) {
    super.init(
      contentRect: frame,
      styleMask: [.borderless],
      backing: .buffered,
      defer: false
    )
    isReleasedWhenClosed = false
  }
}
