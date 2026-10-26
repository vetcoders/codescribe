import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Cross-generation witnesses: this file uses APIs available on the baseline
/// and candidate, so a baseline failure is behavioral rather than an API error.
@MainActor
final class OverlayWidgetLifecycleRegressionTests: XCTestCase {
  private func owner() throws -> OverlayConversationOwner {
    let session = "widget-lifecycle-fixture"
    return try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session,
        "lease_id": AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: session),
        "channel": "2", "name": "Fixture agent",
      ]))
  }

  func testCanonicalArchiveRetiresDetachedWindowAndPreservesSavedConversation() async throws {
    let suite = "CodescribeTests.widget-archive.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let state = OverlayState()
    let owner = try owner()
    let conversation = OverlayConversation(
      id: owner.id, channel: owner.channel, name: owner.name, owner: owner, messages: [])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
    state.applyChannelRoster([
      .init(
        channel: owner.channel, audience: owner.name, provider: owner.provider,
        providerSessionId: owner.providerSessionID, open: false, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: false, unreadSinceUnixMs: nil)
    ])
    let store = OverlayIntercomDesktopStore(defaults: defaults)
    let controller = OverlayIntercomWidgetController(
      state: state, textScale: TextScaleController(key: suite), desktopStore: store)
    defer { controller.invalidate() }
    state.publishConversationText = { _, _, _, _ in XCTFail("Presentation must not send") }
    var archiveCalls = 0
    state.archiveAgentCommand = { actual, release in
      XCTAssertEqual(actual, owner)
      XCTAssertFalse(release)
      archiveCalls += 1
    }
    state.conversationDrafts[conversation.id] = "Retained exact-owner draft"
    let generation = state.captureGeneration
    controller.setVisible(true, relativeTo: nil)
    XCTAssertTrue(controller.detach(.init(digit: owner.channel, conversation: conversation)))
    let saved = try XCTUnwrap(store.load().instances.first)
    XCTAssertTrue(controller.presentsConversation(conversation.id))

    await state.archiveAgent(owner)
    for _ in 0..<100 {
      if store.load().instances.first?.closed == true { break }
      try await Task.sleep(for: .milliseconds(20))
    }
    let archived = try XCTUnwrap(store.load().instances.first)
    XCTAssertTrue(archived.closed, "Canonical archive must retire its detached native window")
    XCTAssertFalse(controller.presentsConversation(conversation.id))
    XCTAssertEqual(archived.id, saved.id)
    XCTAssertEqual(archived.ownerID, saved.ownerID)
    XCTAssertEqual(archived.frame, saved.frame)
    XCTAssertEqual(state.conversationDrafts[conversation.id], "Retained exact-owner draft")
    XCTAssertEqual(state.conversations.map(\.id), [conversation.id])
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertEqual(archiveCalls, 1)
    XCTAssertTrue(state.pendingTextMessages.isEmpty)
  }

  func testInactiveCurrentOwnerCanDockWithoutSending() throws {
    let suite = "CodescribeTests.widget-inactive.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let state = OverlayState()
    let owner = try owner()
    let conversation = OverlayConversation(
      id: owner.id, channel: owner.channel, name: owner.name, owner: owner, messages: [])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
    state.applyChannelRoster([
      .init(
        channel: owner.channel, audience: owner.name, provider: owner.provider,
        providerSessionId: owner.providerSessionID, open: false, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: false, unreadSinceUnixMs: nil)
    ])
    let store = OverlayIntercomDesktopStore(defaults: defaults)
    let controller = OverlayIntercomWidgetController(
      state: state, textScale: TextScaleController(key: suite), desktopStore: store)
    defer { controller.invalidate() }
    state.publishConversationText = { _, _, _, _ in XCTFail("Docking must not send") }
    XCTAssertTrue(controller.detach(.init(digit: owner.channel, conversation: conversation)))
    let instance = try XCTUnwrap(store.load().instances.first)
    controller.closeInstance(instance.id)
    XCTAssertEqual(store.load().instances.count, 1)
    XCTAssertTrue(try XCTUnwrap(store.load().instances.first).closed)
    XCTAssertEqual(state.visibleChannelRows.map(\.channel), [owner.channel])
    XCTAssertTrue(state.pendingTextMessages.isEmpty)
  }

  func testCompactHintUsesFreeSideWithoutCoveringItsAnchor() {
    for screen in [
      CGRect(x: 0, y: 0, width: 1440, height: 900),
      CGRect(x: -1920, y: 100, width: 1920, height: 1080),
    ] {
      for y in [screen.minY + 10, screen.midY, screen.maxY - 40] {
        let anchor = CGRect(x: screen.minX + 10, y: y, width: 150, height: 30)
        let frame = OverlayHintPanel.frame(
          size: CGSize(width: 360, height: 240), leading: anchor, within: screen)
        XCTAssertGreaterThanOrEqual(frame.minX, anchor.maxX, "Use the free side at the left edge")
        XCTAssertFalse(frame.intersects(anchor), "Preview must leave its control reachable")
        XCTAssertTrue(screen.contains(frame))
      }
    }
  }

  func testIncreasingHeightNeverMovesActionsBelowTheAgentName() {
    for scale in [CGFloat(1), 1.5, 2] {
      let metrics = IntercomContentMetrics(names: ["Agent"], digits: ["2"], scale: scale)
      let width = max(metrics.minimumNameWidth, metrics.actionsWidth - 1)
      let expected = metrics.tier(width: width, cardHeight: metrics.iconSide)
      for height in [2 * metrics.iconSide, 8 * metrics.iconSide, 1800] {
        XCTAssertEqual(metrics.tier(width: width, cardHeight: height), expected)
      }
    }
  }
  func testIntercomMaterialKeepsCanvasDefaultAndHonorsOpaqueAccessibility() throws {
    for palette in [OverlayAppearancePalette.light, .dark] {
      let originalAlpha: CGFloat =
        palette.appearance == .dark && BuildChannel.isNightly() ? 0.92 : 0.84
      XCTAssertEqual(
        OverlayCanvasBackdrop.glassVeil(palette).alphaComponent, originalAlpha, accuracy: 0.001)
      for opaque in [false, true] {
        let host = NSHostingView(
          rootView: OverlayCanvasBackdrop(
            palette: palette, reduceTransparency: opaque
          )
          .frame(width: 300, height: 100))
        host.frame.size = NSSize(width: 300, height: 100)
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        func hasMaterial(_ view: NSView) -> Bool {
          if view is NSVisualEffectView { return true }
          if #available(macOS 26.0, *), view is NSGlassEffectView { return true }
          return view.subviews.contains { hasMaterial($0) }
        }
        XCTAssertEqual(
          hasMaterial(host), !opaque, "Opaque accessibility must bypass the native material")
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        let image = NSImage(cgImage: try XCTUnwrap(bitmap.cgImage), size: host.bounds.size)
        let attachment = XCTAttachment(image: image)
        attachment.name = "intercom-material-\(palette.appearance)-opaque-\(opaque)"
        attachment.lifetime = .keepAlways
        add(attachment)
      }
      let highContrast = NSHostingView(
        rootView: OverlayCanvasBackdrop(
          palette: palette, reduceTransparency: false
        )
        .environment(\.csContrastOverride, .increased).frame(width: 300, height: 100))
      highContrast.frame.size = NSSize(width: 300, height: 100)
      highContrast.layoutSubtreeIfNeeded()
      func nativeGlassCount(_ view: NSView) -> Int {
        let native: Int
        if #available(macOS 26.0, *) {
          native = view is NSGlassEffectView ? 1 : 0
        } else {
          native = view is NSVisualEffectView ? 1 : 0
        }
        return native + view.subviews.map { nativeGlassCount($0) }.reduce(0, +)
      }
      XCTAssertEqual(nativeGlassCount(highContrast), 0)
    }
  }

  func testNativeHeaderClickFoldsOnceWhileDraggingDoesNotFold() throws {
    let suite = "CodescribeTests.widget-header-gesture.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let state = OverlayState()
    let generation = state.captureGeneration
    state.publishConversationText = { _, _, _, _ in XCTFail("Header gestures must not send") }
    let controller = OverlayIntercomWidgetController(
      state: state, textScale: TextScaleController(key: suite),
      desktopStore: OverlayIntercomDesktopStore(defaults: defaults))
    defer { controller.invalidate() }
    let host = NSHostingView(
      rootView: controller.offscreenHeader(width: 400)
        .frame(width: 400, height: 30))
    let panel = IntercomGestureWitnessPanel(
      contentRect: NSRect(x: 0, y: 0, width: 400, height: 30),
      styleMask: [.borderless], backing: .buffered, defer: false)
    panel.contentView = host
    defer {
      panel.contentView = nil
      panel.close()
    }
    host.frame.size = NSSize(width: 400, height: 30)
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    func descendants(_ view: NSView) -> [NSView] {
      [view] + view.subviews.flatMap { descendants($0) }
    }
    let drag = try XCTUnwrap(
      descendants(host).first {
        String(describing: type(of: $0)).contains("OverlayIntercomDragView")
      })
    func event(_ type: NSEvent.EventType, x: CGFloat) throws -> NSEvent {
      try XCTUnwrap(
        NSEvent.mouseEvent(
          with: type, location: NSPoint(x: x, y: 15), modifierFlags: [], timestamp: 0,
          windowNumber: panel.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1
        ))
    }
    let initial = controller.presentation.folded
    drag.mouseDown(with: try event(.leftMouseDown, x: 150))
    drag.mouseUp(with: try event(.leftMouseUp, x: 150))
    XCTAssertEqual(controller.presentation.folded, !initial)
    drag.mouseDown(with: try event(.leftMouseDown, x: 150))
    drag.mouseUp(with: try event(.leftMouseUp, x: 150))
    XCTAssertEqual(controller.presentation.folded, initial)
    let originalDown = try event(.leftMouseDown, x: 150)
    drag.mouseDown(with: originalDown)
    drag.mouseDragged(with: try event(.leftMouseDragged, x: 180))
    drag.mouseUp(with: try event(.leftMouseUp, x: 180))
    XCTAssertEqual(panel.dragCalls, 1, "One native drag per gesture")
    XCTAssertTrue(panel.dragEvent === originalDown, "AppKit receives the original mouse-down")
    XCTAssertEqual(controller.presentation.folded, initial, "Dragging must not also collapse")
    XCTAssertEqual(controller.interactionWindowCounts.owned, 0)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingTextMessages.isEmpty)
  }

}

@MainActor
private final class IntercomGestureWitnessPanel: NSPanel {
  var dragCalls = 0
  var dragEvent: NSEvent?
  override func performDrag(with event: NSEvent) {
    dragCalls += 1
    dragEvent = event
  }
}
