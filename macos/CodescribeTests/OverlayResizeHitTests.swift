import XCTest

@testable import Codescribe

final class OverlayResizeHitTests: XCTestCase {
  private let bounds = NSRect(x: 0, y: 0, width: 400, height: 300)
  private let band = OverlayResizeHit.band

  func testInteriorIsNotAResizeHit() {
    XCTAssertNil(OverlayResizeHit.edge(at: NSPoint(x: 200, y: 150), in: bounds))
  }

  /// The container answers `hitTest` with itself only inside the 16 pt resize
  /// band, so the panel's drag intercept must never treat that band as a drag
  /// handle — otherwise `OverlayContentContainer.mouseDown` (edge tracking)
  /// never receives the click and edge resize is dead. The tracking loop runs
  /// on `window.nextEvent`, which synthetic events cannot feed, so the witness
  /// is the routing decision on the real hierarchy, not the tracked frame.
  @MainActor
  func testRealOverlayResizeBandIsNotAWindowDragHandle() throws {
    let state = OverlayState.previewListening()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayResizeHitTests.resizeBand.textScale")
      ) as? FloatingOverlayPanel
    )
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.setContentSize(NSSize(width: 470, height: 280))
    panel.orderFrontRegardless()
    let root = try XCTUnwrap(panel.contentView)
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    root.layoutSubtreeIfNeeded()

    let edgePoints = [
      ("left-edge", NSPoint(x: 6, y: root.bounds.midY)),
      ("right-edge", NSPoint(x: root.bounds.maxX - 6, y: root.bounds.midY)),
      ("bottom-right-corner", NSPoint(x: root.bounds.maxX - 6, y: 6)),
      ("top-edge", NSPoint(x: root.bounds.midX, y: root.bounds.maxY - 6)),
    ]
    for (region, point) in edgePoints {
      XCTAssertNotNil(
        OverlayResizeHit.edge(at: point, in: root.bounds),
        "\(region) is not inside the resize band"
      )
      XCTAssertTrue(
        root.hitTest(point) === root,
        "\(region): the container must claim the resize band"
      )
      let dragHit = panel.isWindowDragHit(at: point)
      print("W5_T16_RESIZE_BAND region=\(region) point=(\(point.x),\(point.y)) dragHit=\(dragHit)")
      XCTAssertFalse(
        dragHit,
        "\(region): resize band must reach OverlayContentContainer.mouseDown, not the drag intercept"
      )
    }
    XCTAssertTrue(
      panel.isWindowDragHit(at: NSPoint(x: 28, y: root.bounds.maxY - 22)),
      "header interior must still be a drag handle"
    )
  }

  /// Founder 2026-09-08 (build 849, chrome v3): "header chrome overlaya nadal nie
  /// oferuje drag area". The header is justified edge to edge now, so the old
  /// x=28 probe only proves the brand block. Every non-control point across the
  /// header width must be a window drag handle.
  @MainActor
  func testHeaderIsAWindowDragHandleAcrossItsWidth() throws {
    let state = OverlayState.previewListening()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayResizeHitTests.headerWidth.textScale")
      ) as? FloatingOverlayPanel
    )
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.setContentSize(NSSize(width: 470, height: 280))
    panel.orderFrontRegardless()
    let root = try XCTUnwrap(panel.contentView)
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    root.layoutSubtreeIfNeeded()

    let y = root.bounds.maxY - 22
    let probes: [(String, CGFloat)] = [
      ("brand", 28),
      ("after-brand", 150),
      ("center-waveform", root.bounds.midX),
      ("before-timer", root.bounds.maxX - 150),
      ("between-waveform-and-controls", root.bounds.maxX - 160),
    ]
    for (region, x) in probes {
      let point = NSPoint(x: x, y: y)
      let hit = try XCTUnwrap(root.hitTest(point))
      let chain = hitChain(from: hit)
      print(
        "CHROME_V3_HEADER_DRAG region=\(region) x=\(x) dragHit=\(panel.isWindowDragHit(at: point)) chain=\(chain)"
      )
      XCTAssertTrue(
        panel.isWindowDragHit(at: point),
        "header \(region) at x=\(x) is not a window drag handle: \(chain)"
      )
    }
    XCTAssertFalse(
      panel.isWindowDragHit(at: NSPoint(x: root.bounds.maxX - 27, y: y)),
      "The collapse control must answer clicks, not window drags")
  }

  @MainActor
  func testRealOverlayHierarchyRoutesDragControlsAndTranscriptIndependently() throws {
    let state = OverlayState.previewListening()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayResizeHitTests.textScale")
      ) as? FloatingOverlayPanel
    )
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.setContentSize(NSSize(width: 470, height: 280))
    panel.orderFrontRegardless()

    let root = try XCTUnwrap(panel.contentView)
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    root.layoutSubtreeIfNeeded()

    let dragPoints = [
      ("header", NSPoint(x: 28, y: root.bounds.maxY - 22)),
      ("body margin", NSPoint(x: 18, y: root.bounds.midY)),
    ]
    for (region, point) in dragPoints {
      let hit = try XCTUnwrap(root.hitTest(point))
      print("W5_T16_HIT region=\(region) chain=\(hitChain(from: hit))")
      XCTAssertTrue(
        panel.isWindowDragHit(at: point),
        "\(region) exposed no AppKit drag region: \(hitChain(from: hit))"
      )
      XCTAssertFalse(
        hitChain(from: hit).contains("OverlayDragHandleView"),
        "the obsolete AppKit drag layer still owns \(region): \(hitChain(from: hit))"
      )
    }

    // Close lives at the trailing edge of the header; the bottom belongs to text.
    let actionPoint = NSPoint(x: root.bounds.maxX - 28, y: root.bounds.maxY - 30)
    let actionHit = try XCTUnwrap(root.hitTest(actionPoint))
    XCTAssertFalse(
      panel.isWindowDragHit(at: actionPoint),
      "the dock action was misclassified as draggable: \(hitChain(from: actionHit))"
    )
    XCTAssertFalse(
      hitChain(from: actionHit).contains("OverlayDragHandleView"),
      "the action control was stolen by \(hitChain(from: actionHit))"
    )

    let transcript = try XCTUnwrap(descendant(of: LiveTranscriptNativeTextView.self, in: root))
    let transcriptPoint = root.convert(
      NSPoint(x: transcript.bounds.midX, y: transcript.bounds.midY),
      from: transcript
    )
    let transcriptHit = try XCTUnwrap(root.hitTest(transcriptPoint))
    XCTAssertTrue(
      transcriptHit === transcript || transcriptHit.isDescendant(of: transcript),
      "the transcript was stolen by \(hitChain(from: transcriptHit))"
    )
    XCTAssertTrue(transcript.isSelectable)
  }

  @MainActor
  func testRealOverlaySyntheticDragMovesWindowFromEveryInertRegion() throws {
    let state = OverlayState.previewListening()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayResizeHitTests.dragEffect.textScale")
      ) as? FloatingOverlayPanel
    )
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }

    panel.setContentSize(NSSize(width: 470, height: 280))
    let visibleFrame = try XCTUnwrap(NSScreen.main?.visibleFrame)
    let resetOrigin = NSPoint(x: visibleFrame.minX + 80, y: visibleFrame.minY + 80)
    panel.setFrameOrigin(resetOrigin)
    panel.orderFrontRegardless()

    let root = try XCTUnwrap(panel.contentView)
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    root.layoutSubtreeIfNeeded()

    let requested = NSSize(width: 100, height: 50)
    let dragPoints = [
      ("header", NSPoint(x: 28, y: root.bounds.maxY - 22)),
      ("body-margin", NSPoint(x: 18, y: root.bounds.midY)),
    ]

    for (index, (region, point)) in dragPoints.enumerated() {
      panel.setFrameOrigin(resetOrigin)
      let before = panel.frame.origin
      sendSyntheticDrag(
        through: panel,
        from: point,
        delta: requested,
        eventNumberBase: 10_000 + index * 10
      )
      RunLoop.main.run(until: Date().addingTimeInterval(0.02))
      let after = panel.frame.origin
      let moved = NSSize(width: after.x - before.x, height: after.y - before.y)
      print(
        "W5_T16_DRAG region=\(region) before=(\(before.x),\(before.y)) "
          + "after=(\(after.x),\(after.y)) moved=(\(moved.width),\(moved.height)) "
          + "requested=(\(requested.width),\(requested.height))"
      )
      XCTAssertGreaterThanOrEqual(
        moved.width,
        requested.width * 0.8,
        "\(region) moved only \(moved.width) pt horizontally; before=\(before), after=\(after)"
      )
      XCTAssertGreaterThanOrEqual(
        moved.height,
        requested.height * 0.8,
        "\(region) moved only \(moved.height) pt vertically; before=\(before), after=\(after)"
      )
      XCTAssertLessThanOrEqual(moved.width, requested.width * 1.2)
      XCTAssertLessThanOrEqual(moved.height, requested.height * 1.2)
    }
  }

  @MainActor
  func testRealOverlayKeepsTranscriptBetweenHeaderAndDock() throws {
    let state = OverlayState()
    state.toggleCollapsed()  // This test measures the explicitly expanded transcript.
    project(
      "first line with uncut caps\nsecond line\nthird line\nfourth line\nfifth line\nlast line above the dock",
      sequence: 1,
      to: state
    )
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayResizeHitTests.layout.textScale")
      ) as? FloatingOverlayPanel
    )
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.setContentSize(NSSize(width: 470, height: 260))
    panel.orderFrontRegardless()

    let root = try XCTUnwrap(panel.contentView)
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.08))
    root.layoutSubtreeIfNeeded()

    let transcript = try XCTUnwrap(descendant(of: LiveTranscriptNativeTextView.self, in: root))
    let header = try XCTUnwrap(
      descendant(identifier: "overlay-header-drag-region", in: root)
    )
    XCTAssertNil(descendant(identifier: "overlay-dock-drag-region", in: root))
    let length = (transcript.string as NSString).length
    XCTAssertGreaterThan(length, 0)
    let firstLine = transcript.firstRect(
      forCharacterRange: NSRange(location: 0, length: 1),
      actualRange: nil
    )
    let lastLine = transcript.firstRect(
      forCharacterRange: NSRange(location: length - 1, length: 1),
      actualRange: nil
    )
    let headerFrame = screenFrame(of: header, in: panel)
    let transcriptFrame = screenFrame(of: transcript.enclosingScrollView!, in: panel)
    XCTAssertLessThanOrEqual(firstLine.maxY, headerFrame.minY + 1)
    XCTAssertGreaterThanOrEqual(lastLine.minY, transcriptFrame.minY - 1)
    XCTAssertLessThan(
      transcriptFrame.minY - panel.frame.minY, 24,
      "no dock or reserved action padding may consume the transcript bottom")
  }

  @MainActor
  func testProjectedTextCannotUnfoldTheRecordingBar() throws {
    let state = OverlayState()
    var builtPanel: FloatingOverlayPanel?
    let controller = OverlayController(
      state: state, engine: nil,
      overlayEnabledProvider: { true }, assistiveStatusProvider: { false },
      panelFactory: { state, scale in
        let panel = DictationOverlayWindow.make(state: state, textScale: scale)
        builtPanel = panel as? FloatingOverlayPanel
        return panel
      }, orderPanelFront: { $0.orderFrontRegardless() }, orderPanelOut: { $0.orderOut(nil) }
    )
    controller.show()
    let panel = try XCTUnwrap(builtPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    let savedSize = panel.sizeForPersistence
    let text = (1...80).map { "recorded words \($0)" }.joined(separator: "\n")
    project(text, sequence: 1, to: state)
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    XCTAssertTrue(state.isCollapsed)
    XCTAssertEqual(panel.frame.height, DictationOverlayWindow.collapsedHeight, accuracy: 0.5)
    XCTAssertEqual(panel.sizeForPersistence, savedSize)
    XCTAssertEqual(state.activeText, text)
    state.toggleCollapsed()
    XCTAssertEqual(panel.frame.size, savedSize)
    project(text + "\nmore words", sequence: 2, to: state)
    XCTAssertGreaterThanOrEqual(panel.frame.height, savedSize.height)
  }

  @MainActor
  func testRealOverlayBreathesToSixtyPercentThenHonorsManualResize() throws {
    let state = OverlayState()
    state.toggleCollapsed()  // Auto-sizing applies only to the expanded transcript.
    var builtPanel: FloatingOverlayPanel?
    let controller = OverlayController(
      state: state,
      engine: nil,
      overlayEnabledProvider: { true },
      assistiveStatusProvider: { false },
      panelFactory: { state, textScale in
        guard
          let panel = DictationOverlayWindow.make(state: state, textScale: textScale)
            as? FloatingOverlayPanel
        else {
          XCTFail("DictationOverlayWindow.make did not return FloatingOverlayPanel")
          return NSPanel()
        }
        // Override restored geometry before the controller installs its resize
        // ownership callback. This fixture never writes the real defaults.
        panel.setContentSize(NSSize(width: 470, height: 260))
        builtPanel = panel
        return panel
      },
      orderPanelFront: { $0.orderFrontRegardless() },
      orderPanelOut: { $0.orderOut(nil) }
    )
    controller.show()
    let panel = try XCTUnwrap(builtPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    let screen = try XCTUnwrap(panel.screen ?? NSScreen.main)
    XCTAssertEqual(panel.frame.width, 470, accuracy: 0.5)
    XCTAssertEqual(panel.frame.height, 260, accuracy: 0.5)
    let before = panel.frame.height
    let manyLines = (1...80).map { "projected line \($0)" }.joined(separator: "\n")

    project(manyLines, sequence: 1, to: state)
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    let grown = panel.frame.height
    let maximum = floor(screen.visibleFrame.height * OverlayContentSizePolicy.maximumScreenFraction)
    XCTAssertEqual(OverlayContentSizePolicy.maximumScreenFraction, 0.60)
    print("W5_T16_BREATH before=\(before) grown=\(grown) maximum=\(maximum)")
    XCTAssertGreaterThan(grown, before)
    XCTAssertLessThanOrEqual(grown, maximum + 0.5)

    let manualHeight = max(DictationOverlayWindow.minSize.height, grown - 80)
    panel.setFrame(
      NSRect(
        x: panel.frame.minX,
        y: panel.frame.maxY - manualHeight,
        width: panel.frame.width,
        height: manualHeight
      ),
      display: true
    )
    project(manyLines + "\nmore projected content", sequence: 2, to: state)
    XCTAssertEqual(panel.frame.height, manualHeight, accuracy: 0.5)

    // The automatic cap must not become a ceiling on an explicit user size.
    let oversized = NSSize(width: panel.frame.width + 40, height: maximum + 80)
    panel.setFrame(
      NSRect(
        x: panel.frame.minX,
        y: panel.frame.maxY - oversized.height,
        width: oversized.width,
        height: oversized.height
      ),
      display: true
    )
    XCTAssertGreaterThan(panel.frame.height, maximum)
    project("short projected content", sequence: 3, to: state)
    XCTAssertEqual(panel.frame.width, oversized.width, accuracy: 0.5)
    XCTAssertEqual(panel.frame.height, oversized.height, accuracy: 0.5)
    project(manyLines + "\neven more projected content", sequence: 4, to: state)
    XCTAssertEqual(panel.frame.width, oversized.width, accuracy: 0.5)
    XCTAssertEqual(panel.frame.height, oversized.height, accuracy: 0.5)
  }

  @MainActor
  func testNativeCanvasPreservesEveryEngineByteAcrossUnicodeEquivalentUpdates() throws {
    let state = OverlayState()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "OverlayResizeHitTests.byteExact"))
        as? FloatingOverlayPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.orderFrontRegardless()
    let root = try XCTUnwrap(panel.contentView)
    for (index, text) in ["  raz raz\né 👩‍💻  ", "  raz raz\ne\u{301} 👩‍💻  ", "", "raz raz"].enumerated()
    {
      project(text, sequence: UInt64(index + 1), to: state)
      RunLoop.main.run(until: Date().addingTimeInterval(0.05))
      root.layoutSubtreeIfNeeded()
      let canvas = try XCTUnwrap(descendant(of: LiveTranscriptNativeTextView.self, in: root))
      XCTAssertEqual(Array(canvas.string.utf8), Array(text.utf8), "projection \(index + 1)")
      XCTAssertFalse(canvas.isEditable)
    }
  }

  func testPersistedContentSizeRoundTripsThroughDefaults() throws {
    let suiteName = "OverlayResizeHitTests.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
    defer { defaults.removePersistentDomain(forName: suiteName) }

    let expected = NSSize(width: 612, height: 377)
    DictationOverlayWindow.persist(size: expected, defaults: defaults)

    XCTAssertEqual(
      DictationOverlayWindow.restoredContentSize(for: nil, defaults: defaults),
      expected
    )
  }

  func testEdgesAndCornersUseTheFatBand() {
    XCTAssertEqual(OverlayResizeHit.band, 16)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 2, y: 150), in: bounds), .left)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 398, y: 150), in: bounds), .right)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 200, y: 298), in: bounds), .top)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 200, y: 2), in: bounds), .bottom)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 2, y: 298), in: bounds), .topLeft)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 398, y: 298), in: bounds), .topRight)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 2, y: 2), in: bounds), .bottomLeft)
    XCTAssertEqual(OverlayResizeHit.edge(at: NSPoint(x: 398, y: 2), in: bounds), .bottomRight)
  }

  func testJustInsideTheBandIsStillInterior() {
    let inset = band + 1
    XCTAssertNil(OverlayResizeHit.edge(at: NSPoint(x: inset, y: 150), in: bounds))
    XCTAssertNil(OverlayResizeHit.edge(at: NSPoint(x: 200, y: inset), in: bounds))
  }

  func testApplyKeepsMinSizeWhenDraggingInward() {
    let start = NSRect(x: 100, y: 80, width: 400, height: 320)
    let minSize = DictationOverlayWindow.minSize
    let crushed = OverlayResizeHit.apply(
      edge: .right,
      start: start,
      dx: -200,
      dy: 0,
      minSize: minSize
    )
    XCTAssertEqual(crushed.width, minSize.width)
    XCTAssertEqual(crushed.origin.x, start.origin.x)
  }

  func testLeftAndBottomKeepTheOppositeEdgePinned() {
    let start = NSRect(x: 100, y: 80, width: 400, height: 320)
    let minSize = DictationOverlayWindow.minSize
    let left = OverlayResizeHit.apply(
      edge: .left,
      start: start,
      dx: 20,
      dy: 0,
      minSize: minSize
    )
    XCTAssertEqual(left.maxX, start.maxX)
    XCTAssertEqual(left.width, 380)
    let bottom = OverlayResizeHit.apply(
      edge: .bottom,
      start: start,
      dx: 0,
      dy: 20,
      minSize: minSize
    )
    XCTAssertEqual(bottom.maxY, start.maxY)
    XCTAssertEqual(bottom.height, 300)
  }

  @MainActor
  private func descendant<View: NSView>(of type: View.Type, in root: NSView) -> View? {
    if let root = root as? View { return root }
    return root.subviews.lazy.compactMap { self.descendant(of: type, in: $0) }.first
  }

  @MainActor
  private func descendant(identifier: String, in root: NSView) -> NSView? {
    if root.accessibilityIdentifier() == identifier { return root }
    return root.subviews.lazy.compactMap { self.descendant(identifier: identifier, in: $0) }.first
  }

  @MainActor
  private func screenFrame(of view: NSView, in window: NSWindow) -> NSRect {
    window.convertToScreen(view.convert(view.bounds, to: nil))
  }

  @MainActor
  private func project(_ text: String, sequence: UInt64, to state: OverlayState) {
    state.applyTranscriptProjection(
      CsTranscriptProjectionEvent(
        schema: "codescribe.transcript_projection.v1",
        sequence: sequence,
        emittedAt: "2026-09-06T00:00:00Z",
        sessionId: "w5-t16-overlay-fixture",
        mode: "dictation",
        reducerRevision: sequence,
        reducerAction: "w5_t16_projection_fixture",
        occurrenceSessionId: "w5-t16-overlay-fixture",
        captureEpoch: 1,
        sampleStart: (sequence - 1) * 16_000,
        sampleEnd: sequence * 16_000,
        documentIndex: sequence - 1,
        label: "live",
        renderedText: text,
        phase: "listening",
        canPaste: false,
        canInsert: false,
        canCopy: !text.isEmpty,
        canRetranscribe: false,
        canFormat: false,
        canSendToAgent: false,
        terminal: false,
        lifecycleTerminal: false,
        delivery: .unattempted,
        acousticReceipts: [],
        sealCoverage: nil,
        consultationPresentations: []
      )
    )
  }

  @MainActor
  private func sendSyntheticDrag(
    through window: NSWindow,
    from start: NSPoint,
    delta: NSSize,
    eventNumberBase: Int
  ) {
    let timestamp = ProcessInfo.processInfo.systemUptime
    let points = (0...5).map { step in
      let fraction = CGFloat(step) / 5
      return NSPoint(
        x: start.x + delta.width * fraction,
        y: start.y + delta.height * fraction
      )
    }
    let events =
      [
        mouseEvent(
          .leftMouseDown,
          at: points[0],
          in: window,
          timestamp: timestamp,
          eventNumber: eventNumberBase
        )
      ]
      + points.dropFirst().enumerated().map { offset, point in
        mouseEvent(
          .leftMouseDragged,
          at: point,
          in: window,
          timestamp: timestamp + Double(offset + 1) * 0.01,
          eventNumber: eventNumberBase + offset + 1
        )
      } + [
        mouseEvent(
          .leftMouseUp,
          at: points.last!,
          in: window,
          timestamp: timestamp + 0.07,
          eventNumber: eventNumberBase + 7,
          pressure: 0
        )
      ]
    // Freeze each NSEvent's global CG location before the first drag beat moves
    // the window. Otherwise a synthetic window-local event lazily reprojects
    // through the already-moved frame and exaggerates the requested delta.
    let frozenEvents = events.map { event -> NSEvent in
      guard let cgEvent = event.cgEvent?.copy(), let frozen = NSEvent(cgEvent: cgEvent) else {
        XCTFail("failed to freeze synthetic \(event.type) event")
        preconditionFailure("NSEvent CG copy returned nil")
      }
      return frozen
    }
    for event in frozenEvents { window.sendEvent(event) }
  }

  @MainActor
  private func mouseEvent(
    _ type: NSEvent.EventType,
    at point: NSPoint,
    in window: NSWindow,
    timestamp: TimeInterval,
    eventNumber: Int,
    pressure: Float = 1
  ) -> NSEvent {
    guard
      let event = NSEvent.mouseEvent(
        with: type,
        location: point,
        modifierFlags: [],
        timestamp: timestamp,
        windowNumber: window.windowNumber,
        context: nil,
        eventNumber: eventNumber,
        clickCount: 1,
        pressure: pressure
      )
    else {
      XCTFail("failed to create synthetic \(type) event")
      preconditionFailure("NSEvent.mouseEvent returned nil")
    }
    return event
  }

  @MainActor
  private func hitChain(from view: NSView) -> String {
    var chain: [String] = []
    var candidate: NSView? = view
    while let current = candidate {
      let rawIdentifier = current.accessibilityIdentifier()
      let identifier = rawIdentifier.isEmpty ? "" : "#\(rawIdentifier)"
      let recognizers = current.gestureRecognizers
        .map { String(describing: type(of: $0)) }
        .joined(separator: ",")
      let recognizerDetail = recognizers.isEmpty ? "" : "{\(recognizers)}"
      chain.append("\(String(describing: type(of: current)))\(identifier)\(recognizerDetail)")
      candidate = current.superview
    }
    return chain.joined(separator: " <- ")
  }
}
