import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayLostCopyTests: XCTestCase {
  func testNativeSelectionInvokesTheTextViewsCopyOverride() throws {
    let panel = OverlayLostCopySupport.panel()
    let editor = OverlayLostCopySupport.Editor(frame: NSRect(x: 0, y: 0, width: 300, height: 80))
    panel.contentView = editor
    defer { panel.contentView = nil; panel.close() }
    editor.string = "alpha βeta gamma"
    editor.setSelectedRange(NSRange(location: 6, length: 4))
    XCTAssertTrue(panel.makeFirstResponder(editor))
    XCTAssertTrue(panel.performCopyChord(try OverlayLostCopySupport.commandC(in: panel)))
    XCTAssertEqual(editor.copiedSelections, ["βeta"])
    XCTAssertFalse(panel.isVisible)
  }

  func testEmptyComposerDoesNotClaimSuccessfulCopyWithoutPayload() throws {
    for draft in ["", "a nonempty draft with only an insertion point"] {
      let panel = OverlayLostCopySupport.panel()
      let editor = OverlayLostCopySupport.Editor(frame: NSRect(x: 0, y: 0, width: 300, height: 80))
      panel.contentView = editor
      defer { panel.contentView = nil; panel.close() }
      editor.string = draft
      editor.setSelectedRange(NSRange(location: (draft as NSString).length, length: 0))
      XCTAssertTrue(panel.makeFirstResponder(editor))
      XCTAssertFalse(panel.performCopyChord(try OverlayLostCopySupport.commandC(in: panel)),
        "An empty selection must fall through, never report a successful native no-op")
      XCTAssertTrue(editor.copiedSelections.isEmpty)
      XCTAssertEqual(editor.string, draft)
    }
  }

  func testLosingKeyStatusPreventsCopyOfAStaleNativeSelection() throws {
    let panel = OverlayLostCopySupport.panel()
    let editor = OverlayLostCopySupport.Editor(frame: NSRect(x: 0, y: 0, width: 300, height: 80))
    panel.contentView = editor
    defer { panel.contentView = nil; panel.close() }
    editor.string = "old selection"
    editor.setSelectedRange(NSRange(location: 0, length: 3))
    XCTAssertTrue(panel.makeFirstResponder(editor))
    let chord = try OverlayLostCopySupport.commandC(in: panel)
    XCTAssertTrue(panel.performCopyChord(chord))
    panel.keyFocus = false
    XCTAssertFalse(panel.performCopyChord(chord))
    XCTAssertEqual(editor.copiedSelections, ["old"], "No second copy after key resignation")
  }

  func testCopyDoesNotUseAnotherWindowsNativeSelection() throws {
    let first = OverlayLostCopySupport.panel()
    let second = OverlayLostCopySupport.panel()
    let empty = OverlayLostCopySupport.Editor(frame: NSRect(x: 0, y: 0, width: 300, height: 80))
    let selected = OverlayLostCopySupport.Editor(frame: empty.frame)
    first.contentView = empty
    second.contentView = selected
    defer {
      first.contentView = nil; second.contentView = nil
      first.close(); second.close()
    }
    empty.string = "unselected first draft"
    empty.setSelectedRange(NSRange(location: 0, length: 0))
    selected.string = "second window secret"
    selected.setSelectedRange(NSRange(location: 0, length: 6))
    XCTAssertTrue(first.makeFirstResponder(empty))
    XCTAssertTrue(second.makeFirstResponder(selected))
    second.keyFocus = false
    XCTAssertFalse(first.performCopyChord(try OverlayLostCopySupport.commandC(in: first)))
    XCTAssertTrue(selected.copiedSelections.isEmpty)
    XCTAssertTrue(empty.copiedSelections.isEmpty)
    XCTAssertFalse(second.performCopyChord(try OverlayLostCopySupport.commandC(in: second)))
  }

  func testBothPanelsRegisterAndWithdrawRegionsWhenReparented() throws {
    let first = FloatingOverlayPanel(
      contentRect: NSRect(x: -10000, y: -10000, width: 320, height: 200),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    let second = OverlayIntercomPanel()
    first.isReleasedWhenClosed = false
    let a = NSView(frame: NSRect(x: 0, y: 0, width: 320, height: 200))
    let b = NSView(frame: a.frame)
    first.contentView = a
    second.contentView = b
    defer {
      first.contentView = nil; second.contentView = nil
      first.close(); second.close()
    }
    let region = OverlaySelectableTextRegionView(frame: NSRect(x: 20, y: 20, width: 100, height: 40))
    a.addSubview(region)
    let firstRegistry = try OverlayLostCopySupport.registry(in: first)
    let secondRegistry = try OverlayLostCopySupport.registry(in: second)
    XCTAssertTrue(firstRegistry.contains(region))
    XCTAssertFalse(secondRegistry.contains(region))
    XCTAssertTrue(region.contains(windowPoint: NSPoint(x: 40, y: 30)))
    a.isHidden = true
    XCTAssertFalse(region.contains(windowPoint: NSPoint(x: 40, y: 30)))
    a.isHidden = false
    XCTAssertFalse(region.contains(windowPoint: NSPoint(x: 200, y: 130)))
    XCTAssertNil(region.hitTest(NSPoint(x: 10, y: 10)), "Registration cannot intercept text clicks")
    b.addSubview(region)
    XCTAssertTrue(region.window === second)
    XCTAssertFalse(firstRegistry.contains(region), "The old window must not retain a copy candidate")
    XCTAssertTrue(secondRegistry.contains(region))
    region.removeFromSuperview()
    XCTAssertFalse(firstRegistry.contains(region))
    XCTAssertFalse(secondRegistry.contains(region))
    XCTAssertFalse(region.contains(windowPoint: NSPoint(x: 40, y: 30)))
  }

  func testMountedBubbleKeepsItsFullRawPayloadIncludingOversizedTail() throws {
    for raw in ["**raw** [link](https://example.com) Ω", String(repeating: "raw Ω\n", count: 6000) + "TAIL"] {
      let message = OverlayConversationMessage(
        id: "o2-copy", kind: .user, text: raw, order: 1,
        emittedAt: "2026-10-31T12:00:00Z", owner: nil, recipients: [], deliveryID: nil,
        replyTo: nil, unsolicited: false, playback: nil, busPath: "")
      let panel = FloatingOverlayPanel(
        contentRect: NSRect(x: -10000, y: -10000, width: 320, height: 400),
        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
      panel.isReleasedWhenClosed = false
      let host = NSHostingView(
        rootView: OverlayConversationMessageContent(message: message, palette: .dark)
          .frame(width: 300))
      panel.contentView = host
      defer { panel.contentView = nil; panel.close() }
      host.layoutSubtreeIfNeeded()
      let region = try XCTUnwrap(
        OverlayLostCopySupport.descendants(OverlaySelectableTextRegionView.self, in: host).first)
      XCTAssertEqual(OverlayLostCopySupport.field("copyText", in: region, as: String.self), raw,
        "A bubble registers raw message text, including the oversized tail, not its rendered head")
    }
  }

  func testPassiveHoverDoesNotOpenIntercomKeyPermission() throws {
    let panel = OverlayIntercomPanel()
    panel.setFrame(NSRect(x: -10000, y: -10000, width: 320, height: 200), display: false)
    let root = NSView(frame: NSRect(x: 0, y: 0, width: 320, height: 200))
    panel.contentView = root
    defer { panel.contentView = nil; panel.close() }
    root.addSubview(OverlaySelectableTextRegionView(frame: NSRect(x: 20, y: 20, width: 100, height: 40)))
    let keyBefore = NSApp.keyWindow
    let motion = try XCTUnwrap(NSEvent.mouseEvent(
      with: .mouseMoved, location: NSPoint(x: 40, y: 30), modifierFlags: [], timestamp: 0,
      windowNumber: panel.windowNumber, context: nil, eventNumber: 1, clickCount: 0, pressure: 0))
    panel.sendEvent(motion)
    XCTAssertFalse(panel.canBecomeKey)
    XCTAssertFalse(panel.isKeyWindow)
    XCTAssertTrue(NSApp.keyWindow === keyBefore)
  }

  func testExplicitTextClickOpensIntercomGateAndResignClosesIt() throws {
    // makeKey is an intentional focus transition. Only the integrator's
    // isolated GUI lane may execute this case; it has no event-timing waits.
    try BackgroundTestWindow.requireIsolatedGUI()
    let panel = OverlayIntercomPanel()
    panel.setFrame(NSRect(x: -10000, y: -10000, width: 320, height: 200), display: false)
    let root = OverlayLostCopySupport.ClickSink(
      frame: NSRect(x: 0, y: 0, width: 320, height: 200))
    panel.contentView = root
    defer { panel.resignKey(); panel.contentView = nil; panel.close() }
    root.addSubview(OverlaySelectableTextRegionView(
      frame: NSRect(x: 20, y: 20, width: 100, height: 40)))
    BackgroundTestWindow.present(panel)
    XCTAssertFalse(panel.acceptsConversationFocus)
    XCTAssertFalse(panel.canBecomeKey)
    let down = try XCTUnwrap(NSEvent.mouseEvent(
      with: .leftMouseDown, location: NSPoint(x: 40, y: 30), modifierFlags: [], timestamp: 0,
      windowNumber: panel.windowNumber, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
    panel.sendEvent(down)
    XCTAssertTrue(panel.canBecomeKey, "An explicit visible text interaction grants keyboard permission")
    panel.resignKey()
    XCTAssertFalse(panel.canBecomeKey, "Temporary selection permission ends on key resignation")
    panel.acceptsConversationFocus = true
    panel.resignKey()
    XCTAssertTrue(panel.canBecomeKey, "An explicitly opened conversation retains its focus policy")
  }

  func testEmptySelectionRoutesToTheWindowLocalBubbleBeforeAppFallback() throws {
    let source = try OverlayLostBehaviorSource.overlay("DictationOverlayWindow.swift")
    let copy = try OverlayLostBehaviorSource.section(
      source, from: "func performCopyChord(", to: "return NSApp.sendAction(")
    try OverlayLostBehaviorSource.ordered(
      ["guard isKeyWindow", "textView.selectedRange().length > 0", "textView.copy(nil)",
        "conversationTextRegions?.allObjects", "$0.contains(windowPoint: mouseLocationOutsideOfEventStream)",
        "region.copyContent()", "if firstResponder is NSTextView { return false }"], in: copy)
    let regions = try OverlayLostBehaviorSource.overlay("OverlayWindowDragRegion.swift")
    try OverlayLostBehaviorSource.ordered(
      ["func copyContent() -> Bool", "guard copyText.contains(where:", "let text = copyText",
        "Task { @MainActor in await chatCopy(text) }"], in: regions)
    let content = try OverlayLostBehaviorSource.overlay("OverlayConversationView.swift")
    XCTAssertTrue(content.contains("OverlaySelectableTextRegion(copyText: message.text)"))
  }

  func testExplicitTextInteractionOpensOnlyTheTemporaryGateAndResignClearsIt() throws {
    let source = try OverlayLostBehaviorSource.overlay("OverlayIntercomWidget.swift")
    let panel = try OverlayLostBehaviorSource.section(
      source, from: "final class OverlayIntercomPanel: NSPanel {",
      to: "private struct OverlayIntercomHeader: View {")
    try OverlayLostBehaviorSource.ordered(
      ["if event.type == .leftMouseDown", "selectableTextRegions.allObjects",
        "region.contains(windowPoint: event.locationInWindow)", "allowsKeyForSelection = true",
        "if !isKeyWindow { makeKey() }", "override func resignKey()", "super.resignKey()",
        "allowsKeyForSelection = false"], in: panel)
    XCTAssertTrue(OverlayLostBehaviorSource.compact(panel).contains(
      "override var canBecomeKey: Bool { acceptsConversationFocus || allowsKeyForSelection }"))
  }

  func testRawUnicodeMarkdownUsesTheExistingTaggedCopyAuthority() async throws {
    let raw = "**raw** [link](https://example.com) Ω\nTAIL"
    var writes: [String] = []
    let previous = UserTextClipboard.bridgeWrite
    UserTextClipboard.bridgeWrite = { writes.append($0) }
    defer { UserTextClipboard.bridgeWrite = previous }
    let copied = await chatCopy(raw)
    XCTAssertTrue(copied)
    XCTAssertEqual(writes, [raw])
  }
}
