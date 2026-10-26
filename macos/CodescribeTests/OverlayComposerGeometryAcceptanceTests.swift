import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayComposerGeometryAcceptanceTests: XCTestCase {
  func testMultilineEditorUsesTheWholeComposerWidthAndKeepsTheLastLineVisible() throws {
    for dark in [false, true] {
      for width in [CGFloat(280), 420, 640] {
        let draft = (1...12).map { "Line \($0): readable text across the whole editor." }
          .joined(separator: "\n")
        let palette: OverlayAppearancePalette = dark ? .dark : .light
        let host = NSHostingView(
          rootView: OverlayConversationComposer(
            palette: palette, draft: .constant(draft), maximumEditorHeight: 96,
            sending: false, onSubmit: {}, onToggleDraftMicrophone: {},
            draftSyncChoices: [.init(id: "peer", title: "Peer", selected: false, enabled: true)],
            draftSyncEnabled: true, onActivateDraftSync: {}
          )
          .environment(\.colorScheme, dark ? .dark : .light)
          .environment(\.csTextScale, 1)
          .background(palette.desktopBackground.color))
        host.sizingOptions = []
        host.frame = NSRect(x: 0, y: 0, width: width, height: 180)
        let window = NSWindow(
          contentRect: NSRect(x: -10000, y: -10000, width: width, height: 180),
          styleMask: .borderless, backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        defer { window.close() }
        for _ in 0..<3 {
          host.layoutSubtreeIfNeeded()
          RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.04))
        }
        let editor = try XCTUnwrap(editableText(in: host))
        let scroll = try XCTUnwrap(editor.enclosingScrollView)
        let viewport = scroll.convert(scroll.bounds, to: host)
        XCTAssertGreaterThanOrEqual(
          viewport.width, width - 40,
          "Actions must sit below prose instead of reserving a full-height side column")
        XCTAssertGreaterThan(viewport.maxX, width - 24)
        XCTAssertEqual(scroll.scrollerStyle, .overlay)
        XCTAssertTrue(scroll.autohidesScrollers)
        XCTAssertFalse(scroll.hasHorizontalScroller)
        let range = NSRange(location: (draft as NSString).length - 1, length: 1)
        editor.scrollRangeToVisible(range)
        editor.layoutSubtreeIfNeeded()
        let manager = try XCTUnwrap(editor.layoutManager)
        let container = try XCTUnwrap(editor.textContainer)
        manager.ensureLayout(for: container)
        var glyph = manager.boundingRect(
          forGlyphRange: manager.glyphRange(forCharacterRange: range, actualCharacterRange: nil),
          in: container)
        glyph.origin.x += editor.textContainerOrigin.x
        glyph.origin.y += editor.textContainerOrigin.y
        XCTAssertGreaterThanOrEqual(glyph.minY, editor.visibleRect.minY - 1)
        XCTAssertLessThanOrEqual(glyph.maxY, editor.visibleRect.maxY + 1)
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        let image = NSImage(size: host.bounds.size)
        image.addRepresentation(bitmap)
        let attachment = XCTAttachment(image: image)
        attachment.name = "Composer-\(dark ? "dark" : "light")-\(Int(width))-multiline"
        attachment.lifetime = .keepAlways
        add(attachment)
      }
    }
  }

  private func editableText(in view: NSView) -> NSTextView? {
    if let text = view as? NSTextView, text.isEditable { return text }
    return view.subviews.lazy.compactMap { self.editableText(in: $0) }.first
  }
}
