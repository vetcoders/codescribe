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

  func testEvidenceChipExpandsForHoverOrFocusOnlyWhileToolsAreClosed() {
    for hovered in [false, true] {
      for focused in [false, true] {
        XCTAssertEqual(
          OverlayEvidencePresentation.isExpanded(
            hovered: hovered, focused: focused, actionsOpen: false), hovered || focused)
        XCTAssertFalse(
          OverlayEvidencePresentation.isExpanded(
            hovered: hovered, focused: focused, actionsOpen: true))
      }
    }
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
      of: overlay, from: "private func sharedChromeContainer", to: "private func canvasStack")
    XCTAssertTrue(container.contains("GlassEffectContainer(spacing: 0)"))
    XCTAssertTrue(container.contains("canvasStack(intentRail)"))
    let bottom = try section(
      of: overlay, from: ".overlay(alignment: .bottom)",
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

  func testEvidenceChipIsOnePassiveFocusableLineWithAnInstantReducedMotionSwap() throws {
    let evidence = try source(at: "Codescribe/Screens/Overlay/OverlayEvidenceList.swift")
    XCTAssertTrue(evidence.contains(".lineLimit(1)"))
    XCTAssertTrue(evidence.contains(".truncationMode(.head)"))
    XCTAssertTrue(evidence.contains(".frame(height: OverlayResizeChrome.actionsHeight)"))
    XCTAssertTrue(evidence.contains(".onHover { hovered = $0 }"))
    XCTAssertTrue(evidence.contains(".focusable()"))
    XCTAssertTrue(evidence.contains(".focused($focused)"))
    XCTAssertTrue(evidence.contains("OverlayEvidencePresentation.isExpanded("))
    XCTAssertTrue(evidence.contains(".accessibilityElement(children: .ignore)"))
    XCTAssertTrue(evidence.contains("Also heard, not committed:"))
    XCTAssertTrue(evidence.contains(".help(\"Also heard · not committed:"))
    for forbidden in [
      "Button", "onTapGesture", "accessibilityAction", ".tint(", ".allowsHitTesting(false)",
    ] {
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
      of: overlay, from: ".overlay(alignment: .bottom)",
      to: "} else if let label = OverlayActionsPresentation.finishingLabel(")
    XCTAssertEqual(
      bottom.components(separatedBy: "glassNamespace: bottomChromeNamespace").count - 1, 2)
    let actionSurface = try XCTUnwrap(bottom.range(of: ".modifier(OverlayActionsSurface("))
    let clearMargin = try XCTUnwrap(bottom.range(of: ".padding(.bottom, OverlayResizeChrome"))
    XCTAssertLessThan(actionSurface.lowerBound, clearMargin.lowerBound)
    XCTAssertFalse(
      bottom.contains(".glassEffect("), "Only the two capsules may claim glass hit regions")
    XCTAssertTrue(
      bottom.contains(
        ".animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: actions.phase)"))
  }

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
    XCTAssertTrue(tools.contains("if actions.phase == .open {\n              intentRail"))
    XCTAssertTrue(tools.contains("OverlayActionsPresentation.pillLabel("))
    XCTAssertTrue(tools.contains("phase: actions.phase, notice: state.toast"))
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
    let source = try overlaySource()
    let tools = try section(of: source, from: "HStack(spacing: 2)", to: "/// 1px separator")
    let cap = try section(of: tools, from: "Button {", to: "if actions.phase == .open {")
    XCTAssertTrue(cap.contains("actions.toggle()"))
    XCTAssertTrue(cap.contains(".accessibilityIdentifier(\"overlay-tools-handle\")"))
    XCTAssertTrue(
      cap.contains(".accessibilityValue(actions.phase == .open ? \"Open\" : \"Collapsed\")"))
    XCTAssertTrue(cap.contains("if state.hasRecoverableSupersededWork && actions.phase != .open {"))
    XCTAssertTrue(cap.contains("overlay-retained-work-badge"))
    XCTAssertTrue(tools.contains("if actions.phase == .open {\n              intentRail"))
    XCTAssertTrue(source.contains("intents: OverlayIntentRail.projectedIntents(for: state)"))
    let rail = try section(
      of: railSource(), from: "var body: some View", to: "private var retranscribeMenu")
    XCTAssertTrue(rail.contains(".accessibilityIdentifier(\"overlay-intent-dock\")"))
    XCTAssertTrue(
      rail.contains(
        "if intents.contains(.recoverSuperseded) || intents.contains(.discardSuperseded) {\n"
          + "        previousTakeMenu"))

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
    XCTAssertTrue(rail.contains("ForEach(intents, id: \\.self) { intent in"))
    XCTAssertTrue(rail.contains("identifier: \"overlay-intent-\\(intent.rawValue)\""))
    actions.toggle()
    XCTAssertEqual(actions.phase, .idle, "The same cap removes the guarded rail again")
    XCTAssertNil(actions.hideDeadline)
    // `accessibilityVoiceOverEnabled` is read-only in the environment, so the
    // guard pins its absence; no XCTest environment write or AX walk is needed.
    XCTAssertFalse(source.contains("accessibilityVoiceOverEnabled"))
    XCTAssertTrue(source.contains("@State private var actions = OverlayActionsPresentation()"))
    XCTAssertEqual(OverlayActionsPresentation().phase, .idle)
  }

  func testAaMenuOffersCorrectionSmartAndMaxWithSettingsPrimaryAction() throws {
    let source = try railSource()
    let menu = try section(of: source, from: "private var formatMenu", to: "private var formatHelp")
    let choices = try section(of: menu, from: "Menu {", to: "} label: {")
    XCTAssertTrue(
      choices.contains("Text(\"Settings: \\(formatLevel.visibleName)\")\n        .disabled(true)"))
    XCTAssertEqual(choices.components(separatedBy: "Button(").count - 1, 3)
    XCTAssertTrue(choices.contains("Button(\"Correction\") { formatOnce(.correction) }"))
    XCTAssertTrue(choices.contains("Button(\"Smart\") { formatOnce(.smart) }"))
    XCTAssertTrue(choices.contains("Button(\"Max\") { formatOnce(.max) }"))
    let correction = try XCTUnwrap(choices.range(of: "Button(\"Correction\")"))
    let smart = try XCTUnwrap(choices.range(of: "Button(\"Smart\")"))
    let max = try XCTUnwrap(choices.range(of: "Button(\"Max\")"))
    XCTAssertLessThan(correction.lowerBound, smart.lowerBound)
    XCTAssertLessThan(smart.lowerBound, max.lowerBound)
    for forbidden in ["\"Off\"", ".off", "ForEach", "Picker", "checkmark", "Binding"] {
      XCTAssertFalse(choices.contains(forbidden), forbidden)
    }
    XCTAssertTrue(menu.contains("} primaryAction: {\n      dispatch(.format)"))
    XCTAssertTrue(menu.contains(".menuIndicator(.visible)"))
    XCTAssertTrue(menu.contains(".help(formatHelp)"))
    XCTAssertTrue(menu.contains(".accessibilityHint(formatHelp)"))
    XCTAssertTrue(menu.contains(".accessibilityIdentifier(\"overlay-intent-format\")"))
    XCTAssertFalse(source.contains("overlay-format-level-picker"))
    XCTAssertFalse(source.contains("onFormatLevel"))
    let wiring = try overlaySource()
    XCTAssertTrue(wiring.contains("onFormatOnce: { state.formatTranscript(at: $0) }"))
    XCTAssertTrue(wiring.contains("formatLevel: state.autoFormatLevel"))
    XCTAssertTrue(wiring.contains("onIntent: state.relayIntent"))
    for level in FormattingPolicyOption.allCases {
      let rail = OverlayIntentRail(
        phase: "formatted", intents: [.format], palette: .dark, formatLevel: level,
        onIntent: { _ in })
      XCTAssertEqual(
        rail.caption(for: "format"),
        "Format (Settings: \(level.visibleName)) · menu: Correction, Smart or Max once")
    }
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

  func testOpenRowKeepsSevenToolsAndRecordingKeepsItsThreeTools() throws {
    let source = try railSource()
    let row = try section(of: source, from: "var body: some View", to: ".buttonStyle(.plain)")
    XCTAssertTrue(row.contains("if historyAvailable {\n        historyMenu"))
    XCTAssertTrue(
      row.contains(
        "if intents.contains(.recoverSuperseded) || intents.contains(.discardSuperseded) {\n"
          + "        previousTakeMenu"))
    XCTAssertTrue(row.contains("ForEach(intents, id: \\.self) { intent in"))
    let retranscribe = try section(
      of: row, from: "if intent == .retranscribe {", to: "} else if intent == .format {")
    XCTAssertTrue(retranscribe.contains("retranscribeMenu"))
    let format = try section(
      of: row, from: "} else if intent == .format {", to: "} else if intent != .close")
    XCTAssertTrue(format.contains("formatMenu"))
    XCTAssertFalse(format.contains("OverlayDockButton("))
    let buttons = try XCTUnwrap(
      row.range(
        of:
          "} else if intent != .close && intent != .recoverSuperseded && intent != .discardSuperseded {"
      ))
    XCTAssertTrue(row[buttons.upperBound...].contains("OverlayDockButton("))
    XCTAssertTrue(
      row[buttons.upperBound...].contains("identifier: \"overlay-intent-\\(intent.rawValue)\""))
    let menus = [
      "overlay-history-menu": try section(
        of: source, from: "private var historyMenu", to: "private var formatMenu"),
      "overlay-previous-take-menu": try section(
        of: source, from: "private var previousTakeMenu", to: "private var historyMenu"),
      "overlay-intent-format": try section(
        of: source, from: "private var formatMenu", to: "private func setHovered"),
    ]
    let retranscribeMenu = try section(
      of: source, from: "private var retranscribeMenu", to: "private var previousTakeMenu")
    XCTAssertTrue(
      retranscribeMenu.contains(
        ".accessibilityIdentifier(\"overlay-intent-\\(OverlayIntent.retranscribe.rawValue)\")"))
    let rows: [(String, [OverlayIntent], Bool, [String])] = [
      (
        "formatted",
        [
          .recoverSuperseded, .discardSuperseded, .insertPaste, .copy,
          .retranscribe, .format, .sendToAgent, .close,
        ], true,
        [
          "overlay-history-menu", "overlay-previous-take-menu", "overlay-intent-insert-paste",
          "overlay-intent-copy", "overlay-intent-retranscribe",
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
      XCTAssertEqual(rail.historyAvailable, history)
      XCTAssertTrue(rail.intents.contains(.recoverSuperseded))
      XCTAssertTrue(rail.intents.contains(.discardSuperseded))
      for identifier in identifiers {
        if let menu = menus[identifier] {
          XCTAssertTrue(menu.contains(".accessibilityIdentifier(\"\(identifier)\")"), identifier)
        } else {
          let rawValue = String(identifier.dropFirst("overlay-intent-".count))
          let intent = try XCTUnwrap(OverlayIntent(rawValue: rawValue), identifier)
          XCTAssertTrue(rail.intents.contains(intent), identifier)
        }
      }
      XCTAssertEqual(
        rail.intents.contains(.format), history, "Only the formatted row has the Aa menu")
      XCTAssertFalse(row.contains("\"overlay-intent-close\""))
      XCTAssertTrue(
        rail.intents.contains(.close), "The guarded row excludes close even when projected")
      XCTAssertEqual(identifiers.count, history ? 7 : 3)
      XCTAssertFalse(identifiers.contains("overlay-format-level-picker"))
    }
  }

  func testTakeStartAndCollapseRemoveMountedTools() throws {
    let source = try overlaySource()
    let canvas = try section(of: source, from: "private func canvasStack", to: "/// 1px separator")
    XCTAssertTrue(canvas.contains("if !state.isCollapsed {\n        HStack(spacing: 6)"))
    XCTAssertTrue(canvas.contains("if actions.phase == .open {\n              intentRail"))
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
    state.toggleCollapsed()
    XCTAssertFalse(state.isCollapsed)
    XCTAssertEqual(actions.phase, .idle)
  }

  func testCaptionSlotPrioritizesToolThenNoticeThenDimmedEngine() {
    let cases: [(String?, String?, String?, String?, Bool?)] = [
      ("Copy", "Copied", "Whisper", "Copy", false),
      (nil, "Copied", "Whisper", "Copied", false),
      (nil, nil, "Whisper", "Whisper", true),
      (nil, nil, nil, nil, nil),
      ("", "Copied", "Whisper", "Copied", false),
      ("", "", "Whisper", "Whisper", true),
      ("", "", "", nil, nil),
    ]
    for (hovered, notice, engine, text, dimmed) in cases {
      let slot = OverlayActionsPresentation.captionSlot(
        hovered: hovered, notice: notice, engineLabel: engine)
      XCTAssertEqual(slot?.text, text)
      XCTAssertEqual(slot?.dimmed, dimmed)
    }
  }

  func testClosedPillNoticeOutranksHoverWithoutOpeningOrSchedulingHide() {
    var actions = OverlayActionsPresentation()
    XCTAssertNil(OverlayActionsPresentation.pillLabel(phase: actions.phase, notice: nil))
    for hovering in [false, true] {
      actions.pointerChanged(hovering)
      let phase = actions.phase
      XCTAssertEqual(
        OverlayActionsPresentation.pillLabel(phase: phase, notice: "Copied"), "Copied")
      for notice in [nil, ""] as [String?] {
        XCTAssertEqual(
          OverlayActionsPresentation.pillLabel(phase: phase, notice: notice),
          hovering ? "Actions…" : nil)
      }
      XCTAssertEqual(actions.phase, hovering ? .hover : .idle)
      XCTAssertNil(actions.hideDeadline)
    }
    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
    XCTAssertNil(OverlayActionsPresentation.pillLabel(phase: actions.phase, notice: "Copied"))
    XCTAssertNil(actions.hideDeadline, "A notice must not start a deadline under the pointer")
    actions.pointerChanged(false)
    let deadline = actions.hideDeadline
    XCTAssertNotNil(deadline)
    _ = OverlayActionsPresentation.pillLabel(phase: actions.phase, notice: "Format failed")
    XCTAssertEqual(actions.hideDeadline, deadline, "Notice rendering must not restart auto-hide")
  }

  func testCaptionNoticeAndEngineRenderInsideTheToolRowWithoutAFloatingCard() throws {
    let source = try railSource()
    let body = try section(
      of: source, from: "var body: some View {\n    HStack", to: "/// Retranscribe")
    let row = try section(of: body, from: "HStack(spacing: 2)", to: ".buttonStyle(.plain)")
    XCTAssertTrue(row.contains("OverlayActionsPresentation.captionSlot("))
    XCTAssertTrue(row.contains("hovered: caption(for: hoveredControl ?? focusedControl)"))
    XCTAssertTrue(row.contains("notice: footerNotice, engineLabel: footerEngineLabel"))
    XCTAssertTrue(row.contains("Text(slot.text)"))
    XCTAssertTrue(row.contains("slot.dimmed ? palette.mutedText.color : palette.primaryText.color"))
    XCTAssertTrue(row.contains(".lineLimit(1)"))
    XCTAssertTrue(row.contains(".truncationMode(.tail)"))
    XCTAssertTrue(row.contains("OverlayCaptionLayout {"))
    XCTAssertFalse(row.contains(".frame(maxWidth: 200, alignment: .leading)"))
    XCTAssertTrue(row.contains(".padding(.leading, 4)"))
    XCTAssertFalse(row.contains(".padding(.horizontal, 4)"))
    XCTAssertTrue(row.contains(".layoutPriority(-1)"))
    XCTAssertTrue(row.contains(".allowsHitTesting(false)"))
    XCTAssertTrue(row.contains(".accessibilityIdentifier(\"overlay-tool-caption\")"))
    XCTAssertTrue(body.contains(".fixedSize(horizontal: false, vertical: true)"))
    for forbidden in [
      ".overlay", ".background", ".offset", ".padding(.bottom", "OverlayActionsSurface",
    ] {
      XCTAssertFalse(body.contains(forbidden), forbidden)
    }
    for forbidden in [".frame(width: 264)", "engineChip", "footerNoticeText", "footerEngineDot"] {
      XCTAssertFalse(source.contains(forbidden), forbidden)
    }
  }

  func testCaptionWidthHugsShortTextCapsLongTextAndYieldsToNarrowProposals() {
    // Synthetic natural widths, independent of installed fonts and screen scale.
    for proposed in [nil, .infinity, 320, 200] as [CGFloat?] {
      XCTAssertEqual(OverlayCaptionLayout.width(natural: 66, proposed: proposed), 66)
      XCTAssertEqual(OverlayCaptionLayout.width(natural: 280, proposed: proposed), 200)
    }
    // The caption receives the width remaining after tools, gaps and insets.
    XCTAssertEqual(OverlayCaptionLayout.width(natural: 66, proposed: 40), 40)
    XCTAssertEqual(OverlayCaptionLayout.width(natural: 280, proposed: 120), 120)
    XCTAssertEqual(OverlayCaptionLayout.width(natural: 280, proposed: 0), 0)
    XCTAssertEqual(OverlayCaptionLayout.width(natural: 200, proposed: nil), 200)
    XCTAssertEqual(OverlayCaptionLayout.width(natural: 0, proposed: nil), 0)
  }

  func testCaptionLayoutMeasuresIdealTextAndPlacesItWithinTheAcceptedWidth() throws {
    let layout = try section(
      of: railSource(), from: "struct OverlayCaptionLayout: Layout", to: "/// The overlay's sole")
    XCTAssertTrue(layout.contains("min(natural, 200, proposed ?? .infinity)"))
    XCTAssertTrue(layout.contains("caption.sizeThatFits(.unspecified)"))
    XCTAssertTrue(layout.contains("Self.width(natural: natural.width, proposed: proposal.width)"))
    XCTAssertTrue(layout.contains("ProposedViewSize(width: width, height: proposal.height)"))
    XCTAssertTrue(layout.contains("CGSize(width: width, height: fitted.height)"))
    XCTAssertTrue(layout.contains("ProposedViewSize(width: bounds.width, height: bounds.height)"))
  }

  func testBottomCapsulesAreCenteredWithSymmetricOpenActionsInsets() throws {
    let bottom = try section(
      of: overlaySource(), from: ".overlay(alignment: .bottom)",
      to: "} else if let label = OverlayActionsPresentation.finishingLabel(")
    let capsule = try section(
      of: bottom, from: "HStack(spacing: 2)", to: ".modifier(OverlayActionsSurface(")
    XCTAssertTrue(capsule.contains(".padding(.horizontal, actions.phase == .open ? 10 : 0)"))
    XCTAssertTrue(capsule.contains(".padding(.horizontal, actions.phase == .open ? 0 : 10)"))
    XCTAssertTrue(capsule.contains("minWidth: actions.phase == .open"))
    XCTAssertTrue(
      capsule.contains("? nil : OverlayResizeChrome.actionsWidth(narrow: actions.phase != .hover)"))
    XCTAssertFalse(capsule.contains(".padding(.trailing"))
    XCTAssertFalse(capsule.contains(".padding(.leading"))
    XCTAssertTrue(capsule.contains(".fixedSize(horizontal: false, vertical: true)"))
    let groupFrame = try XCTUnwrap(
      bottom.range(of: ".frame(maxWidth: .infinity, alignment: .center)"))
    let capsuleSurface = try XCTUnwrap(bottom.range(of: ".modifier(OverlayActionsSurface("))
    let clearMargin = try XCTUnwrap(bottom.range(of: ".padding(.horizontal, OverlayResizeChrome"))
    XCTAssertLessThan(capsuleSurface.lowerBound, groupFrame.lowerBound)
    XCTAssertLessThan(groupFrame.lowerBound, clearMargin.lowerBound)
    XCTAssertFalse(bottom.contains("Spacer("))
  }

  func testOpenRailMountsOnlyInsideTheCapCapsuleAndClosedNoticeUsesItsLabel() throws {
    let source = try overlaySource()
    let canvas = try section(of: source, from: "private func canvasStack", to: "/// 1px separator")
    let capsule = try section(
      of: canvas, from: "HStack(spacing: 2)",
      to: ".padding(.vertical, actions.phase == .open ? 2 : 0)")
    XCTAssertTrue(capsule.contains("if actions.phase == .open {\n              intentRail"))
    XCTAssertEqual(
      canvas.components(separatedBy: "\n").filter {
        $0.trimmingCharacters(in: .whitespaces) == "intentRail"
      }.count, 1)
    XCTAssertTrue(capsule.contains("OverlayActionsPresentation.pillLabel("))
    XCTAssertTrue(capsule.contains("phase: actions.phase, notice: state.toast"))
    XCTAssertTrue(capsule.contains("Text(label)"))
    XCTAssertTrue(capsule.contains(".lineLimit(1)"))
    XCTAssertTrue(capsule.contains(".truncationMode(.tail)"))
    XCTAssertTrue(capsule.contains(".frame(maxWidth: 200, alignment: .leading)"))
    XCTAssertTrue(capsule.contains(".frame(height: OverlayResizeChrome.actionsHeight)"))
    XCTAssertEqual(canvas.components(separatedBy: ".modifier(OverlayActionsSurface").count - 1, 1)
    XCTAssertTrue(canvas.contains(".padding(.bottom, OverlayResizeChrome.actionsBottomInset)"))
    XCTAssertFalse(canvas.contains(".onChange(of: state.toast)"))
    XCTAssertFalse(canvas.contains(".task(id: state.toast)"))
    XCTAssertTrue(source.contains("footerNotice: state.toast"))
    XCTAssertTrue(source.contains("footerEngineLabel: state.footerEngineLabel"))
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
    for control in ["history", "previous-take"] {
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
    XCTAssertTrue(material.contains(".background { Capsule().fill(.regularMaterial) }"))
  }

  func testFinishingNoticeSurvivesFoldWithoutChangingBarHeight() throws {
    let canvas = try section(
      of: overlaySource(), from: "private func canvasStack", to: "/// 1px separator")
    let expanded = try section(of: canvas, from: "if !state.isCollapsed,", to: "bodySection")
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
      of: source, from: "private func canvasStack", to: "/// 1px separator")
    XCTAssertTrue(
      chrome.contains("? nil : OverlayResizeChrome.actionsWidth(narrow: actions.phase != .hover)")
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
