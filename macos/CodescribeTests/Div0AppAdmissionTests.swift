import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class Div0AppAdmissionTests: XCTestCase {
  func testExistingMarkdownLinkExposesPassivePointerRegion() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 100, y: 100, width: 420, height: 120),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    defer { panel.close() }
    let host = NSHostingView(
      rootView: MarkdownText(raw: "see [the report](https://example.com/report) after the link")
        .frame(width: 400, height: 100, alignment: .topLeading))
    host.frame = NSRect(x: 0, y: 0, width: 420, height: 120)
    panel.contentView = host
    panel.orderFrontRegardless()
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.3))
    host.layoutSubtreeIfNeeded()
    let regions = panel.menuCursorRegions.allObjects
      .compactMap { $0 as? OverlayMenuCursorRegionView }
      .filter { $0.window === panel && !$0.bounds.isEmpty }
    XCTAssertEqual(regions.count, 1, "A rendered link must answer the pointer")
  }
}
