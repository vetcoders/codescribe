import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Opened URLs and refusals, recorded on the main actor.
@MainActor
private final class LinkOpenLog {
  var opened: [URL] = []
  var refusals = 0
}

@MainActor
final class MarkdownLinkPointerTests: XCTestCase {
  /// Founder, 2026-10-15: a click on a link in the overlay did nothing. A link
  /// lights up in the system accent under the pointer, shows the arrow instead
  /// of the text I-beam, and opens on click; text beside it does not.
  func testOverlayLinkShowsArrowLightsUpAndOpensOnClick() throws {
    let log = LinkOpenLog()
    let opener = MarkdownLinkOpener(
      open: { url in
        log.opened.append(url)
        return true
      },
      refused: { log.refusals += 1 })
    let panel = mount("see [the report](https://example.com/report) after the link", opener: opener)
    defer { panel.close() }

    let regions = linkRegions(in: panel)
    XCTAssertEqual(regions.count, 1, "One link, one pointer region")
    let region = try XCTUnwrap(regions.first)
    let link = region.convert(NSPoint(x: region.bounds.midX, y: region.bounds.midY), to: nil)
    let beside = NSPoint(x: 4, y: link.y)

    let saved = NSCursor.current
    defer { saved.set() }
    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: link))
    XCTAssertEqual(NSCursor.current, .arrow)

    panel.sendEvent(try mouse(.mouseMoved, at: link, in: panel))
    XCTAssertTrue(region.hovered)
    panel.sendEvent(try mouse(.mouseMoved, at: beside, in: panel))
    XCTAssertFalse(region.hovered)

    try click(at: link, in: panel)
    XCTAssertEqual(log.opened, [URL(string: "https://example.com/report")!])
    try click(at: beside, in: panel)
    XCTAssertEqual(log.opened.count, 1, "Text beside the link opens nothing")
    XCTAssertEqual(log.refusals, 0)
  }

  func testTextWithoutLinksMountsNoPointerRegion() {
    let panel = mount("plain words, `code`, no link", opener: .workspace)
    defer { panel.close() }
    XCTAssertTrue(linkRegions(in: panel).isEmpty)
  }

  /// The screenshot link named a path on another Mac; opening it failed
  /// without a sound or a sign. A missing file is refused, never handed on.
  func testMissingLocalFileIsRefusedAndWorkspaceRefusalIsReported() {
    var handed: [URL] = []
    let missing = URL(fileURLWithPath: "/Users/tester/.vibecrafted/reports/handoff.md")
    XCTAssertEqual(
      ConversationLocalFileLinks.open(
        missing, fileExists: { _ in false },
        open: {
          handed.append($0)
          return true
        }),
      .missingFile)
    XCTAssertTrue(handed.isEmpty)
    XCTAssertEqual(
      ConversationLocalFileLinks.open(
        URL(string: "https://example.com")!, fileExists: { _ in false }, open: { _ in false }),
      .refused)
    XCTAssertEqual(
      ConversationLocalFileLinks.open(
        URL(fileURLWithPath: "/tmp/report.md:12"), fileExists: { $0 == "/tmp/report.md" },
        open: {
          handed.append($0)
          return true
        }),
      .opened)
    XCTAssertEqual(handed, [URL(fileURLWithPath: "/tmp/report.md")])
  }

  private func mount(_ markdown: String, opener: MarkdownLinkOpener) -> FloatingOverlayPanel {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 100, y: 100, width: 420, height: 120),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    let host = NSHostingView(
      rootView: MarkdownText(raw: markdown)
        .environment(\.markdownLinkOpener, opener)
        .frame(width: 400, height: 100, alignment: .topLeading))
    host.frame = NSRect(x: 0, y: 0, width: 420, height: 120)
    panel.contentView = host
    panel.orderFrontRegardless()
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.3))
    host.layoutSubtreeIfNeeded()
    return panel
  }

  private func linkRegions(in panel: FloatingOverlayPanel) -> [OverlayMenuCursorRegionView] {
    panel.menuCursorRegions.allObjects
      .compactMap { $0 as? OverlayMenuCursorRegionView }
      .filter { $0.window === panel && !$0.bounds.isEmpty }
  }

  private func mouse(_ type: NSEvent.EventType, at point: NSPoint, in window: NSWindow) throws
    -> NSEvent
  {
    try XCTUnwrap(
      NSEvent.mouseEvent(
        with: type, location: point, modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
        context: nil, eventNumber: 1, clickCount: type == .mouseMoved ? 0 : 1, pressure: 1))
  }

  private func click(at point: NSPoint, in window: NSWindow) throws {
    for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
      window.sendEvent(try mouse(type, at: point, in: window))
      RunLoop.main.run(until: Date().addingTimeInterval(0.15))
    }
  }
}
