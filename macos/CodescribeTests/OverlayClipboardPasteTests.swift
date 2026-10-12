import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

final class OverlayClipboardPasteTests: XCTestCase {
  @MainActor
  func testComposerClickRetainsKeyAfterTheTranscriptResigns() throws {
    for prompt in [String(localized: "Send to everyone"), String(localized: "Message the Agent")] {
      var draft = ""
      var sends = 0
      let host = NSHostingView(
        rootView: OverlayConversationComposer(
          palette: .dark, draft: Binding(get: { draft }, set: { draft = $0 }), sending: false,
          onSubmit: { sends += 1 }, placeholder: prompt))
      host.sizingOptions = []
      host.frame = NSRect(x: 0, y: 0, width: 400, height: 100)
      let panel = FloatingOverlayPanel(
        contentRect: NSRect(x: 100, y: 100, width: 400, height: 300),
        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
      panel.isReleasedWhenClosed = false
      let content = NSView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
      let canvas = LiveTranscriptNativeTextView(
        frame: NSRect(x: 0, y: 150, width: 400, height: 100))
      canvas.isEditable = false
      canvas.isSelectable = true
      canvas.string = "Selected transcript"
      content.addSubview(canvas)
      content.addSubview(host)
      panel.contentView = content
      panel.orderFrontRegardless()
      defer {
        panel.makeFirstResponder(nil)
        panel.close()
      }
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.03))
      func editor(in view: NSView) -> NSTextView? {
        if let text = view as? NSTextView, text.isEditable { return text }
        return view.subviews.lazy.compactMap { editor(in: $0) }.first
      }
      let text = try XCTUnwrap(editor(in: host))
      XCTAssertTrue(panel.makeFirstResponder(canvas))
      panel.takeKeyForTranscript()
      XCTAssertTrue(panel.isKeyWindow)

      // A real native mouse-down transfers the responder. The old canvas
      // closes its keyboard gate while AppKit is accepting the new editor.
      let point = text.convert(NSPoint(x: 10, y: 10), to: nil)
      XCTAssertTrue(content.hitTest(point) === text, "The click must hit the visible composer")
      func mouse(_ type: NSEvent.EventType) throws -> NSEvent {
        try XCTUnwrap(
          NSEvent.mouseEvent(
            with: type, location: point, modifierFlags: [], timestamp: 0,
            windowNumber: panel.windowNumber, context: nil, eventNumber: 1,
            clickCount: 1, pressure: type == .leftMouseDown ? 1 : 0))
      }
      NSApp.postEvent(try mouse(.leftMouseUp), atStart: true)
      panel.sendEvent(try mouse(.leftMouseDown))
      XCTAssertTrue(
        panel.firstResponder === text,
        "\(prompt): responder=\(String(describing: panel.firstResponder)), editor=\(text)")
      XCTAssertTrue(panel.canBecomeKey, "The old canvas must not close the composer's gate")
      XCTAssertTrue(panel.isKeyWindow, "A visible caret must also receive keyboard input")
      let key = try XCTUnwrap(
        NSEvent.keyEvent(
          with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
          windowNumber: panel.windowNumber, context: nil, characters: "x",
          charactersIgnoringModifiers: "x", isARepeat: false, keyCode: 7))
      panel.sendEvent(key)
      XCTAssertEqual(draft, "x")
      XCTAssertEqual(sends, 0, "Focus and typing never send a message")
      XCTAssertEqual(canvas.string, "Selected transcript")
    }
  }

  @MainActor
  func testImageOnlyCommandVPastesThroughTheFocusedNativeComposer() throws {
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    let image = try bitmap()
    let pixels: [(Int, Int, NSColor)] = [
      (0, 0, .red), (1, 0, .green), (0, 1, .blue), (1, 1, .white),
    ]
    let rgba: [UInt8] = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]
    let buffer = try XCTUnwrap(image.bitmapData)
    rgba.withUnsafeBufferPointer { bytes in
      buffer.update(from: bytes.baseAddress!, count: bytes.count)
    }
    let png = try XCTUnwrap(image.representation(using: .png, properties: [:]))
    board.setData(png, forType: .png)
    XCTAssertNil(board.string(forType: .string))
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }
    let asset = directory.appendingPathComponent("image-only.png")
    var draft = "Obejrzyj:"
    var sends = 0
    var saves = 0
    let host = NSHostingView(
      rootView: OverlayConversationComposer(
        palette: .dark, draft: Binding(get: { draft }, set: { draft = $0 }), sending: false,
        onSubmit: { sends += 1 }, clipboard: board,
        saveImage: { bytes in
          saves += 1
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
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.03))
    func editor(_ view: NSView) -> NSTextView? {
      if let text = view as? NSTextView, text.isEditable { return text }
      return view.subviews.lazy.compactMap { editor($0) }.first
    }
    let text = try XCTUnwrap(editor(host))
    XCTAssertTrue(window.makeFirstResponder(text))
    text.setSelectedRange(NSRange(location: text.string.utf16.count, length: 0))
    func commandV(_ flags: NSEvent.ModifierFlags = .command) throws -> NSEvent {
      try XCTUnwrap(
        NSEvent.keyEvent(
          with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
          windowNumber: window.windowNumber, context: nil, characters: "v",
          charactersIgnoringModifiers: "v", isARepeat: false, keyCode: 9))
    }
    XCTAssertTrue(text.performKeyEquivalent(with: try commandV()))
    XCTAssertEqual(draft, "Obejrzyj:\n[attachment] \(asset.path)\n")
    let stored = try XCTUnwrap(NSBitmapImageRep(data: Data(contentsOf: asset)))
    XCTAssertEqual(stored.pixelsWide, 2)
    XCTAssertEqual(stored.pixelsHigh, 2)
    for (x, y, expected) in pixels {
      let color = try XCTUnwrap(stored.colorAt(x: x, y: y))
      let reference = try XCTUnwrap(expected.usingColorSpace(.deviceRGB))
      XCTAssertEqual(color.redComponent, reference.redComponent, accuracy: 1.0 / 255)
      XCTAssertEqual(color.greenComponent, reference.greenComponent, accuracy: 1.0 / 255)
      XCTAssertEqual(color.blueComponent, reference.blueComponent, accuracy: 1.0 / 255)
      XCTAssertEqual(color.alphaComponent, reference.alphaComponent, accuracy: 1.0 / 255)
    }
    XCTAssertEqual(saves, 1, "One shortcut must persist exactly one attachment")
    XCTAssertEqual(sends, 0, "Pasting never sends the draft")
    let pasted = draft
    _ = text.performKeyEquivalent(with: try commandV([.command, .shift]))
    XCTAssertEqual(draft, pasted, "Modified shortcuts must not paste an attachment")
    XCTAssertEqual(saves, 1)
    XCTAssertTrue(window.makeFirstResponder(nil))
    _ = text.performKeyEquivalent(with: try commandV())
    XCTAssertEqual(draft, pasted, "An unfocused composer must not steal Paste")
    XCTAssertEqual(saves, 1)
  }

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
  func testImagePayloadWinsOverTextButFinderFilesKeepOriginalBytes() throws {
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    let png = try XCTUnwrap(bitmap().representation(using: .png, properties: [:]))
    for type in [NSPasteboard.PasteboardType.string, .fileURL] {
      board.clearContents()
      board.setData(png, forType: .png)
      board.setString("file:///tmp/example.png", forType: type)
      if type == .fileURL {
        XCTAssertNil(try ConversationClipboardImage.png(from: board))
      } else {
        XCTAssertNotNil(try ConversationClipboardImage.png(from: board))
      }
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
    board.setString("Copied image.png", forType: .string)
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
    XCTAssertEqual(draft, "Obejrzyj:\n[attachment] \(asset.path)\n")
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
    let input = directory.appendingPathComponent("original.zip")
    let bytes = Data([80, 75, 3, 4, 0, 255])
    try bytes.write(to: input)
    board.clearContents()
    board.writeObjects([input as NSURL])
    text.setSelectedRange(NSRange(location: 0, length: "Obejrzyj:".utf16.count))
    text.paste(nil)
    XCTAssertFalse(draft.contains("Obejrzyj:"), "native paste replaces the actual selection")
    let copied = try XCTUnwrap(ConversationAttachmentPointer.extract(from: draft).paths.first)
    XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: copied)), bytes)
    text.keyDown(with: enter)
    XCTAssertEqual(sent, draft)
    try FileManager.default.removeItem(at: input)
    let retained = draft
    text.paste(nil)
    XCTAssertEqual(draft, retained, "unreadable files must retain the draft and its selection")
  }

  @MainActor
  func testFinderFilesAreCopiedByteExactlyAndSendOnlyPointers() throws {
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }
    let inputs = [
      ("notatka ze spacją.txt", Data("długi kontekst dla agenta".utf8)),
      ("archiwum.zip", Data([80, 75, 3, 4, 0, 255])),
      ("program.bin", Data([0, 1, 255, 0, 127])),
    ]
    let files = try inputs.map { name, data in
      let file = directory.appendingPathComponent(name)
      try data.write(to: file)
      return file
    }
    XCTAssertTrue(board.writeObjects(files as [NSURL]))
    let text = try XCTUnwrap(
      ConversationClipboardAttachment.text(
        from: board,
        saveImage: { _ in
          XCTFail("Finder must preserve original file bytes")
          return directory
        },
        saveFile: { data, name in
          URL(fileURLWithPath: try savePastedAttachment(data: data, name: name))
        }))
    let extracted = ConversationAttachmentPointer.extract(from: text)
    XCTAssertEqual(extracted.paths.count, 3)
    XCTAssertTrue(extracted.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    for (index, path) in extracted.paths.enumerated() {
      XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: path)), inputs[index].1)
      let root = URL(fileURLWithPath: CodescribeConfig().configDir()).resolvingSymlinksInPath()
      XCTAssertTrue(path.hasPrefix(root.appendingPathComponent("attachments").path + "/"))
    }
    // Removing the original files must not break the pending message.
    for file in files { try FileManager.default.removeItem(at: file) }
    XCTAssertTrue(extracted.paths.allSatisfy { FileManager.default.fileExists(atPath: $0) })
    XCTAssertFalse(text.contains("długi kontekst dla agenta"))
  }

  @MainActor
  func testLongUTF8PasteIsExternalizedAndShortTextStaysEditable() throws {
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    let long = String(repeating: "zażółć gęślą jaźń\n", count: 1000)
    board.setString(long, forType: .string)
    let pasted = try XCTUnwrap(
      ConversationClipboardAttachment.text(
        from: board, saveImage: { _ in throw CocoaError(.fileReadCorruptFile) },
        saveFile: { data, name in
          URL(fileURLWithPath: try savePastedAttachment(data: data, name: name))
        }))
    XCTAssertLessThan(pasted.utf8.count, 1024)
    let path = try XCTUnwrap(ConversationAttachmentPointer.extract(from: pasted).paths.first)
    XCTAssertEqual(try String(contentsOfFile: path, encoding: .utf8), long)
    board.clearContents()
    board.setString("krótki tekst", forType: .string)
    XCTAssertEqual(
      try ConversationClipboardAttachment.text(
        from: board, saveImage: { _ in throw CocoaError(.fileWriteUnknown) },
        saveFile: { _, _ in
          XCTFail("short text must not create a file")
          return URL(fileURLWithPath: "/")
        }),
      "krótki tekst")
  }

  @MainActor
  func testPDFObjectAndStoredFileIdentity() throws {
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    let bytes = Data("%PDF-test\n".utf8)
    board.setData(bytes, forType: .pdf)
    let pointer = try XCTUnwrap(
      ConversationClipboardAttachment.text(
        from: board, saveImage: { _ in throw CocoaError(.fileWriteUnknown) },
        saveFile: { data, name in
          URL(fileURLWithPath: try savePastedAttachment(data: data, name: name))
        }))
    let path = try XCTUnwrap(ConversationAttachmentPointer.extract(from: pointer).paths.first)
    XCTAssertTrue(path.hasSuffix(".pdf"))
    XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: path)), bytes)
    XCTAssertEqual(
      URL(fileURLWithPath: try savePastedAttachment(data: bytes, name: "Pasted.pdf"))
        .resolvingSymlinksInPath().path, path)
    let changed = try savePastedAttachment(data: Data([1, 2, 3]), name: "Pasted.pdf")
    XCTAssertNotEqual(changed, path)
    XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: path)), bytes)
    XCTAssertThrowsError(try savePastedAttachment(data: Data(), name: "empty.bin"))
  }

  @MainActor
  func testOversizeOrDirectoryPasteDoesNotSaveAnyPointer() throws {
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    let file = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    XCTAssertTrue(FileManager.default.createFile(atPath: file.path, contents: Data()))
    defer { try? FileManager.default.removeItem(at: file) }
    let handle = try FileHandle(forWritingTo: file)
    try handle.truncate(atOffset: UInt64(ConversationClipboardAttachment.maximumBytes + 1))
    try handle.close()
    for source in [file, FileManager.default.temporaryDirectory] {
      board.clearContents()
      board.writeObjects([source as NSURL])
      XCTAssertThrowsError(
        try ConversationClipboardAttachment.text(
          from: board,
          saveImage: { _ in
            XCTFail("must refuse before save")
            return file
          },
          saveFile: { _, _ in
            XCTFail("must refuse before save")
            return file
          }))
    }
  }

  func testAttachmentMarkersRetainProseRejectOutsidePathsAndFollowDraftDeletion() {
    let root = URL(fileURLWithPath: "/tmp/cs-attachments-test")
    let pointer = root.appendingPathComponent("attachments/clipboard_abc_note.txt")
    let text =
      "proszę przeczytaj\n" + ConversationAttachmentPointer.line(pointer) + "\npotem odpowiedz"
    let extracted = ConversationAttachmentPointer.extract(from: text, configDirectory: root)
    XCTAssertEqual(extracted.text, "proszę przeczytaj\npotem odpowiedz")
    XCTAssertEqual(extracted.paths, [pointer.path])
    let removed = ConversationAttachmentPointer.extract(from: extracted.text, configDirectory: root)
    XCTAssertTrue(removed.paths.isEmpty)
    for outside in [
      "/etc/passwd", root.path + "/attachments-other/a.txt",
      root.path + "/attachments/../settings.json",
    ] {
      let draft = "[attachment] " + outside
      let parsed = ConversationAttachmentPointer.extract(from: draft, configDirectory: root)
      XCTAssertEqual(parsed.text, draft)
      XCTAssertTrue(parsed.paths.isEmpty)
    }
  }

  @MainActor
  func testAppPublisherPassesAttachmentArgumentsWithoutInliningTheirContents() async throws {
    let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    let bin = home.appendingPathComponent(".local/bin")
    try FileManager.default.createDirectory(at: bin, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: home) }
    let executable = bin.appendingPathComponent("cs-bus")
    try """
    #!/usr/bin/python3
    # codescribe-managed-command: {"managed_id":"clipboard-test"}
    import json, pathlib, sys
    (pathlib.Path(__file__).parent / 'published.json').write_text(
        json.dumps({'arguments': sys.argv[1:], 'text': sys.stdin.read()}))
    """.write(to: executable, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: executable.path)
    let file = try savePastedAttachment(
      data: Data("NEVER INLINE THIS PAYLOAD".utf8), name: "data.txt")
    let draft = "Przeczytaj plik\n[attachment] \(file)\n"
    try await RealAgentBridgeInstaller.sendBusText(
      owner: nil, text: draft,
      installer: RealAgentBridgeInstaller(resourceRoot: nil, homeDirectory: home, environment: [:]))
    let receipt = try XCTUnwrap(
      JSONSerialization.jsonObject(
        with: Data(contentsOf: bin.appendingPathComponent("published.json")))
        as? [String: Any])
    let arguments = try XCTUnwrap(receipt["arguments"] as? [String])
    XCTAssertEqual(arguments.prefix(3), ["--send-text", "--channel", "0"])
    XCTAssertEqual(
      Array(arguments.suffix(2)),
      ["--attach-file", URL(fileURLWithPath: file).resolvingSymlinksInPath().path])
    let text = try XCTUnwrap(receipt["text"] as? String)
    XCTAssertEqual(text.trimmingCharacters(in: .whitespacesAndNewlines), "Przeczytaj plik")
    XCTAssertFalse(text.contains("NEVER INLINE"))
    XCTAssertFalse(
      text.contains("[attachment]"), "canonical helper appends exactly one metadata pointer")
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
