import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayHoverInteractionTests: XCTestCase {
  func testToolPanelKeepsRailOpenUntilItCloses() {
    var actions = OverlayActionsPresentation()
    actions.pointerChanged(true)
    XCTAssertEqual(actions.phase, .idle)
    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
    actions.panelChanged(true)
    actions.pointerChanged(false)
    actions.expire(at: .now.advanced(by: .seconds(20)))
    XCTAssertEqual(actions.phase, .open)
    XCTAssertNil(actions.hideDeadline)
    actions.panelChanged(false)
    XCTAssertNotNil(actions.hideDeadline)
    actions.expire(at: .now.advanced(by: .seconds(4)))
    XCTAssertEqual(actions.phase, .idle)
  }

  func testKeyboardFocusDoesNotOpenToolsBeforeActivation() {
    var actions = OverlayActionsPresentation()
    actions.focusChanged(true)
    XCTAssertEqual(actions.phase, .idle)
    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
    XCTAssertNil(actions.hideDeadline)
    actions.focusChanged(false)
    XCTAssertNotNil(actions.hideDeadline)
  }

  func testHoverPanelDoesNotResizeItsAnchorOrPerformAnAction() throws {
    let model = HarnessModel()
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 400, y: 300, width: 320, height: 120),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    let host = FirstMouseHostingView(rootView: Harness(model: model))
    host.sizingOptions = []
    panel.contentView = host
    panel.orderFrontRegardless()
    defer {
      model.presented = nil
      panel.orderOut(nil)
    }
    settle(host)
    let before = model.anchor
    XCTAssertGreaterThan(before.width, 0)
    click(panel, host: host, anchor: before)
    settle(host)
    XCTAssertEqual(model.actions, 1, "Baseline synthetic click must reach the button")
    model.actions = 0
    let keyBefore = NSApp.keyWindow
    model.presented = "hover-probe"
    settle(host)
    XCTAssertEqual(model.anchor, before, "A long tooltip must not move the icon")
    XCTAssertEqual(model.actions, 0, "Presentation is never execution")
    XCTAssertFalse(panel.canBecomeKey)
    XCTAssertTrue(NSApp.keyWindow === keyBefore, "Hover must not steal the typing focus")
    let popup = try XCTUnwrap(
      NSApp.windows.first {
        $0.isVisible && String(describing: type(of: $0)).contains("Popover")
      })
    let anchorInWindow = host.convert(before, to: nil)
    let anchorOnScreen = panel.convertToScreen(anchorInWindow)
    XCTAssertGreaterThan(popup.frame.midY, anchorOnScreen.midY, "Tooltip belongs above its button")
    let content = try XCTUnwrap(popup.contentView)
    let bitmap = try XCTUnwrap(content.bitmapImageRepForCachingDisplay(in: content.bounds))
    content.cacheDisplay(in: content.bounds, to: bitmap)
    try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(
      to: FileManager.default.temporaryDirectory.appendingPathComponent(
        "codescribe-hover-panel.png"))

    // Route a paired click through the real non-activating panel, not the closure.
    // A visible tooltip must not consume the first click intended for its button.
    click(panel, host: host, anchor: before)
    settle(host)
    XCTAssertEqual(model.actions, 1, "One click executes once even with a tooltip visible")
  }

  func testDetailsRequireClickAndRemainOpenWithoutHover() throws {
    let model = HarnessModel()
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 400, y: 300, width: 320, height: 120),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    let host = FirstMouseHostingView(rootView: Harness(model: model, opensDetails: true))
    host.sizingOptions = []
    panel.contentView = host
    panel.orderFrontRegardless()
    defer {
      model.presented = nil
      panel.orderOut(nil)
    }
    settle(host)
    XCTAssertNil(model.presented)
    let anchor = model.anchor
    click(panel, host: host, anchor: anchor)
    settle(host)
    XCTAssertEqual(model.presented, "hover-probe")
    RunLoop.main.run(until: Date().addingTimeInterval(0.3))
    XCTAssertEqual(
      model.presented, "hover-probe", "Leaving the anchor must not dismiss clicked details")
    XCTAssertEqual(model.actions, 0, "Opening choices must not execute a command")
    XCTAssertEqual(model.anchor, anchor)
  }

  private func click(_ panel: NSPanel, host: NSView, anchor: CGRect) {
    let rect = host.convert(anchor, to: nil)
    let location = NSPoint(x: rect.midX, y: rect.midY)
    for (type, number) in [(NSEvent.EventType.leftMouseDown, 1), (.leftMouseUp, 2)] {
      let event = NSEvent.mouseEvent(
        with: type, location: location, modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime,
        windowNumber: panel.windowNumber, context: nil, eventNumber: number, clickCount: 1,
        pressure: 1)!
      panel.sendEvent(event)
    }
  }

  private final class FirstMouseHostingView<Content: View>: NSHostingView<Content> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  }

  private func settle(_ view: NSView) {
    view.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.12))
    view.layoutSubtreeIfNeeded()
  }

  @Observable final class HarnessModel {
    var presented: String?
    var actions = 0
    var anchor: CGRect = .zero
  }

  private struct Harness: View {
    @Bindable var model: HarnessModel
    var opensDetails = false

    var body: some View {
      HStack {
        OverlayHoverControl(
          id: "hover-probe", title: "Copy transcript", palette: .dark,
          presented: $model.presented, action: opensDetails ? nil : { model.actions += 1 }
        ) {
          Image(systemName: "doc.on.doc").frame(width: 24, height: 24)
        } detail: { _ in
          Text("Copy this transcript without changing the original recording")
        }
        .onGeometryChange(for: CGRect.self) {
          $0.frame(in: .global)
        } action: {
          model.anchor = $0
        }
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
  }
}
