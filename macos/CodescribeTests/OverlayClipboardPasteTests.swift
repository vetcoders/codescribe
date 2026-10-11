import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

final class OverlayClipboardPasteTests: XCTestCase {
  @MainActor
  func testPastedImageUsesCanonicalAssetStoreAndReusesIdenticalBytes() throws {
    let isolatedRoot = CodescribeConfig().configDir()
    XCTAssertNotEqual(isolatedRoot, NSHomeDirectory() + "/.codescribe")
    guard isolatedRoot != NSHomeDirectory() + "/.codescribe" else { return }
    let png = try XCTUnwrap(bitmap().representation(using: .png, properties: [:]))
    let path = try savePastedImage(data: png)
    XCTAssertTrue(path.hasPrefix(isolatedRoot + "/assets/inline_"))
    XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: path)), png)
    XCTAssertEqual(try savePastedImage(data: png), path)
  }

  @MainActor
  private func bitmap() throws -> NSBitmapImageRep {
    try XCTUnwrap(
      NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: 2, pixelsHigh: 2,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 8, bitsPerPixel: 32))
  }

  @MainActor
  func testPNGAndTIFFProduceReadablePNG() throws {
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    let image = try bitmap()
    for (type, format) in [
      (NSPasteboard.PasteboardType.png, NSBitmapImageRep.FileType.png),
      (.tiff, .tiff),
    ] {
      board.clearContents()
      board.setData(
        try XCTUnwrap(image.representation(using: format, properties: [:])), forType: type)
      let png = try XCTUnwrap(ConversationClipboardImage.png(from: board))
      XCTAssertEqual(Array(png.prefix(8)), [137, 80, 78, 71, 13, 10, 26, 10])
      XCTAssertEqual(NSBitmapImageRep(data: png)?.pixelsWide, 2)
    }
  }

  @MainActor
  func testTextAndFinderFilesDoNotBecomeImageAttachments() throws {
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    let png = try XCTUnwrap(bitmap().representation(using: .png, properties: [:]))
    for type in [NSPasteboard.PasteboardType.string, .fileURL] {
      board.clearContents()
      board.setData(png, forType: .png)
      board.setString("file:///tmp/example.png", forType: type)
      XCTAssertNil(try ConversationClipboardImage.png(from: board))
    }
    board.clearContents()
    board.setData(Data([1, 2, 3]), forType: .png)
    XCTAssertThrowsError(try ConversationClipboardImage.png(from: board))
  }

  @MainActor
  func testBroadcastComposerExposesItsOwnAccessiblePromptWithoutChangingAgentDefault() throws {
    for prompt in [String(localized: "Send to everyone"), String(localized: "Message the Agent")] {
      let host = NSHostingView(
        rootView: OverlayConversationComposer(
          palette: .dark, draft: .constant(""), sending: false, onSubmit: {}, placeholder: prompt))
      host.sizingOptions = []
      host.frame = NSRect(x: 0, y: 0, width: 400, height: 100)
      let window = NSWindow(
        contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      defer { window.close() }
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.03))
      func editor(in view: NSView) -> NSTextView? {
        if let text = view as? NSTextView, text.isEditable { return text }
        return view.subviews.lazy.compactMap { editor(in: $0) }.first
      }
      let text = try XCTUnwrap(editor(in: host))
      XCTAssertEqual(text.accessibilityLabel(), prompt)
      XCTAssertTrue(text.string.isEmpty, "placeholder must not enter the message payload")
    }
  }

  @MainActor
  func testComposerGrowsWhileTypingWithoutScrollingBeforeItsHeightLimit() throws {
    let draft = ComposerTypingDraft()
    let host = NSHostingView(rootView: ComposerTypingSurface(draft: draft))
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 300, height: 240)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    func settle() {
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.01))
      host.layoutSubtreeIfNeeded()
    }
    settle()
    func editor(in view: NSView) -> NSTextView? {
      if let text = view as? NSTextView, text.isEditable { return text }
      return view.subviews.lazy.compactMap { editor(in: $0) }.first
    }
    let text = try XCTUnwrap(editor(in: host))
    let scroll = try XCTUnwrap(text.enclosingScrollView)
    window.makeFirstResponder(text)
    let initialHeight = scroll.frame.height
    var previousHeight = initialHeight
    var growingHeights: Set<Int> = []
    for index in 1...70 {
      text.insertText("słowo\(index) ", replacementRange: text.selectedRange())
      settle()
      XCTAssertEqual(text.string, draft.text)
      XCTAssertGreaterThanOrEqual(scroll.frame.height, previousHeight - 1, "word \(index)")
      XCTAssertLessThanOrEqual(scroll.frame.height, 113)
      if scroll.frame.height < 111 {
        growingHeights.insert(Int(scroll.frame.height.rounded()))
        XCTAssertEqual(
          scroll.documentVisibleRect.minY, 0, accuracy: 1,
          "the field must grow before it starts scrolling, word \(index)")
        XCTAssertLessThanOrEqual(text.frame.height, scroll.contentSize.height + 1)
      }
      let layout = try XCTUnwrap(text.layoutManager)
      let container = try XCTUnwrap(text.textContainer)
      layout.ensureLayout(for: container)
      let glyph = layout.glyphIndexForCharacter(at: text.selectedRange().location - 1)
      let line = layout.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
        .offsetBy(dx: 0, dy: text.textContainerOrigin.y)
      XCTAssertLessThanOrEqual(line.maxY, scroll.documentVisibleRect.maxY + 1, "word \(index)")
      XCTAssertGreaterThanOrEqual(line.minY, scroll.documentVisibleRect.minY - 1, "word \(index)")
      previousHeight = scroll.frame.height
    }
    XCTAssertGreaterThan(growingHeights.count, 2, "the viewport must grow through multiple lines")
    XCTAssertGreaterThan(scroll.frame.height, initialHeight + 40)
    XCTAssertGreaterThan(text.frame.height, scroll.contentSize.height)
    let heightAtLimit = scroll.frame.height
    scroll.contentView.scroll(to: .zero)
    scroll.reflectScrolledClipView(scroll.contentView)
    draft.repaint += 1
    settle()
    XCTAssertEqual(scroll.documentVisibleRect.minY, 0, accuracy: 1)
    XCTAssertEqual(scroll.frame.height, heightAtLimit, accuracy: 1)
    draft.text = ""
    settle()
    XCTAssertEqual(text.string, "")
    XCTAssertEqual(scroll.frame.height, initialHeight, accuracy: 1)
    XCTAssertEqual(scroll.documentVisibleRect.minY, 0, accuracy: 1)
  }

  @MainActor
  func testOverflowingComposerKeepsCaretVisibleAfterRepaintAndAllowsManualScroll() throws {
    var draft = ""
    func composer() -> OverlayConversationComposer {
      OverlayConversationComposer(
        palette: .dark, draft: Binding(get: { draft }, set: { draft = $0 }),
        sending: false, onSubmit: {})
    }
    let host = NSHostingView(rootView: composer())
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 400, height: 160)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    host.layoutSubtreeIfNeeded()
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.03))
    func editor(in view: NSView) -> NSTextView? {
      if let text = view as? NSTextView, text.isEditable { return text }
      return view.subviews.lazy.compactMap { editor(in: $0) }.first
    }
    let text = try XCTUnwrap(editor(in: host))
    let scroll = try XCTUnwrap(text.enclosingScrollView)
    func repaint() {
      host.rootView = composer()
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.03))
      host.layoutSubtreeIfNeeded()
    }
    func assertCaretVisible(file: StaticString = #filePath, line: UInt = #line) throws {
      let layout = try XCTUnwrap(text.layoutManager)
      let container = try XCTUnwrap(text.textContainer)
      layout.ensureLayout(for: container)
      let glyph = layout.glyphIndexForCharacter(at: text.selectedRange().location - 1)
      var caret = layout.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
      caret.origin.y += text.textContainerOrigin.y
      XCTAssertGreaterThanOrEqual(
        caret.minY, scroll.documentVisibleRect.minY - 1, file: file, line: line)
      XCTAssertLessThanOrEqual(
        caret.maxY, scroll.documentVisibleRect.maxY + 1, file: file, line: line)
    }
    text.insertText(
      (1...24).map { "line \($0) with enough words to wrap after the composer viewport narrows" }
        .joined(separator: "\n"),
      replacementRange: NSRange(location: 0, length: 0))
    host.frame.size.width = 280
    repaint()
    XCTAssertGreaterThan(text.frame.height, scroll.contentSize.height)
    try assertCaretVisible()
    scroll.contentView.scroll(to: .zero)
    scroll.reflectScrolledClipView(scroll.contentView)
    repaint()
    XCTAssertEqual(
      scroll.documentVisibleRect.minY, 0, accuracy: 1,
      "an idle repaint must not fight intentional scrolling")
    text.insertText(" continued", replacementRange: text.selectedRange())
    repaint()
    try assertCaretVisible()
    XCTAssertTrue(draft.hasSuffix("continued"))
  }

  @MainActor
  func testNativePasteInsertsPersistedPointerAndReturnSendsIt() throws {
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    board.setData(
      try XCTUnwrap(bitmap().representation(using: .png, properties: [:])), forType: .png)
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }
    let asset = directory.appendingPathComponent("pasted.png")
    var draft = "Obejrzyj:"
    var sent: String?
    var rejectSave = false
    let host = NSHostingView(
      rootView: OverlayConversationComposer(
        palette: .dark, draft: Binding(get: { draft }, set: { draft = $0 }), sending: false,
        onSubmit: { sent = draft }, clipboard: board,
        saveImage: { bytes in
          if rejectSave { throw CocoaError(.fileWriteNoPermission) }
          try bytes.write(to: asset)
          return asset
        }))
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 400, height: 160)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    host.layoutSubtreeIfNeeded()
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
    func editor(in view: NSView) -> NSTextView? {
      if let text = view as? NSTextView, text.isEditable { return text }
      return view.subviews.lazy.compactMap { editor(in: $0) }.first
    }
    let text = try XCTUnwrap(editor(in: host))
    text.setSelectedRange(NSRange(location: text.string.utf16.count, length: 0))
    text.paste(nil)
    XCTAssertNil(sent, "Pasting must not publish before Send or Return")
    XCTAssertEqual(draft, "Obejrzyj:\n\(asset.path)\n")
    XCTAssertNotNil(NSImage(contentsOf: asset))
    let enter = try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
        windowNumber: window.windowNumber, context: nil, characters: "\r",
        charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36))
    text.keyDown(with: enter)
    XCTAssertEqual(sent, draft)
    rejectSave = true
    let beforeFailedPaste = draft
    text.paste(nil)
    XCTAssertEqual(draft, beforeFailedPaste, "A failed save must not insert a broken pointer")
  }
}

@MainActor
private final class ComposerTypingDraft: ObservableObject {
  @Published var text = ""
  @Published var repaint = 0
}

private struct ComposerTypingSurface: View {
  @ObservedObject var draft: ComposerTypingDraft

  var body: some View {
    VStack {
      Spacer(minLength: 0)
      OverlayConversationComposer(
        palette: draft.repaint.isMultiple(of: 2) ? .dark : .light,
        draft: $draft.text, sending: false, onSubmit: {})
    }
  }
}
