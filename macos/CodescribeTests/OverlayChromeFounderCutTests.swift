import AppKit
import SwiftUI
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
  /// Maciej, 2026-10-19: "na chuj mie te 5000+ jako informacja, jak ja sobie
  /// moge odpalić voice lab i mieć czytelniejsze i lepsze". The uncommitted
  /// words are read in Voice Lab; the bar keeps only the actions capsule and
  /// the transcript never shows them either.
  func testBottomBarCarriesNoUncommittedEvidenceChip() throws {
    XCTAssertThrowsError(
      try source(at: "Codescribe/Screens/Overlay/OverlayEvidenceList.swift"),
      "the evidence chip is removed, not hidden")
    let overlay = try overlaySource()
    XCTAssertFalse(overlay.contains("OverlayEvidence"))
    let transcript = try section(
      of: overlay, from: "private var transcriptScroll", to: "private var noSpeechBody")
    XCTAssertFalse(
      transcript.contains("state.liveEvidence"),
      "Uncommitted hypotheses never take transcript height")
    XCTAssertFalse(overlay.contains("bottomChromeContainer"))
    XCTAssertFalse(overlay.contains("overlay-tools-handle"))
    XCTAssertFalse(overlay.contains("OverlayActionsSurface"))
    let footer = try section(
      of: overlay,
      from: "if !state.isCollapsed && state.windowContentSurface == .transcription {",
      to: "} else if let label = OverlayActionsPresentation.finishingLabel(")
    XCTAssertFalse(footer.contains("intentRail"), "notices stay out of the actions column")
    XCTAssertTrue(footer.contains(".padding(.bottom, OverlayResizeChrome.actionsBottomInset)"))
    XCTAssertTrue(overlay.contains("CSSidebarMaterial()"))
    XCTAssertTrue(overlay.contains("overlay-transcript-actions"))
  }

  func testActionsGlassMorphsInTheBottomNamespaceWithoutPaintingTheBottomBar() throws {
    let overlay = try overlaySource()
    XCTAssertFalse(overlay.contains("OverlayActionsSurface"))
    XCTAssertFalse(overlay.contains(".glassEffect("))
    let column = try section(
      of: overlay, from: "private func transcriptActionsColumn",
      to: "private func agentDrawerColumn")
    XCTAssertTrue(column.contains("CSSidebarMaterial()"))
    XCTAssertTrue(column.contains(".focusable(false)"))
    XCTAssertTrue(column.contains("overlay-transcript-actions"))
    let material = try source(at: "Codescribe/DesignSystem/CSSidebarMaterial.swift")
    XCTAssertTrue(material.contains("reduceTransparency"))
    XCTAssertFalse(material.contains(".glassEffect("))
  }

  func testCoverageWarningStacksWithActionsWithoutHidingOperationErrors() throws {
    let source = try overlaySource()
    XCTAssertTrue(source.contains("@AppStorage(DictationOverlayGate.labModeDefaultsKey)"))
    XCTAssertTrue(source.contains("DeveloperSurface.isPowerModeEnabled(labMode: labMode)"))
    let header = try headerSource(source)
    XCTAssertTrue(
      header.contains("showsDiagnostics && state.compactProjection?.degraded == true"))
    XCTAssertTrue(header.contains("if let error = state.expansionPreferenceError"))
    let refusal = try section(
      of: source, from: "private var transcriptStatus", to: "private var transcriptScroll")
    XCTAssertFalse(refusal.contains("coverageRefusedBody"))
    XCTAssertFalse(
      source.contains("revisionStatusRow"),
      "footerMessage is the single status owner; no duplicate revisionStatusRow remains")
    let footerMessageRow = try section(
      of: source, from: "private var footerMessageRow", to: "private func footerNoticeLabel")
    XCTAssertTrue(footerMessageRow.contains("if footerMessageIsPassiveNotice {"))
    XCTAssertTrue(
      footerMessageRow.contains("showsTooltip: opensMaxSettings"),
      "non-Max footer status must not attach a floating tooltip that duplicates the label"
    )
    XCTAssertTrue(
      footerMessageRow.contains("footerMessageDetail(message)"),
      "error and diagnostic footer rows keep click-popover access to full details"
    )
    XCTAssertTrue(
      source.contains(
        "if bottomChromeSlots.showsCoverageWarning, let warning = state.footerWarning {"))
    XCTAssertTrue(source.contains("OverlayCoverageStatus("))
    XCTAssertFalse(
      source.contains("diagnosticNotice"),
      "the refusal sentence is the chip's own copy, never a lab-only second notice")
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
    try BackgroundTestWindow.requireIsolatedGUI()
    let state = OverlayState.previewFormatted()
    try withPanel(state: state) { panel, root in
      let visible = try XCTUnwrap(panel.screen ?? NSScreen.main).visibleFrame
      state.toggleCollapsed()
      settle(root)
      panel.setFrameOrigin(NSPoint(x: visible.minX + 20, y: visible.minY + 12))
      state.setPresentationMode(.expanded)
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
    XCTAssertEqual(state.presentationMode, .mini)
    state.showTranscription()
    XCTAssertFalse(state.isCollapsed)
    var collapses: [Bool] = []
    state.onPresentationModeChanged = { collapses.append($0 != .expanded) }
    state.handleRecordingPreparing()
    XCTAssertFalse(state.isCollapsed)
    state.handleRecordingStarted()
    XCTAssertFalse(state.isCollapsed)
    XCTAssertTrue(collapses.isEmpty, "Starting a take must not transiently collapse the pane")
    XCTAssertTrue(engine.expansionWrites.isEmpty)
    state.finishControllerRecording()
  }

  /// Header-click cut (2026-10-17): the header is an upward switch. Mini opens
  /// midi and each further click alternates midi <-> expanded; it never folds
  /// back to mini, which stays the rotating preview control's exit.
  func testHeaderClickClimbsThenAlternatesBetweenMidiAndExpanded() {
    let state = OverlayState()
    state.engine = OverlayChromePolicyEngine()
    state.attach()
    XCTAssertEqual(state.presentationMode, .mini)
    state.advancePresentationFromHeader()
    XCTAssertEqual(state.presentationMode, .midi)
    state.advancePresentationFromHeader()
    XCTAssertEqual(state.presentationMode, .expanded)
    state.advancePresentationFromHeader()
    XCTAssertEqual(state.presentationMode, .midi, "the header never returns to mini")
    state.advancePresentationFromHeader()
    XCTAssertEqual(state.presentationMode, .expanded)
    // The rotating preview control keeps its unchanged full cycle.
    state.toggleCollapsed()
    XCTAssertEqual(state.presentationMode, .mini)
  }

  /// Outside mini only the header band advances the presentation: a still
  /// click on the body drag region keeps its ordinary meaning.
  func testPresentationClickHitsHeaderButNotBodyOutsideMini() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let state = OverlayState.previewFormatted()
    try withPanel(state: state) { panel, root in
      for mode in [OverlayPresentationMode.midi, .expanded] {
        state.setPresentationMode(mode)
        settle(root)
        let header = try XCTUnwrap(
          regionClickPoint(identifier: "overlay-header-drag-region", in: root, panel: panel),
          "\(mode): no clickable header drag point")
        XCTAssertTrue(panel.isPresentationClickHit(at: header), "\(mode): header must advance")
        if let body = regionClickPoint(
          identifier: "overlay-body-drag-region", in: root, panel: panel)
        {
          XCTAssertFalse(panel.isPresentationClickHit(at: body), "\(mode): body must not advance")
        }
      }
      state.setPresentationMode(.mini)
      settle(root)
      let anyRegion = try XCTUnwrap(
        dragRegionViews(in: root).lazy.compactMap { view in
          self.regionClickPoint(
            identifier: view.accessibilityIdentifier(), in: root, panel: panel)
        }.first, "mini: no clickable drag region")
      XCTAssertTrue(
        panel.isPresentationClickHit(at: anyRegion), "mini keeps the whole-chrome click")
    }
  }

  /// The first point across the region's width whose window hit-test resolves
  /// to that drag region (controls above the band win their own clicks).
  private func dragRegionViews(in root: NSView) -> [NSView] {
    var found: [NSView] = []
    func walk(_ view: NSView) {
      if view is OverlayWindowDragRegionView { found.append(view) }
      view.subviews.forEach(walk)
    }
    walk(root)
    return found
  }

  private func regionClickPoint(
    identifier: String, in root: NSView, panel: FloatingOverlayPanel
  ) -> NSPoint? {
    for view in dragRegionViews(in: root) {
      guard view.accessibilityIdentifier() == identifier, !view.isHiddenOrHasHiddenAncestor
      else { continue }
      let frame = root.convert(view.bounds, from: view)
      for step in stride(from: 0.1, through: 0.9, by: 0.1) {
        let point = NSPoint(x: frame.minX + frame.width * step, y: frame.midY)
        if root.hitTest(point) === view, panel.isWindowDragHit(at: point) { return point }
      }
    }
    return nil
  }

  func testHeaderStartHonorsThePersistedTakeStartPreference() {
    for expanded in [false, true] {
      for initialMode in [OverlayPresentationMode.mini, .midi, .expanded] {
        let engine = OverlayChromePolicyEngine()
        engine.expanded = expanded
        let state = OverlayState(micAccessProvider: { true })
        state.engine = engine
        state.attach()
        state.setPresentationMode(initialMode)

        state.requestHeaderRecording(.startRecording)
        XCTAssertEqual(state.presentationMode, expanded ? .expanded : .midi)
        state.handleRecordingPreparing()
        XCTAssertEqual(state.presentationMode, expanded ? .expanded : .midi)
        state.handleRecordingStarted()
        XCTAssertEqual(state.presentationMode, expanded ? .expanded : .midi)
        XCTAssertEqual(state.expandedByDefault, expanded)
        XCTAssertTrue(engine.expansionWrites.isEmpty)
        state.finishControllerRecording()
      }
    }
  }

  func testHeaderStopKeepsTheCurrentWidgetAndDoesNotArmTheNextTake() {
    let engine = OverlayChromePolicyEngine()
    engine.expanded = false
    let state = OverlayState(micAccessProvider: { true })
    state.engine = engine
    state.attach()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertEqual(state.presentationMode, .midi)

    state.requestHeaderRecording(.finish)
    XCTAssertEqual(state.presentationMode, .midi)
    state.finishControllerRecording()
    engine.expanded = true
    state.handleRecordingPreparing()
    XCTAssertEqual(state.presentationMode, .expanded)
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
    XCTAssertEqual(reopened.presentationMode, .mini)
    XCTAssertTrue(engine.expansionWrites.isEmpty)
  }

  func testChevronOnlyChangesViewEvenWhenPreferenceStorageRefusesWrites() {
    let engine = OverlayChromePolicyEngine()
    let state = OverlayState()
    state.engine = engine
    state.attach()
    state.showTranscription()
    engine.expansionWriteAllowed = false
    var collapses: [Bool] = []
    state.onPresentationModeChanged = { collapses.append($0 != .expanded) }
    state.toggleCollapsed()
    XCTAssertTrue(state.isCollapsed)
    state.toggleCollapsed()
    XCTAssertEqual(state.presentationMode, .midi)
    state.toggleCollapsed()
    XCTAssertFalse(state.isCollapsed)
    XCTAssertEqual(collapses, [true, true, false])
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
    state.setPresentationMode(.expanded)
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
    state.setPresentationMode(.expanded)
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

    let menu = try source(at: "Codescribe/Screens/Overlay/OverlayHeaderMenu.swift")
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
    XCTAssertFalse(panel.styleMask.contains(.resizable), "MINI is a drag surface")
  }

  func testRouterRestoresCanvasAndDrawerFromMidiWithoutChangingCapture() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let state = OverlayState.previewFormatted()
    let text = state.activeText
    let generation = state.captureGeneration
    try withPanel(state: state, width: 700) { panel, root in
      let canvas = try XCTUnwrap(findTranscript(in: root))
      let fullSize = panel.frame.size
      let pin = NSPoint(x: panel.frame.maxX, y: panel.frame.maxY)
      state.setPresentationMode(.mini)
      settle(root)
      for mode in [OverlayPresentationMode.midi, .expanded, .mini, .midi, .expanded, .mini] {
        state.toggleCollapsed()
        settle(root)
        XCTAssertEqual(state.presentationMode, mode)
        XCTAssertEqual(panel.frame.height, mode == .expanded ? fullSize.height : 46, accuracy: 0.5)
        XCTAssertEqual(panel.frame.maxX, pin.x, accuracy: 0.5)
        XCTAssertEqual(panel.frame.maxY, pin.y, accuracy: 0.5)
        XCTAssertEqual(panel.sizeForPersistence, fullSize)
        XCTAssertTrue(findTranscript(in: root) === canvas)
        XCTAssertEqual(state.activeText, text)
        XCTAssertEqual(state.captureGeneration, generation)
      }
      state.setPresentationMode(.midi)
      settle(root)
      XCTAssertEqual(panel.frame.width, DictationOverlayWindow.midiSize.width, accuracy: 0.5)
      state.showAgentMonitor()
      settle(root)
      XCTAssertEqual(state.presentationMode, .expanded)
      XCTAssertTrue(state.showsAgentMonitor)
      XCTAssertTrue(findTranscript(in: root) === canvas)
      XCTAssertEqual(
        panel.frame.size, DictationOverlayWindow.defaultSize,
        "Agents keeps its own profile; opening it does not adopt the transcription frame")
      state.hideAgentSidebar()
      XCTAssertTrue(findTranscript(in: root) === canvas)
    }
  }

  func testInterruptedMorphKeepsOriginalCanvasSizeAndTopEdge() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let state = OverlayState.previewFormatted()
    try withPanel(state: state, width: 700) { panel, root in
      let frame = panel.frame
      state.setPresentationMode(.mini)
      XCTAssertTrue(panel.isFrameTransitioning)
      state.setPresentationMode(.midi)
      state.toggleCollapsed()
      settle(root)
      XCTAssertEqual(state.presentationMode, .expanded)
      XCTAssertEqual(panel.frame.size, frame.size)
      XCTAssertEqual(panel.frame.maxY, frame.maxY, accuracy: 0.5)
      XCTAssertTrue(panel.styleMask.contains(.resizable))
      panel.setPresentationMode(.mini, animated: false)
      XCTAssertFalse(panel.isFrameTransitioning)
      XCTAssertEqual(panel.frame.size, DictationOverlayWindow.collapsedSize)
    }
  }

  func testClampedHoverStripReturnsToItsOriginalMiniPosition() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let state = OverlayState.previewFormatted()
    try withPanel(state: state, width: 700) { panel, root in
      state.setPresentationMode(.mini)
      settle(root)
      let screen = try XCTUnwrap(panel.screen?.visibleFrame)
      panel.setFrameOrigin(NSPoint(x: screen.minX + 20, y: screen.maxY - 120))
      let original = panel.frame
      state.setPresentationMode(.midi)
      settle(root)
      XCTAssertGreaterThanOrEqual(panel.frame.minX, screen.minX)
      XCTAssertLessThanOrEqual(panel.frame.maxX, screen.maxX)
      state.setPresentationMode(.mini)
      settle(root)
      XCTAssertEqual(
        panel.frame, original, "hover at the screen edge must not move the parked widget")
    }
  }

  func testSettingsAndTrayOpenWidgetUseTheCachedPanelWithoutStartingCapture() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let state = OverlayState.previewFormatted()
    let text = state.activeText
    let captureGeneration = state.captureGeneration
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "Widget.Tray")) as? FloatingOverlayPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    var creations = 0
    let controller = OverlayController(
      state: state, engine: nil,
      overlayEnabledProvider: { false }, assistiveStatusProvider: { false },
      panelFactory: { _, _ in
        creations += 1
        return panel
      },
      orderPanelFront: { BackgroundTestWindow.present($0) }, orderPanelOut: { $0.orderOut(nil) })
    let preference = state.expandedByDefault
    let settingsModel = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe())
    settingsModel.onQuickStartOpenWidget = { controller.showWidget() }
    settingsModel.performQuickStart(.openWidget)
    let root = try XCTUnwrap(panel.contentView)
    settle(root)
    let canvas = try XCTUnwrap(findTranscript(in: root))
    XCTAssertEqual(state.presentationMode, .mini)
    XCTAssertTrue(panel.isVisible)
    state.showTranscription()
    settle(root)
    controller.showWidget()
    XCTAssertEqual(
      state.presentationMode, .expanded, "opening an already visible widget keeps its view")
    controller.dismiss()
    XCTAssertFalse(panel.isVisible)
    controller.showWidget()
    settle(root)
    XCTAssertEqual(state.presentationMode, .mini)
    XCTAssertTrue(findTranscript(in: root) === canvas)
    XCTAssertEqual(creations, 1)
    XCTAssertEqual(state.activeText, text)
    XCTAssertEqual(state.expandedByDefault, preference)
    XCTAssertFalse(state.transcriptOverlayEnabled, "an explicit open is not a preference write")
    XCTAssertFalse(state.recording)
    XCTAssertEqual(state.captureGeneration, captureGeneration)
  }

  func testOverlayCursorComesFromItsOwnNativeHitSurface() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let state = OverlayState.previewFormatted()
    try withPanel(state: state, width: 700) { panel, root in
      let canvas = try XCTUnwrap(findTranscript(in: root))
      let textPoint = canvas.convert(
        NSPoint(x: canvas.visibleRect.midX, y: canvas.visibleRect.midY), to: nil)
      XCTAssertEqual(
        panel.cursor(at: textPoint), .iBeam,
        "Text hit \(textPoint), visible \(canvas.visibleRect), panel \(panel.frame), native hit \(String(describing: panel.contentView?.hitTest(textPoint)))"
      )
      XCTAssertEqual(
        panel.cursor(at: NSPoint(x: 1, y: panel.frame.height / 2)),
        OverlayResizeHit.cursor(for: .left))
      let chromePoint = NSPoint(x: panel.frame.width - 40, y: panel.frame.height - 23)
      XCTAssertEqual(panel.cursor(at: chromePoint), .arrow)
      let saved = NSCursor.current
      defer { saved.set() }
      NSCursor.iBeam.set()
      let moved = try XCTUnwrap(
        NSEvent.mouseEvent(
          with: .mouseMoved, location: chromePoint, modifierFlags: [],
          timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
          context: nil, eventNumber: 1, clickCount: 0, pressure: 0))
      panel.sendEvent(moved)
      XCTAssertEqual(
        NSCursor.current, .arrow, "the inactive app below cannot leave its text cursor here")
      state.setPresentationMode(.mini)
      settle(root)
      XCTAssertEqual(
        panel.cursor(at: NSPoint(x: 1, y: 23)), .arrow,
        "MINI is a drag surface; resizing belongs to MIDI and expanded")
    }
  }

  func testRepeatedPointerMotionDoesNotSetAnAlreadyCorrectCursor() throws {
    let state = OverlayState.previewFormatted()
    try withPanel(state: state, width: 700) { panel, _ in
      let previous = NSCursor.current
      defer { previous.set() }
      let chrome = NSPoint(x: panel.frame.width - 40, y: panel.frame.height - 23)
      NSCursor.iBeam.set()
      XCTAssertTrue(panel.refreshCursor(at: chrome))
      XCTAssertEqual(NSCursor.current, .arrow)
      for _ in 0..<100 {
        XCTAssertFalse(panel.refreshCursor(at: chrome))
      }
      XCTAssertEqual(NSCursor.current, .arrow)
    }
  }

  func testNativeTextCursorIsNotReplacedAfterAppKitDispatch() throws {
    let state = OverlayState.previewFormatted()
    try withPanel(state: state, width: 700) { panel, root in
      let canvas = try XCTUnwrap(findTranscript(in: root))
      let point = canvas.convert(
        NSPoint(x: canvas.visibleRect.midX, y: canvas.visibleRect.midY), to: nil)
      XCTAssertEqual(panel.cursor(at: point), .iBeam)
      let saved = NSCursor.current
      defer { saved.set() }
      // AppKit can choose a link or selection cursor inside an NSTextView.
      // Reproduce the post-super.sendEvent state rather than a fixed iBeam.
      for nativeCursor in [NSCursor.pointingHand, .arrow, .iBeam] {
        nativeCursor.set()
        for _ in 0..<100 {
          XCTAssertFalse(panel.refreshCursor(at: point))
          XCTAssertEqual(NSCursor.current, nativeCursor)
        }
      }
      let edge = NSPoint(x: 1, y: panel.frame.height / 2)
      NSCursor.pointingHand.set()
      XCTAssertTrue(panel.refreshCursor(at: edge))
      XCTAssertEqual(NSCursor.current, OverlayResizeHit.cursor(for: .left))
    }
  }

  func testHeaderMicrophoneAndTheNextTakeUseTheSavedPreference() {
    for expanded in [false, true] {
      let engine = OverlayChromePolicyEngine()
      engine.expanded = expanded
      let state = OverlayState(micAccessProvider: { true })
      state.engine = engine
      state.attach()
      state.requestHeaderRecording(.startRecording)
      XCTAssertEqual(state.presentationMode, expanded ? .expanded : .midi)
      state.handleRecordingPreparing()
      state.handleRecordingStarted()
      XCTAssertEqual(state.presentationMode, expanded ? .expanded : .midi)
      XCTAssertTrue(engine.expansionWrites.isEmpty)
      state.finishControllerRecording()
      state.handleRecordingPreparing()
      XCTAssertEqual(state.presentationMode, expanded ? .expanded : .midi)
      state.finishControllerRecording()
    }
  }

  func testCollapsePreservesTextSizeAndTopEdgeAcrossBarDrag() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let state = OverlayState.previewFormatted()
    let text = state.activeText
    try withPanel(state: state, width: 700) { panel, root in
      panel.setFrame(
        NSRect(
          x: 140, y: 180, width: 700,
          height: DictationOverlayWindow.minSize.height + 100),
        display: true)
      let expanded = panel.frame
      let native = try XCTUnwrap(findTranscript(in: root))
      state.toggleCollapsed()
      settle(root)
      XCTAssertEqual(panel.frame.maxY, expanded.maxY, accuracy: 0.5)
      XCTAssertEqual(panel.frame.height, DictationOverlayWindow.collapsedHeight, accuracy: 0.5)
      XCTAssertEqual(panel.frame.width, DictationOverlayWindow.collapsedSize.width, accuracy: 0.5)
      XCTAssertEqual(panel.sizeForPersistence, expanded.size)
      XCTAssertEqual(state.activeText, text)
      XCTAssertTrue(findTranscript(in: root) === native, "Folding must not recreate the editor")
      panel.setFrameOrigin(NSPoint(x: 200, y: panel.frame.minY + 40))
      let movedTop = panel.frame.maxY
      state.setPresentationMode(.expanded)
      settle(root)
      XCTAssertEqual(panel.frame.size, expanded.size)
      XCTAssertEqual(panel.frame.maxY, movedTop, accuracy: 0.5)
      XCTAssertEqual(state.activeText, text)
      XCTAssertTrue(findTranscript(in: root) === native)
    }
  }

  // UX35C-a contracts, authored under W1; execution belongs to the integrator.
  /// The transcript column is the surface, not a second bar opened by a toggle.
  /// Pin, VoiceOver and the idle clock do not mount or hide those commands.
  func testActionsAreInsertedOnlyAfterActivationAndHaveNoPinState() throws {
    let source = try overlaySource()
    XCTAssertFalse(containsGuardedIntentRail(source))
    XCTAssertFalse(source.contains("actions.toggle()"))
    XCTAssertFalse(source.contains("actions.pointerChanged"))
    XCTAssertFalse(source.contains("overlay-retained-work-badge"))
    XCTAssertFalse(source.contains("overlay-tools-handle"))
    XCTAssertFalse(source.contains("togglePin"))
    XCTAssertFalse(source.contains("isPinned"))
    XCTAssertFalse(source.contains("voiceOverEnabled"))
    XCTAssertFalse(source.contains(".task(id: actions.hideDeadline)"))
    XCTAssertFalse(source.contains("@State private var actions"))
    let canvas = try section(
      of: source, from: "private func canvasStack", to: "private var header: some View")
    XCTAssertEqual(canvas.components(separatedBy: "transcriptActionsColumn(").count - 1, 1)
    XCTAssertTrue(
      canvas.contains("let transcriptActionsShown = transcriptionSurface && !state.isCollapsed"))
    let column = try section(
      of: source, from: "private func transcriptActionsColumn",
      to: "private func agentDrawerColumn")
    XCTAssertTrue(column.contains(".focusable(false)"))
    XCTAssertEqual(column.components(separatedBy: "overlay-transcript-actions").count - 1, 1)
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

  func testPointerNeverOpensToolsAfterDismissal() {
    var actions = OverlayActionsPresentation()
    actions.pointerChanged(true)
    XCTAssertEqual(actions.phase, .idle)
    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
    actions.dismiss()
    actions.pointerChanged(false)
    actions.pointerChanged(true)
    XCTAssertEqual(actions.phase, .idle)
    actions.reset()
    XCTAssertFalse(actions.pointerInside)
    XCTAssertNil(actions.hideDeadline)
  }

  func testIdleRetainedTakeAndVoiceOverDoNotMountToolsThenCapOpensAndCloses() throws {
    let source = try overlaySource()
    XCTAssertFalse(source.contains("overlay-tools-handle"))
    XCTAssertFalse(source.contains("overlay-retained-work-badge"))
    XCTAssertFalse(containsGuardedIntentRail(source))
    XCTAssertTrue(source.contains("overlay-transcript-actions"))
    XCTAssertTrue(
      source.contains(
        "let transcriptActionsShown = transcriptionSurface && !state.isCollapsed"))
    XCTAssertTrue(source.contains("OverlayIntentRail.projectedIntents(for: state)"))
    XCTAssertTrue(source.contains("OverlayRecordingControls.railIntents(from: projectedIntents)"))
    XCTAssertTrue(source.contains("intents: railIntents"))
    let rail = try section(
      of: railSource(), from: "var body: some View", to: "static func projectedIntents")
    XCTAssertTrue(rail.contains(".accessibilityIdentifier(\"overlay-intent-dock\")"))
    XCTAssertTrue(rail.contains("overlay-previous-take-menu"))

    let state = OverlayState.previewFormatted()
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Retained edit")
    state.endTranscriptEdit()
    state.handleRecordingPreparing()
    defer { state.finishControllerRecording() }
    XCTAssertTrue(state.hasRecoverableSupersededWork)
    XCTAssertFalse(state.isCollapsed)
    var actions = OverlayActionsPresentation()
    XCTAssertEqual(
      actions.phase, .idle, "Retained work must not open the guarded rail or its menus")
    XCTAssertNil(actions.hideDeadline)
    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
    let intents = OverlayIntentRail.projectedIntents(for: state)
    XCTAssertTrue(intents.contains(.recoverSuperseded))
    XCTAssertTrue(intents.contains(.discardSuperseded))
    XCTAssertTrue(intents.contains(.finish))
    XCTAssertTrue(rail.contains("enabledSlots.contains(.intent(.sendToAgent))"))
    XCTAssertTrue(rail.contains("id: \"overlay-intent-\\(intent.rawValue)\""))
    actions.toggle()
    XCTAssertEqual(actions.phase, .idle, "The same cap removes the guarded rail again")
    XCTAssertNil(actions.hideDeadline)
    // `accessibilityVoiceOverEnabled` is read-only in the environment, so the
    // guard pins its absence; no XCTest environment write or AX walk is needed.
    XCTAssertFalse(source.contains("accessibilityVoiceOverEnabled"))
    XCTAssertFalse(source.contains("@State private var actions = OverlayActionsPresentation()"))
    XCTAssertEqual(OverlayActionsPresentation().phase, .idle)
  }

  /// Process notices have one owner; actionable failures still open their
  /// full details after the two worker cuts are combined.
  func testProcessStageHasOneVisibleOwnerInTheFooter() throws {
    let source = try overlaySource()
    let footer = try section(
      of: source, from: "private var footerMessage: String?",
      to: "private var footerMessageRow")
    XCTAssertTrue(footer.contains("\"Formatting revision…\""))
    XCTAssertTrue(footer.contains("\"Committing revision…\""))
    XCTAssertTrue(footer.contains("\"Draft · not committed\""))
    XCTAssertFalse(source.contains("private var revisionStatusRow"))
    let row = try section(
      of: source, from: "private var footerMessageRow",
      to: "private func footerNoticeLabel")
    XCTAssertTrue(
      row.contains("footerMessageDetail(message)"),
      "actionable failures keep their full click detail")
    let detail = try section(
      of: source, from: "private func footerMessageDetail",
      to: "private func openMaxSettings")
    XCTAssertTrue(detail.contains("errorBody"))
  }

  func testActionsHandleUsesPhaseForSymbolLabelAndTooltip() throws {
    let source = try overlaySource()
    XCTAssertFalse(source.contains("actions.toggle()"))
    XCTAssertFalse(source.contains("overlay-tools-handle"))
    let column = try section(
      of: source, from: "private func transcriptActionsColumn",
      to: "private func agentDrawerColumn")
    XCTAssertTrue(column.contains(".focusable(false)"))
    XCTAssertTrue(column.contains("CSSidebarMaterial()"))
    let rail = try railSource()
    XCTAssertTrue(rail.contains("OverlayHoverControl("))
    XCTAssertTrue(rail.contains(".accessibilityIdentifier(\"overlay-intent-dock\")"))
  }

  func testFormatRequiresAnExplicitChoiceAndNeverUsesAPrimaryAction() throws {
    let source = try railSource()
    XCTAssertFalse(source.contains("primaryAction:"))
    XCTAssertTrue(source.contains("intent == .format || intent == .retranscribe ? nil"))
    XCTAssertTrue(source.contains("FormattingPolicyOption.correction, .smart, .max"))
    XCTAssertTrue(source.contains("formatOnce(level)"))
    XCTAssertTrue(try overlaySource().contains("onFormatOnce: { state.formatTranscript(at: $0) }"))
  }

  func testNoOverlayFileCanWriteTheSettingsFormattingLevel() throws {
    let directory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent().appendingPathComponent("Codescribe/Screens/Overlay")
    let files = try FileManager.default.contentsOfDirectory(
      at: directory, includingPropertiesForKeys: nil
    ).filter { $0.pathExtension == "swift" }
    XCTAssertFalse(files.isEmpty)
    for file in files {
      let source = try String(contentsOf: file, encoding: .utf8)
      XCTAssertFalse(source.contains("setAutoFormatLevel"), file.lastPathComponent)
    }
    let rail = try railSource()
    for forbidden in ["UserDefaults", "@AppStorage", "setSetting", "setConfig"] {
      XCTAssertFalse(rail.contains(forbidden), forbidden)
    }
  }

  func testOpenRowKeepsProjectedToolsWithoutAnInlineCaption() throws {
    let source = try railSource()
    XCTAssertFalse(source.contains("OverlayCaptionLayout"))
    XCTAssertFalse(source.contains("captionSlot"))
    XCTAssertFalse(source.contains(".help("))
    XCTAssertTrue(source.contains("OverlayHoverControl("))
    let all: [OverlayIntent] = [.insertPaste, .copy, .retranscribe, .format, .sendToAgent, .close]
    XCTAssertEqual(
      OverlayIntentRail.actionAvailability(intents: all, versions: .empty)
        .filter(\.isEnabled).map(\.slot),
      [
        .history, .intent(.insertPaste), .intent(.copy), .intent(.retranscribe),
        .intent(.format), .intent(.sendToAgent),
      ])
  }

  func testTakeStartAndCollapseRemoveMountedTools() throws {
    let source = try overlaySource()
    let canvas = try section(
      of: source, from: "private func canvasStack", to: "private var header: some View")
    XCTAssertTrue(
      canvas.contains(
        "let transcriptActionsShown = transcriptionSurface && !state.isCollapsed"))
    XCTAssertTrue(
      canvas.contains("let railShown = agentsSurface && !state.isCollapsed && !listOnScreen"))
    XCTAssertTrue(
      canvas.range(
        of: #"footerMessageRow\s*\.frame\(height: 18\)"#,
        options: .regularExpression) != nil,
      "Footer notices keep one fixed-height message row")
    XCTAssertFalse(canvas.contains("transcriptStatus"))
    XCTAssertFalse(containsGuardedIntentRail(canvas))
    XCTAssertFalse(canvas.contains("actions.toggle()"))
    XCTAssertTrue(
      canvas.contains("state.refreshRetranscriptionAvailability()"))
    XCTAssertFalse(canvas.contains("actions.reset()"))

    let state = OverlayState.previewFormatted()
    defer { state.finishControllerRecording() }
    var actions = OverlayActionsPresentation()
    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
    let generation = state.captureGeneration
    state.handleRecordingPreparing()
    XCTAssertGreaterThan(state.captureGeneration, generation)
    // Exercise the response pinned to each onChange above, without claiming
    // that this independent value observes SwiftUI state by itself.
    actions.reset()
    XCTAssertEqual(actions.phase, .idle)
    XCTAssertNil(actions.hideDeadline)
    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
    XCTAssertFalse(state.isCollapsed)
    state.toggleCollapsed()
    XCTAssertTrue(state.isCollapsed)
    actions.reset()
    XCTAssertEqual(actions.phase, .idle)
    XCTAssertFalse(actions.pointerInside)
    XCTAssertNil(actions.hideDeadline)
    state.setPresentationMode(.expanded)
    XCTAssertFalse(state.isCollapsed)
    XCTAssertEqual(actions.phase, .idle)
  }

  func testBottomCapsulesAreCenteredWithSymmetricOpenActionsInsets() throws {
    let source = try overlaySource()
    let canvas = try section(
      of: source, from: "private func canvasStack", to: "private var header: some View")
    let columnCall = canvas.range(of: "transcriptActionsColumn(")
    let rail = canvas.range(of: "if railShown {")
    XCTAssertNotNil(columnCall)
    XCTAssertNotNil(rail)
    XCTAssertFalse(canvas.contains("OverlayActionsSurface"))
    XCTAssertFalse(canvas.contains("HStack(spacing: 2)"))
    XCTAssertTrue(canvas.contains("transcriptActionsWidth"))
    let column = try section(
      of: source, from: "private func transcriptActionsColumn",
      to: "private func agentDrawerColumn")
    XCTAssertFalse(
      column.contains("Spacer("),
      "action buttons stay in the column and do not push across the text")
  }

  func testNoticesAndCoverageStayOutsideTheActionCapsule() throws {
    let source = try overlaySource()
    let footer = try section(
      of: source,
      from: "if !state.isCollapsed && state.windowContentSurface == .transcription {",
      to: "} else if let label = OverlayActionsPresentation.finishingLabel(")
    XCTAssertFalse(footer.contains("intentRail"))
    XCTAssertFalse(footer.contains("state.toast"))
    XCTAssertTrue(footer.contains("footerMessageRow"))
    XCTAssertTrue(source.contains("overlay-footer-notice"))
    XCTAssertTrue(source.contains("OverlayCoverageStatus("))
    XCTAssertFalse(
      source.contains("coverageRefusalDetail"),
      "ledger refusal detail is Voice Lab's, not the bar's")
  }

  func testEveryToolHasACaptionAndDispatchResetsInteraction() {
    var interactions = 0
    var dispatched: [OverlayIntent] = []
    let rail = OverlayIntentRail(
      phase: "formatted", intents: OverlayIntent.allCases, palette: .dark,
      onIntent: { dispatched.append($0) }, onInteraction: { interactions += 1 })
    for intent in OverlayIntent.allCases where intent != .close {
      XCTAssertFalse(intent.accessibilityLabel.isEmpty)
    }
    rail.dispatch(.copy)
    XCTAssertEqual(dispatched, [.copy])
    XCTAssertEqual(interactions, 1)
    rail.retranscribe(.fullHq)
    XCTAssertEqual(interactions, 2)
  }

  func testReducedMotionAndTransparencyApplyToTheEntireActionsSurface() throws {
    let source = try overlaySource()
    let canvas = try section(
      of: source, from: "private func canvasStack", to: "private var header: some View")
    XCTAssertTrue(canvas.contains(".animation(reduceMotion ? nil :"))
    XCTAssertTrue(canvas.contains("transaction.animation = nil"))
    XCTAssertTrue(canvas.contains("transaction.disablesAnimations = true"))
    let column = try section(
      of: source, from: "private func transcriptActionsColumn",
      to: "private func agentDrawerColumn")
    XCTAssertTrue(column.contains("CSSidebarMaterial()"))
    let material = try self.source(at: "Codescribe/DesignSystem/CSSidebarMaterial.swift")
    XCTAssertTrue(material.contains("reduceTransparency"))
    XCTAssertTrue(material.contains("increaseContrast"))
    XCTAssertTrue(material.contains("colorSchemeContrast"))
  }

  func testFinishingNoticeSurvivesFoldWithoutChangingBarHeight() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let canvas = try section(
      of: overlaySource(), from: "private func canvasStack", to: "private var header: some View")
    let expanded = try section(of: canvas, from: "if !state.isCollapsed,", to: ".onGeometryChange(")
    let folded = try section(
      of: canvas, from: "} else if let label = OverlayActionsPresentation.finishingLabel(",
      to: ".overlay(alignment: .bottom)")
    for branch in [expanded, folded] {
      XCTAssertTrue(branch.contains("OverlayActionsPresentation.finishingLabel("))
      XCTAssertTrue(
        branch.contains(
          "mode: state.mode, transcribing: state.transcribing, terminal: state.terminal)"))
      XCTAssertTrue(branch.contains("Text(label)"))
      XCTAssertTrue(branch.contains(".accessibilityIdentifier(\"overlay-finishing\")"))
      XCTAssertTrue(branch.contains(".allowsHitTesting(false)"))
    }
    let state = OverlayState.previewListening()
    state.handleRecordingStarted()
    defer { state.finishControllerRecording() }
    state.handleRecordingFinalising()
    let words = state.canvasText
    try withPanel(state: state) { panel, root in
      XCTAssertFalse(state.isCollapsed)
      XCTAssertEqual(
        OverlayActionsPresentation.finishingLabel(
          mode: state.mode, transcribing: state.transcribing, terminal: state.terminal),
        "Finishing…")
      state.toggleCollapsed()
      settle(root)
      XCTAssertTrue(state.isCollapsed)
      XCTAssertEqual(
        OverlayActionsPresentation.finishingLabel(
          mode: state.mode, transcribing: state.transcribing, terminal: state.terminal),
        "Finishing…")
      XCTAssertEqual(panel.frame.height, DictationOverlayWindow.collapsedHeight, accuracy: 0.5)
      XCTAssertEqual(state.canvasText, words)
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
      of: source, from: "private func canvasStack", to: "private var header: some View")
    XCTAssertTrue(chrome.contains("OverlayTranscriptActionsLayout.columnWidth"))
    XCTAssertFalse(chrome.contains("overlay-tools-handle"))
    XCTAssertTrue(chrome.contains(".padding(.bottom, OverlayResizeChrome.actionsBottomInset)"))
    XCTAssertTrue(chrome.contains("width: OverlayResizeChrome.gripSize.width"))
    XCTAssertTrue(chrome.contains("height: OverlayResizeChrome.gripSize.height"))
    XCTAssertTrue(chrome.contains(".padding(.bottom, OverlayResizeChrome.gripBottomInset)"))
    XCTAssertTrue(chrome.contains("sideIndicatorOpacity(pointerInside: pointerInsideOverlay)"))
    XCTAssertTrue(chrome.contains("sideIndicatorAnimation(reduceMotion: reduceMotion)"))
    XCTAssertTrue(chrome.contains("transaction.disablesAnimations = true"))
    XCTAssertTrue(source.contains("pointerInsideOverlay = inside"))
    XCTAssertEqual(chrome.components(separatedBy: ".allowsHitTesting(false)").count - 1, 8)
    XCTAssertEqual(chrome.components(separatedBy: ".accessibilityHidden(true)").count - 1, 4)
    // The folded list's rail (2026-10-19) brought two of them: its leading
    // hairline and its frame reader, neither of which takes a click.
    let rail = try section(
      of: chrome, from: "private func agentRail", to: "private func agentDrawerResizeHandle")
    XCTAssertEqual(rail.components(separatedBy: ".allowsHitTesting(false)").count - 1, 2)
    XCTAssertEqual(rail.components(separatedBy: ".accessibilityHidden(true)").count - 1, 1)
    // The docked agents column's hairline became its resize handle (15d5640d5):
    // that handle owns the chrome's only drag, and nothing else adds one.
    XCTAssertEqual(chrome.components(separatedBy: "DragGesture").count - 1, 1)
    let handle = try section(
      of: chrome, from: "private func agentDrawerResizeHandle", to: ".onEnded")
    XCTAssertTrue(handle.contains("DragGesture"))
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
        of: header, from: "private var closeButton: some View",
        to: "private func recordingControls")
      XCTAssertTrue(close.contains("ModeDot("))
      XCTAssertTrue(close.contains("color: CSColor.terracotta"))
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
    var lifecycleCloses = 0
    state.onCloseIntent = { closes += 1 }
    state.onClose = { lifecycleCloses += 1 }
    state.relayIntent(.close)
    XCTAssertEqual(closes, 1, "The close intent the brand dot relays must reach onCloseIntent")
    XCTAssertEqual(
      lifecycleCloses, 0,
      "The human close is not an automatic hide an open channel may veto")

    let source = try overlaySource()
    let header = try headerSource(source)
    let close = try section(
      of: header, from: "private var closeButton: some View",
      to: "private func recordingControls")
    XCTAssertTrue(
      close.contains("state.relayIntent(.close)"),
      "The brand dot must relay the close intent")
    XCTAssertTrue(
      header.contains(".accessibilityIdentifier(\"overlay-brand-close-dot\")"),
      "The brand dot must stay discoverable as the close control")
    XCTAssertTrue(
      header.contains(".accessibilityLabel(OverlayIntent.close.accessibilityLabel)"))
    XCTAssertTrue(
      close.contains("color: CSColor.terracotta"),
      "The close control keeps the brand color regardless of engine state")
    XCTAssertEqual(header.components(separatedBy: "state.relayIntent(.close)").count - 1, 1)
    XCTAssertEqual(header.components(separatedBy: "overlay-brand-close-dot").count - 1, 1)
    // The dot keeps its pre-b83e95538 place: the hit target grows through the
    // content shape, never through a frame that shifts the dot or the wordmark.
    XCTAssertTrue(close.contains("size: 9"))
    XCTAssertFalse(close.contains("compact ?"), "Close size must not depend on header width")
    XCTAssertFalse(close.contains("state.mode"), "Close must not signal engine state")
    XCTAssertFalse(close.contains("size: closeDotHovered"))
    XCTAssertTrue(close.contains(".scaleEffect(closeDotHovered"))
    let scale = try XCTUnwrap(close.range(of: ".scaleEffect(")?.lowerBound)
    let hitShape = try XCTUnwrap(close.range(of: ".contentShape(")?.lowerBound)
    XCTAssertLessThan(
      scale, hitShape, "Hover growth must not change the button's layout or hit shape")
    XCTAssertTrue(close.contains("closeDotHovered = $0"))
    XCTAssertFalse(close.contains("setPresentationMode"), "Close hover must never morph the window")
    XCTAssertTrue(close.contains(".contentShape(Circle().inset(by: -7.5))"))
    XCTAssertFalse(close.contains(".frame("), "A frame would move the dot")
    // The overlay draws the same lockup as every other window; the dot slot
    // carries the close button.
    XCTAssertTrue(header.contains("Wordmark("))
    let components = try self.source(at: "Codescribe/DesignSystem/Components.swift")
    XCTAssertTrue(components.contains("Text(verbatim: \"codescribe\")"))
    XCTAssertFalse(components.contains("\"cs\""), "no surface reads cs since spec §13.4")
    XCTAssertTrue(header.contains(".allowsHitTesting(false)"))
    // The brand block sits on an inert drag region so the dot answers clicks,
    // not window drags (Founder 19:18: the dot next to codescribe closes).
    XCTAssertTrue(header.contains("overlay-header-inert-drag-region"))
  }

  /// The header keeps one explicit Pin / Unpin where the position button sat;
  /// placement moved behind the header's secondary click.
  func testHeaderPinIsAnExplicitToggleAndPlacementLivesInTheHeaderMenu() throws {
    let header = try headerSource(try overlaySource())
    XCTAssertTrue(header.contains("OverlayPinButton(state: state, palette: palette)"))
    XCTAssertFalse(header.contains("OverlayPlacementMenu"))
    XCTAssertTrue(
      try overlaySource().contains(".contextMenu { OverlayHeaderMenu(state: state) }"),
      "one secondary-click menu serves every header form")

    let menu = try source(at: "Codescribe/Screens/Overlay/OverlayHeaderMenu.swift")
    XCTAssertTrue(menu.contains("state.setKeepVisibleBetweenTakes(!pinned)"))
    XCTAssertTrue(
      menu.contains(
        "symbol: pinned ? OverlayControlSymbols.pinned : OverlayControlSymbols.pin"))
    XCTAssertTrue(menu.contains(".accessibilityAddTraits(.isToggle)"))
    XCTAssertTrue(menu.contains("state.selectPlacementAnchor(anchor)"))
    XCTAssertTrue(menu.contains("state.selectFreeMotion()"))
    let pinButton = try XCTUnwrap(menu.range(of: "struct OverlayPinButton:"))
    XCTAssertFalse(
      menu[pinButton.lowerBound...].contains("Menu {"),
      "the pin button toggles directly instead of opening a placement menu")
  }

  /// The header's content controls are one Dingbats character each, in the
  /// family of the roster's ❖, and every macOS font cascade can paint them.
  func testHeaderGlyphsAreTextMarksThatSystemFontsCover() throws {
    XCTAssertEqual(OverlayHeaderGlyph.agents, "\u{2726}")
    XCTAssertEqual(OverlayHeaderGlyph.transcription, "\u{270D}\u{FE0E}")
    let base = NSFont.systemFont(ofSize: 11) as CTFont
    for glyph in [OverlayHeaderGlyph.agents, OverlayHeaderGlyph.transcription] {
      let scalar = try XCTUnwrap(glyph.unicodeScalars.first)
      let font = CTFontCreateForString(
        base, String(scalar) as CFString, CFRange(location: 0, length: 1))
      var characters = Array(String(scalar).utf16)
      var glyphs = [CGGlyph](repeating: 0, count: characters.count)
      XCTAssertTrue(
        CTFontGetGlyphsForCharacters(font, &characters, &glyphs, characters.count),
        "\(glyph) must not paint as a missing-glyph box")
      let menuImage = OverlayHeaderGlyph.menuImage(glyph)
      XCTAssertTrue(menuImage.isTemplate)
      XCTAssertGreaterThan(menuImage.size.width, 0)
    }
    let header = try headerSource(try overlaySource())
    XCTAssertTrue(header.contains("character: OverlayHeaderGlyph.transcription"))
    let roster = try source(at: "Codescribe/Screens/Overlay/OverlayChannelStatusView.swift")
    XCTAssertTrue(roster.contains("character: OverlayHeaderGlyph.agents"))
    // sidebar.right is the list's own corner button (Maciej, 2026-10-19),
    // never the header's Agents mark.
    let glyph = try XCTUnwrap(roster.range(of: "private var headerGlyph: some View"))
    XCTAssertFalse(roster[glyph.lowerBound...].prefix(700).contains("sidebar.right"))
  }

  /// A secondary click on a passive header point resolves the header menu:
  /// the drag region under it hands the click up to the SwiftUI host.
  func testHeaderSecondaryClickOffersPinAndPlacement() throws {
    let state = OverlayState.previewListening()
    state.setPresentationMode(.expanded)
    try withPanel(state: state) { panel, root in
      let point = NSPoint(x: 150, y: root.bounds.maxY - 22)
      let event = try XCTUnwrap(
        NSEvent.mouseEvent(
          with: .rightMouseDown, location: root.convert(point, to: nil),
          modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
          windowNumber: panel.windowNumber, context: nil, eventNumber: 9_100,
          clickCount: 1, pressure: 1))
      var view = root.hitTest(point)
      var menu: NSMenu?
      while let current = view, menu == nil {
        menu = current.menu(for: event)
        view = current.superview
      }
      let resolved = try XCTUnwrap(menu, "no context menu resolved at the passive header")
      resolved.delegate?.menuNeedsUpdate?(resolved)
      let titles = resolved.items.map(\.title)
      for title in [
        "Agents", "Transcription", "Pin", "Free motion", "Show transcript by default",
        "Auto fold after… 10 s",
      ] {
        XCTAssertTrue(titles.contains(title), "\(title) missing from \(titles)")
      }
    }
  }

  /// The title names the current interval (Lena, release 0.17.2); the chosen
  /// row inside carries a checkmark in the plain submenu (decyzja Macieja,
  /// 2026-10-20), so the rows are not pinned here.
  func testAutoFoldMenuNamesTheCurrentInterval() throws {
    let choices: [UInt32] = [5, 10, 30, 60, 0]
    for seconds in choices {
      let state = OverlayState()
      let engine = OverlayChromePolicyEngine()
      engine.autoFoldSeconds = seconds
      state.engine = engine
      state.attach()
      state.setPresentationMode(.expanded)
      try withPanel(state: state) { panel, root in
        let point = NSPoint(x: 150, y: root.bounds.maxY - 22)
        let event = try XCTUnwrap(
          NSEvent.mouseEvent(
            with: .rightMouseDown, location: root.convert(point, to: nil),
            modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: panel.windowNumber, context: nil, eventNumber: 9_101,
            clickCount: 1, pressure: 1))
        var view = root.hitTest(point)
        var menu: NSMenu?
        while let current = view, menu == nil {
          menu = current.menu(for: event)
          view = current.superview
        }
        let resolved = try XCTUnwrap(menu)
        resolved.delegate?.menuNeedsUpdate?(resolved)
        let fold = try XCTUnwrap(
          resolved.items.first { $0.title.hasPrefix("Auto fold after") })
        let label = seconds == 0 ? "Never" : "\(seconds) s"
        XCTAssertEqual(fold.title, "Auto fold after… \(label)")
        XCTAssertNotNil(fold.submenu)
      }
    }
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

  /// Annex A2: Auto Paste leaves the overlay header. Paste mode itself stays
  /// where it is — Settings › Shortcuts and the tray cycle own `paste_mode` —
  /// and the overlay keeps no path that could write it.
  func testHeaderHasNoAutoPasteControlAndOverlayCannotWritePasteMode() throws {
    try withPanel(state: .previewListening()) { _, root in
      let elements = accessibilityTree(root)
      XCTAssertFalse(elements.isEmpty, "The rendered accessibility hierarchy must be observable")
      XCTAssertFalse(elements.contains { $0.accessibilityIdentifier() == "overlay-auto-paste" })
    }
    let overlay = try overlaySource()
    let header = try headerSource(overlay)
    for gone in ["overlay-auto-paste", "autoPaste", "AutoPaste", "pasteMode"] {
      XCTAssertFalse(overlay.contains(gone), "overlay view still carries \(gone)")
    }
    // The microphone mark belongs to recording only; the header spends its
    // width on the waveform, the recording control and one agent glyph.
    XCTAssertTrue(header.contains("chromeWaveform(barCount:"))
    XCTAssertFalse(header.contains("OverlayRecordingLightView("))
    XCTAssertTrue(header.contains("recordingLight: state.recordingLight"))
    XCTAssertTrue(header.contains("OverlayChannelStatusView("))
    XCTAssertTrue(header.contains("OverlayRecordingControls("))

    let state = try source(at: "Codescribe/Screens/Overlay/OverlayState.swift")
    for writer in ["setAutoPasteEnabled", "setPasteMode", "autoPasteEnabled"] {
      XCTAssertFalse(state.contains(writer), "overlay state still reaches \(writer)")
    }
    XCTAssertFalse(
      try source(at: "Codescribe/Core/AppModel.swift").contains("setAutoPasteControlAvailable"))
  }

  func testLivePreviewRouterShowsTheDirectionOfItsNextState() throws {
    let state = OverlayState.previewListening()
    state.setPresentationMode(.mini)
    let text = state.activeText
    for (mode, symbol, label) in [
      (OverlayPresentationMode.mini, "chevron.left", "Expand to compact widget"),
      (.midi, "chevron.down", "Expand transcript"),
      (.expanded, "arrow.up.right", "Collapse widget"),
    ] {
      state.setPresentationMode(mode)
      let controls = OverlayRecordingControls(
        canFinish: true, presentationMode: state.presentationMode, compact: false,
        palette: .dark, onIntent: { _ in }, onPreviewToggle: state.toggleCollapsed)
      XCTAssertEqual(controls.previewSymbol, symbol)
      XCTAssertEqual(controls.previewAccessibilityLabel, label)
      controls.togglePreview()
      XCTAssertEqual(
        state.presentationMode, mode == .mini ? .midi : mode == .midi ? .expanded : .mini)
      XCTAssertEqual(state.activeText, text)
    }
    XCTAssertEqual(state.presentationMode, .mini)
    let overlay = try overlaySource()
    XCTAssertFalse(overlay.contains("\"eye"))
    XCTAssertTrue(overlay.contains("Image(systemName: previewSymbol)"))
    XCTAssertTrue(overlay.contains(".accessibilityIdentifier(\"overlay-live-preview-toggle\")"))
  }

  func testDrawerCanOpenEmptyBroadcastAndReturnToTranscriptionWithoutChangingCapture() {
    let state = OverlayState.previewListening()
    let broadcast = OverlayConversation(
      id: "0", channel: "0", name: "All", owner: nil, messages: [])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast]))
    let recording = state.recording
    state.setPresentationMode(.midi)
    state.showAgentMonitor()
    let view = OverlayChannelStatusView(
      channels: [], unavailable: false, palette: .dark, animates: false,
      conversations: state.conversations,
      onSelectConversation: { state.selectConversation($0) })
    XCTAssertEqual(view.currentConversations.map(\.id), ["0"])
    view.viewConversation(
      .init(channel: "0", agent: "All", deliveryID: nil, stage: nil, isOpen: false))
    XCTAssertEqual(state.selectedConversation?.id, "0")
    XCTAssertTrue(state.selectedConversation?.messages.isEmpty == true)
    XCTAssertFalse(state.showsAgentMonitor)
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(state.recording, recording)
    state.showTranscription()
    XCTAssertTrue(state.showsMyDictation)
    XCTAssertEqual(state.conversations, [broadcast])
    XCTAssertEqual(state.recording, recording)
  }

  func testRecordingControlMorphsBetweenIdleLiveAndFinalizing() throws {
    let source = try overlaySource()
    XCTAssertTrue(source.contains("if showsRecordingButton { recordingButton }"))
    XCTAssertTrue(
      source.contains("OverlayMicrophoneGlyph(symbol: recordingSymbol, tint: recordingTint)"))
    XCTAssertTrue(source.contains(".accessibilityIdentifier(recordingIdentifier)"))
    XCTAssertTrue(source.contains(".disabled(recordingDisabled)"))
    XCTAssertTrue(source.contains("canFinish: state.recording && !state.transcribing"))
    XCTAssertTrue(source.contains("isFinalizing: state.isFinalizingDictation,"))

    for state in [OverlayState(), OverlayState.previewFormatted()] {
      XCTAssertFalse(state.recording)
      let control = OverlayRecordingControls(
        canFinish: false, presentationMode: state.presentationMode, compact: false,
        palette: .dark, onIntent: { _ in }, onPreviewToggle: {})
      XCTAssertEqual(control.recordingSymbol, "mic.fill")
      XCTAssertEqual(control.recordingIdentifier, "overlay-start-recording")
    }
    let live = OverlayState.previewListening()
    live.handleRecordingPreparing()
    XCTAssertTrue(live.recording)
    let stop = OverlayRecordingControls(
      canFinish: live.recording, presentationMode: .expanded, compact: false,
      palette: .dark, onIntent: { _ in }, onPreviewToggle: {})
    XCTAssertEqual(stop.recordingSymbol, "stop.fill")
    XCTAssertEqual(stop.recordingIdentifier, "overlay-stop-recording")
  }

  /// Monika, 2026-10-22 (bus intercom spec §13.4): "cs" leaves mini; the
  /// brand dot leads and the channel counter takes the word's place. Maciej's
  /// 2026-10-20 rule stays for the rest: mini always carries the agents button
  /// beside the ordinary microphone and the expand arrow, whether or not an
  /// agent records. An AX walk of the test panel does not surface the SwiftUI
  /// header, so the probe reads AppKit truth: one mounted hover region per
  /// header control and the drag region that backs the brand.
  func testMiniCarriesTheChannelCounterInPlaceOfCs() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let state = OverlayState()
    state.setPresentationMode(.mini)
    XCTAssertFalse(state.channelAudioCaptureActive)
    func roster(open: Bool) -> [CsChannelRosterState] {
      [
        .init(
          channel: "4", audience: "bruno", provider: "codex", providerSessionId: "bus-agent",
          open: open, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
          unreadSinceUnixMs: nil)
      ]
    }
    try withPanel(state: state) { panel, root in
      @MainActor func headerControls() -> Int {
        panel.menuCursorRegions.allObjects.filter { view in
          guard let region = view as? OverlayMenuCursorRegionView, region.onHover != nil,
            region.window === panel, !region.isHiddenOrHasHiddenAncestor
          else { return false }
          let rect = root.convert(region.bounds, from: region)
          let top = root.isFlipped ? rect.minY : root.bounds.height - rect.maxY
          return rect.width <= 34 && rect.height <= 30 && top < 46
        }.count
      }
      @MainActor func brandWidth() throws -> CGFloat {
        let region = try XCTUnwrap(
          self.dragRegionViews(in: root).first {
            $0.accessibilityIdentifier() == "overlay-mini-brand-drag-region"
              && !$0.isHiddenOrHasHiddenAncestor
          }, "mini: no brand drag region")
        return region.bounds.width
      }
      @MainActor func settleHeader() {
        self.settle(root)
        RunLoop.main.run(until: Date().addingTimeInterval(0.35))
        root.layoutSubtreeIfNeeded()
      }
      let restingControls = headerControls()
      XCTAssertGreaterThanOrEqual(
        restingControls, 3, "mini with no agent: agents, microphone and expand arrow")
      XCTAssertLessThan(try brandWidth(), 12, "mini draws the brand dot alone, no cs")
      XCTAssertEqual(
        panel.frame.width, DictationOverlayWindow.collapsedSize.width, accuracy: 0.5)

      // A bound agent puts the counter beside the dot; mini keeps 200 pt.
      state.applyChannelRoster(roster(open: false))
      state.setPresentationMode(.mini)
      settleHeader()
      XCTAssertEqual(headerControls(), restingControls + 1, "the counter joins mini")
      XCTAssertLessThan(try brandWidth(), 12, "the counter, not cs, follows the dot")
      XCTAssertEqual(
        panel.frame.width, DictationOverlayWindow.collapsedSize.width, accuracy: 0.5,
        "mini stays 200 pt with its counter")
      let trailing = panel.frame.maxX

      // The agent records: the counter becomes the open digit with its badge,
      // and mini hugs its controls at 164 pt (09:36, 2026-10-19).
      state.applyChannelRoster(roster(open: true))
      state.setPresentationMode(.mini)
      settleHeader()
      XCTAssertEqual(headerControls(), restingControls + 1, "capture adds no control to mini")
      XCTAssertEqual(
        panel.frame.width, DictationOverlayWindow.agentMiniSize.width, accuracy: 0.5)
      XCTAssertEqual(panel.frame.maxX, trailing, accuracy: 0.5, "the controls stay put")

      state.applyChannelRoster([])
      settleHeader()
      XCTAssertEqual(headerControls(), restingControls, "the agents button stays")
      XCTAssertEqual(
        panel.frame.width, DictationOverlayWindow.collapsedSize.width, accuracy: 0.5)
      XCTAssertEqual(panel.frame.maxX, trailing, accuracy: 0.5, "the controls stay put")
    }
  }

  /// The All pair is an outline megaphone and a speaker of one optical size.
  /// Agent rows keep the microphone. Recording uses the speaker's violet.
  func testBroadcastRowDrawsAMegaphoneAndAgentRowsKeepTheMicrophone() throws {
    func controls(open: Bool, broadcast: Bool) -> OverlayAgentAudioControls {
      OverlayAgentAudioControls(
        open: open, muted: false, microphoneEnabled: true, playbackEnabled: true,
        palette: .dark, onMicrophone: {}, onPlayback: {}, broadcast: broadcast)
    }
    XCTAssertEqual(controls(open: false, broadcast: true).microphoneSymbol, "megaphone")
    XCTAssertEqual(controls(open: true, broadcast: true).microphoneSymbol, "megaphone")
    XCTAssertEqual(controls(open: false, broadcast: false).microphoneSymbol, "mic")
    XCTAssertEqual(controls(open: true, broadcast: false).microphoneSymbol, "mic.fill")
    XCTAssertEqual(OverlayAgentAudioControls.broadcastScale, 1.33)
    XCTAssertEqual(OverlayAgentAudioControls.controlFrame, 16)
    let roster = try source(at: "Codescribe/Screens/Overlay/OverlayChannelStatusView.swift")
    XCTAssertTrue(roster.contains(".overlayMirrored(broadcast)"), "one mirror drawing, shared")
    XCTAssertTrue(
      roster.contains("open ? recordingAccent"),
      "an open microphone takes the active-speaker accent")
    XCTAssertFalse(roster.contains("open ? Color.red"), "recording is no longer a separate red")
    let all = try section(
      of: roster, from: "private func scaledMonitorBody(", to: "var currentConversations")
    XCTAssertTrue(all.contains(".lineLimit(3)"), "three preview lines belong to All")
    XCTAssertTrue(all.contains("overlay-all-preview"))
    XCTAssertTrue(
      roster.contains("includePreview: false"), "the title button does not repeat the preview")
    let agentLabel = try source(at: "Codescribe/Screens/Overlay/OverlayAgentRosterLabel.swift")
    XCTAssertTrue(agentLabel.contains(".lineLimit(2)"), "agent rows keep a two-line preview")
    let agentBudget = try section(
      of: roster, from: "private func rosterRowLabel(", to: "func rowStateText(")
    XCTAssertFalse(agentBudget.contains(".lineLimit(3)"), "three lines do not enter an agent row")
  }

  /// Maciej, 2026-10-20: the megaphone faces left, and only the megaphone.
  func testAgentMegaphoneIsMirroredAndTheWordmarkIsNot() throws {
    let roster = try source(at: "Codescribe/Screens/Overlay/OverlayChannelStatusView.swift")
    XCTAssertTrue(
      roster.contains(
        "symbol: \"megaphone.fill\", tint: OverlayRecordingLight.agent.color, mirrored: true"))
    XCTAssertTrue(roster.contains(".scaleEffect(x: mirrored ? -1 : 1, y: 1)"))
    let header = try headerSource(try overlaySource())
    // Spec §13.4: mini draws the dot without the word; the counter follows.
    XCTAssertTrue(header.contains("showsWord: !mini"))
    XCTAssertTrue(header.contains("if mini, let map = channelMap {"))
    XCTAssertFalse(header.contains("short: mini"))
    XCTAssertFalse(header.contains("miniAgentMicrophone"))
  }

  private func withPanel(
    state: OverlayState, width: CGFloat = 470,
    _ check: (FloatingOverlayPanel, NSView) throws -> Void
  ) throws {
    let suite = "OverlayChromeFounderCutTests.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suite)!
    defer { defaults.removePersistentDomain(forName: suite) }
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayChromeFounderCutTests.scale"),
        sizeDefaults: defaults
      ) as? FloatingOverlayPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.setContentSize(NSSize(width: width, height: DictationOverlayWindow.minSize.height))
    if let visible = NSScreen.main?.visibleFrame {
      // Anchor by the top-right corner, 40 pt inside the visible frame, at
      // whatever height the panel actually took.
      panel.setFrameOrigin(
        NSPoint(x: visible.maxX - width - 40, y: visible.maxY - panel.frame.height - 40))
    }
    BackgroundTestWindow.present(panel)
    let root = try XCTUnwrap(panel.contentView)
    settle(root)
    try check(panel, root)
  }

  private func settle(_ root: NSView) {
    root.layoutSubtreeIfNeeded()
    let deadline = Date().addingTimeInterval(1.5)
    while (root.window as? FloatingOverlayPanel)?.isFrameTransitioning == true && Date() < deadline
    {
      RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    }
    XCTAssertNotEqual((root.window as? FloatingOverlayPanel)?.isFrameTransitioning, true)
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

  /// `justifiedHeader(density:)` — the single header tree behind mini, midi and
  /// expanded, so one guard covers every density.
  private func headerSource(_ source: String) throws -> String {
    try section(
      of: source, from: "private var closeButton: some View",
      to: "private func chromeWaveform")
  }

  private func containsGuardedIntentRail(_ source: String) -> Bool {
    source.range(
      of: #"if actions\.phase == \.open \{\s*intentRail\s*\}"#,
      options: .regularExpression) != nil
  }

  private func section(of source: String, from start: String, to end: String) throws -> String {
    let lower = try XCTUnwrap(source.range(of: start), start)
    let upper = try XCTUnwrap(source.range(of: end, range: lower.upperBound..<source.endIndex), end)
    return String(source[lower.lowerBound..<upper.lowerBound])
  }
}

@MainActor
final class OverlayChromePolicyEngine: DictationEngine {
  var expansionWrites: [Bool] = []
  var pinWrites: [Bool] = []
  var pinEnabled = false
  func overlayKeepVisibleBetweenTakes() -> Bool { pinEnabled }
  func setOverlayKeepVisibleBetweenTakes(_ enabled: Bool) -> Bool {
    pinWrites.append(enabled)
    pinEnabled = enabled
    return true
  }
  var autoFoldSeconds: UInt32 = 10
  var autoFoldWrites: [UInt32] = []
  func overlayAutoFoldSeconds() -> UInt32 { autoFoldSeconds }
  func setOverlayAutoFoldSeconds(_ seconds: UInt32) -> Bool {
    autoFoldWrites.append(seconds)
    guard [UInt32(0), 5, 10, 30, 60].contains(seconds) else { return false }
    autoFoldSeconds = seconds
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
  func setListener(_ listener: CsTranscriptionListener) {}
  func startsInAssistiveMode() -> Bool { false }
  func startRecording(assistive: Bool, language: CsLanguage?) async throws {}
  func stopRecording() async throws -> String { "" }
  func isRecording() async -> Bool { false }
  func initModel() async throws {}
  func isModelLoaded() -> Bool { true }
  func currentOverlayPolicy() -> OverlayPolicySnapshot? {
    OverlayPolicySnapshot(autoFormatLevel: .correction)
  }
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
