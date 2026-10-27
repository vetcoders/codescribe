import AppKit
import QuartzCore
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class ComposerCornerAcceptanceTests: XCTestCase {
  @MainActor
  private final class Fixture {
    let host: NSHostingView<OverlayConversationComposer>
    let window: NSWindow
    let editor: NSTextView
    let scroll: NSScrollView

    init(text: String, width: CGFloat = 360, draft: Binding<String>? = nil) throws {
      host = NSHostingView(
        rootView: OverlayConversationComposer(
          palette: .dark, draft: draft ?? .constant(text), maximumEditorHeight: 120, sending: false,
          onSubmit: { XCTFail("Geometry must never send") }))
      host.sizingOptions = []
      host.frame = NSRect(x: 0, y: 0, width: width, height: 140)
      window = NSWindow(
        contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      BackgroundTestWindow.present(window)
      for _ in 0..<3 {
        host.layoutSubtreeIfNeeded()
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.005))
      }
      func find(_ view: NSView) -> NSTextView? {
        if let text = view as? NSTextView, text.isEditable { return text }
        return view.subviews.lazy.compactMap { find($0) }.first
      }
      editor = try XCTUnwrap(find(host))
      scroll = try XCTUnwrap(editor.enclosingScrollView)
    }

    func settle() {
      host.layoutSubtreeIfNeeded()
      scroll.layoutSubtreeIfNeeded()
      host.layoutSubtreeIfNeeded()
    }

    func scrollTo(_ y: CGFloat) {
      scroll.contentView.scroll(to: NSPoint(x: 0, y: y))
      scroll.reflectScrolledClipView(scroll.contentView)
      settle()
    }
  }

  func testShortDraftHasWritableMiddleBetweenNativeCornerControls() throws {
    let fixture = try Fixture(text: "abc", width: 300)
    defer { fixture.window.close() }
    let container = try XCTUnwrap(fixture.editor.textContainer)
    XCTAssertEqual(container.exclusionPaths.count, 2)
    let left = container.exclusionPaths[0].bounds
    let right = container.exclusionPaths[1].bounds
    XCTAssertLessThan(left.maxX, right.minX, "Corners must leave editable text between them")
    let point = fixture.editor.convert(
      NSPoint(x: (left.maxX + right.minX) / 2, y: left.midY), to: fixture.host)
    XCTAssertTrue(fixture.host.hitTest(point) === fixture.editor)
    XCTAssertEqual(fixture.editor.string, "abc")
  }

  func testPixelScrollOfLongDraftStaysWithinFrameBudgetAndPreservesText() throws {
    let text = (0..<1500).map { "Line \($0): native text, emoji 🦊 and combining é.\n" }.joined()
    let fixture = try Fixture(text: text)
    defer { fixture.window.close() }
    fixture.scrollTo(fixture.editor.frame.height / 2)
    let origin = fixture.scroll.contentView.bounds.origin.y
    let start = CFAbsoluteTimeGetCurrent()
    for offset in 0..<96 { fixture.scrollTo(origin + CGFloat(offset)) }
    let elapsed = CFAbsoluteTimeGetCurrent() - start
    print("COMPOSER_SCROLL chars=\(text.utf16.count) steps=96 elapsed=\(elapsed)")
    XCTAssertLessThan(elapsed, 1.5, "96 pixel scroll steps must average below a 60 Hz frame")
    XCTAssertEqual(fixture.editor.string, text)
    XCTAssertEqual(fixture.scroll.contentView.bounds.origin.y, origin + 95, accuracy: 1)
    XCTAssertTrue(fixture.editor.frame.height.isFinite)
  }

  func testRepeatedEndAndReverseScrollDoesNotGrowBlankTail() throws {
    let text = (0..<300).map {
      "Line \($0) wraps across the native composer with repeated words.\n"
    }.joined()
    let fixture = try Fixture(text: text, width: 280)
    defer { fixture.window.close() }
    let initialHeight = fixture.editor.frame.height
    for _ in 0..<8 {
      fixture.scrollTo(max(0, fixture.editor.frame.height - fixture.scroll.contentSize.height))
      fixture.scrollTo(0)
    }
    XCTAssertEqual(fixture.editor.string, text)
    XCTAssertEqual(fixture.scroll.contentView.bounds.origin.y, 0, accuracy: 1)
    XCTAssertLessThanOrEqual(
      fixture.editor.frame.height, initialHeight + 3 * ComposerTextLayout.lineHeight(fontSize: 14),
      "Viewport-relative wrapping must not accumulate an empty document tail")
    let manager = try XCTUnwrap(fixture.editor.layoutManager)
    let container = try XCTUnwrap(fixture.editor.textContainer)
    manager.ensureLayout(for: container)
    XCTAssertLessThanOrEqual(
      fixture.editor.frame.height - manager.usedRect(for: container).maxY,
      fixture.scroll.contentSize.height + 40)
  }

  func testCaretRevealKeepsTheNativeSelectionAtBothEndsOfLongDraft() throws {
    let text = String(repeating: "Native caret with 🦊 and é must remain visible.\n", count: 300)
    let fixture = try Fixture(text: text)
    defer { fixture.window.close() }
    XCTAssertTrue(fixture.window.makeFirstResponder(fixture.editor))
    for index in [text.utf16.count, 0, text.utf16.count] {
      let selection = NSRange(location: index, length: 0)
      fixture.editor.setSelectedRange(selection)
      fixture.editor.scrollRangeToVisible(selection)
      for _ in 0..<3 { fixture.settle() }
      XCTAssertEqual(fixture.editor.selectedRange(), selection)
      XCTAssertEqual(fixture.editor.string, text)
      let manager = try XCTUnwrap(fixture.editor.layoutManager)
      let container = try XCTUnwrap(fixture.editor.textContainer)
      manager.ensureLayout(for: container)
      let glyph = manager.glyphIndexForCharacter(at: min(index, text.utf16.count - 1))
      let line = manager.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
        .offsetBy(
          dx: fixture.editor.textContainerOrigin.x, dy: fixture.editor.textContainerOrigin.y)
      XCTAssertTrue(
        fixture.scroll.contentView.bounds.insetBy(dx: -2, dy: -2).intersects(line),
        "Native caret line must remain in the viewport after corner reconciliation")
    }
  }

  func testNativeEditsPublishDraftAndShrinkTheOverflowDocument() throws {
    let original = String(repeating: "Editable native text with 🦊 and é.\n", count: 300)
    var draft = original
    let fixture = try Fixture(
      text: original, draft: Binding(get: { draft }, set: { draft = $0 }))
    defer { fixture.window.close() }
    XCTAssertTrue(fixture.window.makeFirstResponder(fixture.editor))
    fixture.editor.insertText(
      "last words", replacementRange: NSRange(location: original.utf16.count, length: 0))
    for _ in 0..<3 {
      fixture.settle()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.005))
    }
    XCTAssertEqual(draft, original + "last words")
    XCTAssertEqual(fixture.editor.string, draft)
    let longHeight = fixture.editor.frame.height
    fixture.editor.insertText(
      "short", replacementRange: NSRange(location: 0, length: draft.utf16.count))
    for _ in 0..<3 {
      fixture.settle()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.005))
    }
    XCTAssertEqual(draft, "short")
    XCTAssertEqual(fixture.editor.string, "short")
    XCTAssertLessThan(fixture.editor.frame.height, longHeight / 2)
    XCTAssertEqual(fixture.scroll.contentView.bounds.origin.y, 0, accuracy: 1)
  }

  func testRevealedCaretIsInsideTheActualPaintClip() throws {
    let text = String(repeating: "The final caret must clear both corner controls.\n", count: 300)
    let fixture = try Fixture(text: text)
    defer { fixture.window.close() }
    XCTAssertTrue(fixture.window.makeFirstResponder(fixture.editor))
    for index in [text.utf16.count, 0, text.utf16.count] {
      let range = NSRange(location: index, length: 0)
      fixture.editor.setSelectedRange(range)
      fixture.editor.scrollRangeToVisible(range)
      for _ in 0..<3 { fixture.settle() }
      let screen = fixture.editor.firstRect(forCharacterRange: range, actualRange: nil)
      XCTAssertGreaterThan(screen.height, 0)
      let clip = fixture.scroll.contentView
      let caret = clip.convert(fixture.window.convertFromScreen(screen), from: nil)
      let center = NSPoint(x: caret.midX, y: caret.midY)
      XCTAssertTrue(clip.bounds.insetBy(dx: -2, dy: -2).contains(center))
      if let mask = clip.layer?.mask as? CAShapeLayer, let path = mask.path {
        XCTAssertTrue(
          path.contains(clip.convertToLayer(center)),
          "An in-bounds caret must also remain painted outside the corner cutouts")
      }
      XCTAssertEqual(fixture.editor.selectedRange(), range)
      XCTAssertEqual(fixture.editor.string, text)
    }
  }
}
