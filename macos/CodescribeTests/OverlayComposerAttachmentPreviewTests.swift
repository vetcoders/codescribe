import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Falsifiers for the composer-preview cut (Maciej, channel 0, 2026-10-17
/// 01:30Z, two screenshots): a click on an image in a bubble opens the in-app
/// preview, the same click on the attachment chip in the overlay composer
/// opened nothing, and the preview tucked the image's own corners under the
/// card's rounding. These pin the chip body being one preview button, the
/// remove cross staying its own control, and the inset that keeps the image
/// clear of the card's corner arc.
@MainActor
final class OverlayComposerAttachmentPreviewTests: XCTestCase {

  // MARK: - Fixtures

  private func source(at relative: String) throws -> String {
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent()
    return try String(contentsOf: root.appendingPathComponent(relative), encoding: .utf8)
  }

  private func section(of source: String, from: String, to: String) throws -> String {
    let start = try XCTUnwrap(source.range(of: from), "missing section start: \(from)")
    let end = try XCTUnwrap(
      source.range(of: to, range: start.upperBound..<source.endIndex),
      "missing section end: \(to)")
    return String(source[start.lowerBound..<end.lowerBound])
  }

  /// One real click through the window's event path: down, up, settle.
  private func click(_ window: NSWindow, at location: NSPoint) throws {
    for (kind, number, pressure) in [
      (NSEvent.EventType.leftMouseDown, 1, 1.0), (.leftMouseUp, 2, 0.0),
    ] {
      let event = try XCTUnwrap(
        NSEvent.mouseEvent(
          with: kind, location: location, modifierFlags: [],
          timestamp: ProcessInfo.processInfo.systemUptime,
          windowNumber: window.windowNumber, context: nil,
          eventNumber: number, clickCount: 1, pressure: Float(pressure)))
      window.sendEvent(event)
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.03))
    }
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
  }

  /// A pasted PNG on disk, the shape the composer's pills carry.
  private func pastedImage() throws -> String {
    let directory = FileManager.default.temporaryDirectory
      .appendingPathComponent("composer-preview-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let image = NSImage(size: NSSize(width: 8, height: 8), flipped: false) { rect in
      NSColor.systemRed.setFill()
      rect.fill()
      return true
    }
    let tiff = try XCTUnwrap(image.tiffRepresentation)
    let png = try XCTUnwrap(
      NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]))
    let file = directory.appendingPathComponent("pasted.png")
    try png.write(to: file)
    addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
    return file.path
  }

  /// The pill hosted at its own size, so the thumbnail sits at the leading
  /// edge and the remove cross at the trailing one.
  private func host(
    _ pill: OverlayComposerAttachmentPill
  ) -> (window: NSWindow, host: NSHostingView<OverlayComposerAttachmentPill>) {
    let host = NSHostingView(rootView: pill)
    host.sizingOptions = []
    host.frame = NSRect(origin: .zero, size: host.fittingSize)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    BackgroundTestWindow.present(window)
    host.layoutSubtreeIfNeeded()
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
    host.layoutSubtreeIfNeeded()
    return (window, host)
  }

  /// Inside the thumbnail: 9 pt of pill padding, then the 18 pt glyph.
  private func bodyPoint(in host: NSView) -> NSPoint {
    NSPoint(x: 18, y: host.bounds.midY)
  }

  /// Inside the remove cross: 9 pt of pill padding, then the 14 pt button.
  private func crossPoint(in host: NSView) -> NSPoint {
    NSPoint(x: host.bounds.maxX - 16, y: host.bounds.midY)
  }

  // MARK: - Composer chip

  func testClickingTheChipBodyPresentsThePreviewSheet() throws {
    let path = try pastedImage()
    var removed = 0
    let hosted = host(
      OverlayComposerAttachmentPill(path: path, palette: .dark, onRemove: { removed += 1 }))
    defer { hosted.window.close() }
    XCTAssertGreaterThan(hosted.host.bounds.width, 60, "the pill laid out at its own width")
    let windowsBefore = Set(NSApp.windows.map(ObjectIdentifier.init))

    try click(hosted.window, at: bodyPoint(in: hosted.host))
    let deadline = Date(timeIntervalSinceNow: 2)
    while hosted.window.attachedSheet == nil,
      Set(NSApp.windows.map(ObjectIdentifier.init)) == windowsBefore, Date() < deadline
    {
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
    }
    let presented =
      hosted.window.attachedSheet != nil
      || Set(NSApp.windows.map(ObjectIdentifier.init)) != windowsBefore
    XCTAssertTrue(presented, "the chip body opens the preview sheet")
    XCTAssertEqual(removed, 0, "previewing does not remove the attachment")
    if let sheet = hosted.window.attachedSheet { hosted.window.endSheet(sheet) }
  }

  func testClickingTheRemoveCrossRemovesWithoutPresentingThePreview() throws {
    let path = try pastedImage()
    var removed = 0
    let hosted = host(
      OverlayComposerAttachmentPill(path: path, palette: .dark, onRemove: { removed += 1 }))
    defer { hosted.window.close() }
    let windowsBefore = Set(NSApp.windows.map(ObjectIdentifier.init))

    try click(hosted.window, at: crossPoint(in: hosted.host))
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3))
    XCTAssertEqual(removed, 1, "the cross is its own control")
    XCTAssertNil(hosted.window.attachedSheet, "removing never opens the preview")
    XCTAssertEqual(
      Set(NSApp.windows.map(ObjectIdentifier.init)), windowsBefore,
      "removing never opens the preview")
  }

  func testComposerChipPresentsTheSamePreviewSheetAsABubbleAttachment() throws {
    let composer = try source(at: "Codescribe/Screens/Overlay/OverlayConversationComposer.swift")
    let pill = try section(
      of: composer, from: "struct OverlayComposerAttachmentPill: View",
      to: "private struct ConversationMessageField")
    XCTAssertTrue(pill.contains(".sheet(isPresented: $showPreview)"))
    XCTAssertTrue(pill.contains("AttachmentPreviewSheet(attachment: attachment)"))
    XCTAssertTrue(
      pill.contains("MessageAttachment(name: name, url: URL(fileURLWithPath: path))"),
      "the composer builds the same attachment shape the bubbles do")
    // The cross keeps its own action; the preview button never wraps it.
    let previewButton = try section(
      of: pill, from: "showPreview = true",
      to: "\"overlay-conversation-attachment-preview\"")
    XCTAssertFalse(previewButton.contains("onRemove"))
  }

  // MARK: - Preview sheet corners

  func testPreviewSheetKeepsTheImageClearOfTheCardCornerArc() throws {
    let source = try source(at: "Codescribe/Screens/AgentChat/MessageList.swift")
    let sheet = try section(
      of: source, from: "struct AttachmentPreviewSheet: View", to: "private func missingBanner")
    let declaration = try XCTUnwrap(
      sheet.firstMatch(of: /static let imageInset: CGFloat = ([0-9.]+)/),
      "the sheet names the inset that keeps the image off the card's corners")
    let inset = try XCTUnwrap(Double(declaration.1))
    XCTAssertGreaterThanOrEqual(
      inset, CSRadius.card, "an inset below the card radius lets the corner arc cut the image")
    let viewport = try section(
      of: sheet, from: "ScrollView([.horizontal, .vertical])",
      to: ".clipShape(RoundedRectangle(cornerRadius: CSRadius.card")
    XCTAssertTrue(viewport.contains(".padding(Self.imageInset)"), "the image carries the inset")
  }
}
