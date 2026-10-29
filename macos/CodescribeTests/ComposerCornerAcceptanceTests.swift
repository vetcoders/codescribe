import AppKit
import Observation
import QuartzCore
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class ComposerCornerAcceptanceTests: XCTestCase {
  @MainActor @Observable
  final class DraftStore {
    var text = ""
  }

  private struct EditableComposer: View {
    @Bindable var store: DraftStore
    var maximumHeight: CGFloat = 120
    var inConversation = false

    var body: some View {
      if inConversation {
        OverlayConversationView(
          conversation: OverlayConversation(
            id: "geometry", channel: "0", name: "All", owner: nil, messages: []),
          palette: .dark, topInset: 0, bottomInset: 8,
          pendingControls: [], controlErrors: [:], onControl: { _, _ in },
          draft: $store.text, sending: false, sendError: nil, onSend: {},
          composerEditorHeightLimit: maximumHeight, onToggleDraftMicrophone: {})
      } else {
        OverlayConversationComposer(
          palette: .dark, draft: $store.text, maximumEditorHeight: maximumHeight, sending: false,
          onSubmit: { XCTFail("Geometry must never send") }, onToggleDraftMicrophone: {})
      }
    }
  }

  @MainActor
  private final class Fixture {
    let host: NSHostingView<AnyView>
    let window: NSWindow
    let editor: NSTextView
    let scroll: NSScrollView

    init(
      text: String, width: CGFloat = 360, draft: Binding<String>? = nil,
      microphone: Bool = false, root: AnyView? = nil
    ) throws {
      host = NSHostingView(
        rootView: root
          ?? AnyView(
            OverlayConversationComposer(
              palette: .dark, draft: draft ?? .constant(text), maximumEditorHeight: 120,
              sending: false,
              onSubmit: { XCTFail("Geometry must never send") },
              onToggleDraftMicrophone: microphone ? {} : nil)))
      host.sizingOptions = []
      host.frame = NSRect(x: 0, y: 0, width: width, height: 140)
      window = NSWindow(
        contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      BackgroundTestWindow.present(window)
      if root != nil {
        // Paint/AX fixtures stay off the desktop, but retain ordinary opacity.
        window.setFrameOrigin(NSPoint(x: -10000, y: -10000))
        window.alphaValue = 1
      }
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

    func refresh(width: CGFloat? = nil) {
      if let width {
        window.setContentSize(NSSize(width: width, height: 220))
      }
      for _ in 0..<5 {
        settle()
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.005))
      }
    }

    func assertInsertionIsPainted(file: StaticString = #filePath, line: UInt = #line) throws {
      let range = editor.selectedRange()
      let screen = editor.firstRect(forCharacterRange: range, actualRange: nil)
      XCTAssertGreaterThan(screen.height, 0, file: file, line: line)
      let clip = scroll.contentView
      let caret = clip.convert(window.convertFromScreen(screen), from: nil)
      let center = NSPoint(x: caret.midX, y: caret.midY)
      XCTAssertTrue(
        clip.bounds.insetBy(dx: -2, dy: -2).contains(center),
        "Typing insertion must stay in the visible viewport: \(caret), \(clip.bounds)",
        file: file, line: line)
      if let mask = clip.layer?.mask as? CAShapeLayer {
        let path = try XCTUnwrap(mask.path, file: file, line: line)
        XCTAssertTrue(
          path.contains(clip.convertToLayer(center)), "Typing must survive the paint mask",
          file: file, line: line)
      }
      let manager = try XCTUnwrap(editor.layoutManager, file: file, line: line)
      let container = try XCTUnwrap(editor.textContainer, file: file, line: line)
      manager.ensureLayout(for: container)
      if range.location > 0 {
        let glyph = manager.glyphIndexForCharacter(at: range.location - 1)
        let rect = manager.boundingRect(
          forGlyphRange: NSRange(location: glyph, length: 1), in: container
        )
        .offsetBy(dx: editor.textContainerOrigin.x, dy: editor.textContainerOrigin.y)
        XCTAssertTrue(
          clip.bounds.insetBy(dx: -2, dy: -2).contains(rect),
          "The character just typed must remain visible", file: file, line: line)
        if let path = (clip.layer?.mask as? CAShapeLayer)?.path {
          for point in [
            NSPoint(x: rect.minX + 0.5, y: rect.midY),
            NSPoint(x: rect.maxX - 0.5, y: rect.midY),
          ] {
            XCTAssertTrue(
              path.contains(clip.convertToLayer(point)), "The typed glyph must survive clipping",
              file: file, line: line)
          }
        }
      }
    }

    func assertVisibleGlyphsClearControls() throws {
      let screenTop = try XCTUnwrap(NSScreen.screens.first).frame.maxY
      let buttons = try ["overlay-conversation-draft-microphone", "overlay-conversation-send"].map {
        id in
        let elements = try NativeAccessibilityProbe.waitForElements(id).elements
        XCTAssertEqual(elements.count, 1, "Measure the actual synthetic host control")
        let element = try XCTUnwrap(elements.first)
        let screen = NSRect(
          x: element.x, y: screenTop - element.y - element.height,
          width: element.width, height: element.height)
        return editor.convert(window.convertFromScreen(screen), from: nil)
      }
      let manager = try XCTUnwrap(editor.layoutManager)
      let container = try XCTUnwrap(editor.textContainer)
      manager.ensureLayout(for: container)
      let clip = scroll.contentView
      let paint = (clip.layer?.mask as? CAShapeLayer)?.path
      for index in 0..<manager.numberOfGlyphs {
        let glyph = manager.boundingRect(
          forGlyphRange: NSRange(location: index, length: 1), in: container
        ).offsetBy(dx: editor.textContainerOrigin.x, dy: editor.textContainerOrigin.y)
        guard glyph.width > 0, glyph.height > 0, clip.bounds.intersects(glyph) else { continue }
        let point = NSPoint(x: glyph.midX, y: glyph.midY)
        guard paint?.contains(clip.convertToLayer(point)) ?? true else { continue }
        XCTAssertFalse(
          buttons.contains { $0.intersects(glyph) },
          "Painted glyph \(index) at \(glyph) must clear actual controls \(buttons)")
      }
    }
  }

  func testVisibleGlyphsClearActualMicrophoneAndSendWhileEditingEarlierText() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    for scale in [CGFloat(1), 1.8] {
      let store = DraftStore()
      let fixture = try Fixture(
        text: "", width: 580,
        root: AnyView(
          EditableComposer(store: store, maximumHeight: 32)
            .environment(\.csTextScale, scale)))
      defer { fixture.window.close() }
      fixture.window.setFrameOrigin(NSPoint(x: 100, y: 100))
      try BackgroundTestWindow.takeKeyboard(fixture.window)
      XCTAssertTrue(fixture.window.makeFirstResponder(fixture.editor))
      fixture.editor.insertText(
        "A native draft stays readable while typing, attaching files and resizing the conversation window.",
        replacementRange: NSRange(location: 0, length: 0))
      fixture.refresh()
      fixture.editor.setSelectedRange(NSRange(location: 2, length: 0))
      fixture.editor.insertText(" ", replacementRange: fixture.editor.selectedRange())
      fixture.refresh()
      try fixture.assertInsertionIsPainted()
      try fixture.assertVisibleGlyphsClearControls()
    }
  }

  func testTypingWithMicrophoneSurvivesWrapAttachmentsAndResize() throws {
    let store = DraftStore()
    let attachment = try makeAttachment()
    let fixture = try Fixture(
      text: "", width: 300, root: AnyView(EditableComposer(store: store)))
    defer { fixture.window.close() }
    XCTAssertTrue(fixture.window.makeFirstResponder(fixture.editor))
    for piece in [
      "abc", " typing across the microphone and send controls", " and wrapping again 🦊",
    ] {
      fixture.editor.insertText(piece, replacementRange: fixture.editor.selectedRange())
      fixture.refresh()
      XCTAssertEqual(fixture.editor.string, ConversationDraft.split(store.text).text)
      try fixture.assertInsertionIsPainted()
    }
    fixture.editor.insertText(
      String(repeating: " long native line 🦊\n", count: 40),
      replacementRange: fixture.editor.selectedRange())
    fixture.refresh()
    try fixture.assertInsertionIsPainted()
    fixture.scrollTo(0)
    fixture.editor.insertText("after scroll", replacementRange: fixture.editor.selectedRange())
    fixture.refresh()
    try fixture.assertInsertionIsPainted()
    let prose = ConversationDraft.split(store.text).text
    store.text = ConversationDraft.compose(text: prose, attachments: [attachment.path])
    XCTAssertEqual(ConversationDraft.split(store.text).attachments, [attachment.path])
    fixture.refresh(width: 240)
    fixture.editor.insertText(" after attachment", replacementRange: fixture.editor.selectedRange())
    fixture.refresh()
    try fixture.assertInsertionIsPainted()
    XCTAssertEqual(ConversationDraft.split(store.text).attachments, [attachment.path])
    store.text = ConversationDraft.split(store.text).text
    fixture.refresh(width: 480)
    fixture.editor.insertText(
      "short", replacementRange: NSRange(location: 0, length: fixture.editor.string.utf16.count))
    fixture.refresh()
    try fixture.assertInsertionIsPainted()
    XCTAssertEqual(store.text, "short")
    let caret = fixture.editor.firstRect(
      forCharacterRange: fixture.editor.selectedRange(), actualRange: nil)
    let localCaret = fixture.scroll.contentView.convert(
      fixture.window.convertFromScreen(caret), from: nil)
    XCTAssertLessThanOrEqual(
      localCaret.maxY,
      fixture.scroll.contentView.bounds.maxY - ComposerControlMetrics.hitTargetSize + 2,
      "A multiline draft keeps the entire control row free after shortening")
  }

  func testTypingIntoAttachmentFirstConversationAtSmallHeightBudget() throws {
    let path = try makeAttachment().path
    for width in [CGFloat(300), 700] {
      let store = DraftStore()
      store.text = ConversationDraft.compose(text: "", attachments: [path])
      XCTAssertEqual(ConversationDraft.split(store.text).attachments, [path])
      let fixture = try Fixture(
        text: "", width: width,
        root: AnyView(EditableComposer(store: store, maximumHeight: 32, inConversation: true)))
      defer { fixture.window.close() }
      XCTAssertTrue(fixture.window.makeFirstResponder(fixture.editor))
      for character in "Typing with attachment, microphone and send, then wrapping to another line."
      {
        fixture.editor.insertText(
          String(character), replacementRange: fixture.editor.selectedRange())
        fixture.refresh()
        try fixture.assertInsertionIsPainted()
        XCTAssertEqual(ConversationDraft.split(store.text).attachments, [path])
      }
    }
  }

  private func makeAttachment() throws -> URL {
    let file = ConversationAttachmentPointer.configDirectory
      .appendingPathComponent("attachments/composer-\(UUID().uuidString).txt")
    try FileManager.default.createDirectory(
      at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
    try Data("Native composer fixture".utf8).write(to: file)
    let canonical = file.resolvingSymlinksInPath()
    addTeardownBlock { try? FileManager.default.removeItem(at: canonical) }
    return canonical
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
      let mask = try XCTUnwrap(clip.layer?.mask as? CAShapeLayer)
      let path = try XCTUnwrap(mask.path)
      XCTAssertTrue(
        path.contains(clip.convertToLayer(center)),
        "An in-bounds caret must also remain painted outside the corner cutouts")
      XCTAssertEqual(fixture.editor.selectedRange(), range)
      XCTAssertEqual(fixture.editor.string, text)
    }
  }
}
