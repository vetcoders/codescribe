import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayComposerGeometryAcceptanceTests: XCTestCase {
  func testSharedAudioStateReviewImagesWithoutMotion() throws {
    for dark in [false, true] {
      let palette: OverlayAppearancePalette = dark ? .dark : .light
      let host = NSHostingView(
        rootView: HStack(spacing: 24) {
          ForEach(0..<3) { state in
            VStack {
              Text(verbatim: ["Unavailable", "Off", "On"][state]).font(.system(size: 11))
                .foregroundStyle(palette.primaryText.color)
              OverlayAgentAudioControls(
                open: state == 2, muted: state == 0 ? nil : state == 1,
                microphoneEnabled: state != 0, playbackEnabled: state != 0,
                palette: palette,
                onMicrophone: { XCTFail("Review must not start capture") },
                onPlayback: { XCTFail("Review must not change playback") },
                chrome: false, diameter: 30)
            }.frame(width: 90)
          }
        }
        .padding(12).background(palette.desktopBackground.color)
        .symbolEffectsRemoved()
        .transaction {
          $0.animation = nil
          $0.disablesAnimations = true
        }
        .environment(\.colorScheme, dark ? .dark : .light))
      host.sizingOptions = []
      host.frame = NSRect(x: 0, y: 0, width: 360, height: 100)
      let window = NSWindow(
        contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      defer { window.close() }
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.04))
      let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
      host.cacheDisplay(in: host.bounds, to: bitmap)
      let image = NSImage(size: host.bounds.size)
      image.addRepresentation(bitmap)
      let attachment = XCTAttachment(image: image)
      attachment.name = "Shared-audio-\(dark ? "dark" : "light")-without-motion"
      attachment.lifetime = .keepAlways
      add(attachment)
    }
  }

  func testControlStateReviewImagesInBothAppearances() throws {
    for dark in [false, true] {
      for state in ["unavailable", "off", "linked", "recording", "sending"] {
        let palette: OverlayAppearancePalette = dark ? .dark : .light
        let unavailable = state == "unavailable"
        let host = NSHostingView(
          rootView: OverlayConversationComposer(
            palette: palette,
            draft: .constant(state == "sending" ? "Sending draft" : ""),
            maximumEditorHeight: 96, sending: state == "sending", onSubmit: {},
            draftMicrophoneRecording: state == "recording",
            onToggleDraftMicrophone: unavailable ? nil : {},
            draftSyncChoices: [.init(id: "peer", title: "Peer", selected: false, enabled: true)],
            draftSyncNames: state == "linked" ? ["Agent", "Peer"] : [],
            draftSyncEnabled: !unavailable, onActivateDraftSync: {}
          )
          .environment(\.colorScheme, dark ? .dark : .light)
          .environment(\.csTextScale, 1)
          .background(palette.desktopBackground.color))
        host.sizingOptions = []
        host.frame = NSRect(x: 0, y: 0, width: 360, height: 120)
        let window = NSWindow(
          contentRect: NSRect(x: -10000, y: -10000, width: 360, height: 120),
          styleMask: .borderless, backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        defer { window.close() }
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.04))
        host.layoutSubtreeIfNeeded()
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        let image = NSImage(size: host.bounds.size)
        image.addRepresentation(bitmap)
        let attachment = XCTAttachment(image: image)
        attachment.name = "Composer-state-\(dark ? "dark" : "light")-\(state)"
        attachment.lifetime = .keepAlways
        add(attachment)
      }
    }
  }

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

  func testMultilineComposerKeepsTextAndScrollerBelowTheGlassEdgeWhileScrolling() throws {
    for width in [CGFloat(280), 420] {
      for scale in [CGFloat(1), 1.7] {
        let draft = (1...30).map { "Żółć \($0): the draft keeps a clear glass edge." }
          .joined(separator: "\n")
        let host = NSHostingView(
          rootView: OverlayConversationComposer(
            palette: .light, draft: .constant(draft), maximumEditorHeight: 96,
            sending: false, onSubmit: {}, onToggleDraftMicrophone: {}
          ).environment(\.csTextScale, scale))
        host.sizingOptions = []
        host.frame = NSRect(x: 0, y: 0, width: width, height: 180)
        let window = NSWindow(
          contentRect: NSRect(x: -10000, y: -10000, width: width, height: 180),
          styleMask: .borderless, backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        defer { window.close() }
        for _ in 0..<5 {
          host.layoutSubtreeIfNeeded()
          RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.02))
        }
        let editor = try XCTUnwrap(editableText(in: host))
        let scroll = try XCTUnwrap(editor.enclosingScrollView)
        let clip = scroll.contentView
        XCTAssertEqual(scroll.frame.height, 96, accuracy: 0.5, "the outer editor cap is unchanged")
        for fraction in [CGFloat(0), 0.5, 1] {
          let end = max(0, editor.frame.height - clip.bounds.height)
          clip.scroll(to: NSPoint(x: 0, y: end * fraction))
          scroll.reflectScrolledClipView(clip)
          host.layoutSubtreeIfNeeded()
          XCTAssertGreaterThanOrEqual(
            clip.frame.minY - scroll.bounds.minY, ComposerControlMetrics.textVerticalInset,
            "the top breathing space belongs to the viewport, not the scrolling document")
          XCTAssertEqual(clip.frame.maxY, scroll.bounds.maxY, accuracy: 0.5)
          let scroller = try XCTUnwrap(scroll.verticalScroller)
          XCTAssertGreaterThanOrEqual(scroller.frame.minY, clip.frame.minY)
        }
        let range = NSRange(location: (draft as NSString).length - 1, length: 1)
        editor.scrollRangeToVisible(range)
        let manager = try XCTUnwrap(editor.layoutManager)
        let container = try XCTUnwrap(editor.textContainer)
        manager.ensureLayout(for: container)
        let glyph = manager.boundingRect(
          forGlyphRange: manager.glyphRange(forCharacterRange: range, actualCharacterRange: nil),
          in: container).offsetBy(dx: editor.textContainerOrigin.x, dy: editor.textContainerOrigin.y)
        XCTAssertLessThanOrEqual(
          glyph.maxY, clip.bounds.maxY - ComposerControlMetrics.hitTargetSize + 1,
          "the last glyph stays above the stationary controls")
      }
    }
  }

  private func editableText(in view: NSView) -> NSTextView? {
    if let text = view as? NSTextView, text.isEditable { return text }
    return view.subviews.lazy.compactMap { self.editableText(in: $0) }.first
  }
}
