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
    engine.expansionWriteAllowed = false
    state.engine = engine
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

  func testChevronPersistsExpansionAcrossPreparingStartedAndReattach() {
    let engine = OverlayChromePolicyEngine()
    let first = OverlayState()
    first.engine = engine
    first.attach()
    XCTAssertTrue(first.isCollapsed)
    first.setExpandedByDefault(true)
    XCTAssertEqual(engine.expansionWrites, [true])
    XCTAssertFalse(first.isCollapsed)
    first.toggleCollapsed()
    XCTAssertTrue(first.isCollapsed)
    XCTAssertEqual(engine.expansionWrites, [true, false])
    XCTAssertFalse(first.expandedByDefault)
    first.handleRecordingPreparing()
    XCTAssertTrue(first.isCollapsed)
    first.handleRecordingStarted()
    XCTAssertTrue(first.isCollapsed)
    first.finishControllerRecording()
    first.toggleCollapsed()
    XCTAssertEqual(engine.expansionWrites, [true, false, true])
    first.handleRecordingPreparing()
    first.handleRecordingStarted()
    XCTAssertFalse(first.isCollapsed)
    let reopened = OverlayState()
    reopened.engine = engine
    reopened.attach()
    XCTAssertFalse(reopened.isCollapsed)
    engine.expansionWriteAllowed = false
    reopened.setExpandedByDefault(false)
    XCTAssertFalse(reopened.isCollapsed)
    XCTAssertTrue(reopened.expandedByDefault)
  }

  func testChevronSaveFailureKeepsLocalChoiceAndReportsError() {
    let engine = OverlayChromePolicyEngine()
    let state = OverlayState()
    state.engine = engine
    state.attach()
    engine.expansionWriteAllowed = false
    var collapses: [Bool] = []
    state.onCollapseChanged = { collapses.append($0) }
    state.toggleCollapsed()
    XCTAssertEqual(engine.expansionWrites, [true])
    XCTAssertFalse(state.isCollapsed)
    XCTAssertFalse(state.expandedByDefault, "The persisted preference did not change")
    XCTAssertEqual(collapses, [false])
    XCTAssertEqual(state.expansionPreferenceError, "Couldn't save overlay preference")
    state.toggleCollapsed()
    XCTAssertTrue(state.isCollapsed)
    XCTAssertEqual(engine.expansionWrites, [true, false])
    XCTAssertEqual(collapses, [false, true])
    engine.expansionWriteAllowed = true
    state.toggleCollapsed()
    XCTAssertNil(state.expansionPreferenceError)
    XCTAssertTrue(state.expandedByDefault)
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

  func testActionsControlPinsWithoutChangingCanvasHoverOrDocumentOwnership() throws {
    let source = try overlaySource()
    XCTAssertTrue(source.contains("overlay-tools-handle"))
    XCTAssertTrue(source.contains("overlay-collapse-toggle"))
    let tools = try section(
      of: source, from: "VStack(spacing: 2)", to: "private var actionsVisible")
    XCTAssertTrue(tools.contains(".fixedSize(horizontal: true, vertical: true)"))
    XCTAssertTrue(tools.contains("actions.pointerInside = hovering"))
    XCTAssertTrue(tools.contains("actions.togglePin()"))
    XCTAssertTrue(tools.contains("Image(systemName: OverlayControlSymbols.actions)"))
    XCTAssertTrue(tools.contains("if !narrowActions { Text(\"Actions\") }"))
    XCTAssertTrue(tools.contains("geometry.size.width <= 360"))
    XCTAssertTrue(tools.contains(".accessibilityLabel(\"Actions\")"))
    XCTAssertTrue(tools.contains("actions.isPinned ? \"Pinned\" : \"Hidden\""))
    let hover = try section(of: tools, from: ".onHover { hovering in", to: ".animation(")
    XCTAssertFalse(hover.contains("reset()"))
    XCTAssertFalse(hover.contains("dismiss()"))
    XCTAssertTrue(tools.contains("if collapsed { actions.reset() }"))
    XCTAssertTrue(tools.contains(".onChange(of: state.captureGeneration)"))
    XCTAssertTrue(source.contains("retainedWork: state.hasRecoverableSupersededWork"))
    let stateSource = try self.source(at: "Codescribe/Screens/Overlay/OverlayState.swift")
    XCTAssertFalse(stateSource.contains("@ObservationIgnored private(set) var captureGeneration"))
    let rail = try railSource()
    XCTAssertTrue(rail.contains(".onExitCommand {"))
    XCTAssertTrue(rail.contains("focusedControl = nil"))
    XCTAssertTrue(rail.contains("onDismiss()"))
    XCTAssertTrue(source.contains("onDismiss: { actions.dismiss() }"))
    XCTAssertFalse(source.contains("NSApp.isFullKeyboardAccessEnabled"))
    XCTAssertFalse(
      source.contains("pointerInside = inside"), "Whole canvas hover must not reveal tools")
    XCTAssertFalse(
      OverlayChromeVisibility.actionsVisible(
        pointerInside: false, keyboardFocus: false, voiceOver: false))
  }

  func testActionsPinSurvivesPointerExitAndEndsOnSecondClickEscapeOrReset() {
    var actions = OverlayActionsPresentation()
    XCTAssertFalse(actions.isVisible())
    actions.pointerInside = true
    XCTAssertTrue(actions.isVisible(), "Hover remains ephemeral")
    actions.togglePin()
    actions.pointerInside = false
    XCTAssertTrue(actions.isPinned)
    XCTAssertTrue(actions.isVisible())
    actions.togglePin()
    XCTAssertFalse(actions.isPinned)
    XCTAssertFalse(actions.isVisible())

    actions.togglePin()
    actions.keyboardFocus = true
    actions.dismiss()
    XCTAssertFalse(actions.isPinned)
    XCTAssertFalse(actions.isVisible())

    for _ in 0..<2 {
      // Both new-capture and collapse observers use this same reset.
      actions.togglePin()
      actions.pointerInside = true
      actions.keyboardFocus = true
      actions.reset()
      XCTAssertFalse(actions.isPinned)
      XCTAssertFalse(actions.isVisible())
    }
    XCTAssertTrue(actions.isVisible(retainedWork: true))
    XCTAssertTrue(actions.isVisible(voiceOver: true))
  }

  func testResizeChromeUsesTheGeometryContractsWithoutAddingSwiftUIHitTargets() throws {
    let source = try overlaySource()
    let chrome = try section(
      of: source, from: "private func canvasStack", to: "private var actionsVisible")
    XCTAssertTrue(chrome.contains("width: OverlayResizeChrome.actionsWidth(narrow: narrowActions)"))
    XCTAssertTrue(chrome.contains("height: OverlayResizeChrome.actionsHeight"))
    XCTAssertTrue(chrome.contains(".padding(.bottom, OverlayResizeChrome.actionsBottomInset)"))
    XCTAssertTrue(chrome.contains("width: OverlayResizeChrome.gripSize.width"))
    XCTAssertTrue(chrome.contains("height: OverlayResizeChrome.gripSize.height"))
    XCTAssertTrue(chrome.contains(".padding(.bottom, OverlayResizeChrome.gripBottomInset)"))
    XCTAssertTrue(chrome.contains("sideIndicatorOpacity(pointerInside: pointerInsideOverlay)"))
    XCTAssertTrue(chrome.contains("sideIndicatorAnimation(reduceMotion: reduceMotion)"))
    XCTAssertTrue(chrome.contains("transaction.disablesAnimations = true"))
    XCTAssertTrue(source.contains("pointerInsideOverlay = inside"))
    XCTAssertEqual(chrome.components(separatedBy: ".allowsHitTesting(false)").count - 1, 2)
    XCTAssertEqual(chrome.components(separatedBy: ".accessibilityHidden(true)").count - 1, 2)
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
  var expanded = false
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
    sessionId: String, sourceRevision: UInt64
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
