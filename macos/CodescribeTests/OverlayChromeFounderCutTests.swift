import AppKit
import XCTest

@testable import Codescribe

/// Founder cut 2026-09-08 19:18 for the overlay header: the brand dot is the
/// close control, Auto Paste is toggled from the header, and no phase capsule
/// ("listening" pill) renders anywhere.
///
/// The accepted chrome (agy, `b9c7e8f47`) builds these controls as SwiftUI
/// `Button`s. SwiftUI does not project its accessibility subtree into the
/// AppKit `subviews` / `accessibilityChildren()` graph under XCTest, so the
/// controls are pinned by their state seams (`OverlayState`) plus source
/// guards on `DictationOverlayView.swift` — the same pattern as
/// `OverlayStateTests`. The rendered-panel checks below cover what AppKit
/// does expose: the native hierarchy and window drag hit-testing.
@MainActor
final class OverlayChromeFounderCutTests: XCTestCase {
  func testAcousticWarningsUseVoiceLabGateWithoutHidingOperationErrors() throws {
    let source = try overlaySource()
    XCTAssertTrue(source.contains("@AppStorage(DictationOverlayGate.labModeDefaultsKey)"))
    XCTAssertTrue(source.contains("DeveloperSurface.isPowerModeEnabled(labMode: labMode)"))
    let header = try headerSource(source)
    XCTAssertTrue(
      header.contains("if showsDiagnostics && state.compactProjection?.degraded == true"))
    XCTAssertTrue(header.contains("if let error = state.expansionPreferenceError"))
    let refusal = try section(of: source, from: "case .coverageRefused:", to: "case .noSpeech:")
    XCTAssertTrue(refusal.contains("if showsDiagnostics {"))
    XCTAssertTrue(refusal.contains("coverageRefusedBody"))
  }

  func testExpansionClampsBottomAnchorsLowDragsAndSmallerNegativeDisplay() {
    let visible = NSRect(x: -1400, y: -200, width: 1200, height: 800)
    let expanded = NSSize(width: 700, height: 400)
    for anchor in [OverlayAnchor.bottomLeft, .bottomCenter, .bottomRight] {
      let barSize = NSSize(width: expanded.width, height: DictationOverlayWindow.collapsedHeight)
      let bar = NSRect(
        origin: OverlayPlacement.origin(for: anchor, size: barSize, in: visible), size: barSize)
      let proposed = NSRect(
        x: bar.minX, y: bar.maxY - expanded.height, width: expanded.width, height: expanded.height)
      let restored = DictationOverlayWindow.visibleExpansionFrame(proposed, in: visible)
      XCTAssertTrue(visible.contains(restored), "\(anchor)")
      XCTAssertEqual(restored.size, expanded)
      XCTAssertEqual(restored.minY, visible.minY)
    }
    let lowDrag = NSRect(x: -500, y: -580, width: 700, height: 400)
    XCTAssertTrue(
      visible.contains(DictationOverlayWindow.visibleExpansionFrame(lowDrag, in: visible)))
    let smallDisplay = NSRect(x: -800, y: -600, width: 500, height: 300)
    XCTAssertEqual(
      DictationOverlayWindow.visibleExpansionFrame(lowDrag, in: smallDisplay), smallDisplay)
    let fitting = NSRect(x: -1300, y: 100, width: 700, height: 400)
    XCTAssertEqual(DictationOverlayWindow.visibleExpansionFrame(fitting, in: visible), fitting)
  }

  func testPreferenceSaveFailureIsVisibleWithoutExpandingTheBar() throws {
    let state = OverlayState()
    let engine = OverlayChromePolicyEngine()
    engine.expanded = false
    state.engine = engine
    state.attach()
    engine.expansionWriteAllowed = false
    state.setExpandedByDefault(true)
    XCTAssertTrue(state.isCollapsed)
    XCTAssertFalse(state.expandedByDefault)
    XCTAssertEqual(state.expansionPreferenceError, "Couldn't save overlay preference")
    let header = try headerSource(overlaySource())
    XCTAssertTrue(header.contains("state.expansionPreferenceError"))
    XCTAssertTrue(header.contains("overlay-preference-save-error"))
    XCTAssertTrue(header.contains(".accessibilityLabel(error)"))
    engine.expansionWriteAllowed = true
    state.setExpandedByDefault(false)
    XCTAssertNil(state.expansionPreferenceError)
    XCTAssertTrue(state.isCollapsed)
  }

  func testBottomDraggedBarExpandsInsideItsRealScreen() throws {
    let state = OverlayState.previewFormatted()
    try withPanel(state: state) { panel, root in
      let visible = try XCTUnwrap(panel.screen ?? NSScreen.main).visibleFrame
      state.toggleCollapsed()
      panel.setFrameOrigin(NSPoint(x: visible.minX + 20, y: visible.minY + 12))
      state.toggleCollapsed()
      settle(root)
      XCTAssertTrue(visible.contains(panel.frame))
      XCTAssertEqual(panel.frame.minY, visible.minY, accuracy: 0.5)
    }
  }

  func testTakeStartShowsTranscriptAndNeverCollapsesAnExpandedPane() {
    let engine = OverlayChromePolicyEngine()
    let state = OverlayState()
    state.engine = engine
    state.attach()
    XCTAssertTrue(state.expandedByDefault)
    XCTAssertFalse(state.isCollapsed)
    var collapses: [Bool] = []
    state.onCollapseChanged = { collapses.append($0) }
    state.handleRecordingPreparing()
    XCTAssertFalse(state.isCollapsed)
    state.handleRecordingStarted()
    XCTAssertFalse(state.isCollapsed)
    XCTAssertTrue(collapses.isEmpty, "Starting a take must not transiently collapse the pane")
    XCTAssertTrue(engine.expansionWrites.isEmpty)
    state.finishControllerRecording()
  }

  func testChevronCollapseIsLocalAndNextTakeExpandsWithoutWritingPreference() {
    let engine = OverlayChromePolicyEngine()
    let state = OverlayState()
    state.engine = engine
    state.attach()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.toggleCollapsed()
    XCTAssertTrue(state.isCollapsed)
    XCTAssertTrue(state.expandedByDefault)
    XCTAssertTrue(engine.expanded)
    XCTAssertTrue(engine.expansionWrites.isEmpty)
    state.finishControllerRecording()
    state.handleRecordingPreparing()
    XCTAssertFalse(state.isCollapsed)
    state.handleRecordingStarted()
    XCTAssertFalse(state.isCollapsed)
    XCTAssertTrue(engine.expansionWrites.isEmpty)
    state.finishControllerRecording()
    let reopened = OverlayState()
    reopened.engine = engine
    reopened.attach()
    XCTAssertFalse(reopened.isCollapsed)
    XCTAssertTrue(engine.expansionWrites.isEmpty)
  }

  func testChevronOnlyChangesViewEvenWhenPreferenceStorageRefusesWrites() {
    let engine = OverlayChromePolicyEngine()
    let state = OverlayState()
    state.engine = engine
    state.attach()
    engine.expansionWriteAllowed = false
    var collapses: [Bool] = []
    state.onCollapseChanged = { collapses.append($0) }
    state.toggleCollapsed()
    XCTAssertTrue(state.isCollapsed)
    state.toggleCollapsed()
    XCTAssertFalse(state.isCollapsed)
    XCTAssertEqual(collapses, [true, false])
    XCTAssertTrue(state.expandedByDefault)
    XCTAssertTrue(engine.expanded)
    XCTAssertTrue(engine.expansionWrites.isEmpty)
    XCTAssertNil(state.expansionPreferenceError)
  }

  func testTakeStartHonorsOptOutAfterLocalExpansion() {
    let engine = OverlayChromePolicyEngine()
    engine.expanded = false
    let state = OverlayState()
    state.engine = engine
    state.attach()
    XCTAssertTrue(state.isCollapsed)
    state.toggleCollapsed()
    XCTAssertFalse(state.isCollapsed)
    state.handleRecordingPreparing()
    XCTAssertTrue(state.isCollapsed)
    state.handleRecordingStarted()
    XCTAssertTrue(state.isCollapsed)
    XCTAssertTrue(state.recording)
    XCTAssertFalse(state.expandedByDefault)
    XCTAssertFalse(engine.expanded)
    XCTAssertTrue(engine.expansionWrites.isEmpty)
    state.finishControllerRecording()
  }

  func testTakeStartWithoutPreparingAppliesPreference() {
    for expanded in [true, false] {
      let engine = OverlayChromePolicyEngine()
      engine.expanded = expanded
      let state = OverlayState()
      state.engine = engine
      state.attach()
      state.toggleCollapsed()
      state.handleRecordingStarted()
      XCTAssertEqual(state.isCollapsed, !expanded)
      XCTAssertTrue(engine.expansionWrites.isEmpty)
      state.finishControllerRecording()
    }
  }

  func testTakeStartDoesNotReapplyPreferenceDuringAnOpenCapture() {
    let engine = OverlayChromePolicyEngine()
    let state = OverlayState()
    state.engine = engine
    state.attach()
    state.handleRecordingPreparing()
    state.toggleCollapsed()
    state.handleRecordingStarted()
    XCTAssertTrue(state.isCollapsed, "Ready acknowledgement is still the same take")
    state.handleRecordingPreparing()
    XCTAssertTrue(state.isCollapsed, "Duplicate lifecycle callbacks preserve the current view")
    state.handleRecordingStarted()
    XCTAssertTrue(state.isCollapsed)
    XCTAssertTrue(state.expandedByDefault)
    XCTAssertTrue(engine.expansionWrites.isEmpty)
    state.finishControllerRecording()
  }

  func testMenuToggleAlonePersistsTakeStartPreference() throws {
    let engine = OverlayChromePolicyEngine()
    let state = OverlayState()
    state.engine = engine
    state.attach()
    state.setExpandedByDefault(false)
    XCTAssertTrue(state.isCollapsed)
    XCTAssertFalse(state.expandedByDefault)
    state.toggleCollapsed()
    XCTAssertFalse(state.isCollapsed)
    XCTAssertFalse(state.expandedByDefault)
    XCTAssertEqual(engine.expansionWrites, [false])
    let reopened = OverlayState()
    reopened.engine = engine
    reopened.attach()
    XCTAssertTrue(reopened.isCollapsed)
    state.setExpandedByDefault(true)
    XCTAssertTrue(state.expandedByDefault)
    XCTAssertEqual(engine.expansionWrites, [false, true])
    engine.expansionWriteAllowed = false
    state.setExpandedByDefault(false)
    XCTAssertFalse(state.isCollapsed)
    XCTAssertTrue(state.expandedByDefault)
    XCTAssertEqual(state.expansionPreferenceError, "Couldn't save overlay preference")

    let menu = try source(at: "Codescribe/Screens/Overlay/OverlayPlacementMenu.swift")
    XCTAssertTrue(menu.contains("Show transcript by default"))
    XCTAssertTrue(menu.contains("overlay-expanded-by-default"))
    XCTAssertTrue(menu.contains("state.setExpandedByDefault($0)"))
    XCTAssertTrue(
      menu.contains(
        "New recordings open with the transcript. Turn off to show only the recording bar."))
  }

  func testSavedPinLoadsWithoutWritingAgain() {
    let engine = OverlayChromePolicyEngine()
    let first = OverlayState()
    first.engine = engine
    first.attach()
    XCTAssertFalse(first.keepVisibleBetweenTakes)
    first.setKeepVisibleBetweenTakes(true)
    XCTAssertEqual(engine.pinWrites, [true])

    let reopened = OverlayState()
    reopened.engine = engine
    reopened.attach()
    XCTAssertTrue(reopened.keepVisibleBetweenTakes)
    XCTAssertEqual(engine.pinWrites, [true])
  }
  func testOverlayStartsAsRecordingBar() throws {
    let state = OverlayState()
    XCTAssertTrue(state.isCollapsed)
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "OverlayCollapse.default")
      ) as? FloatingOverlayPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    XCTAssertEqual(panel.frame.height, DictationOverlayWindow.collapsedHeight, accuracy: 0.5)
    XCTAssertFalse(panel.styleMask.contains(.resizable))
  }

  func testCollapsePreservesTextSizeAndTopEdgeAcrossBarDrag() throws {
    let state = OverlayState.previewFormatted()
    let text = state.activeText
    try withPanel(state: state, width: 700) { panel, root in
      panel.setFrame(NSRect(x: 140, y: 180, width: 700, height: 400), display: true)
      let expanded = panel.frame
      let native = try XCTUnwrap(findTranscript(in: root))
      state.toggleCollapsed()
      settle(root)
      XCTAssertEqual(panel.frame.maxY, expanded.maxY, accuracy: 0.5)
      XCTAssertEqual(panel.frame.height, DictationOverlayWindow.collapsedHeight, accuracy: 0.5)
      XCTAssertEqual(panel.sizeForPersistence, expanded.size)
      XCTAssertEqual(state.activeText, text)
      XCTAssertTrue(findTranscript(in: root) === native, "Folding must not recreate the editor")
      panel.setFrameOrigin(NSPoint(x: 200, y: panel.frame.minY + 40))
      let movedTop = panel.frame.maxY
      state.toggleCollapsed()
      settle(root)
      XCTAssertEqual(panel.frame.size, expanded.size)
      XCTAssertEqual(panel.frame.maxY, movedTop, accuracy: 0.5)
      XCTAssertEqual(state.activeText, text)
      XCTAssertTrue(findTranscript(in: root) === native)
    }
  }

  // UX35C-a contracts, authored under W1; execution belongs to the integrator.
  func testActionsAreInsertedOnlyAfterActivationAndHaveNoPinState() throws {
    let source = try overlaySource()
    let tools = try section(of: source, from: "HStack(spacing: 2)", to: "/// 1px separator")
    XCTAssertTrue(tools.contains("if actions.phase == .open {\n            intentRail"))
    XCTAssertTrue(tools.contains("if actions.phase == .hover { Text(\"Actions…\") }"))
    XCTAssertTrue(tools.contains("actions.toggle()"))
    XCTAssertTrue(tools.contains(".onHover { actions.pointerChanged($0) }"))
    XCTAssertTrue(tools.contains(".focusable()"))
    XCTAssertTrue(tools.contains("actions.phase == .open ? \"Open\" : \"Collapsed\""))
    XCTAssertTrue(tools.contains("overlay-retained-work-badge"))
    XCTAssertFalse(source.contains("togglePin"))
    XCTAssertFalse(source.contains("isPinned"))
    XCTAssertFalse(source.contains("voiceOverEnabled"))
    XCTAssertTrue(tools.contains(".onExitCommand { actions.dismiss() }"))
    XCTAssertTrue(tools.contains("if collapsed { actions.reset() }"))
    XCTAssertTrue(
      tools.contains(".onChange(of: state.captureGeneration) { _, _ in actions.reset() }"))
    XCTAssertTrue(tools.contains(".task(id: actions.hideDeadline)"))
    XCTAssertTrue(tools.contains("guard !Task.isCancelled else { return }"))
  }

  func testActionsExpireThreeSecondsAfterLastOutsideInteraction() {
    let start = ContinuousClock.now
    var actions = OverlayActionsPresentation()
    actions.toggle(at: start)
    actions.expire(at: start.advanced(by: .milliseconds(2999)))
    XCTAssertEqual(actions.phase, .open)
    actions.expire(at: start.advanced(by: .seconds(3)))
    XCTAssertEqual(actions.phase, .idle)
    XCTAssertNil(actions.hideDeadline)

    actions.toggle(at: start)
    actions.interact(at: start.advanced(by: .milliseconds(2500)))
    actions.expire(at: start.advanced(by: .seconds(4)))
    XCTAssertEqual(actions.phase, .open, "A tool click renews the deadline")
    actions.expire(at: start.advanced(by: .milliseconds(5500)))
    XCTAssertEqual(actions.phase, .idle)
  }

  func testPointerInsideCancelsDeadlineAndExitStartsFreshGracePeriod() {
    let start = ContinuousClock.now
    var actions = OverlayActionsPresentation()
    actions.toggle(at: start)
    actions.pointerChanged(true, at: start.advanced(by: .seconds(2)))
    XCTAssertNil(actions.hideDeadline)
    actions.expire(at: start.advanced(by: .seconds(10)))
    XCTAssertEqual(actions.phase, .open)
    actions.pointerChanged(false, at: start.advanced(by: .seconds(10)))
    actions.expire(at: start.advanced(by: .seconds(12)))
    XCTAssertEqual(actions.phase, .open)
    actions.expire(at: start.advanced(by: .seconds(13)))
    XCTAssertEqual(actions.phase, .idle)
  }

  func testLeadingCapEscapeAndLifecycleResetCloseWithoutHoverReopeningTools() {
    var actions = OverlayActionsPresentation()
    actions.pointerChanged(true)
    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
    actions.toggle()
    XCTAssertEqual(actions.phase, .idle)
    actions.pointerChanged(false)
    actions.pointerChanged(true)
    XCTAssertEqual(actions.phase, .hover)
    actions.toggle()
    actions.dismiss()
    XCTAssertEqual(actions.phase, .idle)
    for _ in 0..<2 {
      actions.toggle()
      actions.reset()
      XCTAssertEqual(actions.phase, .idle)
      XCTAssertFalse(actions.pointerInside)
      XCTAssertNil(actions.hideDeadline)
    }
  }

  func testIdleRetainedTakeAndVoiceOverDoNotMountToolsThenCapOpensAndCloses() throws {
    let state = OverlayState.previewFormatted()
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Retained edit")
    state.endTranscriptEdit()
    state.handleRecordingPreparing()
    XCTAssertTrue(state.hasRecoverableSupersededWork)
    try withPanel(state: state, width: 320) { _, root in
      func element(_ identifier: String) -> (any NSAccessibilityProtocol)? {
        accessibilityTree(root).first { $0.accessibilityIdentifier() == identifier }
      }
      let cap = try XCTUnwrap(element("overlay-tools-handle"))
      XCTAssertEqual(cap.accessibilityValue() as? String, "Collapsed")
      XCTAssertNil(element("overlay-intent-dock"))
      XCTAssertNil(element("overlay-previous-take-menu"))
      XCTAssertTrue(cap.accessibilityPerformPress())
      settle(root)
      XCTAssertNotNil(element("overlay-intent-dock"))
      XCTAssertNotNil(element("overlay-previous-take-menu"))
      XCTAssertNotNil(element("overlay-intent-finish"))
      XCTAssertTrue(try XCTUnwrap(element("overlay-tools-handle")).accessibilityPerformPress())
      settle(root)
      XCTAssertNil(element("overlay-intent-dock"))
      state.finishControllerRecording()
    }
    let host = NSHostingView(
      rootView: DictationOverlayView(state: .previewFormatted())
        .environment(\.accessibilityVoiceOverEnabled, true))
    host.frame = CGRect(x: 0, y: 0, width: 320, height: 280)
    settle(host)
    XCTAssertFalse(
      accessibilityTree(host).contains {
        $0.accessibilityIdentifier() == "overlay-intent-dock"
      })
  }

  func testOpenRowKeepsEightToolsAndRecordingKeepsItsThreeTools() throws {
    let rows: [(String, [OverlayIntent], Bool, [String])] = [
      (
        "formatted",
        [
          .recoverSuperseded, .discardSuperseded, .insertPaste, .copy,
          .retranscribe, .format, .sendToAgent, .close,
        ], true,
        [
          "overlay-history-menu", "overlay-previous-take-menu", "overlay-intent-insert-paste",
          "overlay-intent-copy", "overlay-intent-retranscribe", "overlay-format-level-picker",
          "overlay-intent-format", "overlay-intent-send-to-agent",
        ]
      ),
      (
        "listening", [.recoverSuperseded, .discardSuperseded, .finish, .copy, .close], false,
        ["overlay-previous-take-menu", "overlay-intent-finish", "overlay-intent-copy"]
      ),
    ]
    for (phase, intents, history, identifiers) in rows {
      let rail = OverlayIntentRail(
        phase: phase, intents: intents, palette: .dark, historyAvailable: history,
        onIntent: { _ in })
      let host = NSHostingView(rootView: rail)
      host.frame = CGRect(x: 0, y: 0, width: 280, height: 80)
      settle(host)
      let rendered = Set(accessibilityTree(host).compactMap { $0.accessibilityIdentifier() })
      for identifier in identifiers { XCTAssertTrue(rendered.contains(identifier), identifier) }
      XCTAssertFalse(rendered.contains("overlay-intent-close"))
      XCTAssertEqual(identifiers.count, history ? 8 : 3)
    }
  }

  func testTakeStartAndCollapseRemoveMountedTools() throws {
    let state = OverlayState.previewFormatted()
    try withPanel(state: state) { _, root in
      func pressCap() throws {
        let cap = try XCTUnwrap(
          accessibilityTree(root).first {
            $0.accessibilityIdentifier() == "overlay-tools-handle"
          })
        XCTAssertTrue(cap.accessibilityPerformPress())
        settle(root)
      }
      func hasTools() -> Bool {
        accessibilityTree(root).contains { $0.accessibilityIdentifier() == "overlay-intent-dock" }
      }
      try pressCap()
      XCTAssertTrue(hasTools())
      state.handleRecordingPreparing()
      settle(root)
      XCTAssertFalse(hasTools())
      try pressCap()
      XCTAssertTrue(hasTools())
      state.toggleCollapsed()
      settle(root)
      XCTAssertFalse(hasTools())
      state.toggleCollapsed()
      settle(root)
      XCTAssertFalse(hasTools())
      state.finishControllerRecording()
    }
  }

  func testEveryToolHasACaptionAndDispatchResetsInteraction() {
    var interactions = 0
    var dispatched: [OverlayIntent] = []
    let rail = OverlayIntentRail(
      phase: "formatted", intents: OverlayIntent.allCases, palette: .dark,
      onIntent: { dispatched.append($0) }, onInteraction: { interactions += 1 })
    for intent in OverlayIntent.allCases where intent != .close {
      XCTAssertFalse(rail.caption(for: intent.rawValue)?.isEmpty ?? true, intent.rawValue)
    }
    for control in ["history", "previous-take", "format-level"] {
      XCTAssertFalse(rail.caption(for: control)?.isEmpty ?? true)
    }
    XCTAssertNotEqual(rail.caption(for: "history"), rail.caption(for: "previous-take"))
    rail.dispatch(.copy)
    XCTAssertEqual(dispatched, [.copy])
    XCTAssertEqual(interactions, 1)
    rail.retranscribe(.fullHq)
    XCTAssertEqual(interactions, 2)
  }

  func testReducedMotionAndTransparencyApplyToTheEntireActionsSurface() throws {
    let source = try overlaySource()
    let tools = try section(of: source, from: "HStack(spacing: 2)", to: "/// 1px separator")
    XCTAssertTrue(tools.contains(".animation(reduceMotion ? nil :"))
    XCTAssertTrue(tools.contains("transaction.animation = nil"))
    XCTAssertTrue(tools.contains("transaction.disablesAnimations = true"))
    let material = try section(
      of: railSource(), from: "struct OverlayActionsSurface", to: "/// Symbols shared")
    XCTAssertTrue(material.contains("if reduceTransparency"))
    XCTAssertTrue(material.contains("Capsule().fill(palette.desktopBackground.color)"))
    XCTAssertTrue(material.contains("} else {\n          Capsule().fill(.regularMaterial)"))
  }

  func testFinishingNoticeSurvivesFoldWithoutChangingBarHeight() throws {
    let state = OverlayState.previewListening()
    state.handleRecordingStarted()
    state.handleRecordingFinalising()
    let words = state.canvasText
    try withPanel(state: state) { panel, root in
      func noticeExists() -> Bool {
        accessibilityTree(root).contains { $0.accessibilityIdentifier() == "overlay-finishing" }
      }
      XCTAssertTrue(noticeExists())
      state.toggleCollapsed()
      settle(root)
      XCTAssertTrue(noticeExists())
      XCTAssertEqual(panel.frame.height, DictationOverlayWindow.collapsedHeight, accuracy: 0.5)
      XCTAssertEqual(state.canvasText, words)
      state.finishControllerRecording()
    }
  }

  func testFinishingFollowsStopLifecycleAndTerminalReceiptWithoutADuration() {
    let state = OverlayState.previewListening()
    state.handleRecordingStarted()
    XCTAssertNil(
      OverlayActionsPresentation.finishingLabel(
        mode: state.mode, transcribing: state.transcribing, terminal: state.terminal))
    state.handleRecordingFinalising()
    XCTAssertEqual(
      OverlayActionsPresentation.finishingLabel(
        mode: state.mode, transcribing: state.transcribing, terminal: state.terminal), "Finishing…")
    state.finishControllerRecording()
    XCTAssertNil(
      OverlayActionsPresentation.finishingLabel(
        mode: .formatted, transcribing: state.transcribing, terminal: true))
    XCTAssertEqual(
      OverlayActionsPresentation.finishingLabel(
        mode: .finalizing, transcribing: false, terminal: false), "Finishing…")
    XCTAssertNil(
      OverlayActionsPresentation.finishingLabel(
        mode: .finalizing, transcribing: true, terminal: true))
  }

  func testResizeChromeUsesTheGeometryContractsWithoutAddingSwiftUIHitTargets() throws {
    let source = try overlaySource()
    let chrome = try section(
      of: source, from: "private func canvasStack", to: "/// 1px separator")
    XCTAssertTrue(
      chrome.contains("width: OverlayResizeChrome.actionsWidth(narrow: actions.phase != .hover)"))
    XCTAssertTrue(chrome.contains("height: OverlayResizeChrome.actionsHeight"))
    XCTAssertTrue(chrome.contains(".padding(.bottom, OverlayResizeChrome.actionsBottomInset)"))
    XCTAssertTrue(chrome.contains("width: OverlayResizeChrome.gripSize.width"))
    XCTAssertTrue(chrome.contains("height: OverlayResizeChrome.gripSize.height"))
    XCTAssertTrue(chrome.contains(".padding(.bottom, OverlayResizeChrome.gripBottomInset)"))
    XCTAssertTrue(chrome.contains("sideIndicatorOpacity(pointerInside: pointerInsideOverlay)"))
    XCTAssertTrue(chrome.contains("sideIndicatorAnimation(reduceMotion: reduceMotion)"))
    XCTAssertTrue(chrome.contains("transaction.disablesAnimations = true"))
    XCTAssertTrue(source.contains("pointerInsideOverlay = inside"))
    XCTAssertEqual(chrome.components(separatedBy: ".allowsHitTesting(false)").count - 1, 4)
    XCTAssertEqual(chrome.components(separatedBy: ".accessibilityHidden(true)").count - 1, 3)
    XCTAssertFalse(chrome.contains("DragGesture"))
  }

  private func findTranscript(in view: NSView) -> LiveTranscriptNativeTextView? {
    if let text = view as? LiveTranscriptNativeTextView { return text }
    return view.subviews.lazy.compactMap { self.findTranscript(in: $0) }.first
  }

  func testHeaderCloseMarkIsOnTheBrandDot() throws {
    try withPanel(state: .previewListening()) { panel, root in
      let elements = accessibilityTree(root)
      XCTAssertFalse(elements.isEmpty, "The rendered accessibility hierarchy must be observable")
      let header = try headerSource(overlaySource())
      let close = try section(
        of: header, from: "Button {\n          state.relayIntent(.close)",
        to: "Text(\"codescribe\")")
      XCTAssertTrue(close.contains("ModeDot("))
      XCTAssertTrue(close.contains("color: palette.statusToken(for: state.mode).color"))
      XCTAssertTrue(close.contains("if closeDotHovered {"))
      XCTAssertTrue(close.contains("OverlayCloseCross()"))
      XCTAssertTrue(close.contains(".accessibilityHidden(true)"))
      XCTAssertFalse(header.contains("Text(\"×\")"), "The mark must not be a sibling glyph")
      XCTAssertNotNil(panel.contentView)
    }
  }

  func testBrandDotClosesTheOverlay() throws {
    let state = OverlayState.previewListening()
    var closes = 0
    state.onClose = { closes += 1 }
    state.relayIntent(.close)
    XCTAssertEqual(closes, 1, "The close intent the brand dot relays must reach onClose")

    let source = try overlaySource()
    let header = try headerSource(source)
    let close = try section(
      of: header, from: "Button {\n          state.relayIntent(.close)",
      to: "Text(\"codescribe\")")
    XCTAssertTrue(
      close.contains("state.relayIntent(.close)"),
      "The brand dot must relay the close intent")
    XCTAssertTrue(
      header.contains(".accessibilityIdentifier(\"overlay-brand-close-dot\")"),
      "The brand dot must stay discoverable as the close control")
    XCTAssertTrue(
      header.contains(".accessibilityLabel(OverlayIntent.close.accessibilityLabel)"))
    XCTAssertTrue(
      close.contains("color: palette.statusToken(for: state.mode).color"),
      "The close control is the brand status dot")
    XCTAssertEqual(header.components(separatedBy: "state.relayIntent(.close)").count - 1, 1)
    XCTAssertEqual(header.components(separatedBy: "overlay-brand-close-dot").count - 1, 1)
    // The dot keeps its pre-b83e95538 place: the hit target grows through the
    // content shape, never through a frame that shifts the dot or the wordmark.
    XCTAssertTrue(close.contains("size: compact ? 5.25 : 7"))
    XCTAssertFalse(close.contains("size: closeDotHovered"))
    XCTAssertTrue(close.contains(".scaleEffect(closeDotHovered"))
    let scale = try XCTUnwrap(close.range(of: ".scaleEffect(")?.lowerBound)
    let hitShape = try XCTUnwrap(close.range(of: ".contentShape(")?.lowerBound)
    XCTAssertLessThan(
      scale, hitShape, "Hover growth must not change the button's layout or hit shape")
    XCTAssertTrue(close.contains(".onHover { closeDotHovered = $0 }"))
    XCTAssertTrue(close.contains(".contentShape(Circle().inset(by: compact ? -9.375 : -8.5))"))
    XCTAssertFalse(close.contains(".frame("), "A frame would move the dot")
    XCTAssertTrue(header.contains("Text(\"codescribe\")"))
    XCTAssertTrue(header.contains(".allowsHitTesting(false)"))
    // The brand block sits on an inert drag region so the dot answers clicks,
    // not window drags (Founder 19:18: the dot next to codescribe closes).
    XCTAssertTrue(header.contains("overlay-header-inert-drag-region"))
  }

  func testAnchorMenuShowsPersistedPinWithoutOpeningIt() throws {
    let menu = try source(at: "Codescribe/Screens/Overlay/OverlayPlacementMenu.swift")
    XCTAssertTrue(menu.contains("\"Keep visible between takes\""))
    XCTAssertTrue(menu.contains("state.setKeepVisibleBetweenTakes($0)"))
    XCTAssertTrue(menu.contains("if state.keepVisibleBetweenTakes {"))
    XCTAssertTrue(menu.contains("Image(systemName: \"pin.fill\")"))
    XCTAssertTrue(menu.contains("overlay-keep-visible-between-takes"))
  }

  func testPhasePillIsGone() throws {
    let state = OverlayState.previewListening()
    try withPanel(state: state) { _, root in
      let elements = accessibilityTree(root)
      XCTAssertFalse(elements.contains { $0.accessibilityIdentifier() == "overlay-phase-status" })
    }
    let source = try overlaySource()
    XCTAssertFalse(source.contains("StatusPill("), "No phase capsule may render in the header")
    XCTAssertFalse(source.contains("StaticStatusPill("))
    XCTAssertFalse(source.contains("phaseStatus(text:"))
    // The phase survives for assistive tech only: the intent dock carries it
    // as its accessibility value, never as visible chrome.
    XCTAssertTrue(source.contains("phase: state.statusText"))
    let rail = try railSource()
    XCTAssertTrue(rail.contains(".accessibilityValue(Self.accessibilityValue(for: phase))"))
    XCTAssertFalse(rail.contains("StatusPill("))
    XCTAssertFalse(rail.contains("Text(phase"))
  }

  func testAutoPasteToggleMirrorsStateAndFlipsIt() throws {
    let engine = OverlayChromePolicyEngine()
    let state = OverlayState.previewListening()
    state.engine = engine
    XCTAssertTrue(state.autoPasteEnabled)
    XCTAssertTrue(state.autoPasteControlAvailable)

    // The header control flips the durable policy through the engine and
    // re-reads truth; it never paints an optimistic switch.
    state.setAutoPasteEnabled(!state.autoPasteEnabled)
    XCTAssertEqual(engine.writes, [false])
    XCTAssertFalse(state.autoPasteEnabled)
    state.setAutoPasteEnabled(!state.autoPasteEnabled)
    XCTAssertEqual(engine.writes, [false, true])
    XCTAssertTrue(state.autoPasteEnabled)

    // Unavailable (agent session armed): the control is disabled and writes
    // are refused, the policy stays where it was.
    state.setAutoPasteControlAvailable(false)
    XCTAssertFalse(state.autoPasteControlAvailable)
    state.setAutoPasteEnabled(false)
    XCTAssertEqual(engine.writes, [false, true])
    XCTAssertTrue(state.autoPasteEnabled)

    let source = try overlaySource()
    let header = try headerSource(source)
    XCTAssertTrue(
      header.contains("autoPasteControl"),
      "Both header widths are built by justifiedHeader and must carry the toggle")
    let control = try autoPasteControlSource(source)
    XCTAssertTrue(control.contains("state.setAutoPasteEnabled(!state.autoPasteEnabled)"))
    XCTAssertTrue(control.contains(".disabled(!state.autoPasteControlAvailable)"))
    XCTAssertTrue(control.contains(".accessibilityIdentifier(\"overlay-auto-paste\")"))
    XCTAssertTrue(
      control.contains(".accessibilityValue(state.autoPasteEnabled ? \"On\" : \"Off\")"))
  }

  private func withPanel(
    state: OverlayState, width: CGFloat = 470,
    _ check: (FloatingOverlayPanel, NSView) throws -> Void
  ) throws {
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "OverlayChromeFounderCutTests.scale")
      ) as? FloatingOverlayPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.setContentSize(NSSize(width: width, height: 280))
    panel.orderFrontRegardless()
    let root = try XCTUnwrap(panel.contentView)
    settle(root)
    try check(panel, root)
  }

  private func settle(_ root: NSView) {
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    root.layoutSubtreeIfNeeded()
  }

  private func accessibilityTree(_ root: Any) -> [any NSAccessibilityProtocol] {
    func walk(_ object: Any) -> [any NSAccessibilityProtocol] {
      guard let element = object as? any NSAccessibilityProtocol
      else { return [] }
      // AppKit wrapper views can have no AX children while hosting a SwiftUI
      // accessibility subtree. Traverse both graphs, deduplicated by identity.
      let nativeChildren = (object as? NSView)?.subviews ?? []
      return [element] + ((element.accessibilityChildren() ?? []) + nativeChildren).flatMap(walk)
    }
    return walk(root)
  }

  private func overlaySource() throws -> String {
    try source(at: "Codescribe/Screens/Overlay/DictationOverlayView.swift")
  }

  private func railSource() throws -> String {
    try source(at: "Codescribe/Screens/Overlay/OverlayIntentRail.swift")
  }

  private func source(at relativePath: String) throws -> String {
    let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent(relativePath)
    return try String(contentsOf: url, encoding: .utf8)
  }

  /// `justifiedHeader(compact:)` — the single builder behind fullHeader and
  /// narrowHeader, so one guard covers both widths.
  private func headerSource(_ source: String) throws -> String {
    try section(
      of: source, from: "private func justifiedHeader(compact: Bool)",
      to: "private var autoPasteControl")
  }

  private func autoPasteControlSource(_ source: String) throws -> String {
    try section(of: source, from: "private var autoPasteControl", to: "private func chromeWaveform")
  }

  private func section(of source: String, from start: String, to end: String) throws -> String {
    let lower = try XCTUnwrap(source.range(of: start), start)
    let upper = try XCTUnwrap(source.range(of: end, range: lower.upperBound..<source.endIndex), end)
    return String(source[lower.lowerBound..<upper.lowerBound])
  }
}

@MainActor
private final class OverlayChromePolicyEngine: DictationEngine {
  var expansionWrites: [Bool] = []
  var pinWrites: [Bool] = []
  var pinEnabled = false
  func overlayKeepVisibleBetweenTakes() -> Bool { pinEnabled }
  func setOverlayKeepVisibleBetweenTakes(_ enabled: Bool) -> Bool {
    pinWrites.append(enabled)
    pinEnabled = enabled
    return true
  }
  var expanded = true
  var expansionWriteAllowed = true
  func overlayExpandedByDefault() -> Bool { expanded }
  func setOverlayExpandedByDefault(_ enabled: Bool) -> Bool {
    expansionWrites.append(enabled)
    guard expansionWriteAllowed else { return false }
    expanded = enabled
    return true
  }
  var writes: [Bool] = []
  var enabled = true
  func setListener(_ listener: CsTranscriptionListener) {}
  func startRecording(language: CsLanguage?) async throws {}
  func stopRecording() async throws -> String { "" }
  func isRecording() async -> Bool { false }
  func initModel() async throws {}
  func isModelLoaded() -> Bool { true }
  func currentOverlayPolicy() -> OverlayPolicySnapshot? {
    OverlayPolicySnapshot(autoPasteEnabled: enabled, autoFormatLevel: .correction)
  }
  func setAutoPasteEnabled(_ enabled: Bool) {
    writes.append(enabled)
    self.enabled = enabled
  }
  func setAutoFormatLevel(_ level: FormattingPolicyOption) {}
  func commitUserRevision(
    sessionId: String, sourceRevision: UInt64, renderedText: String
  ) async throws -> CsUserRevisionResult { throw CocoaError(.featureUnsupported) }
  func commitFormatterRevision(
    sessionId: String, sourceRevision: UInt64, level: FormattingPolicyOption?
  ) async throws -> CsUserRevisionResult { throw CocoaError(.featureUnsupported) }
  func pasteText(text: String) async throws -> CsPasteResult {
    throw CocoaError(.featureUnsupported)
  }
  func deferText(text: String) async throws -> CsPasteResult {
    throw CocoaError(.featureUnsupported)
  }
  func copyTaggedTranscript(text: String) async throws {}
  func pasteTargetAppName() async -> String? { nil }
  func sendAssistiveTranscript(text: String) async throws -> Bool { false }
  func transcribeFile(path: String) async throws -> CsTranscription {
    throw CocoaError(.featureUnsupported)
  }
}
