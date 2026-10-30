import AppKit
import Observation
import SwiftUI
import XCTest
import os

@testable import Codescribe

/// Independent contracts for 2bbee3d9e and its cursor-predicate parent.
/// Source assertions pin private SwiftUI ownership; they do not count body/layout passes.
@MainActor
final class OverlayHoverLayoutChurnTests: XCTestCase {
  func testCanvasRootDelegatesBothHoverPaintStatesToLocalOwners() throws {
    let source = try overlaySource()
    let root = try section(
      source, from: "struct DictationOverlayView: View", to: "private func canvasStack")
    let close = try section(
      source, from: "private struct OverlayCloseButton", to: "struct DictationOverlayView")
    let boundary = try section(
      source, from: "private struct OverlayPointerHoverChrome",
      to: "private struct OverlayCloseButton")
    XCTAssertFalse(root.contains("closeDotHovered"))
    XCTAssertFalse(root.contains("pointerInsideOverlay"))
    XCTAssertTrue(close.contains("@State private var closeDotHovered"))
    XCTAssertTrue(boundary.contains("@State private var pointerInsideOverlay"))
    XCTAssertTrue(root.contains("OverlayPointerHoverChrome("))
    XCTAssertTrue(
      root.contains("onHover: { state.setPointerHovering($0) }"), "Retain auto-hide/fold policy")
    let call = try section(
      source, from: "private var closeButton", to: "private func recordingControls")
    XCTAssertTrue(call.contains("OverlayCloseButton(palette: palette)"))
    XCTAssertTrue(call.contains("state.relayIntent(.close)"))
    XCTAssertFalse(close.contains("GeometryReader"))
    XCTAssertFalse(boundary.contains("GeometryReader"))
  }

  func testMainHeaderAndMapProjectionDoesNotSubscribeToChooserFocus() {
    let state = chooserState()
    defer { state.handleAgentGesture(.cancel) }
    let view = DictationOverlayView(state: state)
    let invalidated = OSAllocatedUnfairLock(initialState: false)
    withObservationTracking {
      XCTAssertNil(view.channelStatusView.gestureFocusID)
    } onChange: {
      invalidated.withLock { $0 = true }
    }
    state.hoverAgentGestureTarget("second")
    XCTAssertEqual(state.agentGestureFocusID, "second", "The input actually changed")
    XCTAssertFalse(
      invalidated.withLock { $0 }, "Unused chooser focus must not invalidate header/map")
    XCTAssertNil(view.channelStatusView.gestureFocusID)
  }

  func testDrawerAndRailObserveFocusInsideTheirOwnScope() throws {
    let source = try overlaySource()
    for (start, end) in [
      ("private func agentDrawerColumn", "private func conversationEdgeControls"),
      ("private func agentRail(", "private func agentDrawerResizeHandle"),
    ] {
      let slot = try section(source, from: start, to: end)
      let scope = try XCTUnwrap(slot.range(of: "OverlayObservationScope {"))
      let focus = try XCTUnwrap(slot.range(of: "roster.gestureFocusID = state.agentGestureFocusID"))
      let paint = try XCTUnwrap(slot.range(of: "return roster."))
      XCTAssertLessThan(scope.lowerBound, focus.lowerBound)
      XCTAssertLessThan(focus.lowerBound, paint.lowerBound)
    }
    let state = chooserState()
    defer { state.handleAgentGesture(.cancel) }
    let invalidated = OSAllocatedUnfairLock(initialState: false)
    withObservationTracking {
      XCTAssertEqual(
        OverlayChannelStatusView.intercomRoster(state: state, palette: .dark, animates: false)
          .gestureFocusID,
        "first", "The default Intercom projection retains focus")
    } onChange: {
      invalidated.withLock { $0 = true }
    }
    state.hoverAgentGestureTarget("second")
    XCTAssertTrue(invalidated.withLock { $0 })
    XCTAssertEqual(
      OverlayChannelStatusView.intercomRoster(state: state, palette: .dark, animates: false)
        .gestureFocusID, "second")
  }

  func testGestureRingTargetsTheHoveredRowBeforeTheSameTurnClick() throws {
    let source = try readSource("OverlayChannelStatusView.swift")
    let ring = try section(
      source, from: "struct OverlayGestureFocusRing", to: "struct OverlayAgentControlStyle")
    XCTAssertTrue(ring.contains("OverlayMenuCursorRegion(coalescesHover: false)"))
    var opened: [String] = []
    let state = chooserState(onOpen: { opened.append($0) })
    defer { state.handleAgentGesture(.cancel) }
    let host = NSHostingView(
      rootView: Color.clear.frame(width: 80, height: 30).modifier(
        OverlayGestureFocusRing(
          focused: false, shape: RoundedRectangle(cornerRadius: 4), palette: .dark,
          targetID: "second", onHover: { state.hoverAgentGestureTarget($0) })))
    host.frame = NSRect(x: 0, y: 0, width: 80, height: 30)
    host.sizingOptions = []
    let parent = panel(content: host)
    defer { parent.close() }
    host.layoutSubtreeIfNeeded()
    let region = try XCTUnwrap(regions(in: host).first { $0.onHover != nil })
    region.updateHover(at: region.convert(NSPoint(x: 10, y: 10), to: nil))
    XCTAssertTrue(region.hovered)
    XCTAssertEqual(
      state.agentGestureFocusID, "second", "No actor turn or paint interval before input")
    state.handleAgentGesture(.leftClick)
    XCTAssertEqual(opened, ["second"])
  }

  func testSameIntervalTransitionsDeliverOnlyLatestMembershipAndDeduplicate() async throws {
    let region = OverlayMenuCursorRegionView(frame: NSRect(x: 0, y: 0, width: 80, height: 30))
    let parent = panel(content: region)
    defer { parent.close() }
    var delivered: [Bool] = []
    region.onHover = { delivered.append($0) }
    for _ in 0..<100 {
      region.updateHover(at: NSPoint(x: 10, y: 10))
      XCTAssertTrue(region.hovered)
      region.updateHover(at: nil)
      XCTAssertFalse(region.hovered)
    }
    region.updateHover(at: NSPoint(x: 10, y: 10))
    XCTAssertTrue(delivered.isEmpty, "Raw membership changes before paint delivery")
    try await OverlayHoverDeliveryTestSupport.wait { delivered == [true] }
    for _ in 0..<100 { region.updateHover(at: NSPoint(x: 20, y: 10)) }
    try await OverlayHoverDeliveryTestSupport.passInterval(of: region)
    XCTAssertEqual(delivered, [true], "Unchanged membership never redelivers")
    region.updateHover(at: nil)
    XCTAssertFalse(region.hovered)
    XCTAssertEqual(delivered, [true], "A delivered exit is also deferred")
    try await OverlayHoverDeliveryTestSupport.wait { delivered == [true, false] }
  }

  func testUndeliveredEnterExitBurstNeverPaintsTransientHover() async throws {
    let region = OverlayMenuCursorRegionView(frame: NSRect(x: 0, y: 0, width: 80, height: 30))
    let parent = panel(content: region)
    defer { parent.close() }
    var delivered: [Bool] = []
    region.onHover = { delivered.append($0) }
    region.updateHover(at: NSPoint(x: 10, y: 10))
    region.updateHover(at: nil)
    XCTAssertFalse(region.hovered)
    XCTAssertTrue(delivered.isEmpty)
    try await OverlayHoverDeliveryTestSupport.passInterval(of: region)
    XCTAssertTrue(delivered.isEmpty, "Latest false equals the initial unpainted state")
  }

  func testCursorResolutionUsesMountedGeometryBeforeVisualHoverDelivery() async throws {
    let parent = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 300, height: 200),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    parent.isReleasedWhenClosed = false
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: 200))
    parent.contentView = content
    let region = OverlayMenuCursorRegionView(frame: NSRect(x: 20, y: 30, width: 200, height: 40))
    region.cursor = .resizeLeftRight
    content.addSubview(region)
    defer { parent.close() }
    var delivered: [Bool] = []
    region.onHover = { delivered.append($0) }
    let point = NSPoint(x: 100, y: 50)
    region.updateHover(at: point)
    XCTAssertTrue(region.hovered)
    XCTAssertTrue(delivered.isEmpty)
    XCTAssertTrue(parent.menuCursorRegions.contains(region))
    // Query the production decision without refreshCursor/sendEvent or a global cursor write.
    XCTAssertEqual(parent.cursor(at: point), .resizeLeftRight)
    region.removeFromSuperview()
    XCTAssertFalse(region.hovered)
    XCTAssertFalse(parent.menuCursorRegions.contains(region))
    XCTAssertEqual(
      parent.cursor(at: point), .arrow, "Removed geometry no longer owns the divider cursor")
    try await OverlayHoverDeliveryTestSupport.passInterval(of: region)
    XCTAssertTrue(delivered.isEmpty)
    XCTAssertFalse(parent.isVisible)
    XCTAssertFalse(parent.isKeyWindow)
  }

  func testUnmountCancelsPendingEntryAndImmediatelyClearsDeliveredHover() async throws {
    let region = OverlayMenuCursorRegionView(frame: NSRect(x: 0, y: 0, width: 80, height: 30))
    let content = NSView(frame: region.frame)
    content.addSubview(region)
    let parent = panel(content: content)
    defer { parent.close() }
    var delivered: [Bool] = []
    region.onHover = { delivered.append($0) }
    region.updateHover(at: NSPoint(x: 10, y: 10))
    XCTAssertTrue(region.hovered)
    XCTAssertTrue(delivered.isEmpty)
    region.removeFromSuperview()
    XCTAssertFalse(region.hovered)
    try await OverlayHoverDeliveryTestSupport.passInterval(of: region)
    XCTAssertTrue(delivered.isEmpty, "An unmounted pending entry must not flash")
    content.addSubview(region)
    region.updateHover(at: NSPoint(x: 10, y: 10))
    try await OverlayHoverDeliveryTestSupport.wait { delivered == [true] }
    region.updateHover(at: nil)  // There is now a pending visual exit.
    XCTAssertEqual(delivered, [true])
    region.removeFromSuperview()
    XCTAssertEqual(delivered, [true, false], "Unmount cannot wait for another paint interval")
    try await OverlayHoverDeliveryTestSupport.passInterval(of: region)
    XCTAssertEqual(delivered, [true, false], "Cancelled exit must not redeliver")
  }

  func testRetiredAreaAndForeignWindowCannotClearPendingOrDeliveredMembership() async throws {
    let region = OverlayMenuCursorRegionView(frame: NSRect(x: 0, y: 0, width: 80, height: 30))
    let parent = panel(content: region)
    let foreign = panel(content: NSView(frame: region.frame))
    defer {
      parent.close()
      foreign.close()
    }
    region.updateTrackingAreas()
    let retired = try XCTUnwrap(region.trackingAreas.first)
    region.setFrameSize(NSSize(width: 60, height: 30))
    region.updateTrackingAreas()
    let current = try XCTUnwrap(region.trackingAreas.first)
    XCTAssertFalse(current === retired)
    var delivered: [Bool] = []
    region.onHover = { delivered.append($0) }
    region.handleHoverEvent(at: NSPoint(x: 10, y: 10), from: current, in: parent)
    for _ in 0..<2 {
      region.handleHoverEvent(at: nil, from: retired, in: parent)
      region.handleHoverEvent(at: nil, from: current, in: foreign)
      XCTAssertTrue(region.hovered)
      if delivered.isEmpty {
        try await OverlayHoverDeliveryTestSupport.wait { delivered == [true] }
      } else {
        try await OverlayHoverDeliveryTestSupport.passInterval(of: region)
      }
      XCTAssertEqual(delivered, [true])
    }
    region.handleHoverEvent(at: nil, from: current, in: parent)
    XCTAssertFalse(region.hovered)
    XCTAssertEqual(delivered, [true])
    try await OverlayHoverDeliveryTestSupport.wait { delivered == [true, false] }
  }

  func testVisualTimerIsBoundedAndDoesNotOwnCursorOrHitDecisions() throws {
    let source = try readSource("OverlayWindowDragRegion.swift")
    let scheduling = try section(
      source, from: "private func setHovered", to: "override func updateTrackingAreas")
    XCTAssertTrue(
      scheduling.contains("max(30, min(window?.screen?.maximumFramesPerSecond ?? 60, 120))"))
    XCTAssertTrue(scheduling.contains("1_000_000_000 / framesPerSecond"))
    XCTAssertFalse(scheduling.contains("NSCursor"))
    XCTAssertFalse(scheduling.contains("refreshCursor"))
    let input = try section(source, from: "func updateHover", to: "private func setHovered")
    XCTAssertTrue(input.contains("bounds.intersection(visibleRect).contains"))
    XCTAssertFalse(input.contains("Task"))
    let panelSource = try readSource("DictationOverlayWindow.swift")
    let dispatch = try section(
      panelSource, from: "override func sendEvent", to: "if event.type == .leftMouseDown {")
    XCTAssertTrue(dispatch.contains("region.updateHover(at: event.locationInWindow)"))
    XCTAssertTrue(dispatch.contains("applyCursor(decision)"))
    XCTAssertFalse(dispatch.contains("await"))
    XCTAssertFalse(dispatch.contains("Task"))
  }

  func testDelayedHintCancellationLivesOutsideSwiftUIStateAndFrameWritesAreGuarded() throws {
    let source = try readSource("OverlayHeaderControlChrome.swift")
    let chrome = try section(
      source, from: "struct OverlayHeaderControlChrome", to: "final class OverlayHintAnchor")
    let anchor = try section(
      source, from: "final class OverlayHintAnchor", to: "private struct OverlayHintAnchorView")
    XCTAssertFalse(chrome.contains("pendingHint"))
    XCTAssertTrue(anchor.contains("private var pendingHint: Task<Void, Never>?"))
    XCTAssertFalse(anchor.contains("@State"))
    let hide = try section(anchor, from: "func hide(", to: "\n}")
    XCTAssertTrue(hide.contains("pendingHint?.cancel()"))
    XCTAssertTrue(
      source.contains("if panel.frame != frame { panel.setFrame(frame, display: true) }"))
  }

  func testDelayedHintExitAndUnmountCannotResurrectTheCancelledTitle() async throws {
    let source = try readSource("OverlayHeaderControlChrome.swift")
    let chrome = try section(
      source, from: "struct OverlayHeaderControlChrome", to: "final class OverlayHintAnchor")
    XCTAssertFalse(
      chrome.contains("pendingHint"), "The anchor owns cancellation across view updates")
    let host = NSHostingView(
      rootView: Color.clear.frame(width: 80, height: 30).modifier(
        OverlayHeaderControlChrome(
          title: "Delayed", accent: nil, palette: .dark, hintDelay: 0.02)))
    host.frame = NSRect(x: 0, y: 0, width: 80, height: 30)
    host.sizingOptions = []
    let parent = panel(content: host)
    host.layoutSubtreeIfNeeded()
    let region = try XCTUnwrap(regions(in: host).first { $0.onHover != nil })
    defer {
      region.onHover?(false)
      parent.close()
    }
    // Drive delivered visual input directly: timing of the bridge is tested separately.
    region.onHover?(true)
    XCTAssertNil(OverlayHintPanel.shared.presentedPanel)
    await Task.yield()
    region.onHover?(false)
    try await Task.sleep(for: .milliseconds(25))
    XCTAssertNil(OverlayHintPanel.shared.presentedPanel)
    region.onHover?(true)
    try await OverlayHoverDeliveryTestSupport.wait {
      OverlayHintPanel.shared.presentedTitle == "Delayed"
    }
    let hint = try XCTUnwrap(OverlayHintPanel.shared.presentedPanel)
    XCTAssertTrue(hint.ignoresMouseEvents)
    XCTAssertFalse(hint.canBecomeKey)
    XCTAssertTrue(hint.parent === parent)
    region.onHover?(false)
    XCTAssertNil(OverlayHintPanel.shared.presentedPanel)
    region.onHover?(true)
    await Task.yield()
    host.rootView = Color.clear.frame(width: 80, height: 30).modifier(
      OverlayHeaderControlChrome(
        title: "Replacement", accent: nil, palette: .dark, hintDelay: 0.02))
    // Remove the entire mounted control, including its anchor and cancellation owner.
    parent.contentView = NSView()
    host.layoutSubtreeIfNeeded()
    try await Task.sleep(for: .milliseconds(25))
    XCTAssertNil(OverlayHintPanel.shared.presentedPanel)
  }

  func testRegionOnlyCursorTransitionDoesNotWriteButRealChangesStillDo() throws {
    let arrow = NSObject()
    let resize = NSObject()
    let first = NSObject()
    let second = NSObject()
    var state = OverlayCursorState()
    XCTAssertTrue(
      state.update(zone: .menu, cursor: ObjectIdentifier(arrow), region: ObjectIdentifier(first)))
    for region in [second, first, second] {
      XCTAssertFalse(
        state.update(zone: .menu, cursor: ObjectIdentifier(arrow), region: ObjectIdentifier(region))
      )
    }
    XCTAssertTrue(
      state.update(zone: .menu, cursor: ObjectIdentifier(resize), region: ObjectIdentifier(second)))
    XCTAssertTrue(state.update(zone: .resize(.right), cursor: ObjectIdentifier(resize)))
    XCTAssertTrue(
      state.update(zone: .interior, cursor: nil), "Resize release restores the arrow once")
    XCTAssertFalse(state.update(zone: .interior, cursor: nil))
    XCTAssertTrue(state.update(zone: .menu, cursor: ObjectIdentifier(arrow)))
    XCTAssertFalse(state.update(zone: .outside, cursor: nil))
    XCTAssertTrue(
      state.update(zone: .menu, cursor: ObjectIdentifier(arrow)), "True boundary re-entry writes")
    XCTAssertTrue(state.update(zone: .dragStrip, cursor: ObjectIdentifier(arrow)))
    state = OverlayCursorState()  // The panel's boundary/order-out invalidation contract.
    XCTAssertTrue(state.update(zone: .dragStrip, cursor: ObjectIdentifier(arrow)))
    let source = try readSource("DictationOverlayWindow.swift")
    let invalidation = try section(
      source, from: "func invalidateCursorOwnership", to: "private func pointerDecision")
    XCTAssertTrue(invalidation.contains("cursorState = OverlayCursorState()"))
    XCTAssertTrue(invalidation.contains("override func orderOut"))
    XCTAssertTrue(invalidation.contains("invalidateCursorOwnership(reason: .orderOut)"))
  }

  private func chooserState(onOpen: @escaping (String) -> Void = { _ in }) -> OverlayState {
    let state = OverlayState()
    let targets = ["first", "second"].map {
      OverlayConversation(id: $0, channel: "4", name: $0, owner: nil, messages: [])
    }
    state.agentChooserCapture = { _ in }
    state.agentChooserHost = .init(targets: { targets }, open: onOpen)
    state.handleAgentGesture(.middleClick)
    return state
  }

  private func panel(content: NSView) -> NSPanel {
    let panel = NSPanel(
      contentRect: NSRect(x: 300, y: 400, width: 80, height: 30),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.contentView = content
    BackgroundTestWindow.present(panel)
    return panel
  }

  private func regions(in view: NSView) -> [OverlayMenuCursorRegionView] {
    (view as? OverlayMenuCursorRegionView).map { [$0] } ?? view.subviews.flatMap(regions)
  }

  private func overlaySource() throws -> String { try readSource("DictationOverlayView.swift") }

  private func readSource(_ name: String) throws -> String {
    let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent().appendingPathComponent("Codescribe/Screens/Overlay")
      .appendingPathComponent(name)
    return try String(contentsOf: url, encoding: .utf8)
  }

  private func section(_ source: String, from start: String, to end: String) throws -> String {
    let lower = try XCTUnwrap(source.range(of: start), start)
    let upper = try XCTUnwrap(source.range(of: end, range: lower.upperBound..<source.endIndex), end)
    return String(source[lower.lowerBound..<upper.lowerBound])
  }
}

/// Poll only until delivery occurs. Absence/cancellation checks cross one bounded
/// interval after yielding so the production main-actor task can start its timer.
@MainActor
enum OverlayHoverDeliveryTestSupport {
  static func wait(
    file: StaticString = #filePath, line: UInt = #line, until condition: () -> Bool
  ) async throws {
    let clock = ContinuousClock()
    let deadline = clock.now.advanced(by: .milliseconds(250))
    while !condition(), clock.now < deadline { try await Task.sleep(for: .milliseconds(1)) }
    XCTAssertTrue(condition(), "Hover delivery did not settle", file: file, line: line)
  }

  static func passInterval(of region: OverlayMenuCursorRegionView) async throws {
    let rate = max(30, min(region.window?.screen?.maximumFramesPerSecond ?? 60, 120))
    await Task.yield()
    try await Task.sleep(for: .nanoseconds(1_000_000_000 / rate + 1_000_000))
  }
}
