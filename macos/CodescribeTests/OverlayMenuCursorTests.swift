import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayMenuCursorTests: XCTestCase {
  func testContentBoundaryLayoutChurnDoesNotRearmCursorWrites() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 400),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered, defer: false)
    let content = OverlayContentContainer(hosting: NSView())
    panel.contentView = content
    panel.setPresentationMode(.expanded)
    defer { panel.close() }
    content.updateTrackingAreas()
    let boundary = try XCTUnwrap(content.trackingAreas.first)
    let saved = NSCursor.current
    defer { saved.set() }
    let point = NSPoint(x: 5, y: 200)
    XCTAssertTrue(panel.refreshCursor(at: point))
    var writes = 0
    for _ in 0..<100 {
      content.updateTrackingAreas()
      if panel.refreshCursor(at: point) { writes += 1 }
    }
    XCTAssertEqual(content.trackingAreas.count, 1)
    XCTAssertTrue(content.trackingAreas.first === boundary)
    XCTAssertEqual(writes, 0)
    panel.setContentSize(NSSize(width: 500, height: 300))
    content.updateTrackingAreas()
    XCTAssertTrue(content.trackingAreas.first === boundary)
    XCTAssertTrue(boundary.options.contains(.inVisibleRect))
    XCTAssertEqual(content.visibleRect.size, NSSize(width: 500, height: 300))
    XCTAssertFalse(panel.isVisible)
  }

  func testContentBoundaryIgnoresForeignEventsButRearmsRealExitAndEntry() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 400),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered, defer: false)
    let foreign = OverlayIntercomPanel()
    let content = OverlayContentContainer(hosting: NSView())
    panel.contentView = content
    panel.setPresentationMode(.expanded)
    defer {
      panel.close()
      foreign.close()
    }
    content.updateTrackingAreas()
    let boundary = try XCTUnwrap(content.trackingAreas.first)
    let stale = NSTrackingArea(
      rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
      owner: content, userInfo: nil)
    let saved = NSCursor.current
    defer { saved.set() }
    let point = NSPoint(x: 5, y: 200)
    XCTAssertTrue(panel.refreshCursor(at: point))
    content.invalidateCursorOwnership(from: stale, in: panel)
    XCTAssertFalse(panel.refreshCursor(at: point))
    content.invalidateCursorOwnership(from: boundary, in: foreign)
    XCTAssertFalse(panel.refreshCursor(at: point))
    content.invalidateCursorOwnership(from: boundary, in: panel)
    XCTAssertTrue(panel.refreshCursor(at: point), "Actual boundary exit relinquishes ownership")
    XCTAssertFalse(panel.refreshCursor(at: point))
    content.invalidateCursorOwnership(from: boundary, in: panel)
    XCTAssertTrue(panel.refreshCursor(at: point), "Actual re-entry establishes ownership again")
    XCTAssertFalse(panel.isVisible)
  }

  func testRetiredTrackingAreaExitCannotClearANewerNativeHoverEntry() throws {
    let panel = OverlayIntercomPanel()
    let foreign = OverlayIntercomPanel()
    panel.setFrame(NSRect(x: 0, y: 0, width: 80, height: 30), display: false)
    let region = OverlayMenuCursorRegionView(frame: NSRect(x: 0, y: 0, width: 80, height: 30))
    panel.contentView = region
    defer {
      panel.close()
      foreign.close()
    }
    region.updateTrackingAreas()
    let retired = try XCTUnwrap(region.trackingAreas.first)
    region.setFrameSize(NSSize(width: 60, height: 30))
    region.updateTrackingAreas()
    let current = try XCTUnwrap(region.trackingAreas.first)
    XCTAssertFalse(retired === current)
    var changes: [Bool] = []
    region.onHover = { changes.append($0) }
    region.handleHoverEvent(at: NSPoint(x: 10, y: 10), from: current, in: panel)
    XCTAssertTrue(region.hovered)
    region.handleHoverEvent(at: nil, from: retired, in: panel)
    XCTAssertTrue(region.hovered)
    region.handleHoverEvent(at: nil, from: current, in: foreign)
    XCTAssertTrue(region.hovered)
    XCTAssertEqual(changes, [true])
    region.handleHoverEvent(at: nil, from: current, in: panel)
    XCTAssertFalse(region.hovered)
    XCTAssertEqual(changes, [true, false])
    XCTAssertFalse(panel.isVisible)
    XCTAssertFalse(foreign.isVisible)
  }

  func testUnchangedIntercomHoverGeometryKeepsOneTrackingArea() throws {
    let panel = OverlayIntercomPanel()
    panel.setFrame(NSRect(x: 0, y: 0, width: 80, height: 30), display: false)
    let region = OverlayMenuCursorRegionView(frame: NSRect(x: 0, y: 0, width: 80, height: 30))
    panel.contentView = region
    defer { panel.close() }
    region.updateTrackingAreas()
    let original = try XCTUnwrap(region.trackingAreas.first)
    var retained = true
    for _ in 0..<200 {
      region.updateTrackingAreas()
      XCTAssertEqual(region.trackingAreas.count, 1)
      retained = retained && region.trackingAreas.first === original
    }
    XCTAssertTrue(retained)
    region.setFrameSize(NSSize(width: 60, height: 30))
    region.updateTrackingAreas()
    XCTAssertEqual(region.trackingAreas.count, 1)
    XCTAssertFalse(region.trackingAreas.first === original)
    XCTAssertFalse(panel.isVisible)
    XCTAssertFalse(panel.isKeyWindow)
  }

  func testNativeIntercomHoverClearsWhenItsRegionLeavesTheWindow() {
    let panel = OverlayIntercomPanel()
    panel.setFrame(NSRect(x: 0, y: 0, width: 80, height: 30), display: false)
    let region = OverlayMenuCursorRegionView(frame: NSRect(x: 0, y: 0, width: 80, height: 30))
    var changes: [Bool] = []
    region.onHover = { changes.append($0) }
    panel.contentView = region
    defer { panel.close() }
    region.updateHover(at: NSPoint(x: 10, y: 10))
    region.updateHover(at: NSPoint(x: 15, y: 10))
    XCTAssertTrue(region.hovered)
    XCTAssertEqual(changes, [true])
    panel.contentView = NSView()
    XCTAssertFalse(region.hovered)
    XCTAssertEqual(changes, [true, false])
    XCTAssertFalse(panel.isVisible)
    XCTAssertFalse(panel.isKeyWindow)
  }

  func testInteriorDecisionBudgetAcrossTenThousandMoves() {
    let bounds = NSRect(x: 0, y: 0, width: 600, height: 500)
    var state = OverlayCursorState()
    var editableHitDecisions = 0
    var cursorWrites = 0
    for step in 0..<10_000 {
      let point = NSPoint(x: 100 + step % 300, y: 100 + step % 200)
      let zone = OverlayPointerZone.resolve(
        at: point, in: bounds, mode: .expanded, resizable: true,
        menuContains: { false }, dragContains: { false })
      XCTAssertEqual(zone, .interior)
      if zone == .menu { editableHitDecisions += 1 }
      if state.update(zone: zone, cursor: nil) { cursorWrites += 1 }
    }
    XCTAssertEqual(editableHitDecisions, 0)
    XCTAssertEqual(cursorWrites, 0)
  }

  func testGeometryPrecedenceAndPresentationModes() {
    let bounds = NSRect(x: 0, y: 0, width: 600, height: 500)
    var menuLookups = 0
    var dragLookups = 0
    func zone(_ point: NSPoint, mode: OverlayPresentationMode = .expanded) -> OverlayPointerZone {
      OverlayPointerZone.resolve(
        at: point, in: bounds, mode: mode, resizable: true,
        menuContains: {
          menuLookups += 1
          return true
        },
        dragContains: {
          dragLookups += 1
          return true
        })
    }
    for (point, edge) in [
      (NSPoint(x: 5, y: 200), OverlayResizeHit.Edge.left),
      (NSPoint(x: 595, y: 200), .right),
      (NSPoint(x: 200, y: 495), .top),
      (NSPoint(x: 200, y: 5), .bottom),
      (NSPoint(x: 5, y: 495), .topLeft),
      (NSPoint(x: 595, y: 495), .topRight),
      (NSPoint(x: 25, y: 25), .bottomLeft),
      (NSPoint(x: 575, y: 25), .bottomRight),
    ] {
      XCTAssertEqual(zone(point), .resize(edge))
    }
    // Bottom capsule and its margin belong to the same geometry owner.
    let grip = OverlayResizeChrome.gripRect(in: bounds).insetBy(dx: 0, dy: -4)
    for y in [grip.minY, grip.midY, grip.maxY] {
      XCTAssertEqual(zone(NSPoint(x: grip.midX, y: y)), .resize(.bottom))
    }
    XCTAssertEqual(menuLookups, 0, "Resize never inspects a menu or text")
    XCTAssertEqual(dragLookups, 0)
    XCTAssertEqual(zone(NSPoint(x: -1, y: 250)), .outside)
    XCTAssertEqual(menuLookups, 0)
    XCTAssertEqual(zone(NSPoint(x: 300, y: 250)), .menu)
    XCTAssertEqual(dragLookups, 0, "Mounted menu wins over overlapping drag chrome")
    XCTAssertEqual(zone(NSPoint(x: 5, y: 250), mode: .mini), .menu)
    XCTAssertEqual(zone(NSPoint(x: 5, y: 250), mode: .midi), .resize(.left))
  }

  func testCursorStateChangesOnlyForZoneCursorOrMountedOwner() {
    let arrow = NSObject()
    let resize = NSObject()
    let drawer = NSObject()
    let divider = NSObject()
    var state = OverlayCursorState()
    var writes = 0
    for _ in 0..<1_000 {
      if state.update(
        zone: .menu, cursor: ObjectIdentifier(arrow), region: ObjectIdentifier(drawer))
      {
        writes += 1
      }
    }
    XCTAssertEqual(writes, 1)
    XCTAssertTrue(
      state.update(zone: .menu, cursor: ObjectIdentifier(arrow), region: ObjectIdentifier(divider)),
      "A newly mounted owner may need to establish the same cursor")
    XCTAssertTrue(
      state.update(zone: .menu, cursor: ObjectIdentifier(resize), region: ObjectIdentifier(divider))
    )
    XCTAssertTrue(state.update(zone: .resize(.right), cursor: ObjectIdentifier(resize)))
    XCTAssertFalse(state.update(zone: .resize(.right), cursor: ObjectIdentifier(resize)))
    XCTAssertTrue(state.update(zone: .dragStrip, cursor: ObjectIdentifier(arrow)))
    XCTAssertFalse(state.update(zone: .interior, cursor: nil), "Native text releases ownership")
    XCTAssertTrue(state.update(zone: .dragStrip, cursor: ObjectIdentifier(arrow)))
    state = OverlayCursorState()  // Boundary exit/re-entry or orderOut.
    XCTAssertTrue(state.update(zone: .dragStrip, cursor: ObjectIdentifier(arrow)))
  }

  func testPanelInteriorRefreshNeverHitTestsAndSameMenuIgnoresCurrentCursorReset() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered, defer: false)
    let content = CursorHitCountingView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    panel.contentView = content
    let body = OverlayWindowDragRegionView(frame: content.bounds)
    body.setAccessibilityIdentifier("overlay-body-drag-region")
    content.addSubview(body)
    let header = OverlayWindowDragRegionView(frame: NSRect(x: 50, y: 450, width: 500, height: 40))
    header.setAccessibilityIdentifier("overlay-header-drag-region")
    content.addSubview(header)
    let menu = OverlayMenuCursorRegionView(frame: NSRect(x: 300, y: 100, width: 280, height: 300))
    content.addSubview(menu)
    let saved = NSCursor.current
    defer {
      panel.close()
      saved.set()
    }

    content.hits = 0
    for _ in 0..<1_000 {
      XCTAssertFalse(panel.refreshCursor(at: NSPoint(x: 100, y: 200)))
    }
    XCTAssertEqual(content.hits, 0, "The body drag background cannot cause a motion hit test")
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: 100, y: 470)))
    XCTAssertFalse(panel.refreshCursor(at: NSPoint(x: 110, y: 470)))
    XCTAssertEqual(content.hits, 0, "Header motion is geometry only")
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: 400, y: 200)))
    XCTAssertEqual(content.hits, 1, "Only the menu asks about editable text")
    NSCursor.iBeam.set()  // Model an AppKit reset between unchanged events.
    XCTAssertFalse(panel.refreshCursor(at: NSPoint(x: 400, y: 200)))
    XCTAssertEqual(NSCursor.current, .iBeam, "Do not restart the cursor ping-pong")
    panel.invalidateCursorOwnership()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: 400, y: 200)))
    header.removeFromSuperview()
    XCTAssertFalse(panel.windowDragRegions.contains(header))
    menu.removeFromSuperview()
    content.hits = 0
    XCTAssertFalse(panel.refreshCursor(at: NSPoint(x: 400, y: 200)))
    XCTAssertEqual(content.hits, 0, "Removed menu immediately returns the interior budget")
  }

  func testLeavingResizeResetsArrowOnceAndThenYieldsToNativeText() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered, defer: false)
    let content = CursorHitCountingView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    panel.contentView = content
    let saved = NSCursor.current
    defer {
      panel.close()
      saved.set()
    }
    let edge = NSPoint(x: 1, y: 250)
    let interior = NSPoint(x: 200, y: 250)
    XCTAssertTrue(panel.refreshCursor(at: edge))
    XCTAssertEqual(NSCursor.current, OverlayResizeHit.cursor(for: .left))
    XCTAssertTrue(panel.refreshCursor(at: interior))
    XCTAssertEqual(NSCursor.current, .arrow)
    NSCursor.iBeam.set()
    XCTAssertFalse(panel.refreshCursor(at: interior))
    XCTAssertEqual(NSCursor.current, .iBeam)
    XCTAssertEqual(content.hits, 0)

    XCTAssertTrue(panel.refreshCursor(at: edge))
    NSCursor.pointingHand.set()
    XCTAssertFalse(panel.refreshCursor(at: NSPoint(x: -1, y: 250)))
    XCTAssertEqual(NSCursor.current, .pointingHand, "Outside belongs to the destination window")
  }

  func testLeavingResizeForAnEditableMenuReleasesCursorOwnership() {
    let resize = NSObject()
    var state = OverlayCursorState()
    XCTAssertTrue(state.update(zone: .resize(.left), cursor: ObjectIdentifier(resize)))
    XCTAssertTrue(state.update(zone: .menu, cursor: nil))
    XCTAssertFalse(state.update(zone: .menu, cursor: nil))
  }

  private struct DividerIdentityWitness: View {
    var tick: Int

    var body: some View {
      HStack(spacing: 0) {
        Text(String(tick)).frame(width: 700, height: 400)
        Rectangle().frame(width: 10, height: 400)
          .background { OverlayMenuCursorRegion(cursor: .resizeLeftRight) }
        Color.clear.frame(width: 290, height: 400)
      }
      .frame(width: 1000, height: 500)
    }
  }

  func testHostedSidebarDividerUpdatesKeepCursorAndRegionIdentity() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 1000, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    let host = NSHostingView(rootView: DividerIdentityWitness(tick: 0))
    panel.contentView = host
    host.layoutSubtreeIfNeeded()
    let region = try XCTUnwrap(
      panel.menuCursorRegions.allObjects.first as? OverlayMenuCursorRegionView)
    let originalFrame = panel.frame
    let regionFrame = region.frame
    let saved = NSCursor.current
    defer {
      panel.close()
      saved.set()
    }
    let point = region.convert(NSPoint(x: region.bounds.midX, y: region.bounds.midY), to: nil)
    XCTAssertTrue(panel.refreshCursor(at: point))
    var retainedCursors: [NSCursor] = [region.cursor]
    var writes = 0
    for tick in 1...100 {
      host.rootView = DividerIdentityWitness(tick: tick)
      host.layoutSubtreeIfNeeded()
      let mounted = try XCTUnwrap(
        panel.menuCursorRegions.allObjects.first as? OverlayMenuCursorRegionView)
      retainedCursors.append(mounted.cursor)
      XCTAssertTrue(mounted === region, "an ordinary root update must preserve the divider")
      XCTAssertEqual(mounted.frame, regionFrame)
      XCTAssertEqual(panel.frame, originalFrame)
      if panel.refreshCursor(at: point) { writes += 1 }
    }
    let cursorIDs = Set(retainedCursors.map(ObjectIdentifier.init))
    print(
      "divider-identity-witness updates=100 retained=\(retainedCursors.count) cursorIDs=\(cursorIDs.count) regionIDs=1 writesAfterInitial=\(writes) presented=\(panel.isVisible)"
    )
    XCTAssertEqual(cursorIDs.count, 1)
    XCTAssertEqual(writes, 0)
    XCTAssertFalse(panel.isVisible, "the fixture never presents a window on the shared desktop")
  }

  func testSidebarDividerCursorWinsInsideDrawerAndYieldsWhenRemoved() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 1000, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 1000, height: 500))
    panel.contentView = content
    let drawer = OverlayMenuCursorRegionView(
      frame: NSRect(x: 700, y: 0, width: 300, height: 400))
    let divider = OverlayMenuCursorRegionView(
      frame: NSRect(x: 700, y: 0, width: 10, height: 400))
    divider.cursor = .resizeLeftRight
    content.addSubview(drawer)
    content.addSubview(divider)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 705, y: 200)), .resizeLeftRight)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 800, y: 200)), .arrow)
    XCTAssertEqual(
      panel.cursor(at: NSPoint(x: 705, y: 450)), .arrow,
      "the divider does not own the header")
    divider.removeFromSuperview()
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 705, y: 200)), .arrow)
  }

  func testHoverUsesPanelMotionAndClearsOnNativeExitAndRemoval() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 100, y: 100, width: 300, height: 200),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: 200))
    panel.contentView = content
    let region = OverlayMenuCursorRegionView(frame: NSRect(x: 20, y: 30, width: 200, height: 40))
    var transitions: [Bool] = []
    region.onHover = { transitions.append($0) }
    content.addSubview(region)
    BackgroundTestWindow.present(panel)
    content.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    defer { panel.close() }
    func move(_ point: NSPoint) throws {
      panel.sendEvent(
        try XCTUnwrap(
          NSEvent.mouseEvent(
            with: .mouseMoved, location: point, modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
            context: nil, eventNumber: 1, clickCount: 0, pressure: 0)))
    }
    try move(NSPoint(x: 100, y: 50))
    try move(NSPoint(x: 110, y: 50))
    XCTAssertEqual(transitions, [true], "Motion inside does not repeat the enter")
    try move(NSPoint(x: 350, y: 250))
    XCTAssertEqual(transitions, [true, false])
    try move(NSPoint(x: 100, y: 50))
    region.mouseExited(
      with: try XCTUnwrap(
        NSEvent.enterExitEvent(
          with: .mouseExited, location: NSPoint(x: 350, y: 250), modifierFlags: [],
          timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
          context: nil, eventNumber: 2, trackingNumber: 0, userData: nil)))
    XCTAssertEqual(transitions, [true, false, true, false])
    try move(NSPoint(x: 100, y: 50))
    region.removeFromSuperview()
    XCTAssertEqual(transitions, [true, false, true, false, true, false])
    XCTAssertTrue(panel.menuCursorRegions.allObjects.isEmpty)
  }

  func testProductionHeaderRegistersItsVisibleControlsWithThePanel() throws {
    let state = OverlayState()
    state.setPresentationMode(.midi)
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "OverlayMenuCursorTests.header"))
        as? FloatingOverlayPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    BackgroundTestWindow.present(panel)
    let content = try XCTUnwrap(panel.contentView)
    content.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    content.layoutSubtreeIfNeeded()
    let controls = panel.menuCursorRegions.allObjects.filter {
      $0.window === panel && !$0.isHiddenOrHasHiddenAncestor && !$0.visibleRect.isEmpty
    }
    XCTAssertGreaterThanOrEqual(
      controls.count, 4, "The real header must wire its controls, not only the drawer")
    let saved = NSCursor.current
    defer { saved.set() }
    for control in controls {
      let point = control.convert(NSPoint(x: control.bounds.midX, y: control.bounds.midY), to: nil)
      NSCursor.iBeam.set()
      XCTAssertTrue(panel.refreshCursor(at: point), "Mounted control at \(point)")
      XCTAssertEqual(NSCursor.current, .arrow)
    }
  }

  /// Founder, 2026-10-15: an I-beam showed between and over the agent's
  /// microphone and speaker, which float above message text. The pair and
  /// the gap between its buttons claim the arrow from the text below.
  func testAgentAudioControlsClaimTheArrowAboveText() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 300, height: 120),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: 120))
    let text = NSTextView(frame: content.bounds)
    text.isHorizontallyResizable = false
    text.isVerticallyResizable = false
    text.isEditable = false
    text.string = String(repeating: "Wiadomość od agenta pod przyciskami. ", count: 12)
    content.addSubview(text)
    let controls = NSHostingView(
      rootView: OverlayAgentAudioControls(
        open: false, muted: false, microphoneEnabled: true, playbackEnabled: true,
        palette: .dark, onMicrophone: {}, onPlayback: {}))
    controls.frame = NSRect(x: 180, y: 70, width: 100, height: 40)
    content.addSubview(controls)
    panel.contentView = content
    content.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    content.layoutSubtreeIfNeeded()

    let region = try XCTUnwrap(
      panel.menuCursorRegions.allObjects.first { $0.window === panel },
      "the controls register a cursor region")
    let frame = region.convert(region.bounds, to: nil)
    XCTAssertGreaterThan(frame.width, 50, "both buttons and their gap, got \(frame)")
    let gap = NSPoint(x: frame.midX, y: frame.midY)
    XCTAssertEqual(panel.cursor(at: gap), .arrow, "region \(frame)")
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 40, y: 30)), .iBeam, "text keeps its cursor")
  }

  // MARK: - Narrow agent list, composer chips, bubble chips (Maciej, 2026-10-17 01:15Z)

  /// Maciej, 2026-10-17 01:15Z: the non-activating panel gets no native
  /// cursor updates, so an I-beam a text view set stays until the panel
  /// resets it, and it resets it only above registered regions. Below the
  /// threshold the agents list covers the window (2026-10-19) and claims the
  /// arrow across its whole width, as the docked column does.
  func testNarrowAgentListClaimsTheArrowAcrossTheWindow() throws {
    let state = OverlayState.previewFormatted()
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "cursor-list",
        "lease_id": String(repeating: "c", count: 32), "channel": "2", "name": "lena",
      ]))
    let conversation = OverlayConversation(
      id: owner.id, channel: "2", name: "lena", owner: owner, messages: [])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
    state.selectConversation(conversation.id)
    state.showAgentMonitor()
    let size = NSSize(width: 470, height: 280)
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(origin: .zero, size: size),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let host = NSHostingView(
      rootView: DictationOverlayView(state: state).frame(width: size.width, height: size.height))
    host.sizingOptions = []
    panel.contentView = host
    BackgroundTestWindow.present(panel)
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    host.layoutSubtreeIfNeeded()
    XCTAssertFalse(state.agentDrawerDocked, "470 wide at 1.0 is below the threshold")

    let lists = panel.menuCursorRegions.allObjects.filter {
      $0.window === panel && !$0.isHiddenOrHasHiddenAncestor && !$0.visibleRect.isEmpty
    }.map { $0.convert($0.bounds, to: nil) }.filter { $0.width > size.width - 20 }
    let list = try XCTUnwrap(lists.first, "the list registers one window-wide region")
    let saved = NSCursor.current
    defer { saved.set() }
    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: list.midX, y: list.midY)), "list \(list)")
    XCTAssertEqual(NSCursor.current, .arrow)
  }

  /// The composer's attachment chips sit above the editor whose tracking set
  /// the I-beam. The chip row claims the arrow; the editable field under it
  /// keeps its text cursor, like the drawer's live search does.
  func testComposerAttachmentChipsClaimTheArrowAndTheEditorKeepsItsTextCursor() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 440, height: 180),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 440, height: 180))
    // Only pointers into Codescribe's own stores become pills, so the fixture
    // lives under the isolated config directory's assets store.
    let configDirectory = CodescribeConfig().configDir()
    guard configDirectory != NSHomeDirectory() + "/.codescribe" else {
      throw XCTSkip("needs an isolated CODESCRIBE_DATA_DIR, as make test-swift provides")
    }
    let store = URL(fileURLWithPath: configDirectory, isDirectory: true)
      .appendingPathComponent("assets", isDirectory: true)
    try FileManager.default.createDirectory(at: store, withIntermediateDirectories: true)
    let asset = store.appendingPathComponent("inline_cursor_\(UUID().uuidString).png")
    try Data([0x89, 0x50, 0x4E, 0x47]).write(to: asset)
    defer { try? FileManager.default.removeItem(at: asset) }
    let draft = ConversationDraft.compose(
      text: "Również pilla informacyjna", attachments: [asset.path])
    XCTAssertEqual(ConversationDraft.split(draft).attachments.count, 1, "fixture is a pill")
    let composer = NSHostingView(
      rootView: OverlayConversationComposer(
        palette: .dark, draft: .constant(draft), sending: false, onSubmit: {}))
    composer.frame = NSRect(x: 10, y: 10, width: 420, height: 160)
    content.addSubview(composer)
    panel.contentView = content
    content.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    content.layoutSubtreeIfNeeded()

    let chips = panel.menuCursorRegions.allObjects.filter {
      $0.window === panel && !$0.isHiddenOrHasHiddenAncestor && !$0.visibleRect.isEmpty
    }
    let chipRow = try XCTUnwrap(
      chips.max { $0.bounds.width < $1.bounds.width },
      "the attachment row registers a cursor region")
    XCTAssertGreaterThan(chipRow.bounds.width, 100, "the whole chip row, not only Send")
    let frame = chipRow.convert(chipRow.bounds, to: nil)
    let saved = NSCursor.current
    defer { saved.set() }
    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: frame.midX, y: frame.midY)), "chips \(frame)")
    XCTAssertEqual(NSCursor.current, .arrow)

    let editor = try XCTUnwrap(editableTextView(in: content), "the composer mounts its editor")
    let typing = editor.convert(NSPoint(x: editor.bounds.midX, y: editor.bounds.midY), to: nil)
    XCTAssertFalse(frame.contains(typing), "chips do not cover the editor: \(frame) vs \(typing)")
    XCTAssertEqual(panel.cursor(at: typing), .iBeam)
    NSCursor.iBeam.set()
    XCTAssertFalse(panel.refreshCursor(at: typing), "typing keeps native tracking")
    XCTAssertEqual(NSCursor.current, .iBeam)
  }

  /// A bubble's attachment chip floats above message text in the same panel.
  func testConversationAttachmentChipClaimsTheArrowAboveMessageText() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 300, height: 120),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: 120))
    let text = NSTextView(frame: content.bounds)
    text.isHorizontallyResizable = false
    text.isVerticallyResizable = false
    text.isEditable = false
    text.string = String(repeating: "Wiadomość z załącznikiem pod chipem. ", count: 12)
    content.addSubview(text)
    let chip = NSHostingView(
      rootView: AttachmentChip(
        attachment: MessageAttachment(
          name: ConversationAttachmentLabel.display(
            fileName: "inline_d0d0…09372302.png", bytes: 1024),
          url: URL(fileURLWithPath: "/tmp/inline_d0d0.png"), type: "image/png")))
    chip.frame = NSRect(x: 120, y: 70, width: 170, height: 32)
    content.addSubview(chip)
    panel.contentView = content
    content.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    content.layoutSubtreeIfNeeded()

    let region = try XCTUnwrap(
      panel.menuCursorRegions.allObjects.first { $0.window === panel },
      "the chip registers a cursor region")
    let frame = region.convert(region.bounds, to: nil)
    XCTAssertGreaterThan(frame.width, 60, "thumbnail and name, got \(frame)")
    let saved = NSCursor.current
    defer { saved.set() }
    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: frame.midX, y: frame.midY)), "chip \(frame)")
    XCTAssertEqual(NSCursor.current, .arrow)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 40, y: 30)), .iBeam, "text keeps its cursor")
  }

  func testReplyAndDraftMicrophoneKeepTheArrowAndReceiveNativeClicks() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let message = OverlayConversationMessage(
      id: "user:cursor-controls", kind: .user, text: "Selectable message above its Reply button.",
      order: 1, emittedAt: "2026-10-21T01:40:00Z", owner: nil, recipients: [],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
    let conversation = OverlayConversation(
      id: "cursor-controls", channel: "0", name: "All", owner: nil, messages: [message])
    var replies = 0
    var microphoneClicks = 0
    let view = OverlayConversationView(
      conversation: conversation, palette: .dark, topInset: 0, bottomInset: 0,
      pendingControls: [], controlErrors: [:], onControl: { _, _ in },
      draft: .constant(""), sending: false, sendError: nil, onSend: {},
      onReply: { _ in replies += 1 })
    let host = NSHostingView(
      rootView: VStack(spacing: 30) {
        view.messageBubble(message)
        OverlayConversationComposer(
          palette: .dark, draft: .constant("Editable draft"), sending: false, onSubmit: {},
          onToggleDraftMicrophone: { microphoneClicks += 1 })
      }.padding(30))
    host.sizingOptions = []
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 200, y: 200, width: 600, height: 350),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.contentView = OverlayContentContainer(hosting: host)
    BackgroundTestWindow.present(panel)
    let saved = NSCursor.current
    defer {
      panel.orderOut(nil)
      saved.set()
    }
    panel.contentView?.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    for identifier in [
      "overlay-message-reply-\(message.id)", "overlay-conversation-draft-microphone",
    ] {
      let button = try XCTUnwrap(
        NativeAccessibilityProbe.waitForElements(identifier).elements.first)
      let screenTop = try XCTUnwrap(NSScreen.screens.first).frame.maxY
      let point = panel.convertPoint(
        fromScreen: NSPoint(
          x: button.x + button.width / 2, y: screenTop - button.y - button.height / 2))
      XCTAssertEqual(panel.cursor(at: point), .arrow, identifier)
      NSCursor.iBeam.set()
      for type in [NSEvent.EventType.mouseMoved, .cursorUpdate] {
        let event =
          type == .cursorUpdate
          ? NSEvent.enterExitEvent(
            with: type, location: point, modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
            context: nil, eventNumber: 1, trackingNumber: 0, userData: nil)
          : NSEvent.mouseEvent(
            with: type, location: point, modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
            context: nil, eventNumber: 1, clickCount: 0, pressure: 0)
        panel.sendEvent(try XCTUnwrap(event))
        XCTAssertEqual(NSCursor.current, .arrow, identifier)
      }
      // AX locates the control; the real panel receives both mouse events.
      for (type, number) in [(NSEvent.EventType.leftMouseDown, 2), (.leftMouseUp, 3)] {
        panel.sendEvent(
          try XCTUnwrap(
            NSEvent.mouseEvent(
              with: type, location: point, modifierFlags: [],
              timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
              context: nil, eventNumber: number, clickCount: 1,
              pressure: type == .leftMouseDown ? 1 : 0)))
        RunLoop.main.run(until: Date().addingTimeInterval(0.03))
      }
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    }
    XCTAssertEqual(replies, 1)
    XCTAssertEqual(microphoneClicks, 1)
    let editor = try XCTUnwrap(editableTextView(in: host))
    let typing = editor.convert(NSPoint(x: editor.bounds.midX, y: editor.bounds.midY), to: nil)
    XCTAssertEqual(panel.cursor(at: typing), .iBeam, "the draft remains a native text input")
  }

  private func editableTextView(in view: NSView) -> NSTextView? {
    if let text = view as? NSTextView, text.isEditable { return text }
    for child in view.subviews {
      if let found = editableTextView(in: child) { return found }
    }
    return nil
  }

  func testDrawerTextInputKeepsItsTextCursor() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    panel.contentView = content
    content.addSubview(
      OverlayMenuCursorRegionView(
        frame: NSRect(x: 300, y: 100, width: 280, height: 350)))
    let search = NSTextField(frame: NSRect(x: 320, y: 400, width: 220, height: 24))
    search.isEditable = true
    content.addSubview(search)
    let point = NSPoint(x: 350, y: 412)
    XCTAssertTrue(content.hitTest(point) === search)
    XCTAssertEqual(
      panel.cursor(at: point), .iBeam, "Live search remains a text input inside chrome")
    let saved = NSCursor.current
    defer { saved.set() }
    NSCursor.iBeam.set()
    XCTAssertFalse(panel.refreshCursor(at: point), "The panel leaves native input tracking alone")
    XCTAssertEqual(NSCursor.current, .iBeam)
  }

  func testHeaderAndDrawerKeepIndependentCursorRegions() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    panel.contentView = content
    let header = OverlayMenuCursorRegionView(frame: NSRect(x: 100, y: 450, width: 22, height: 22))
    let drawer = OverlayMenuCursorRegionView(frame: NSRect(x: 300, y: 100, width: 280, height: 350))
    content.addSubview(header)
    content.addSubview(drawer)
    let saved = NSCursor.current
    defer {
      panel.close()
      saved.set()
    }

    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: 110, y: 460)))
    XCTAssertEqual(NSCursor.current, .arrow, "Mounting the drawer cannot disown header controls")
    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: 400, y: 200)))
    XCTAssertEqual(NSCursor.current, .arrow)

    drawer.removeFromSuperview()
    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: 110, y: 460)))
    XCTAssertEqual(NSCursor.current, .arrow, "Removing one region retains the other")
    header.isHidden = true
    NSCursor.iBeam.set()
    XCTAssertFalse(panel.refreshCursor(at: NSPoint(x: 110, y: 460)))
    XCTAssertEqual(NSCursor.current, .iBeam, "Hidden chrome cannot own the cursor")
    header.isHidden = false
    header.removeFromSuperview()
    XCTAssertFalse(panel.refreshCursor(at: NSPoint(x: 110, y: 460)))
  }

  func testMountedMenuOwnsArrowAboveSelectableTextAndReleasesOnRemoval() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    let text = NSTextView(frame: content.bounds)
    // This fixture tests panel hit ownership, not TextKit's fit-to-content
    // resizing. Keep the selectable surface fixed while layout is realized.
    text.isHorizontallyResizable = false
    text.isVerticallyResizable = false
    text.isEditable = false
    text.string = "Selectable transcript behind the drawer"
    content.addSubview(text)
    panel.contentView = content
    text.frame = content.bounds
    if let container = text.textContainer {
      text.layoutManager?.ensureLayout(for: container)
    }
    content.layoutSubtreeIfNeeded()
    let menu = OverlayMenuCursorRegionView(frame: NSRect(x: 300, y: 100, width: 280, height: 350))
    content.addSubview(menu)

    let outsideMenu = text.convert(NSPoint(x: 100, y: 200), to: nil)
    XCTAssertTrue(
      content.hitTest(outsideMenu) === text,
      "Fixture must hit native text outside menu; text frame=\(text.frame), visible=\(text.visibleRect), menu frame=\(menu.frame)"
    )
    XCTAssertTrue(panel.menuCursorRegions.contains(menu))
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .arrow)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 100, y: 200)), .iBeam)
    XCTAssertEqual(panel.cursor(at: outsideMenu), .iBeam)
    XCTAssertNil(menu.hitTest(NSPoint(x: 10, y: 10)), "Drawer controls retain their own hit path")

    menu.isHidden = true
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .iBeam)
    menu.isHidden = false
    menu.removeFromSuperview()
    XCTAssertTrue(panel.menuCursorRegions.allObjects.isEmpty)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .iBeam)
  }

  func testMenuMotionDoesNotDispatchOverlappingTextTrackingAndReleasesOwnership() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 100, y: 100, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    let text = DrawerCursorTrackingTextView(frame: content.bounds)
    text.isHorizontallyResizable = false
    text.isVerticallyResizable = false
    text.isEditable = false
    text.string = "Selectable transcript behind the agent drawer"
    content.addSubview(text)
    panel.contentView = content
    text.frame = content.bounds
    panel.acceptsMouseMovedEvents = true
    BackgroundTestWindow.present(panel)
    let saved = NSCursor.current
    defer {
      panel.orderOut(nil)
      saved.set()
    }
    text.addTrackingArea(
      NSTrackingArea(
        rect: text.bounds, options: [.mouseMoved, .activeAlways], owner: text, userInfo: nil))
    let selection = NSRange(location: 2, length: 5)
    text.setSelectedRange(selection)
    let menu = OverlayMenuCursorRegionView(frame: NSRect(x: 300, y: 100, width: 280, height: 350))
    content.addSubview(menu)
    let button = NSButton(title: "Agent", target: nil, action: nil)
    button.frame = NSRect(x: 320, y: 120, width: 120, height: 30)
    content.addSubview(button)
    XCTAssertTrue(content.hitTest(NSPoint(x: 350, y: 135)) === button)

    func move(to point: NSPoint) throws {
      let event = try XCTUnwrap(
        NSEvent.mouseEvent(
          with: .mouseMoved, location: point, modifierFlags: [],
          timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
          context: nil, eventNumber: 1, clickCount: 0, pressure: 0))
      panel.sendEvent(event)
    }

    // Prove the mounted native tracking path is active before testing its exclusion.
    try move(to: NSPoint(x: 100, y: 200))
    XCTAssertGreaterThan(text.motions, 0)
    text.motions = 0
    let inside = NSPoint(x: 400, y: 200)
    for _ in 0..<20 {
      try move(to: inside)
      XCTAssertEqual(NSCursor.current, .arrow)
    }
    XCTAssertEqual(text.motions, 0, "Drawer motion must not first set the text cursor")
    XCTAssertEqual(text.selectedRange(), selection)
    XCTAssertFalse(panel.refreshCursor(at: inside), "An unchanged arrow needs no second write")

    try move(to: NSPoint(x: 100, y: 200))
    XCTAssertGreaterThan(text.motions, 0, "Text outside the drawer retains native motion")
    text.motions = 0
    menu.isHidden = true
    try move(to: inside)
    XCTAssertGreaterThan(text.motions, 0, "Hiding the drawer releases native tracking")
    text.motions = 0
    menu.isHidden = false
    try move(to: inside)
    XCTAssertEqual(text.motions, 0, "Showing the drawer restores exclusive cursor ownership")
    menu.removeFromSuperview()
    try move(to: inside)
    XCTAssertGreaterThan(text.motions, 0, "Removing the drawer releases native tracking")
    XCTAssertEqual(text.selectedRange(), selection)
  }

  func testMenuDoesNotTakeResizeOwnershipFromPanelEdges() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    panel.contentView = content
    content.addSubview(OverlayMenuCursorRegionView(frame: content.bounds))

    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .arrow)
    XCTAssertEqual(
      panel.cursor(at: NSPoint(x: 599, y: 200)), OverlayResizeHit.cursor(for: .right))
    XCTAssertEqual(
      panel.cursor(at: NSPoint(x: 400, y: 499)), OverlayResizeHit.cursor(for: .top))
  }
}

@MainActor
private final class CursorHitCountingView: NSView {
  var hits = 0

  override func hitTest(_ point: NSPoint) -> NSView? {
    hits += 1
    return super.hitTest(point)
  }
}

@MainActor
private final class DrawerCursorTrackingTextView: NSTextView {
  var motions = 0

  override func mouseMoved(with event: NSEvent) {
    motions += 1
    NSCursor.iBeam.set()
    super.mouseMoved(with: event)
  }
}
