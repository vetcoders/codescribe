import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayRecordingContextMenuTests: XCTestCase {
  func testBothRecordingControlsOfferAgentsWithoutRecordingOrChangingDensity() throws {
    var agents = 0
    var recording = 0
    var density = 0
    var controls = OverlayRecordingControls(
      canFinish: false, presentationMode: .expanded, compact: false, palette: .dark,
      onIntent: { _ in recording += 1 }, onPreviewToggle: { density += 1 })
    controls.onShowAgents = { agents += 1 }
    for recordingOnly in [true, false] {
      controls.showsRecordingButton = recordingOnly
      controls.showsPreviewButton = !recordingOnly
      let host = NSHostingView(rootView: controls)
      host.sizingOptions = []
      host.frame = NSRect(x: 0, y: 0, width: 80, height: 40)
      let window = NSWindow(
        contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      BackgroundTestWindow.present(window)
      defer { window.close() }
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.08))
      host.layoutSubtreeIfNeeded()
      // Resolve the actual mounted hit area rather than assuming SwiftUI's alignment.
      var resolved: NSMenu?
      scan: for x in stride(from: CGFloat(2), to: host.bounds.width, by: 4) {
        for y in stride(from: CGFloat(2), to: host.bounds.height, by: 4) {
          let point = NSPoint(x: x, y: y)
          let event = try XCTUnwrap(
            NSEvent.mouseEvent(
              with: .rightMouseDown, location: host.convert(point, to: nil),
              modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
              windowNumber: window.windowNumber, context: nil, eventNumber: 1,
              clickCount: 1, pressure: 1))
          var view = host.hitTest(point)
          while let current = view, resolved == nil {
            resolved = current.menu(for: event)
            view = current.superview
          }
          if resolved != nil { break scan }
        }
      }
      let menu = try XCTUnwrap(resolved, "The actual control must resolve a context menu")
      menu.delegate?.menuNeedsUpdate?(menu)
      let index = try XCTUnwrap(menu.items.firstIndex { $0.title == String(localized: "Agents") })
      let before = agents
      menu.performActionForItem(at: index)
      XCTAssertEqual(agents, before + 1)
      XCTAssertEqual(recording, 0)
      XCTAssertEqual(density, 0)
    }
  }
}
