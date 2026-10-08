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
  func testEvidenceChipCountsAllItemsAndShowsLatestTwelveInPCMOrder() throws {
    let evidence = (0..<13).map { index in
      CsUnanchoredEvidence(
        sampleStart: UInt64(index * 1600), sampleEnd: UInt64((index + 1) * 1600),
        text: "word \(index)", reason: "late_apple_word_not_current")
    }
    let chip = try XCTUnwrap(OverlayEvidencePresentation.chip(evidence: evidence))
    XCTAssertEqual(chip.count, 13)
    XCTAssertEqual(chip.line, (1..<13).map { "word \($0)" }.joined(separator: " · "))
    XCTAssertNil(OverlayEvidencePresentation.chip(evidence: []))
  }

  func testEvidenceChipPreservesRepeatedWordsAndVerbatimLabels() throws {
    let evidence = (0..<5).map { index in
      CsUnanchoredEvidence(
        sampleStart: UInt64(index * 1600), sampleEnd: UInt64((index + 1) * 1600),
        text: "Iwo", reason: "late_apple_word_not_current")
    }
    let chip = try XCTUnwrap(OverlayEvidencePresentation.chip(evidence: evidence))
    XCTAssertEqual(chip.count, 5)
    XCTAssertEqual(chip.line, "Iwo · Iwo · Iwo · Iwo · Iwo")
    let verbatim = CsUnanchoredEvidence(
      sampleStart: 0, sampleEnd: 1600, text: " żarty.  Krawędziach ", reason: "unanchored")
    XCTAssertEqual(
      OverlayEvidencePresentation.chip(evidence: [verbatim])?.line, verbatim.text)
  }

  func testEvidenceChipExpandsOnlyAfterSelectionWhileToolsAreClosed() {
    XCTAssertFalse(OverlayEvidencePresentation.isExpanded(selected: false, actionsOpen: false))
    XCTAssertTrue(OverlayEvidencePresentation.isExpanded(selected: true, actionsOpen: false))
    XCTAssertFalse(OverlayEvidencePresentation.isExpanded(selected: true, actionsOpen: true))
  }

  func testEvidenceChipReplacesBodyRowsInsideTheSharedBottomGlassContainer() throws {
    let evidence = try source(at: "Codescribe/Screens/Overlay/OverlayEvidenceList.swift")
    XCTAssertFalse(evidence.contains("ForEach"))
    XCTAssertFalse(evidence.contains("toolsHandleClearance"))
    XCTAssertFalse(evidence.contains("VStack"))
    let overlay = try overlaySource()
    let body = try section(
      of: overlay, from: "private var bodySection", to: "private var transcriptScroll")
    XCTAssertFalse(body.contains("OverlayEvidence"))
    let container = try section(
      of: overlay, from: "private func bottomChromeContainer", to: "private func canvasStack")
    XCTAssertTrue(container.contains("GlassEffectContainer(spacing: 0)"))
    XCTAssertTrue(container.contains("{ intentRail }"))
    XCTAssertFalse(
      container.contains("canvasStack(intentRail)"), "glass must not extract the whole canvas")
    let bottom = try section(
      of: overlay, from: "private func canvasStack",
      to: "} else if let label = OverlayActionsPresentation.finishingLabel(")
    XCTAssertTrue(bottom.contains("OverlayEvidenceChip("))
    XCTAssertTrue(bottom.contains("HStack(spacing: 6)"))
    XCTAssertTrue(bottom.contains("actionsOpen: actions.phase == .open"))
    XCTAssertTrue(bottom.contains("glassNamespace: bottomChromeNamespace"))
    XCTAssertTrue(bottom.contains(".layoutPriority(-1)"))
    XCTAssertTrue(bottom.contains(".padding(.bottom, OverlayResizeChrome.actionsBottomInset)"))
    XCTAssertTrue(bottom.contains(".padding(.horizontal, OverlayResizeChrome.actionsBottomInset)"))
    XCTAssertTrue(overlay.contains("@Namespace private var bottomChromeNamespace"))
    XCTAssertTrue(evidence.contains(".glassEffect(.regular.interactive(), in: Capsule())"))
    XCTAssertTrue(evidence.contains(".glassEffectID(\"overlay-evidence\", in: glassNamespace)"))
    XCTAssertTrue(evidence.contains("if reduceTransparency"))
    XCTAssertTrue(evidence.contains(".background(palette.desktopBackground.color, in: Capsule())"))
    XCTAssertTrue(evidence.contains(".background(.regularMaterial, in: Capsule())"))
  }

  func testEvidenceChipUsesExplicitButtonActivationAndRespectsReducedMotion() throws {
    let evidence = try source(at: "Codescribe/Screens/Overlay/OverlayEvidenceList.swift")
    XCTAssertTrue(evidence.contains(".lineLimit(1)"))
    XCTAssertTrue(evidence.contains(".truncationMode(.head)"))
    XCTAssertTrue(evidence.contains(".frame(height: OverlayResizeChrome.actionsHeight)"))
    XCTAssertFalse(evidence.contains(".onHover"))
    XCTAssertTrue(evidence.contains("selected.toggle()"))
    XCTAssertTrue(evidence.contains("OverlayEvidencePresentation.isExpanded("))
    XCTAssertTrue(evidence.contains(".accessibilityElement(children: .ignore)"))
    XCTAssertTrue(evidence.contains("Also heard, not committed:"))
    XCTAssertTrue(evidence.contains("OverlayMiniTooltip("))
    for forbidden in ["onTapGesture", ".tint(", ".allowsHitTesting(false)"] {
      XCTAssertFalse(evidence.contains(forbidden), forbidden)
    }
    XCTAssertTrue(
      evidence.contains(".glassEffectTransition(reduceMotion ? .identity : .matchedGeometry)"))
    XCTAssertTrue(evidence.contains("transaction.animation = nil"))
    XCTAssertTrue(evidence.contains("transaction.disablesAnimations = true"))
  }

  func testActionsGlassMorphsInTheEvidenceNamespaceWithoutPaintingTheBottomBar() throws {
    let overlay = try overlaySource()
    let material = try section(
      of: railSource(), from: "struct OverlayActionsSurface", to: "/// Symbols shared")
    XCTAssertTrue(material.contains("else if #available(macOS 26.0, *)"))
    XCTAssertTrue(material.contains(".glassEffect(.regular.interactive(), in: Capsule())"))
    XCTAssertTrue(material.contains(".glassEffectID(\"overlay-actions\", in: glassNamespace)"))
    XCTAssertTrue(
      material.contains(".glassEffectTransition(reduceMotion ? .identity : .matchedGeometry)"))
    let glass = try section(
      of: material, from: "else if #available(macOS 26.0, *)", to: "} else {")
    XCTAssertFalse(glass.contains(".background"))
    XCTAssertFalse(glass.contains(".overlay"))
    XCTAssertFalse(glass.contains(".tint"))
    let bottom = try section(
      of: overlay, from: "private func canvasStack",
      to: "} else if let label = OverlayActionsPresentation.finishingLabel(")
    XCTAssertEqual(
      bottom.components(separatedBy: "glassNamespace: bottomChromeNamespace").count - 1, 2)
    let actionSurface = try XCTUnwrap(bottom.range(of: "OverlayActionsSurface(palette:"))
    let clearMargin = try XCTUnwrap(bottom.range(of: ".padding(.bottom, OverlayResizeChrome"))
    XCTAssertLessThan(actionSurface.lowerBound, clearMargin.lowerBound)
    XCTAssertFalse(
      bottom.contains(".glassEffect("), "Only the two capsules may claim glass hit regions")
    XCTAssertTrue(
      bottom.contains(
        ".animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: actions.phase)"))
  }

  func testCoverageWarningStacksWithActionsWithoutHidingOperationErrors() throws {
    let source = try overlaySource()
    XCTAssertTrue(source.contains("@AppStorage(DictationOverlayGate.labModeDefaultsKey)"))
    XCTAssertTrue(source.contains("DeveloperSurface.isPowerModeEnabled(labMode: labMode)"))
    let header = try headerSource(source)
    XCTAssertTrue(
      header.contains("showsDiagnostics && state.compactProjection?.degraded == true"))
    XCTAssertTrue(header.contains("if let error = state.expansionPreferenceError"))
    let refusal = try section(of: source, from: "case .coverageRefused:", to: "case .noSpeech:")
    XCTAssertFalse(refusal.contains("coverageRefusedBody"))
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
    XCTAssertTrue(panel.styleMask.contains(.resizable))
  }

  func testRouterRestoresCanvasAndDrawerFromMidiWithoutChangingCapture() throws {
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
      XCTAssertEqual(panel.frame.size, fullSize)
      state.hideAgentSidebar()
      XCTAssertTrue(findTranscript(in: root) === canvas)
    }
  }

  func testInterruptedMorphKeepsOriginalCanvasSizeAndTopEdge() throws {
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
      orderPanelFront: { $0.orderFrontRegardless() }, orderPanelOut: { $0.orderOut(nil) })
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
        panel.cursor(at: NSPoint(x: 1, y: 23)), OverlayResizeHit.cursor(for: .left),
        "MINI retains the same resize edge as the other forms")
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
  func testActionsAreInsertedOnlyAfterActivationAndHaveNoPinState() throws {
    let source = try overlaySource()
    let tools = try section(
      of: source, from: "HStack(spacing: 2)", to: "private var header: some View")
    XCTAssertTrue(containsGuardedIntentRail(tools))
    XCTAssertTrue(tools.contains("actions.toggle()"))
    XCTAssertTrue(tools.contains(".onHover { actions.pointerChanged($0) }"))
    XCTAssertTrue(tools.contains(".focusable()"))
    XCTAssertTrue(tools.contains("actions.phase == .open ? \"Expanded\" : \"Collapsed\""))
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
    let tools = try section(
      of: source, from: "HStack(spacing: 2)", to: "private var header: some View")
    let cap = try section(of: tools, from: "Button {", to: "if actions.phase == .open {")
    XCTAssertTrue(cap.contains("actions.toggle()"))
    XCTAssertTrue(cap.contains(".accessibilityIdentifier(\"overlay-tools-handle\")"))
    XCTAssertTrue(
      cap.contains(".accessibilityValue(actions.phase == .open ? \"Expanded\" : \"Collapsed\")"))
    XCTAssertTrue(cap.contains("if state.hasRecoverableSupersededWork && actions.phase != .open {"))
    XCTAssertTrue(cap.contains("overlay-retained-work-badge"))
    XCTAssertTrue(containsGuardedIntentRail(tools))
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
    XCTAssertTrue(rail.contains("OverlayDockLayout(projectedIntents: intents).visibleIntents"))
    XCTAssertTrue(rail.contains("id: \"overlay-intent-\\(intent.rawValue)\""))
    actions.toggle()
    XCTAssertEqual(actions.phase, .idle, "The same cap removes the guarded rail again")
    XCTAssertNil(actions.hideDeadline)
    // `accessibilityVoiceOverEnabled` is read-only in the environment, so the
    // guard pins its absence; no XCTest environment write or AX walk is needed.
    XCTAssertFalse(source.contains("accessibilityVoiceOverEnabled"))
    XCTAssertTrue(source.contains("@State private var actions = OverlayActionsPresentation()"))
    XCTAssertEqual(OverlayActionsPresentation().phase, .idle)
  }

  func testActionsHandleUsesPhaseForSymbolLabelAndTooltip() throws {
    let source = try overlaySource()
    let normalized = source.components(separatedBy: .whitespacesAndNewlines)
      .filter { !$0.isEmpty }.joined(separator: " ")
    let handle = try section(
      of: normalized, from: "Button { actions.toggle()",
      to: "if actions.phase == .open {")
    XCTAssertTrue(handle.contains("Image(systemName: actions.controlSymbol)"))
    XCTAssertTrue(
      handle.contains(".contentTransition(reduceMotion ? .identity : .symbolEffect(.replace))"))
    XCTAssertTrue(handle.contains(".accessibilityLabel(actions.controlTitle)"))
    XCTAssertTrue(handle.contains("OverlayMiniTooltip(title: actions.controlTitle"))
    XCTAssertTrue(handle.contains(".accessibilityIdentifier(\"overlay-tools-handle\")"))
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
      OverlayDockLayout(projectedIntents: all).visibleIntents,
      [.insertPaste, .copy, .retranscribe, .format, .sendToAgent])
  }

  func testTakeStartAndCollapseRemoveMountedTools() throws {
    let source = try overlaySource()
    let canvas = try section(
      of: source, from: "private func canvasStack", to: "private var header: some View")
    XCTAssertTrue(
      canvas.range(
        of:
          #"if !state\.isCollapsed && state\.showsMyDictation && !state\.showsAgentMonitor \{\s*VStack\(spacing: CSSpace\.sm\) \{\s*bottomChromeContainer\(\s*HStack\(spacing: 6\)"#,
        options: .regularExpression) != nil)
    XCTAssertTrue(
      canvas.range(
        of: #"footerMessageRow\s*\.frame\(height: 18\)"#,
        options: .regularExpression) != nil,
      "The floating tools retain one fixed-height message row")
    XCTAssertFalse(canvas.contains("transcriptStatus"))
    XCTAssertTrue(containsGuardedIntentRail(canvas))
    XCTAssertTrue(canvas.contains("actions.toggle()"))
    XCTAssertTrue(
      canvas.contains(".onChange(of: state.captureGeneration) { _, _ in actions.reset() }"))
    let fold = try section(
      of: canvas, from: ".onChange(of: state.isCollapsed)",
      to: ".onChange(of: state.captureGeneration)")
    XCTAssertTrue(fold.contains("if collapsed { actions.reset() }"))
    XCTAssertFalse(fold.contains("actions.toggle()"), "Expanding must not reopen tools")

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
    let bottom = try section(
      of: overlaySource(), from: "private func canvasStack",
      to: "} else if let label = OverlayActionsPresentation.finishingLabel(")
    let capsule = try section(
      of: bottom, from: "HStack(spacing: 2)", to: "OverlayActionsSurface(palette:")
    XCTAssertTrue(capsule.contains(".padding(.horizontal, actions.phase == .open ? 10 : 0)"))
    XCTAssertTrue(capsule.contains(".padding(.horizontal, actions.phase == .open ? 0 : 10)"))
    XCTAssertTrue(capsule.contains("minWidth: actions.phase == .open"))
    XCTAssertTrue(
      capsule.contains("? nil : OverlayResizeChrome.actionsWidth(narrow: true)"))
    XCTAssertFalse(capsule.contains(".padding(.trailing"))
    XCTAssertFalse(capsule.contains(".padding(.leading"))
    XCTAssertTrue(capsule.contains(".fixedSize(horizontal: false, vertical: true)"))
    let groupFrame = try XCTUnwrap(
      bottom.range(of: ".frame(maxWidth: .infinity, alignment: .center)"))
    let capsuleSurface = try XCTUnwrap(bottom.range(of: "OverlayActionsSurface(palette:"))
    let clearMargin = try XCTUnwrap(bottom.range(of: ".padding(.horizontal, OverlayResizeChrome"))
    XCTAssertLessThan(capsuleSurface.lowerBound, groupFrame.lowerBound)
    XCTAssertLessThan(groupFrame.lowerBound, clearMargin.lowerBound)
    XCTAssertFalse(bottom.contains("Spacer("))
  }

  func testNoticesAndCoverageStayOutsideTheActionCapsule() throws {
    let source = try overlaySource()
    let capsule = try section(
      of: source, from: "HStack(spacing: 2)",
      to: ".padding(.vertical, actions.phase == .open ? 2 : 0)")
    XCTAssertTrue(containsGuardedIntentRail(capsule))
    XCTAssertFalse(capsule.contains("Text("))
    XCTAssertFalse(capsule.contains("state.toast"))
    XCTAssertTrue(source.contains("overlay-footer-notice"))
    XCTAssertTrue(source.contains("OverlayCoverageStatus("))
    XCTAssertTrue(
      source.contains("diagnosticDetail: showsDiagnostics ? state.coverageRefusalDetail : nil"))
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
    let tools = try section(
      of: source, from: "HStack(spacing: 2)", to: "private var header: some View")
    XCTAssertTrue(tools.contains(".animation(reduceMotion ? nil :"))
    XCTAssertTrue(tools.contains("transaction.animation = nil"))
    XCTAssertTrue(tools.contains("transaction.disablesAnimations = true"))
    let material = try section(
      of: railSource(), from: "struct OverlayActionsSurface", to: "/// Symbols shared")
    XCTAssertTrue(material.contains("if reduceTransparency"))
    XCTAssertTrue(material.contains("Capsule().fill(palette.desktopBackground.color)"))
    XCTAssertTrue(material.contains(".background { Capsule().fill(.regularMaterial) }"))
  }

  func testFinishingNoticeSurvivesFoldWithoutChangingBarHeight() throws {
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
    XCTAssertTrue(
      chrome.contains("? nil : OverlayResizeChrome.actionsWidth(narrow: true)")
    )
    XCTAssertTrue(chrome.contains("height: OverlayResizeChrome.actionsHeight"))
    XCTAssertTrue(chrome.contains(".padding(.bottom, OverlayResizeChrome.actionsBottomInset)"))
    XCTAssertTrue(chrome.contains("width: OverlayResizeChrome.gripSize.width"))
    XCTAssertTrue(chrome.contains("height: OverlayResizeChrome.gripSize.height"))
    XCTAssertTrue(chrome.contains(".padding(.bottom, OverlayResizeChrome.gripBottomInset)"))
    XCTAssertTrue(chrome.contains("sideIndicatorOpacity(pointerInside: pointerInsideOverlay)"))
    XCTAssertTrue(chrome.contains("sideIndicatorAnimation(reduceMotion: reduceMotion)"))
    XCTAssertTrue(chrome.contains("transaction.disablesAnimations = true"))
    XCTAssertTrue(source.contains("pointerInsideOverlay = inside"))
    XCTAssertEqual(chrome.components(separatedBy: ".allowsHitTesting(false)").count - 1, 5)
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
    XCTAssertTrue(header.contains("Text(verbatim: \"codescribe\")"))
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
      (OverlayPresentationMode.mini, "arrow.right", "Expand to compact widget"),
      (.midi, "chevron.down", "Expand transcript"),
      (.expanded, "arrow.left", "Collapse widget"),
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
    XCTAssertTrue(source.contains("|| (!state.recording && state.showsSessionTimer)"))

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
    if let visible = NSScreen.main?.visibleFrame {
      panel.setFrameOrigin(NSPoint(x: visible.maxX - width - 40, y: visible.maxY - 320))
    }
    panel.orderFrontRegardless()
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

  /// `justifiedHeader(compact:)` — the single builder behind fullHeader and
  /// narrowHeader, so one guard covers both widths.
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
