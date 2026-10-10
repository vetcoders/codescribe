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
