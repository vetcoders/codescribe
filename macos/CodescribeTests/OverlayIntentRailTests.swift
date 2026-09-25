import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
private final class OverlayIntentBoundaryEngine: DictationEngine {
  var onTranscribeFile: (() -> Void)?
  var receivedTranscribePath: String?
  var onFormatter: (() -> Void)?
  var onCopyTagged: (() -> Void)?
  var copiedTaggedText: String?
  var formatterRequests: [(sessionId: String, sourceRevision: UInt64)] = []
  var formatterFailure: Error?
  var policy = OverlayPolicySnapshot(autoPasteEnabled: true, autoFormatLevel: .correction)
  var formatLevelWrites: [FormattingPolicyOption] = []

  func setListener(_ listener: CsTranscriptionListener) {}
  func startRecording(language: CsLanguage?) async throws {}
  func stopRecording() async throws -> String { "" }
  func commitUserRevision(
    sessionId: String, sourceRevision: UInt64, renderedText: String
  ) async throws -> CsUserRevisionResult {
    return CsUserRevisionResult(
      sessionId: sessionId,
      sourceRevision: sourceRevision,
      revision: sourceRevision + 1,
      renderedText: renderedText,
      provenanceReceipt: "user-edit-intent-boundary"
    )
  }
  func commitFormatterRevision(
    sessionId: String, sourceRevision: UInt64
  ) async throws -> CsUserRevisionResult {
    formatterRequests.append((sessionId, sourceRevision))
    onFormatter?()
    if let formatterFailure { throw formatterFailure }
    return CsUserRevisionResult(
      sessionId: sessionId,
      sourceRevision: sourceRevision,
      revision: sourceRevision + 1,
      renderedText: "formatted",
      provenanceReceipt: "formatter-intent-boundary"
    )
  }
  func isRecording() async -> Bool { false }
  func initModel() async throws {}
  func isModelLoaded() -> Bool { true }
  func currentOverlayPolicy() -> OverlayPolicySnapshot? { policy }
  func setAutoPasteEnabled(_ enabled: Bool) {}
  func setAutoFormatLevel(_ level: FormattingPolicyOption) {
    formatLevelWrites.append(level)
    policy = OverlayPolicySnapshot(
      autoPasteEnabled: policy.autoPasteEnabled, autoFormatLevel: level)
  }
  func pasteText(text: String) async throws -> CsPasteResult { pasteResult() }
  func deferText(text: String) async throws -> CsPasteResult { pasteResult() }
  func copyTaggedTranscript(text: String) async throws {
    copiedTaggedText = text
    onCopyTagged?()
  }
  func pasteTargetAppName() async -> String? { nil }
  func sendAssistiveTranscript(text: String) async throws -> Bool { false }
  func lastSessionAudioPath() -> String? { "/tmp/overlay-intent-boundary.wav" }
  func sessionAudioPath(sessionId: String) -> String? { "/tmp/overlay-intent-boundary.wav" }
  func transcribeFile(path: String) async throws -> CsTranscription {
    receivedTranscribePath = path
    onTranscribeFile?()
    return CsTranscription(text: "retranscribed", language: "en")
  }

  private func pasteResult() -> CsPasteResult {
    CsPasteResult(
      outcome: .noop,
      targetAppName: nil,
      frontmostAppName: nil,
      deferredInsertShortcut: nil,
      deferredInsertFailure: nil
    )
  }
}

@MainActor
final class OverlayIntentRailTests: XCTestCase {
  func testEventFixturesRenderFrozenProjectionTable() {
    let rows:
      [(
        phase: String, text: String, paste: Bool, insert: Bool, copy: Bool,
        retranscribe: Bool, format: Bool, terminal: Bool, expected: [OverlayIntent]
      )] = [
        ("listening", "live", false, false, true, false, false, false, [.finish, .copy, .close]),
        ("listening", "", false, false, false, false, false, false, [.finish, .close]),
        ("finalizing", "draft", false, false, true, false, false, false, [.copy, .close]),
        (
          "formatted", "final", true, true, true, true, true, true,
          [.insertPaste, .copy, .retranscribe, .format, .close]
        ),
        ("no_speech", "", false, false, false, true, false, true, [.retranscribe, .close]),
        // Refused coverage: the ledger declined the seal, the words are real.
        // Every producer-authorized recovery is painted. Format reaches the
        // existing reducer path and any terminal refusal is shown by name.
        (
          "coverage_refused", "usable words", true, true, true, true, true, true,
          [.insertPaste, .copy, .retranscribe, .format, .close]
        ),
        // Empty typed refusal: nothing to act on, and the rail invents nothing.
        ("coverage_refused", "", false, false, false, false, false, true, [.close]),
        // Contract change (rc-w2-refusal-ui): `error` used to return `[.close]`
        // regardless of the producer's bits. A delivery failure keeps its words
        // and its audio, so that fixed list hid Copy/Insert/Retranscribe behind
        // the word "error" at exactly the moment they were the recovery.
        (
          "error", "draft", true, true, true, true, true, true,
          [.insertPaste, .copy, .retranscribe, .close]
        ),
        // Sink failure with retained audio, no destination left: no Insert.
        (
          "error", "draft", false, false, true, true, false, true,
          [.copy, .retranscribe, .close]
        ),
        // All-false error stays exactly as strict as before.
        ("error", "", false, false, false, false, false, true, [.close]),
      ]

    for row in rows {
      let state = projectedState(
        phase: row.phase,
        text: row.text,
        canPaste: row.paste,
        canInsert: row.insert,
        canCopy: row.copy,
        canRetranscribe: row.retranscribe,
        canFormat: row.format,
        terminal: row.terminal
      )

      XCTAssertEqual(
        OverlayIntentRail.projectedIntents(for: state),
        row.expected,
        "event fixture for \(row.phase) did not paint the frozen action table"
      )
    }
  }

  func testDispatchUsesProductionStateRouteAndLeavesProjectionUntouched() async {
    let state = projectedState(
      phase: "formatted",
      text: "final",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    let engine = OverlayIntentBoundaryEngine()
    state.engine = engine
    let requested = expectation(description: "format intent reached production boundary")
    engine.onFormatter = { requested.fulfill() }
    let rail = OverlayIntentRail(
      phase: state.statusText,
      intents: OverlayIntentRail.projectedIntents(for: state),
      palette: .dark,
      onIntent: state.relayIntent
    )

    rail.dispatch(.format)
    await fulfillment(of: [requested], timeout: 1)

    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(state.formattedText, "final")
    XCTAssertEqual(state.revision, 1)
    XCTAssertTrue(state.canPaste)
    XCTAssertTrue(state.canInsert)
    XCTAssertTrue(state.canCopy)
    XCTAssertTrue(state.canRetranscribe)
    XCTAssertTrue(state.canFormat)
    XCTAssertTrue(state.terminal)
    XCTAssertEqual(engine.formatterRequests.count, 1)
    XCTAssertEqual(engine.formatterRequests[0].sessionId, "intent-rail-fixture")
    XCTAssertEqual(engine.formatterRequests[0].sourceRevision, 1)
    XCTAssertTrue(state.formatterCommitPending, "FFI acknowledgement is not projection")
    XCTAssertNil(state.formatterError)
  }

  func testRefusedSealStillOffersFormatterAndNamesReducerRefusal() async {
    let state = projectedState(
      phase: "coverage_refused", text: "usable words", canPaste: false,
      canInsert: false, canCopy: true, canRetranscribe: true, canFormat: true,
      terminal: true)
    let engine = OverlayIntentBoundaryEngine()
    engine.formatterFailure = NSError(domain: "NotTerminal", code: 1)
    state.engine = engine
    XCTAssertTrue(OverlayIntentRail.projectedIntents(for: state).contains(.format))
    let requested = expectation(description: "refused formatter reached reducer")
    engine.onFormatter = { requested.fulfill() }
    state.relayIntent(.format)
    await fulfillment(of: [requested], timeout: 1)
    await Task.yield()
    XCTAssertEqual(engine.formatterRequests.count, 1)
    XCTAssertEqual(state.mode, .coverageRefused)
    XCTAssertEqual(state.formattedText, "usable words")
    XCTAssertTrue(state.formatterError?.contains("NotTerminal") == true)
  }

  func testFloatingActionsKeepProjectedOrderWithCloseInHeader() {
    let intents: [OverlayIntent] = [
      .insertPaste, .copy, .retranscribe, .format, .close,
    ]
    let layout = OverlayDockLayout(projectedIntents: intents)

    XCTAssertEqual(layout.visibleIntents, intents.filter { $0 != .close })
    XCTAssertEqual(OverlayDockLayout(projectedIntents: []).visibleIntents, [])
    XCTAssertEqual(OverlayDockLayout.minimumCanvasWidth, 320)
  }

  func testDirtyRevisionReplacesDeliveryActionsWithCommitOrDiscard() {
    let state = projectedState(
      phase: "formatted",
      text: "ledger text",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )

    state.beginTranscriptEdit()
    state.updateRevisionDraft("local draft")

    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: state),
      [.commitRevision, .discardRevision, .close]
    )
    XCTAssertEqual(state.formattedText, "ledger text")
    XCTAssertEqual(state.canvasText, "local draft")
  }

  func testRetranscribeMenuPicksThePassAndTheBareIntentStaysLocal() async {
    let state = projectedState(
      phase: "formatted",
      text: "final",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    let engine = OverlayIntentBoundaryEngine()
    state.engine = engine
    // Founder 2026-09-09: the formatting level is tray quick-settings chrome,
    // never a dock control; Retranscribe is opt-in with a Local / Cloud pick.
    let rail = OverlayIntentRail(
      phase: state.statusText,
      intents: OverlayIntentRail.projectedIntents(for: state),
      palette: .dark,
      onIntent: state.relayIntent,
      onRetranscribe: { state.retranscribe(pass: $0) }
    )
    XCTAssertTrue(rail.intents.contains(.retranscribe))

    let cloud = expectation(description: "cloud pass reached the engine")
    engine.onTranscribeFile = { cloud.fulfill() }
    rail.retranscribe(.cloud)
    await fulfillment(of: [cloud], timeout: 1)
    XCTAssertEqual(engine.receivedTranscribePath, "cloud:/tmp/overlay-intent-boundary.wav")

    let local = expectation(description: "bare intent runs the local HQ pass")
    engine.onTranscribeFile = { local.fulfill() }
    rail.dispatch(.retranscribe)
    await fulfillment(of: [local], timeout: 1)
    XCTAssertEqual(engine.receivedTranscribePath, "hq:/tmp/overlay-intent-boundary.wav")
    XCTAssertTrue(engine.formatLevelWrites.isEmpty, "the dock never writes the formatting level")
  }

  func testEveryIntentHasVoiceOverCopyAndRailReportsProjectedPhase() {
    let intents: [OverlayIntent] = [
      .finish, .commitRevision, .discardRevision, .copy, .insertPaste, .retranscribe, .format,
      .recoverSuperseded, .discardSuperseded, .close,
    ]

    XCTAssertEqual(
      intents.map(\.accessibilityLabel),
      [
        "Finish recording",
        "Commit transcript revision",
        "Discard transcript draft",
        "Copy transcript",
        "Insert transcript",
        "Retranscribe recording",
        "Format transcript",
        "Recover previous transcript",
        "Discard previous transcript",
        "Close overlay",
      ]
    )
    XCTAssertTrue(intents.allSatisfy { !$0.accessibilityHint.isEmpty })
    XCTAssertTrue(intents.allSatisfy { !$0.helpText.isEmpty })
    XCTAssertEqual(OverlayDockVisuals.hoverOpacity(isHovering: false), 0)
    XCTAssertGreaterThan(OverlayDockVisuals.hoverOpacity(isHovering: true), 0)
    XCTAssertEqual(OverlayIntentRail.accessibilityValue(for: "no speech"), "no speech")
  }

  // MARK: Retained-work recovery on the sole action surface

  /// Negative then positive: the two recovery commands appear only when work is
  /// actually retained. Error recovery follows usable text and capabilities;
  /// only an empty error with no capabilities has a close-only rail.
  func testRecoveryCommandsAppearOnlyWithRetainedWorkAndSurviveErrorMode() {
    let empty = projectedState(
      phase: "error", text: "", canPaste: false, canInsert: false,
      canCopy: false, canRetranscribe: false, canFormat: false, terminal: true)
    XCTAssertEqual(OverlayIntentRail.projectedIntents(for: empty), [.close])
    XCTAssertEqual(OverlayIntentRail.recoveryIntents(for: empty), [])
    XCTAssertFalse(empty.hasRecoverableSupersededWork)

    let clean = projectedState(
      phase: "error",
      text: "draft",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    XCTAssertEqual(OverlayIntentRail.recoveryIntents(for: clean), [])
    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: clean), [.insertPaste, .copy, .retranscribe, .close])
    XCTAssertFalse(OverlayIntentRail.projectedIntents(for: clean).contains(.format))
    XCTAssertEqual(clean.mode, .error)
    XCTAssertEqual(clean.activeText, "draft")
    XCTAssertFalse(clean.hasRecoverableSupersededWork)

    let retained = stateWithOneRetainedEdit()
    XCTAssertTrue(retained.hasRecoverableSupersededWork)
    XCTAssertEqual(
      OverlayIntentRail.recoveryIntents(for: retained),
      [.recoverSuperseded, .discardSuperseded])

    // A live capture still shows at most the labelled affordance — never the
    // previous words, which stay off the canvas.
    retained.handleRecordingStarted()
    XCTAssertEqual(retained.canvasText, "")
    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: retained),
      [.recoverSuperseded, .discardSuperseded, .finish, .close])

    // And the ephemeral chrome reveals itself, so discovery does not depend on
    // the user guessing to hover a panel that is showing a NEW take.
    XCTAssertTrue(
      OverlayChromeVisibility.actionsVisible(
        pointerInside: false, keyboardFocus: false, voiceOver: false, retainedWork: true))
    XCTAssertFalse(
      OverlayChromeVisibility.actionsVisible(
        pointerInside: false, keyboardFocus: false, voiceOver: false, retainedWork: false))
  }

  /// The rail's own dispatch route reaches the retention owner, and the
  /// retained bytes leave through the injected pasteboard rather than the
  /// reducer, a seal, a delivery or the canvas.
  func testRailDispatchRecoversAndDiscardsThroughTheProductionRoute() {
    let recovered = stateWithOneRetainedEdit()
    let engine = OverlayIntentBoundaryEngine()
    recovered.engine = engine
    var written: [String] = []
    recovered.recoveryClipboardWriter = { text in
      written.append(text)
      return true
    }
    let rail = OverlayIntentRail(
      phase: recovered.statusText,
      intents: OverlayIntentRail.projectedIntents(for: recovered),
      palette: .dark,
      onIntent: recovered.relayIntent
    )
    XCTAssertTrue(rail.intents.contains(.recoverSuperseded))
    XCTAssertTrue(rail.intents.contains(.discardSuperseded))

    rail.dispatch(.recoverSuperseded)

    XCTAssertEqual(written, ["Zdanie z moją poprawką."])
    XCTAssertFalse(recovered.hasRecoverableSupersededWork)
    XCTAssertNil(recovered.recoveryFailure)
    XCTAssertTrue(engine.formatterRequests.isEmpty, "recovery is not a reducer path")
    XCTAssertTrue(engine.formatLevelWrites.isEmpty)
    XCTAssertFalse(recovered.isEditingTranscript, "recovery takes no focus")

    let discarded = stateWithOneRetainedEdit()
    discarded.relayIntent(.discardSuperseded)
    XCTAssertFalse(discarded.hasRecoverableSupersededWork)

    // A refused write keeps the item, so the rail keeps offering both choices.
    let refused = stateWithOneRetainedEdit()
    refused.recoveryClipboardWriter = { _ in false }
    refused.relayIntent(.recoverSuperseded)
    XCTAssertTrue(refused.hasRecoverableSupersededWork)
    XCTAssertNotNil(refused.recoveryFailure)
    XCTAssertEqual(
      OverlayIntentRail.recoveryIntents(for: refused),
      [.recoverSuperseded, .discardSuperseded])
  }

  /// One formatted take with an uncommitted edit, superseded by a new capture.
  private func stateWithOneRetainedEdit() -> OverlayState {
    let state = projectedState(
      phase: "formatted",
      text: "Zdanie.",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Zdanie z moją poprawką.")
    state.endTranscriptEdit()
    state.handleRecordingPreparing()
    return state
  }

  func testProjectedRetranscribeIntentReachesInjectedEngineBoundary() async {
    let engine = OverlayIntentBoundaryEngine()
    let state = projectedState(
      phase: "formatted",
      text: "final",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    state.engine = engine
    let reached = expectation(description: "retranscribe reaches the engine boundary")
    engine.onTranscribeFile = { reached.fulfill() }

    state.relayIntent(.retranscribe)

    await fulfillment(of: [reached], timeout: 0.2)
    XCTAssertEqual(engine.receivedTranscribePath, "hq:/tmp/overlay-intent-boundary.wav")
    XCTAssertEqual(state.toast, "retranscribed — Back keeps the old text")
  }

  func testMissingEngineSurfacesCopyAndInsertFailuresOnCanvas() {
    let state = projectedState(
      phase: "formatted",
      text: "final",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: false,
      canFormat: false,
      terminal: true
    )

    state.relayIntent(.copy)
    XCTAssertEqual(state.toast, "copy unavailable")
    XCTAssertEqual(state.errorMessage, "Copy needs the recording engine")

    state.relayIntent(.insertPaste)
    XCTAssertEqual(state.toast, "insert unavailable")
    XCTAssertEqual(state.errorMessage, "Insert needs the recording engine")
  }

  func testDockRendersAtWindowFloorWithRoundedCanvasCorners() throws {
    let state = OverlayState.previewFormatted()
    let size = CGSize(
      width: OverlayDockLayout.minimumCanvasWidth,
      height: DictationOverlayWindow.minSize.height
    )
    let hostingView = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .frame(width: size.width, height: size.height)
        .preferredColorScheme(.dark)
    )
    hostingView.frame = CGRect(origin: .zero, size: size)
    hostingView.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.03))

    let bitmap = try XCTUnwrap(hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds))
    hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
    let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
    let destination = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-expanded-bottom-dock-min-width.png")
    try png.write(to: destination)

    XCTAssertGreaterThan(png.count, 800)
    XCTAssertEqual(size.width, DictationOverlayWindow.minSize.width)
    XCTAssertLessThan(bitmap.colorAt(x: 0, y: 0)?.alphaComponent ?? 0, 0.2)
    XCTAssertLessThan(
      bitmap.colorAt(x: bitmap.pixelsWide - 1, y: 0)?.alphaComponent ?? 0,
      0.2
    )
  }

  // MARK: Refusal recovery (rc-w2-refusal-ui) — UNRUN under W2

  /// The census claim, stated as a test rather than as prose: every case of
  /// `OverlayMode` is asked for its rail, and the two terminal outcomes that
  /// are not `formatted` must not collapse to a bare Close when the producer
  /// says recovery exists.
  func testEveryProjectionPhaseAnswersTheRailAndOnlySilenceProducesBareClose() {
    // A flat list, not a dictionary: every case is named exactly once, so a
    // seventh phase added to `OverlayMode` without a row here is a visible
    // omission rather than a silently missing key.
    let allBitsOn: [(mode: OverlayMode, expected: [OverlayIntent])] = [
      (.listening, [.finish, .copy, .close]),
      (.finalizing, [.copy, .close]),
      (.formatted, [.insertPaste, .copy, .retranscribe, .format, .close]),
      (.coverageRefused, [.insertPaste, .copy, .retranscribe, .format, .close]),
      (.noSpeech, [.retranscribe, .close]),
      (.error, [.insertPaste, .copy, .retranscribe, .close]),
    ]

    for (mode, expected) in allBitsOn {
      XCTAssertEqual(
        OverlayIntentRail.projectedIntents(
          phase: mode, canPaste: true, canInsert: true, canCopy: true,
          canRetranscribe: true, canFormat: true),
        expected,
        "\(mode) did not paint its full producer-authorized rail"
      )
      let silent = OverlayIntentRail.projectedIntents(
        phase: mode, canPaste: false, canInsert: false, canCopy: false,
        canRetranscribe: false, canFormat: false)
      XCTAssertEqual(
        silent.filter { $0 != .finish }, [.close],
        "\(mode) invented a command the producer did not authorize"
      )
    }
  }

  /// Refused coverage honors the producer bit; an error phase still withholds
  /// Format because the user cannot start a terminal formatter there.
  func testRefusedFormatUsesProducerPermission() {
    XCTAssertTrue(
      OverlayIntentRail.projectedIntents(
        phase: .coverageRefused, canPaste: false, canInsert: false, canCopy: false,
        canRetranscribe: false, canFormat: true
      ).contains(.format))
    for mode in [OverlayMode.error] {
      XCTAssertFalse(
        OverlayIntentRail.projectedIntents(
          phase: mode, canPaste: false, canInsert: false, canCopy: false,
          canRetranscribe: false, canFormat: true
        ).contains(.format),
        "\(mode) projected Format, whose relay refuses outside .formatted"
      )
    }
    XCTAssertTrue(
      OverlayIntentRail.projectedIntents(
        phase: .formatted, canPaste: false, canInsert: false, canCopy: false,
        canRetranscribe: false, canFormat: true
      ).contains(.format)
    )
  }

  /// A refused take reaching the rail through the real projection boundary,
  /// with its recovery commands dispatched into the production relay. Copy is
  /// the falsifier that matters: if the relay had been phase-gated, this would
  /// silently do nothing while the button was on screen.
  func testRefusedCoverageRecoveryReachesTheProductionRelay() async {
    let state = projectedState(
      phase: "coverage_refused",
      text: "usable but unsealed",
      canPaste: false,
      canInsert: false,
      canCopy: true,
      canRetranscribe: true,
      canFormat: false,
      terminal: true
    )
    let engine = OverlayIntentBoundaryEngine()
    state.engine = engine

    XCTAssertEqual(state.mode, .coverageRefused)
    XCTAssertEqual(state.statusText, "unverified coverage")
    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: state), [.copy, .retranscribe, .close])

    let rail = OverlayIntentRail(
      phase: state.statusText,
      intents: OverlayIntentRail.projectedIntents(for: state),
      palette: .dark,
      onIntent: state.relayIntent,
      onRetranscribe: state.retranscribe
    )
    let copied = expectation(description: "copy intent reached the production relay")
    engine.onCopyTagged = { copied.fulfill() }
    rail.dispatch(.copy)
    await fulfillment(of: [copied], timeout: 1)

    XCTAssertEqual(engine.copiedTaggedText, "usable but unsealed")
    XCTAssertEqual(state.mode, .coverageRefused, "recovery must not relabel the phase")
    XCTAssertEqual(OverlayIntentRail.accessibilityValue(for: state.statusText), "unverified coverage")
  }

  func testErrorRecoveryCopyUsesProductionRouteWithoutFormatting() async {
    let state = projectedState(
      phase: "error", text: "usable error words", canPaste: false, canInsert: false,
      canCopy: true, canRetranscribe: true, canFormat: true, terminal: true)
    let engine = OverlayIntentBoundaryEngine()
    state.engine = engine
    let projection = state.latestTranscriptProjection
    let rail = OverlayIntentRail(
      phase: state.statusText,
      intents: OverlayIntentRail.projectedIntents(for: state),
      palette: .dark,
      onIntent: state.relayIntent
    )
    XCTAssertEqual(rail.intents, [.copy, .retranscribe, .close])
    let copied = expectation(description: "error recovery copy reached the production relay")
    engine.onCopyTagged = { copied.fulfill() }
    rail.dispatch(.copy)
    await fulfillment(of: [copied], timeout: 1)

    XCTAssertEqual(engine.copiedTaggedText, "usable error words")
    XCTAssertEqual(state.mode, .error)
    XCTAssertEqual(state.activeText, "usable error words")
    XCTAssertEqual(state.latestTranscriptProjection?.sequence, projection?.sequence)
    XCTAssertEqual(state.latestTranscriptProjection?.reducerRevision, projection?.reducerRevision)
    XCTAssertEqual(state.latestTranscriptProjection?.reducerAction, "intent_rail_fixture")
    XCTAssertTrue(engine.formatterRequests.isEmpty, "copy recovery must not create a revision or seal")
    XCTAssertTrue(engine.formatLevelWrites.isEmpty)
  }

  /// Unacknowledged superseded work still LEADS the rail on a refused take.
  /// The refusal is precisely the case where the phase table is thin, and a
  /// thin table may not be the reason an unsaved edit becomes unreachable.
  func testRetainedWorkStillLeadsTheRailOnARefusedTake() {
    let state = projectedState(
      phase: "formatted",
      text: "previous take",
      canPaste: false,
      canInsert: false,
      canCopy: true,
      canRetranscribe: false,
      canFormat: false,
      terminal: true
    )
    state.beginTranscriptEdit()
    state.updateRevisionDraft("previous take, edited")
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertTrue(state.hasRecoverableSupersededWork)

    state.applyTranscriptProjection(
      CsTranscriptProjectionEvent(
        schema: "codescribe.transcript_projection.v1",
        sequence: 9,
        emittedAt: "2026-09-10T00:00:00Z",
        sessionId: "intent-rail-successor",
        mode: "dictation",
        reducerRevision: 9,
        reducerAction: "session_ended",
        occurrenceSessionId: "intent-rail-successor",
        captureEpoch: 2,
        sampleStart: 16_000,
        sampleEnd: 32_000,
        documentIndex: 1,
        label: "terminal",
        renderedText: "refused words",
        deliveryText: nil,
        phase: "coverage_refused",
        canPaste: false,
        canInsert: false,
        canCopy: true,
        canRetranscribe: true,
        canFormat: false,
        canSendToAgent: false,
        terminal: true,
        lifecycleTerminal: true,
        delivery: .unattempted,
        acousticReceipts: [],
        sealCoverage: nil,
        consultationPresentations: []
      )
    )

    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: state),
      [.recoverSuperseded, .discardSuperseded, .copy, .retranscribe, .close]
    )
  }

  private func projectedState(
    phase: String,
    text: String,
    canPaste: Bool,
    canInsert: Bool,
    canCopy: Bool,
    canRetranscribe: Bool,
    canFormat: Bool,
    terminal: Bool
  ) -> OverlayState {
    let state = OverlayState()
    state.applyTranscriptProjection(
      CsTranscriptProjectionEvent(
        schema: "codescribe.transcript_projection.v1",
        sequence: 1,
        emittedAt: "2026-09-04T00:00:00Z",
        sessionId: "intent-rail-fixture",
        mode: "dictation",
        reducerRevision: 1,
        reducerAction: "intent_rail_fixture",
        occurrenceSessionId: "intent-rail-fixture",
        captureEpoch: 1,
        sampleStart: 0,
        sampleEnd: 16_000,
        documentIndex: 0,
        label: phase,
        renderedText: text,
        deliveryText: nil,
        phase: phase,
        canPaste: canPaste,
        canInsert: canInsert,
        canCopy: canCopy,
        canRetranscribe: canRetranscribe,
        canFormat: canFormat,
        canSendToAgent: false,
        terminal: terminal,
        lifecycleTerminal: terminal,
        delivery: .unattempted,
        acousticReceipts: [],
        sealCoverage: nil,
        consultationPresentations: []
      )
    )
    return state
  }
}
