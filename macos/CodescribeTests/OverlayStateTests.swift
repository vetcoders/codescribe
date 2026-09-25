import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

// Executed by `make test-swift`. The marker-rebase assertions below run for
// real; see CodescribeTests/README.md.

@MainActor
private final class OverlayStateTestEngine: DictationEngine {
  struct RevisionRequest: Equatable {
    let sessionId: String
    let sourceRevision: UInt64
    let renderedText: String
  }

  struct FormatterRequest: Equatable {
    let sessionId: String
    let sourceRevision: UInt64
    var level: FormattingPolicyOption? = nil
  }

  var pastedText: String?
  var onStopRecording: (() -> Void)?
  var pasteCallCount = 0
  var pasteOutcome: CsPasteOutcome = .pasted
  var pasteFrontmostAppNameValue: String?
  var deferredText: String?
  var deferOutcome: CsPasteOutcome = .deferredInsertArmed
  var deferredInsertShortcutValue: String? = "⌘⌥V"
  var deferredInsertFailureValue: String?
  var copiedTaggedText: String?
  var onCopyTagged: (() -> Void)?
  var onPaste: (() -> Void)?
  var onDefer: (() -> Void)?
  var pasteTargetAppNameValue: String?
  var onPasteTargetRead: (() -> Void)?
  var persistedPolicy = OverlayPolicySnapshot(
    autoPasteEnabled: true,
    autoFormatLevel: .correction
  )
  var persistAutoPasteWrites = true
  var autoPasteWrites: [Bool] = []
  var pinEnabled = false
  var pinWrites: [Bool] = []
  func overlayKeepVisibleBetweenTakes() -> Bool { pinEnabled }
  func setOverlayKeepVisibleBetweenTakes(_ enabled: Bool) -> Bool {
    pinEnabled = enabled
    pinWrites.append(enabled)
    return true
  }
  var policyReadCount = 0
  var sentAssistiveTexts: [String] = []
  var assistiveSendResult = true
  var onAssistiveSend: (() -> Void)?
  var assistiveSendHandler: (() async throws -> Bool)?
  var revisionRequests: [RevisionRequest] = []
  var onRevision: (() -> Void)?
  var formatterRequests: [FormatterRequest] = []
  var formatterRenderedText = "Tekst sformatowany."
  var formatterShouldFail = false
  var onFormatter: (() -> Void)?
  var historyEntries: [CsDocumentHistoryEntry] = []
  var onHistoryRead: (() -> Void)?
  var onRestore: (() -> Void)?
  var restoredSelections: [UInt64] = []

  func setListener(_ listener: CsTranscriptionListener) {}
  func startRecording(language: CsLanguage?) async throws {}
  func stopRecording() async throws -> String {
    onStopRecording?()
    return ""
  }
  func commitUserRevision(
    sessionId: String, sourceRevision: UInt64, renderedText: String
  ) async throws -> CsUserRevisionResult {
    revisionRequests.append(
      RevisionRequest(
        sessionId: sessionId,
        sourceRevision: sourceRevision,
        renderedText: renderedText
      )
    )
    onRevision?()
    return CsUserRevisionResult(
      sessionId: sessionId,
      sourceRevision: sourceRevision,
      revision: sourceRevision + 1,
      renderedText: renderedText,
      provenanceReceipt: "user-edit-test-\(sourceRevision + 1)"
    )
  }
  func commitFormatterRevision(
    sessionId: String, sourceRevision: UInt64, level: FormattingPolicyOption?
  ) async throws -> CsUserRevisionResult {
    formatterRequests.append(
      FormatterRequest(sessionId: sessionId, sourceRevision: sourceRevision, level: level)
    )
    onFormatter?()
    if formatterShouldFail {
      throw NSError(
        domain: "OverlayStateTestFormatter", code: 1,
        userInfo: [NSLocalizedDescriptionKey: "gateway unavailable"])
    }
    return CsUserRevisionResult(
      sessionId: sessionId,
      sourceRevision: sourceRevision,
      revision: sourceRevision + 1,
      renderedText: formatterRenderedText,
      provenanceReceipt: "formatter-test-\(sourceRevision + 1)"
    )
  }
  func documentHistory(sessionId _: String) async throws -> [CsDocumentHistoryEntry] {
    onHistoryRead?()
    return historyEntries
  }
  func restoreDocumentRevision(
    sessionId: String, sourceRevision: UInt64, restoreRevision: UInt64
  ) async throws -> CsUserRevisionResult {
    restoredSelections.append(restoreRevision)
    onRestore?()
    let text = historyEntries.first(where: { $0.revision == restoreRevision })!.renderedText
    historyEntries.append(
      CsDocumentHistoryEntry(
        revision: sourceRevision + 1, renderedText: text, provenance: "user-edit",
        emittedAt: "2026-09-25T00:00:04Z"))
    return CsUserRevisionResult(
      sessionId: sessionId, sourceRevision: sourceRevision,
      revision: sourceRevision + 1, renderedText: text,
      provenanceReceipt: "user-edit-test-\(sourceRevision + 1)")
  }
  func isRecording() async -> Bool { false }
  func initModel() async throws {}
  func isModelLoaded() -> Bool { true }
  func currentOverlayPolicy() -> OverlayPolicySnapshot? {
    policyReadCount += 1
    return persistedPolicy
  }
  func setAutoPasteEnabled(_ enabled: Bool) {
    autoPasteWrites.append(enabled)
    guard persistAutoPasteWrites else { return }
    persistedPolicy = OverlayPolicySnapshot(
      autoPasteEnabled: enabled,
      autoFormatLevel: persistedPolicy.autoFormatLevel
    )
  }
  func pasteText(text: String) async throws -> CsPasteResult {
    pastedText = text
    pasteCallCount += 1
    onPaste?()
    return CsPasteResult(
      outcome: pasteOutcome,
      targetAppName: pasteTargetAppNameValue,
      frontmostAppName: pasteFrontmostAppNameValue,
      deferredInsertShortcut: deferredInsertShortcutValue,
      deferredInsertFailure: deferredInsertFailureValue
    )
  }
  func deferText(text: String) async throws -> CsPasteResult {
    deferredText = text
    onDefer?()
    return CsPasteResult(
      outcome: deferOutcome,
      targetAppName: pasteTargetAppNameValue,
      frontmostAppName: "Codescribe",
      deferredInsertShortcut: deferredInsertShortcutValue,
      deferredInsertFailure: deferredInsertFailureValue
    )
  }
  func copyTaggedTranscript(text: String) async throws {
    copiedTaggedText = text
    onCopyTagged?()
  }
  func pasteTargetAppName() async -> String? {
    onPasteTargetRead?()
    return pasteTargetAppNameValue
  }
  func sendAssistiveTranscript(text: String) async throws -> Bool {
    sentAssistiveTexts.append(text)
    onAssistiveSend?()
    if let assistiveSendHandler { return try await assistiveSendHandler() }
    return assistiveSendResult
  }
  var lastSessionAudioPathValue: String?
  var requestedAudioSessionIds: [String] = []
  var audioPathsBySession: [String: String] = [:]
  var transcriptionText = ""
  func lastSessionAudioPath() -> String? { lastSessionAudioPathValue }
  func sessionAudioPath(sessionId: String) -> String? {
    requestedAudioSessionIds.append(sessionId)
    return audioPathsBySession[sessionId] ?? lastSessionAudioPathValue
  }
  func transcribeFile(path _: String) async throws -> CsTranscription {
    CsTranscription(text: transcriptionText, language: "pl")
  }
}

private final class OverlayStateTestClock {
  var now: TimeInterval = 0
}

@MainActor
final class OverlayStateTests: XCTestCase {
  func testCompactPaintCannotAdmitCaptureOrMutateDocument() {
    let state = OverlayState()
    let initial = CsCompactProjection(
      sessionId: "A", captureEpoch: 7, sequence: 1, text: "", degraded: false, evidence: [])
    state.applyCompactProjection(initial)
    XCTAssertNil(state.compactProjection)
    state.handleRecordingPreparing()
    state.applyCompactProjection(initial)
    let warning = CsCompactProjection(
      sessionId: "A", captureEpoch: 7, sequence: 2, text: "…", degraded: true, evidence: [])
    state.applyCompactProjection(warning)
    XCTAssertEqual(state.compactProjection, warning)
    for stale in [
      initial,
      CsCompactProjection(
        sessionId: "A", captureEpoch: 8, sequence: 9, text: "bad epoch", degraded: false,
        evidence: []),
      CsCompactProjection(
        sessionId: "B", captureEpoch: 7, sequence: 9, text: "bad session", degraded: false,
        evidence: []),
    ] {
      state.applyCompactProjection(stale)
      XCTAssertEqual(state.compactProjection, warning)
    }
    XCTAssertNil(state.latestTranscriptProjection)
    XCTAssertTrue(state.formattedText.isEmpty)
    state.finishControllerRecording()
    state.handleRecordingPreparing()
    XCTAssertNil(state.compactProjection)
    state.applyCompactProjection(initial)
    state.applyCompactProjection(warning)
    XCTAssertNil(state.compactProjection)
    let successor = CsCompactProjection(
      sessionId: "B", captureEpoch: 1, sequence: 1, text: "", degraded: false, evidence: [])
    state.applyCompactProjection(successor)
    XCTAssertEqual(state.compactProjection, successor)
    state.finishControllerRecording()
  }

  func testListenerQueuesCompactPaintWithoutReinterpretation() async {
    let channel = AsyncStream<OverlayListenerEvent>.makeStream()
    let listener = DictationListener(continuation: channel.continuation)
    let paint = CsCompactProjection(
      sessionId: "A", captureEpoch: 7, sequence: 2, text: "…", degraded: true, evidence: [])
    listener.onCompactProjection(event: paint)
    channel.continuation.finish()
    var iterator = channel.stream.makeAsyncIterator()
    guard case .compactProjection(let received) = await iterator.next() else {
      return XCTFail("Missing compact projection")
    }
    XCTAssertEqual(received, paint)
  }

  /// Counterexample A at the Swift boundary: a refused Whisper alternative
  /// rides the compact paint as evidence. It is shown beside the canvas while
  /// the take is live and never reaches the canvas or delivered text.
  func testUnanchoredEvidenceIsLivePaintBesideTheCanvasOnly() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    projectText("Apple mówi tak", to: state, sessionId: "A")
    let alternative = CsUnanchoredEvidence(
      sampleStart: 16_000, sampleEnd: 32_000, text: "Whisper mówi inaczej",
      reason: "exclusive_tail_awaiting_whole_span")
    state.applyCompactProjection(
      CsCompactProjection(
        sessionId: "A", captureEpoch: 1, sequence: 1, text: "Apple mówi tak",
        degraded: false, evidence: [alternative]))
    XCTAssertEqual(state.liveEvidence, [alternative])
    XCTAssertEqual(state.canvasText, "Apple mówi tak", "evidence never enters the canvas")
    XCTAssertEqual(state.activeText, "Apple mówi tak", "nor the delivered text")

    projectText("Apple mówi tak", to: state, terminal: true, sessionId: "A")
    XCTAssertTrue(state.liveEvidence.isEmpty, "a terminal take shows its sealed document alone")
    XCTAssertEqual(state.canvasText, "Apple mówi tak")
  }

  private var nextProjectionSequence: UInt64 = 0

  func testListenerQueuesLifecycleEventsInCallbackOrder() async {
    let channel = AsyncStream<OverlayListenerEvent>.makeStream()
    let listener = DictationListener(continuation: channel.continuation)
    listener.onRecordingPreparing()
    listener.onRecordingStarted()
    listener.onRecordingFinalising()
    listener.onRecordingStopped()

    var iterator = channel.stream.makeAsyncIterator()
    guard case .recordingPreparing = await iterator.next() else {
      return XCTFail("preparing callback lost or reordered")
    }
    guard case .recordingStarted = await iterator.next() else {
      return XCTFail("started callback lost or reordered")
    }
    guard case .recordingFinalising = await iterator.next() else {
      return XCTFail("finalising callback lost or reordered")
    }
    guard case .recordingStopped = await iterator.next() else {
      return XCTFail("stopped callback lost or reordered")
    }
  }

  func testListenerCarriesTypedPresentationStatusWithoutTextParsing() async {
    let channel = AsyncStream<OverlayListenerEvent>.makeStream()
    let listener = DictationListener(continuation: channel.continuation)
    listener.onPresentationStatus(event: refusalStatus())

    var iterator = channel.stream.makeAsyncIterator()
    guard case .presentationStatus(let event) = await iterator.next() else {
      return XCTFail("typed presentation status callback was lost")
    }
    XCTAssertEqual(event.code, "admission_calibration_unusable")
    XCTAssertTrue(event.message.contains("Settings › Audio"))
  }

  /// Admit Rust-owned transcript truth through the same projection boundary as
  /// production. Tests may choose rendered documents; they do not replay the
  /// demolished Swift preview/final/patch reducer.
  private func projectText(
    _ text: String,
    to state: OverlayState,
    mode: String = "dictation",
    phase: String? = nil,
    canPaste: Bool = false,
    canInsert: Bool = false,
    canCopy: Bool? = nil,
    canRetranscribe: Bool = false,
    canFormat: Bool = false,
    canSendToAgent: Bool? = nil,
    terminal: Bool = false,
    lifecycleTerminal: Bool? = nil,
    delivery: CsTranscriptDelivery = .unattempted,
    includesWordEvidence: Bool = true,
    sessionId: String = "overlay-state-tests",
    reducerRevision: UInt64? = nil,
    reducerAction: String? = nil,
    manualEditReceipt: String? = nil,
    sealCoverage: CsProjectedSealCoverageReceipt? = nil,
    consultationPresentations: [CsProjectedConsultationPresentation] = []
  ) {
    nextProjectionSequence += 1
    let sequence = nextProjectionSequence
    let sampleStart = (sequence - 1) * 16_000
    let sampleEnd = sequence * 16_000
    let projectedPhase = phase ?? (terminal ? "formatted" : "listening")
    let receipt = CsProjectedAcousticReceipt(
      acousticSerialVersion: 1,
      acousticSerial: "test-acoustic-\(sequence)",
      sessionId: sessionId,
      captureEpoch: 1,
      sampleStart: sampleStart,
      sampleEnd: sampleEnd,
      durationMs: 1_000,
      energyIntegral: 1,
      meanRmsDbfs: -20,
      peakDbfs: -6,
      vadOpenSample: sampleStart,
      vadCloseSample: sampleEnd,
      evidenceCalibrationVersion: "test-v1",
      wordEvidenceReceipts: includesWordEvidence ? ["test-word-evidence-\(sequence)"] : [],
      layerDecisionReceipts: ["test-layer-decision-\(sequence)"],
      sealReceipt: terminal && projectedPhase != "coverage_refused" ? "test-seal-\(sequence)" : nil,
      manualEditReceipt: manualEditReceipt,
      presentationReceipt: nil
    )
    state.applyTranscriptProjection(
      CsTranscriptProjectionEvent(
        schema: "codescribe.transcript_projection.v1",
        sequence: sequence,
        emittedAt: "2026-08-25T00:00:00Z",
        sessionId: sessionId,
        mode: mode,
        reducerRevision: reducerRevision ?? sequence,
        reducerAction: reducerAction
          ?? (terminal
            ? (projectedPhase == "coverage_refused"
              ? "session_ended" : "record_ledger_terminal_seal")
            : "record_ledger_projection"),
        occurrenceSessionId: sessionId,
        captureEpoch: 1,
        sampleStart: sampleStart,
        sampleEnd: sampleEnd,
        documentIndex: sequence - 1,
        label: terminal ? "terminal" : "live",
        renderedText: text,
        deliveryText: nil,
        phase: projectedPhase,
        canPaste: canPaste,
        canInsert: canInsert,
        canCopy: canCopy ?? !text.isEmpty,
        canRetranscribe: canRetranscribe,
        canFormat: canFormat,
        canSendToAgent: canSendToAgent ?? (terminal && !text.isEmpty),
        terminal: terminal,
        lifecycleTerminal: lifecycleTerminal ?? (terminal && reducerAction != "apply_manual_edit"),
        delivery: delivery,
        acousticReceipts: [receipt],
        sealCoverage: sealCoverage,
        consultationPresentations: consultationPresentations
      )
    )
  }

  // W2 contracts: synthetic projections enter the production boundary; unrun
  // until the integrator restores Swift gates and regenerates the bindings.
  func testLiveConsultationProjectionPreservesGroupEvidenceWithoutEndingCapture() {
    let state = OverlayState()
    var ended: [String] = []
    state.onCaptureEnded = { ended.append($0) }
    let group = CsProjectedConsultationPresentation(
      receiptId: "group-receipt", consultationId: "Max", turnId: "turn-1",
      sourceRevision: 4, revision: 5,
      members: [
        CsProjectedConsultationMember(
          sessionId: "max-live", captureEpoch: 1, sampleStart: 0, sampleEnd: 16_000,
          sourceLabel: "Iwo", sealReceipt: "seal-1"),
        CsProjectedConsultationMember(
          sessionId: "max-live", captureEpoch: 1, sampleStart: 16_000, sampleEnd: 32_000,
          sourceLabel: "Iwo", sealReceipt: "seal-2"),
      ],
      renderedText: "git add -- 'plik ze spacją.rs'")
    let text = group.renderedText + " dalsze słowa"
    projectText(
      text, to: state, sessionId: "max-live", reducerRevision: 5,
      reducerAction: "apply_consultation_presentation", consultationPresentations: [group])
    XCTAssertEqual(state.latestTranscriptProjection?.consultationPresentations, [group])
    XCTAssertEqual(state.latestTranscriptProjection?.renderedText, text)
    XCTAssertEqual(state.revisionDraft, text)
    XCTAssertFalse(state.terminal)
    XCTAssertTrue(ended.isEmpty)
    projectText(
      text + " jutro", to: state, sessionId: "max-live", reducerRevision: 6,
      consultationPresentations: [group])
    XCTAssertEqual(state.latestTranscriptProjection?.consultationPresentations, [group])
    XCTAssertEqual(state.revisionDraft, text + " jutro")
    XCTAssertTrue(ended.isEmpty)
  }

  func testComposerCallbackCarriesTheProjectionSessionIdentity() {
    let state = OverlayState()
    var identities: [String] = []
    state.onComposerTranscript = { _, sessionID in
      identities.append(sessionID)
      return .admitted(threadID: UUID())
    }
    projectText(
      "same", to: state, terminal: true, delivery: .composerPending,
      sessionId: "A", reducerAction: "session_ended")
    projectText(
      "same", to: state, terminal: true, delivery: .composerPending,
      sessionId: "B", reducerAction: "session_ended")
    projectText(
      "same", to: state, terminal: true, delivery: .composerPending,
      sessionId: "A", reducerAction: "session_ended")
    XCTAssertEqual(identities, ["A", "B"])
    XCTAssertEqual(state.latestTranscriptProjection?.sessionId, "B")
  }

  func testMissingReceiverRecoverySurvivesNewSessionAndDeduplicatesIdentityOnly() {
    let state = OverlayState()
    let text = "  identical\t🙂  "
    for sessionID in ["A", "A", "B", "A"] {
      projectText(
        text, to: state, terminal: true, delivery: .composerPending,
        sessionId: sessionID, reducerAction: "session_ended")
    }
    XCTAssertEqual(state.retainedComposerDelivery, text + "\n" + text)
    state.onComposerTranscript = { _, _ in .admitted(threadID: UUID()) }
    projectText(
      text, to: state, terminal: true, delivery: .composerPending,
      sessionId: "A", reducerAction: "session_ended")
    XCTAssertEqual(state.retainedComposerDelivery, text)
    XCTAssertEqual(state.latestTranscriptProjection?.sessionId, "B")
  }

  func testFirstObservedComposerTerminalAdmitsExactBytesOnce() throws {
    for receipt in [
      ComposerDeliveryReceipt.admitted(threadID: UUID()),
      ComposerDeliveryReceipt.parked(threadID: UUID()),
    ] {
      let state = OverlayState()
      let text = "  Zażółć\nrepeat repeat\t🙂  "
      var received: [String] = []
      state.onComposerTranscript = { text, _ in
        received.append(text)
        return receipt
      }

      projectText(
        text, to: state, terminal: true, delivery: .composerPending,
        sessionId: "S", reducerAction: "session_ended")
      let terminal = try XCTUnwrap(state.latestTranscriptProjection)
      state.applyTranscriptProjection(terminal)

      XCTAssertEqual(received.count, 1)
      XCTAssertEqual(received.first.map { Array($0.utf8) }, Array(text.utf8))
      XCTAssertEqual(Array(state.activeText.utf8), Array(text.utf8))
      XCTAssertNil(state.retainedComposerDelivery)
    }
  }

  func testNewSessionFirstTerminalReplacesPriorReceiptWithoutDuplicateAdmission() throws {
    let state = OverlayState()
    var received: [String] = []
    state.onComposerTranscript = { text, _ in
      received.append(text)
      return .admitted(threadID: UUID())
    }
    projectText(
      "prior R", to: state, terminal: true, delivery: .composerPending,
      sessionId: "R", reducerAction: "session_ended")
    state.applyTranscriptProjection(try XCTUnwrap(state.latestTranscriptProjection))

    let text = "  new S\nS  "
    projectText(
      text, to: state, terminal: true, delivery: .composerPending,
      sessionId: "S", reducerAction: "session_ended")
    state.applyTranscriptProjection(try XCTUnwrap(state.latestTranscriptProjection))

    XCTAssertEqual(received, ["prior R", text])
    XCTAssertEqual(Array(state.activeText.utf8), Array(text.utf8))
    XCTAssertNil(state.retainedComposerDelivery)
  }

  func testFirstTerminalMissingOrRefusingReceiverRetainsBytesAndAllowsAdmissionRetry() throws {
    for hasPriorSession in [false, true] {
      for hasReceiver in [false, true] {
        let state = OverlayState()
        if hasPriorSession {
          state.onComposerTranscript = { _, _ in .admitted(threadID: UUID()) }
          projectText(
            "prior R", to: state, terminal: true, delivery: .composerPending,
            sessionId: "R", reducerAction: "session_ended")
        }
        var refused: [String] = []
        state.onComposerTranscript = nil
        if hasReceiver {
          state.onComposerTranscript = { text, _ in
            refused.append(text)
            return .retained(text)
          }
        }
        let text = "  refused S\nrepeat repeat\t🙂  "
        projectText(
          text, to: state, phase: "error", terminal: true,
          delivery: .composerPending, sessionId: "S", reducerAction: "session_ended")
        let terminal = try XCTUnwrap(state.latestTranscriptProjection)
        XCTAssertEqual(state.retainedComposerDelivery.map { Array($0.utf8) }, Array(text.utf8))
        state.applyTranscriptProjection(terminal)
        XCTAssertEqual(state.retainedComposerDelivery.map { Array($0.utf8) }, Array(text.utf8))
        XCTAssertEqual(refused, hasReceiver ? [text, text] : [])

        var admitted: [String] = []
        state.onComposerTranscript = { text, _ in
          admitted.append(text)
          return .admitted(threadID: UUID())
        }
        state.applyTranscriptProjection(terminal)
        state.applyTranscriptProjection(terminal)
        XCTAssertEqual(admitted, [text], "refusal must not consume admission")
        XCTAssertNil(state.retainedComposerDelivery)
      }
    }
  }

  func testFirstTerminalDisposesDeliveryBeforeStoppedClearsCaptureOwner() throws {
    for accepts in [false, true] {
      let state = OverlayState()
      // Capture callbacks can precede the first transcript projection.
      state.handleRecordingStarted()
      let owner = UUID()
      var captureOwner: UUID? = owner
      var order: [String] = []
      state.onComposerTranscript = { text, _ in
        XCTAssertEqual(captureOwner, owner)
        order.append("delivery")
        return accepts ? .admitted(threadID: owner) : .retained(text)
      }
      state.onRecordingStopped = { [weak state] in
        order.append("stopped")
        if !accepts { XCTAssertEqual(state?.retainedComposerDelivery, "S words") }
        captureOwner = nil
      }
      projectText(
        "S words", to: state, terminal: true, delivery: .composerPending,
        sessionId: "S", reducerAction: "session_ended")
      XCTAssertEqual(order, ["delivery", "stopped"])
      XCTAssertNil(captureOwner)
      if accepts {
        state.applyTranscriptProjection(try XCTUnwrap(state.latestTranscriptProjection))
        XCTAssertEqual(order, ["delivery", "stopped"])
      }
    }
  }

  func testDocumentRevisionBeforeLifecycleTerminalPreservesDeliveryAndStoppedOrder() throws {
    for manualReceipt in ["user-edit-test-1", "formatter-test-1"] {
      let state = OverlayState()
      state.handleRecordingStarted()
      let owner = UUID()
      var captureOwner: UUID? = owner
      var order: [String] = []
      state.onComposerTranscript = { text, _ in
        XCTAssertEqual(captureOwner, owner)
        XCTAssertEqual(text, "revised S")
        order.append("delivery")
        return .admitted(threadID: owner)
      }
      state.onRecordingStopped = {
        order.append("stopped")
        captureOwner = nil
      }
      projectText(
        "revised S", to: state, terminal: true, lifecycleTerminal: false,
        sessionId: "S", reducerAction: "apply_manual_edit", manualEditReceipt: manualReceipt)
      XCTAssertTrue(order.isEmpty)
      XCTAssertEqual(captureOwner, owner)
      XCTAssertTrue(state.audioReady, "a document revision does not release capture")
      XCTAssertEqual(state.activeText, "revised S")

      projectText(
        "revised S", to: state, terminal: true, delivery: .composerPending,
        sessionId: "S", reducerAction: "session_ended")
      state.applyTranscriptProjection(try XCTUnwrap(state.latestTranscriptProjection))
      XCTAssertEqual(order, ["delivery", "stopped"])
      XCTAssertNil(captureOwner)
      XCTAssertFalse(state.audioReady)
    }
  }

  func testFirstAndDuplicateComposerTerminalsNeverAutoSendAtElapsedDeadline() async throws {
    for receiverKind in ["missing", "refused", "admitted", "parked", "empty"] {
      let clock = OverlayStateTestClock()
      let engine = OverlayStateTestEngine()
      let state = OverlayState(nowProvider: { clock.now }, autoSendEnabled: { true })
      state.engine = engine
      state.applyIndicatorMode(.assistive)
      state.handleRecordingStarted()
      if receiverKind != "missing" {
        state.onComposerTranscript = { text, _ in
          switch receiverKind {
          case "admitted": return .admitted(threadID: UUID())
          case "parked": return .parked(threadID: UUID())
          case "empty": return .empty
          default: return .retained(text)
          }
        }
      }
      let text = receiverKind == "empty" ? "" : "S words"
      projectText(
        text, to: state, terminal: true, delivery: .composerPending,
        sessionId: "S", reducerAction: "session_ended")
      let terminal = try XCTUnwrap(state.latestTranscriptProjection)
      for _ in 0..<2 {
        clock.now += OverlayState.autoHideDelaySeconds
        state.fireAutoHideNowForTests()
        // Drain the MainActor task that an erroneous auto-send would enqueue.
        await Task { @MainActor in }.value
        XCTAssertTrue(engine.sentAssistiveTexts.isEmpty)
        state.applyTranscriptProjection(terminal)
      }
    }
  }

  func testEmptyAndNonComposerTerminalsDoNotConsumeComposerAdmission() throws {
    for delivery in [
      CsTranscriptDelivery.unattempted, .sinkAccepted, .retained, .composerPending,
      .copiedToClipboard, .deferredInsertArmed,
    ] {
      let state = OverlayState()
      var received: [String] = []
      state.onComposerTranscript = { text, _ in
        received.append(text)
        return .admitted(threadID: UUID())
      }
      let initialText = delivery == .composerPending ? " \n\t " : "sink words"
      projectText(
        initialText, to: state, terminal: true, delivery: delivery,
        sessionId: "S", reducerAction: "session_ended")
      state.applyTranscriptProjection(try XCTUnwrap(state.latestTranscriptProjection))
      XCTAssertTrue(received.isEmpty)
      XCTAssertNil(state.retainedComposerDelivery)

      projectText(
        "actual composer words", to: state, terminal: true, delivery: .composerPending,
        sessionId: "S", reducerAction: "session_ended")
      state.applyTranscriptProjection(try XCTUnwrap(state.latestTranscriptProjection))
      XCTAssertEqual(received, ["actual composer words"])
    }
  }

  /// A calibration outcome as the producer actually emits it: `session_id` is
  /// `None` by construction in `PresentationStatusProjection::calibration_*`,
  /// because it is a Settings-owned microphone result, not a capture verdict.
  private func calibrationFailure() -> CsPresentationStatusEvent {
    CsPresentationStatusEvent(
      schema: "codescribe.presentation-status.v1",
      emittedAt: "2026-09-05T00:00:00Z",
      sessionId: nil,
      kind: "calibration_failed",
      code: "calibration_failed",
      statusLabel: "calibration failed",
      headline: "Microphone calibration failed",
      message: "calibration_capture_failed: microphone disconnected",
      isError: true,
      terminal: true,
      calibrationVersion: nil
    )
  }

  private func refusalStatus(sessionId: String? = "refused-session")
    -> CsPresentationStatusEvent
  {
    CsPresentationStatusEvent(
      schema: "codescribe.presentation-status.v1",
      emittedAt: "2026-09-05T00:00:00Z",
      sessionId: sessionId,
      kind: "admission_refused",
      code: "admission_calibration_unusable",
      statusLabel: "recording blocked",
      headline: "Stored microphone calibration cannot be used",
      message:
        "capture generation changed: measured 88200Hz/1ch, current 48000Hz/1ch — Re-run Calibrate microphone in Settings › Audio.",
      isError: true,
      terminal: true,
      calibrationVersion: nil
    )
  }

  private func makeFinalizedState(
    clock: OverlayStateTestClock,
    text: String = "ready transcript"
  ) -> OverlayState {
    let state = OverlayState(nowProvider: { clock.now })
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText(text, to: state, terminal: true)
    state.finishControllerRecording()
    return state
  }

  func testOverlaySessionTimerTracksCaptureAndFreezesOnStop() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    XCTAssertNil(state.elapsedCaptureSeconds())
    XCTAssertEqual(state.sessionTimerText, "00:00")
    XCTAssertTrue(state.sessionTimerPaused)

    clock.now = 100
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertEqual(state.elapsedCaptureSeconds(), 0)
    XCTAssertFalse(state.sessionTimerPaused)

    clock.now = 165
    XCTAssertEqual(state.elapsedCaptureSeconds(), 65)
    XCTAssertEqual(state.sessionTimerText, "01:05")

    // Native finalising freezes the clock — the final pass must not tick.
    state.handleRecordingFinalising()
    XCTAssertTrue(state.sessionTimerPaused)
    clock.now = 200
    XCTAssertEqual(state.elapsedCaptureSeconds(), 65)
    state.finishControllerRecording()
    XCTAssertEqual(state.elapsedCaptureSeconds(), 65)

    // A fresh session restarts from zero and formats hours past 59:59.
    clock.now = 300
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertEqual(state.elapsedCaptureSeconds(), 0)
    XCTAssertFalse(state.sessionTimerPaused)
    clock.now = 3900
    XCTAssertEqual(state.sessionTimerText, "1:00:00")
    XCTAssertTrue(state.showsSessionTimer)
  }

  // IDLE-1: authored under W1; execution belongs to the integrator after close.
  func testCaretActivityStopsWithCaptureEvenWhenProjectionStillSaysListening() {
    let state = OverlayState()
    state.toggleCollapsed()
    XCTAssertFalse(state.animatesTranscriptCaret)
    state.handleRecordingPreparing()
    XCTAssertFalse(state.animatesTranscriptCaret, "Warmup has no live audio yet")
    state.handleRecordingStarted()
    XCTAssertTrue(state.animatesTranscriptCaret)

    state.toggleCollapsed()
    XCTAssertFalse(state.animatesTranscriptCaret, "Clipped content stays mounted")
    state.toggleCollapsed()
    XCTAssertTrue(state.animatesTranscriptCaret)

    projectText("streaming", to: state, phase: "finalizing")
    state.handleRecordingFinalising()
    XCTAssertTrue(state.animatesTranscriptCaret, "Final text can still stream")
    XCTAssertTrue(state.sessionTimerPaused, "Capture duration is already frozen")

    state.finishControllerRecording()
    XCTAssertFalse(state.animatesTranscriptCaret)
    XCTAssertTrue(state.sessionTimerPaused)

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertTrue(state.animatesTranscriptCaret, "The next take restarts motion")
    XCTAssertFalse(state.sessionTimerPaused)
    state.finishControllerRecording()
    XCTAssertEqual(state.mode, .listening)
    XCTAssertFalse(state.animatesTranscriptCaret, "A stale phase is not live capture")
  }

  func testTerminalOverlayPhasesNeverKeepCaretOrTimerRunning() {
    for phase in ["formatted", "no_speech", "error", "coverage_refused"] {
      let clock = OverlayStateTestClock()
      let state = OverlayState(nowProvider: { clock.now })
      state.toggleCollapsed()
      state.handleRecordingPreparing()
      state.handleRecordingStarted()
      XCTAssertTrue(state.animatesTranscriptCaret)
      clock.now += 12
      projectText("kept words", to: state, phase: phase, terminal: true)

      XCTAssertFalse(state.animatesTranscriptCaret, phase)
      XCTAssertTrue(state.sessionTimerPaused, phase)
      XCTAssertTrue(state.showsSessionTimer, "Frozen duration remains readable")
      let frozen = state.sessionTimerText
      clock.now += 10
      XCTAssertEqual(state.sessionTimerText, frozen, phase)
    }
  }

  func testErrorBeforeTerminalProjectionStopsIdleRenderActivity() {
    let state = OverlayState()
    state.toggleCollapsed()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.handleError(message: "Capture failed")
    XCTAssertFalse(state.animatesTranscriptCaret)
    XCTAssertTrue(state.sessionTimerPaused)
  }

  func testCanvasPreservesEmptyAndExactEngineTextWithoutLifecyclePlaceholders() {
    let state = OverlayState()
    XCTAssertEqual(state.activeText, "")
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertEqual(state.activeText, "")

    let text = "  raz raz\nZażółć — e\u{301} 👩‍💻  "
    projectText(text, to: state)
    XCTAssertEqual(Array(state.activeText.utf8), Array(text.utf8))
    state.handleRecordingFinalising()
    XCTAssertEqual(Array(state.activeText.utf8), Array(text.utf8))

    projectText("", to: state)
    XCTAssertEqual(state.activeText, "")
  }

  func testUnknownChromePhaseCannotRejectEngineText() {
    let state = OverlayState()
    let text = "  raz raz\nZażółć — e\u{301} 👩‍💻  "
    projectText(text, to: state, phase: "future_engine_phase")
    XCTAssertEqual(Array(state.activeText.utf8), Array(text.utf8))
    projectText("", to: state, phase: "future_engine_phase", terminal: true)
    XCTAssertEqual(state.activeText, "")
    XCTAssertTrue(state.terminal)
  }

  func testCanonicalProjectionOwnsCanvasAndCopyFromFirstAdmittedRevision() {
    let state = OverlayState()
    XCTAssertFalse(state.canCopy)
    XCTAssertFalse(state.showsSessionTimer)

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertTrue(state.showsSessionTimer)

    projectText("analyze the repo", to: state)
    XCTAssertTrue(state.canCopy)
    XCTAssertEqual(state.activeText, "analyze the repo")
    XCTAssertEqual(state.activeText, "analyze the repo")

    projectText("analyze the repo for duplicate dispatch", to: state)
    XCTAssertTrue(state.canCopy)
    XCTAssertEqual(state.activeText, "analyze the repo for duplicate dispatch")
    XCTAssertEqual(state.activeText, "analyze the repo for duplicate dispatch")
  }

  func testAdmittedProjectionPaintsWithoutSwiftRevalidatingReceipts() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    projectText("unproven shadow", to: state, includesWordEvidence: false)

    XCTAssertTrue(state.canCopy)
    XCTAssertEqual(state.activeText, "unproven shadow")
    XCTAssertEqual(state.mode, .listening)
  }

  func testLatestProjectionIsPaintedWithoutASecondSequenceReducer() {
    let state = OverlayState()
    var successfulSignals = 0
    state.onSuccessfulDictation = { successfulSignals += 1 }
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    projectText("sealed document", to: state, terminal: true)
    projectText("late competing document", to: state)

    XCTAssertEqual(state.activeText, "late competing document")
    XCTAssertEqual(state.formattedText, "late competing document")
    XCTAssertEqual(state.mode, .listening)
    XCTAssertEqual(successfulSignals, 1)
  }

  func testPresencePolicyRisesForScreenshotAndYieldsToAlerts() {
    XCTAssertEqual(
      OverlayPresencePolicy.resolve(screenshotChord: true, shouldYield: true),
      .capture
    )
    XCTAssertEqual(
      OverlayPresencePolicy.resolve(screenshotChord: false, shouldYield: true),
      .yield
    )
    XCTAssertEqual(
      OverlayPresencePolicy.resolve(screenshotChord: false, shouldYield: false),
      .rest
    )
    XCTAssertTrue(
      OverlayPresencePolicy.shouldYield(
        frontmostBundleId: "com.apple.SecurityAgent",
        modalWindowPresent: false
      )
    )
    XCTAssertFalse(
      OverlayPresencePolicy.shouldYield(
        frontmostBundleId: "com.apple.Terminal",
        modalWindowPresent: false
      )
    )
    XCTAssertTrue(
      OverlayPresencePolicy.shouldYield(
        frontmostBundleId: "com.apple.Terminal",
        modalWindowPresent: true
      )
    )
  }

  func testOverlayPolicyRefreshesAtSessionEntryFromPersistedTruth() {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    engine.persistedPolicy = OverlayPolicySnapshot(
      autoPasteEnabled: false,
      autoFormatLevel: .off
    )
    state.engine = engine

    state.handleRecordingPreparing()
    XCTAssertFalse(state.autoPasteEnabled)
    XCTAssertEqual(state.autoFormatLevel, .off)
    XCTAssertEqual(engine.policyReadCount, 1)

    engine.persistedPolicy = OverlayPolicySnapshot(
      autoPasteEnabled: true,
      autoFormatLevel: .max
    )
    state.handleRecordingStarted()
    XCTAssertTrue(state.autoPasteEnabled)
    XCTAssertEqual(state.autoFormatLevel, .max)
    XCTAssertEqual(engine.policyReadCount, 2)
  }

  func testAutoPasteWriteReconcilesSuccessAndFailureWithoutDelivery() {
    for persists in [true, false] {
      let state = OverlayState()
      let engine = OverlayStateTestEngine()
      engine.persistedPolicy = OverlayPolicySnapshot(
        autoPasteEnabled: false,
        autoFormatLevel: .off
      )
      engine.persistAutoPasteWrites = persists
      state.engine = engine
      state.handleRecordingPreparing()

      state.setAutoPasteEnabled(true)

      XCTAssertEqual(engine.autoPasteWrites, [true])
      XCTAssertEqual(state.autoPasteEnabled, persists)
      XCTAssertEqual(state.autoFormatLevel, .off)
      XCTAssertEqual(engine.policyReadCount, 2)
      XCTAssertEqual(engine.pasteCallCount, 0)
    }
  }

  func testAssistiveFenceMakesAutoPasteControlUnavailableAndNonWriting() {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    state.engine = engine
    state.setAutoPasteControlAvailable(false)

    state.setAutoPasteEnabled(false)

    XCTAssertFalse(state.autoPasteControlAvailable)
    XCTAssertTrue(engine.autoPasteWrites.isEmpty)
    XCTAssertEqual(engine.pasteCallCount, 0)
  }

  func testAudioLevelMeterOrdersFiniteEnergyAndRejectsInvalidInput() throws {
    let meter = AudioLevelMeter()
    XCTAssertNil(meter.gain)

    meter.push(rms: 0)
    let silence = try XCTUnwrap(meter.gain)
    meter.reset()
    meter.push(rms: 0.01)
    let quiet = try XCTUnwrap(meter.gain)
    meter.reset()
    meter.push(rms: 0.8)
    let loud = try XCTUnwrap(meter.gain)

    XCTAssertTrue(silence.isFinite && quiet.isFinite && loud.isFinite)
    XCTAssertLessThan(silence, quiet)
    XCTAssertLessThan(quiet, loud)

    meter.reset()
    meter.push(rms: .nan)
    XCTAssertNil(meter.gain)
  }

  func testNoMeasuredLevelRemainsExplicitAndDoesNotClaimAudioEvidence() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    XCTAssertNil(state.levelMeter.gain)
    XCTAssertFalse(state.hasMeasuredAudioLevel)
    XCTAssertEqual(state.statusText, "listening")
    XCTAssertEqual(state.audioLevelAccessibilityValue, "Waiting for measured level")
  }

  func testMeasuredAudioLevelHasAnHonestAccessibilityValue() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    state.applyAudioLevel(0.0001)
    XCTAssertEqual(state.audioLevelAccessibilityValue, "Very quiet")

    state.applyAudioLevel(0.8)
    state.applyAudioLevel(0.8)
    XCTAssertEqual(state.audioLevelAccessibilityValue, "Strong level")
  }

  func testSuccessfulDictationSignalFiresOnceAndNeverForNoSpeech() {
    let successful = OverlayState()
    var successfulSignals = 0
    successful.onSuccessfulDictation = { successfulSignals += 1 }
    successful.handleRecordingPreparing()
    successful.handleRecordingStarted()
    projectText("activation without payload", to: successful, terminal: true)
    successful.finishControllerRecording()
    successful.finishControllerRecording()
    XCTAssertEqual(successfulSignals, 1)

    let silent = OverlayState()
    var silentSignals = 0
    silent.onSuccessfulDictation = { silentSignals += 1 }
    silent.handleRecordingPreparing()
    silent.handleRecordingStarted()
    silent.applyNoSpeech(reason: "no_speech_detected")
    projectText("", to: silent, phase: "no_speech", terminal: true)
    silent.finishControllerRecording()
    XCTAssertEqual(silentSignals, 0)
  }

  func testAudioLevelLifecycleDropsLateSamplesAndResets() {
    let state = OverlayState()

    state.applyAudioLevel(0.8)
    XCTAssertNil(state.levelMeter.gain, "levels before capture must be ignored")

    state.handleRecordingPreparing()
    state.applyAudioLevel(0.2)
    state.handleRecordingStarted()
    XCTAssertNotNil(state.levelMeter.gain)
    XCTAssertTrue(state.hasMeasuredAudioLevel)
    XCTAssertEqual(state.statusText, "listening")

    state.handleRecordingFinalising()
    XCTAssertNil(state.levelMeter.gain)
    XCTAssertFalse(state.hasMeasuredAudioLevel)

    state.applyAudioLevel(0.9)
    XCTAssertNil(state.levelMeter.gain, "late levels during finalisation must be ignored")

    state.finishControllerRecording()
    state.applyAudioLevel(0.9)
    XCTAssertNil(state.levelMeter.gain, "late levels after finalisation must be ignored")

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertNil(state.levelMeter.gain, "a new session must not inherit old amplitude")
    XCTAssertEqual(state.statusText, "listening")
  }

  func testControllerStopDoesNotInventAProjectionPhase() {
    let state = OverlayState()
    var stoppedCallbacks = 0
    state.onRecordingStopped = { stoppedCallbacks += 1 }

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.applyAudioLevel(0.7)
    state.applyVad(true)
    state.handleRecordingFinalising()
    state.finishControllerRecording()

    XCTAssertEqual(state.mode, .listening)
    XCTAssertEqual(state.statusText, "listening")
    XCTAssertNil(state.errorMessage)
    XCTAssertFalse(state.warmingUp)
    XCTAssertFalse(state.transcribing)
    XCTAssertFalse(state.audioReady)
    XCTAssertFalse(state.vadActive)
    XCTAssertFalse(state.isFinalPass)
    XCTAssertFalse(state.hasMeasuredAudioLevel)
    XCTAssertNil(state.levelMeter.gain)
    XCTAssertEqual(stoppedCallbacks, 1)

    state.finishControllerRecording()
    XCTAssertEqual(stoppedCallbacks, 1, "terminal recovery must be idempotent")
  }

  func testNoSpeechSidebandDoesNotReplaceProjectedPhase() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.applyNoSpeech(reason: "no_speech_detected")

    state.finishControllerRecording()
    XCTAssertEqual(state.mode, .listening)

    projectText("", to: state, phase: "no_speech", terminal: true)

    XCTAssertEqual(state.mode, .noSpeech)
    XCTAssertEqual(state.statusText, "no speech")
    XCTAssertEqual(state.noSpeechNotice, OverlayState.defaultNoSpeechNotice)
  }

  func testProjectionOwnsFinalizingAndFormattedPhases() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("captured text", to: state)

    state.handleRecordingFinalising()
    XCTAssertEqual(state.statusText, "listening")

    projectText("captured text", to: state, phase: "finalizing")
    XCTAssertEqual(state.statusText, "finalizing")

    state.applySessionFinalised()
    XCTAssertEqual(state.mode, .finalizing)
    XCTAssertEqual(state.statusText, "finalizing")

    projectText("captured text", to: state, terminal: true)
    state.finishControllerRecording()
    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(state.statusText, "formatted")
    XCTAssertEqual(state.formattedText, "captured text")
  }

  func testFailurePhaseIsExplicit() {
    let state = OverlayState()

    state.handleError(message: "engine unavailable")
    XCTAssertEqual(state.mode, .listening)

    projectText("", to: state, phase: "error", terminal: true)

    XCTAssertEqual(state.mode, .error)
    XCTAssertEqual(state.statusText, "error")
  }

  func testAutoHideDelayIsFiveSeconds() {
    XCTAssertEqual(OverlayState.autoHideDelaySeconds, 5)
  }

  func testPinnedTerminalDoesNotArmOrHonorAnAutoHideWake() {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now })
    state.engine = engine
    state.setKeepVisibleBetweenTakes(true)
    var closes = 0
    state.onClose = { closes += 1 }

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText("sealed", sessionId: "pin-take", sequence: 1, to: state, terminal: true)
    XCTAssertNil(state.autoHideDeadline)
    clock.now += OverlayState.autoHideDelaySeconds + 1
    state.fireAutoHideNowForTests(armedDeadline: 0)
    XCTAssertNil(state.autoHideDeadline)
    XCTAssertEqual(closes, 0)

    state.relayIntent(.close)
    XCTAssertEqual(closes, 1)
    XCTAssertTrue(state.keepVisibleBetweenTakes)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertTrue(state.keepVisibleBetweenTakes)
  }

  func testUnpinRestoresFiveSecondCountdown() throws {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now })
    state.engine = engine
    state.setKeepVisibleBetweenTakes(true)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText("sealed", sessionId: "unpin-take", sequence: 1, to: state, terminal: true)
    state.setKeepVisibleBetweenTakes(false)
    XCTAssertEqual(try XCTUnwrap(state.autoHideDeadline), clock.now + 5)
  }

  func testInjectedClockFiresFiveSecondsAfterFinalization() {
    let clock = OverlayStateTestClock()
    let state = makeFinalizedState(clock: clock)
    var closeCount = 0
    state.onClose = { closeCount += 1 }

    clock.now = 4.9
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 0)

    clock.now = 5
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 1)
  }

  func testWindowDragReanchorsAutoHide() {
    let clock = OverlayStateTestClock()
    let state = makeFinalizedState(clock: clock)
    var closeCount = 0
    state.onClose = { closeCount += 1 }

    clock.now = 4
    state.userDraggedOverlay()
    clock.now = 5
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 0)

    clock.now = 9
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 1)
  }

  func testWindowResizeReanchorsAutoHide() {
    let clock = OverlayStateTestClock()
    let state = makeFinalizedState(clock: clock)
    var closeCount = 0
    state.onClose = { closeCount += 1 }

    clock.now = 4
    state.userResizedOverlay()
    clock.now = 5
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 0)

    clock.now = 9
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 1)
  }

  func testHoverPausesAndPointerExitStartsFreshCountdown() {
    let clock = OverlayStateTestClock()
    let state = makeFinalizedState(clock: clock)
    var closeCount = 0
    state.onClose = { closeCount += 1 }

    clock.now = 4
    state.setPointerHovering(true)
    clock.now = 100
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 0)

    state.setPointerHovering(false)
    clock.now = 104.9
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 0)
    clock.now = 105
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 1)
  }

  func testCopyKeepsOverlayVisibleAndRearmsAutoHide() {
    let clock = OverlayStateTestClock()
    let state = makeFinalizedState(clock: clock)
    let pasteboard = NSPasteboard(
      name: NSPasteboard.Name("codescribe.tests.overlay.\(UUID().uuidString)")
    )
    var closeCount = 0
    state.onClose = { closeCount += 1 }

    clock.now = 4
    state.copyToPasteboard(pasteboard)
    XCTAssertEqual(closeCount, 0)
    XCTAssertEqual(pasteboard.string(forType: .string), "ready transcript")

    clock.now = 5
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 0)
    clock.now = 9
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 1)
  }

  func testPasteUsesLatestProjectedTextKeepsOverlayVisibleAndRearmsAutoHide() async {
    let clock = OverlayStateTestClock()
    let state = makeFinalizedState(clock: clock, text: "original delivered transcript here")
    let engine = OverlayStateTestEngine()
    let pasteCalled = expectation(description: "paste called")
    engine.onPaste = { pasteCalled.fulfill() }
    var closeCount = 0
    state.engine = engine
    state.onClose = { closeCount += 1 }
    state.insertCaretInCodescribeProbe = { false }
    projectText("newest projected transcript", to: state, terminal: true, lifecycleTerminal: false)

    clock.now = 4
    state.pasteToPreviousApp()
    await fulfillment(of: [pasteCalled], timeout: 1)
    await Task.yield()

    XCTAssertEqual(engine.pastedText, "newest projected transcript")
    XCTAssertEqual(closeCount, 0)
    clock.now = 5
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 0)
    clock.now = 9
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 1)
  }

  func testInsertArmsDeferredSlotWithoutCopyWhenCaretIsInCodescribe() async {
    let clock = OverlayStateTestClock()
    let state = makeFinalizedState(clock: clock, text: "guarded transcript")
    let engine = OverlayStateTestEngine()
    let deferCalled = expectation(description: "deferred insert armed")
    engine.onDefer = { deferCalled.fulfill() }
    engine.pasteTargetAppNameValue = "Pensieve"
    state.engine = engine
    state.insertCaretInCodescribeProbe = { true }

    state.pasteToPreviousApp()
    await fulfillment(of: [deferCalled], timeout: 1)
    await Task.yield()

    XCTAssertEqual(engine.deferredText, "guarded transcript")
    XCTAssertNil(engine.copiedTaggedText)
    XCTAssertNil(engine.pastedText, "guard must not fall through to synthetic paste")
    XCTAssertEqual(state.toast, "⌘⌥V")
  }

  func testInsertFallsBackToTaggedCopyWhenHotkeyRegistrationFails() async {
    let clock = OverlayStateTestClock()
    let state = makeFinalizedState(clock: clock, text: "fallback transcript")
    let engine = OverlayStateTestEngine()
    engine.deferOutcome = .copiedToClipboard
    engine.deferredInsertFailureValue = "Paste Here hotkey registration failed"
    engine.pasteTargetAppNameValue = "Pensieve"
    let deferCalled = expectation(description: "deferred insert fallback")
    engine.onDefer = { deferCalled.fulfill() }
    state.engine = engine
    state.insertCaretInCodescribeProbe = { true }

    state.pasteToPreviousApp()
    await fulfillment(of: [deferCalled], timeout: 1)
    await Task.yield()

    XCTAssertEqual(state.toast, "copied")
  }

  func testInsertShowsCopiedToastWhenControllerGuardDegrades() async {
    let clock = OverlayStateTestClock()
    let state = makeFinalizedState(clock: clock, text: "belt and braces transcript")
    let engine = OverlayStateTestEngine()
    engine.pasteOutcome = .copiedToClipboard
    engine.deferredInsertShortcutValue = nil
    engine.pasteTargetAppNameValue = "Pensieve"
    engine.pasteFrontmostAppNameValue = "Alacritty"
    let pasteCalled = expectation(description: "paste called")
    engine.onPaste = { pasteCalled.fulfill() }
    state.engine = engine
    state.insertCaretInCodescribeProbe = { false }

    state.pasteToPreviousApp()
    await fulfillment(of: [pasteCalled], timeout: 1)
    await Task.yield()

    XCTAssertEqual(engine.pastedText, "belt and braces transcript")
    XCTAssertEqual(state.toast, "copied")
  }

  func testStopCopyShowsPassiveFooterWithoutChangingTerminalState() {
    for delivery in [
      CsTranscriptDelivery.copiedToClipboard, .sinkAccepted, .deferredInsertArmed,
    ] {
      let state = OverlayState()
      let engine = OverlayStateTestEngine()
      state.engine = engine
      state.handleRecordingStarted()
      var ended: [String] = []
      var stopped = 0
      state.onCaptureEnded = { ended.append($0) }
      state.onRecordingStopped = { stopped += 1 }

      projectText(
        "stop transcript", to: state, canPaste: true, canInsert: true,
        canCopy: true, canRetranscribe: true, canFormat: true, canSendToAgent: true,
        terminal: true, delivery: delivery, reducerAction: "session_ended")

      XCTAssertEqual(state.toast, delivery == .copiedToClipboard ? "copied" : nil)
      XCTAssertNil(state.errorMessage)
      XCTAssertEqual(state.mode, .formatted)
      XCTAssertTrue(state.terminal)
      XCTAssertTrue(state.finalized)
      XCTAssertFalse(state.recording)
      XCTAssertTrue(state.canPaste)
      XCTAssertTrue(state.canInsert)
      XCTAssertTrue(state.canCopy)
      XCTAssertTrue(state.canRetranscribe)
      XCTAssertTrue(state.canFormat)
      XCTAssertTrue(state.canSendToAgent)
      XCTAssertEqual(state.activeText, "stop transcript")
      XCTAssertEqual(ended, ["overlay-state-tests"])
      XCTAssertEqual(stopped, 1)
      XCTAssertEqual(engine.pasteCallCount, 0)
      XCTAssertNil(engine.deferredText)
      XCTAssertNil(engine.copiedTaggedText)
    }
  }

  func testStopCopyFooterIgnoresRevisionReplayAndRetiredTake() throws {
    let state = OverlayState()
    projectText(
      "stop transcript", to: state, terminal: true, lifecycleTerminal: false,
      delivery: .copiedToClipboard, sessionId: "old", reducerAction: "apply_manual_edit")
    XCTAssertNil(state.toast, "document revision is not stop delivery")

    projectText(
      "stop transcript", to: state, terminal: true, delivery: .copiedToClipboard,
      sessionId: "old", reducerAction: "session_ended")
    XCTAssertEqual(state.toast, "copied")
    let copiedTerminal = try XCTUnwrap(state.latestTranscriptProjection)
    state.showFooterNotice("new notice", persists: true)
    state.applyTranscriptProjection(copiedTerminal)
    XCTAssertEqual(state.toast, "new notice", "replay cannot restart the copied notice")

    projectText("new capture", to: state, sessionId: "new")
    state.showFooterNotice("current notice", persists: true)
    projectText(
      "old transcript", to: state, terminal: true, delivery: .copiedToClipboard,
      sessionId: "old", reducerAction: "session_ended")
    XCTAssertEqual(state.toast, "current notice")
    XCTAssertEqual(state.activeText, "new capture")
    XCTAssertFalse(state.finalized)
  }

  func testInsertShowsAccessibilityPermissionToastWhenEventPostingDenied() async {
    let clock = OverlayStateTestClock()
    let state = makeFinalizedState(clock: clock, text: "permission transcript")
    let engine = OverlayStateTestEngine()
    engine.pasteOutcome = .accessibilityPermissionNeeded
    engine.deferredInsertFailureValue = "Paste Here hotkey registration failed"
    engine.pasteTargetAppNameValue = "Pensieve"
    engine.pasteFrontmostAppNameValue = "Pensieve"
    let pasteCalled = expectation(description: "permission fallback called")
    engine.onPaste = { pasteCalled.fulfill() }
    state.engine = engine
    state.insertCaretInCodescribeProbe = { false }

    state.pasteToPreviousApp()
    await fulfillment(of: [pasteCalled], timeout: 1)
    await Task.yield()

    XCTAssertEqual(engine.pastedText, "permission transcript")
    XCTAssertEqual(state.toast, "no ax")
  }

  func testRailCopyRelaysToControllerWithoutOptimisticProjectionMutation() async {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    state.engine = engine
    projectText(
      "projected transcript",
      to: state,
      phase: "formatted",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    let copied = expectation(description: "copy intent reached controller")
    engine.onCopyTagged = { copied.fulfill() }

    state.relayIntent(.copy)
    await fulfillment(of: [copied], timeout: 1)

    XCTAssertEqual(engine.copiedTaggedText, "projected transcript")
    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(state.formattedText, "projected transcript")
    XCTAssertEqual(state.revision, 1)
    XCTAssertTrue(state.canPaste)
    XCTAssertTrue(state.canInsert)
    XCTAssertTrue(state.canCopy)
    XCTAssertTrue(state.canRetranscribe)
    XCTAssertTrue(state.canFormat)
    XCTAssertTrue(state.terminal)
    XCTAssertEqual(state.toast, "copied")
  }

  func testRailInsertRelaysToControllerWithoutOptimisticProjectionMutation() async {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    engine.pasteOutcome = .accessibilityPermissionNeeded
    state.engine = engine
    state.insertCaretInCodescribeProbe = { false }
    projectText(
      "projected transcript",
      to: state,
      phase: "formatted",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    let pasted = expectation(description: "insert intent reached controller")
    engine.onPaste = { pasted.fulfill() }

    state.relayIntent(.insertPaste)
    await fulfillment(of: [pasted], timeout: 1)

    XCTAssertEqual(engine.pastedText, "projected transcript")
    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(state.formattedText, "projected transcript")
    XCTAssertEqual(state.revision, 1)
    XCTAssertTrue(state.canPaste)
    XCTAssertTrue(state.canInsert)
    XCTAssertTrue(state.canCopy)
    XCTAssertTrue(state.canRetranscribe)
    XCTAssertTrue(state.canFormat)
    XCTAssertTrue(state.terminal)
    XCTAssertEqual(state.toast, "no ax")
    XCTAssertNil(state.errorMessage)
  }

  func testEngineRevisionReplacesTerminalDocumentWithoutALocalEdit() {
    let state = OverlayState()
    projectText("Tekst bazowy", to: state, terminal: true, reducerRevision: 7)
    let text = "  Tekst poprawiony\nraz raz  "
    projectText(
      text, to: state, canCopy: true, terminal: true, reducerRevision: 8,
      reducerAction: "apply_manual_edit", manualEditReceipt: "user-edit-test-7-8")
    XCTAssertEqual(Array(state.activeText.utf8), Array(text.utf8))
    XCTAssertEqual(state.revision, 8)
    XCTAssertEqual(state.userRevisionProvenance, "user-edit-test-7-8")
    XCTAssertEqual(OverlayIntentRail.projectedIntents(for: state), [.copy, .sendToAgent, .close])
  }

  func testUserEditCommitsOnlyThroughReturnedRustProjection() async {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    state.engine = engine
    projectText(
      "Tekst bazowy",
      to: state,
      canPaste: true,
      canInsert: true,
      canCopy: true,
      terminal: true,
      sessionId: "revision-session",
      reducerRevision: 7
    )
    XCTAssertTrue(state.isTranscriptEditable, "a sealed formatted take is the editor")
    XCTAssertEqual(state.canvasText, "Tekst bazowy")

    state.beginTranscriptEdit()
    XCTAssertTrue(state.isEditingTranscript)
    state.updateRevisionDraft("Tekst poprawiony")
    XCTAssertTrue(state.isRevisionDraftDirty)
    XCTAssertEqual(state.canvasText, "Tekst poprawiony", "the canvas paints the local draft")
    XCTAssertEqual(state.formattedText, "Tekst bazowy", "typing never touches projected truth")
    XCTAssertEqual(state.activeText, "Tekst bazowy", "delivery never reads the draft")
    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: state),
      [.commitRevision, .discardRevision, .close]
    )

    let requested = expectation(description: "revision intent reached Rust bridge")
    engine.onRevision = { requested.fulfill() }
    state.relayIntent(.commitRevision)
    await fulfillment(of: [requested], timeout: 1)

    XCTAssertEqual(
      engine.revisionRequests,
      [
        OverlayStateTestEngine.RevisionRequest(
          sessionId: "revision-session",
          sourceRevision: 7,
          renderedText: "Tekst poprawiony"
        )
      ]
    )
    XCTAssertEqual(state.formattedText, "Tekst bazowy", "FFI acknowledgement is not projection")
    XCTAssertEqual(
      state.canvasText, "Tekst poprawiony", "draft stays visible until the ledger answers")
    XCTAssertEqual(state.revision, 7)
    XCTAssertTrue(state.revisionCommitPending)
    XCTAssertFalse(state.isTranscriptEditable, "no second edit while one is in flight")
    XCTAssertEqual(OverlayIntentRail.projectedIntents(for: state), [])

    projectText(
      "Tekst poprawiony",
      to: state,
      canPaste: true,
      canInsert: true,
      canCopy: true,
      terminal: true,
      sessionId: "revision-session",
      reducerRevision: 8,
      reducerAction: "apply_manual_edit",
      manualEditReceipt: "user-edit-revision-session-7-8-1"
    )

    XCTAssertEqual(state.formattedText, "Tekst poprawiony")
    XCTAssertEqual(state.revisionDraft, "Tekst poprawiony")
    XCTAssertFalse(state.isRevisionDraftDirty)
    XCTAssertEqual(state.revision, 8)
    XCTAssertFalse(state.revisionCommitPending)
    XCTAssertTrue(state.isTranscriptEditable, "the new revision is editable again")
    XCTAssertEqual(state.userRevisionProvenance, "user-edit-revision-session-7-8-1")
    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: state),
      [.insertPaste, .copy, .sendToAgent, .close],
      "delivery actions return only after the new ledger projection"
    )

    state.insertCaretInCodescribeProbe = { false }
    let pasted = expectation(description: "new revision reached delivery")
    engine.onPaste = { pasted.fulfill() }
    state.relayIntent(.insertPaste)
    await fulfillment(of: [pasted], timeout: 1)
    XCTAssertEqual(engine.pastedText, "Tekst poprawiony")
  }

  func testFocusExitCommitsDirtyDraftAfterClickGrace() async {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    state.engine = engine
    projectText("Ledger text", to: state, terminal: true, reducerRevision: 3)

    state.beginTranscriptEdit()
    state.updateRevisionDraft("Focus-exit draft")
    state.endTranscriptEdit()
    XCTAssertFalse(state.isEditingTranscript)
    XCTAssertTrue(engine.revisionRequests.isEmpty, "a focus exit waits one click before FFI")

    let requested = expectation(description: "focus-exit commit crossed the bridge")
    engine.onRevision = { requested.fulfill() }
    await fulfillment(of: [requested], timeout: 2)
    XCTAssertEqual(engine.revisionRequests.map(\.renderedText), ["Focus-exit draft"])
    XCTAssertTrue(state.revisionCommitPending)
    XCTAssertEqual(state.formattedText, "Ledger text", "still the ledger's bytes until projection")
  }

  func testDiscardAndCloseCancelDraftWithoutCreatingRevision() async {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    state.engine = engine
    projectText("Ledger text", to: state, terminal: true, reducerRevision: 3)

    // Escape: the canvas discards, then resigns.
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Escaped draft")
    state.discardRevisionDraft()
    state.endTranscriptEdit()
    XCTAssertEqual(state.revisionDraft, "Ledger text")
    XCTAssertEqual(state.canvasText, "Ledger text")

    // Focus exit, then an explicit Discard inside the grace window.
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Focus-exit draft")
    state.endTranscriptEdit()
    state.discardRevisionDraft()
    XCTAssertEqual(state.revisionDraft, "Ledger text")

    // Close with a dirty draft.
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Close draft")
    state.close()
    XCTAssertEqual(state.formattedText, "Ledger text")
    XCTAssertEqual(state.revisionDraft, "Ledger text")

    try? await Task.sleep(
      nanoseconds: OverlayState.focusExitCommitGraceNanoseconds + 200_000_000)
    XCTAssertTrue(
      engine.revisionRequests.isEmpty,
      "discard and close must cancel every deferred focus commit"
    )
    XCTAssertFalse(state.revisionCommitPending)
  }

  func testListeningCanvasIsReadOnlyAndTheDraftFollowsProjection() {
    let state = OverlayState()
    projectText("live words", to: state)
    XCTAssertFalse(state.isTranscriptEditable)
    state.beginTranscriptEdit()
    XCTAssertFalse(state.isEditingTranscript, "a live take never takes the keyboard")
    state.updateRevisionDraft("typed into a live take")
    XCTAssertEqual(state.canvasText, "live words")
    XCTAssertFalse(state.isRevisionDraftDirty)

    projectText("live words and more", to: state)
    XCTAssertEqual(state.canvasText, "live words and more")
    projectText("sealed words", to: state, terminal: true)
    XCTAssertTrue(state.isTranscriptEditable)
    XCTAssertEqual(state.revisionDraft, "sealed words", "the seal seeds the draft")
    XCTAssertEqual(state.canvasText, "sealed words")
  }

  func testEditingAndDirtyDraftHoldAutoHide() {
    let clock = OverlayStateTestClock()
    let state = makeFinalizedState(clock: clock)
    let engine = OverlayStateTestEngine()
    state.engine = engine
    var closeCount = 0
    state.onClose = { closeCount += 1 }

    clock.now = 1
    state.beginTranscriptEdit()
    clock.now = 100
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 0, "a caret in the canvas holds the panel")

    state.updateRevisionDraft("ready transcript, corrected")
    state.discardRevisionDraft()
    state.endTranscriptEdit()
    clock.now = 101
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 0)
    clock.now = 200
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 1, "a clean canvas re-arms the usual countdown")
  }

  func testOneShotFormatterForwardsEveryLevelWithoutWritingSettings() async {
    for level in FormattingPolicyOption.allCases {
      let state = OverlayState()
      let engine = OverlayStateTestEngine()
      engine.persistedPolicy = OverlayPolicySnapshot(
        autoPasteEnabled: true, autoFormatLevel: .smart)
      state.engine = engine
      state.handleRecordingPreparing()
      projectText(
        "source words for one-shot formatting", to: state,
        canCopy: true, canFormat: true, terminal: true,
        sessionId: "one-shot", reducerRevision: 11)
      let requested = expectation(description: "one-shot level forwarded")
      engine.onFormatter = { requested.fulfill() }

      state.formatTranscript(at: level)
      await fulfillment(of: [requested], timeout: 1)

      XCTAssertEqual(
        engine.formatterRequests,
        [.init(sessionId: "one-shot", sourceRevision: 11, level: level)])
      XCTAssertEqual(engine.persistedPolicy.autoFormatLevel, .smart)
      XCTAssertEqual(state.autoFormatLevel, .smart)
      XCTAssertEqual(state.formattedText, "source words for one-shot formatting")
      XCTAssertTrue(state.formatterCommitPending, "only a reducer projection repaints")
    }
  }

  func testFormatCommitsOnlyThroughFormatterProjectionAndFailureStaysVisible() async {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    state.engine = engine
    projectText(
      "tekst bazowy do formatowania",
      to: state,
      canCopy: true,
      canFormat: true,
      terminal: true,
      sessionId: "formatter-session",
      reducerRevision: 11
    )
    let requested = expectation(description: "formatter intent reached Rust bridge")
    engine.onFormatter = { requested.fulfill() }

    state.relayIntent(.format)
    await fulfillment(of: [requested], timeout: 1)

    XCTAssertEqual(
      engine.formatterRequests,
      [OverlayStateTestEngine.FormatterRequest(sessionId: "formatter-session", sourceRevision: 11)]
    )
    XCTAssertEqual(state.formattedText, "tekst bazowy do formatowania")
    XCTAssertTrue(state.formatterCommitPending)
    XCTAssertTrue(OverlayIntentRail.projectedIntents(for: state).isEmpty)

    projectText(
      engine.formatterRenderedText,
      to: state,
      canCopy: true,
      canFormat: true,
      terminal: true,
      sessionId: "formatter-session",
      reducerRevision: 12,
      reducerAction: "apply_manual_edit",
      manualEditReceipt: "formatter-formatter-session-11-12-0"
    )

    XCTAssertEqual(state.formattedText, engine.formatterRenderedText)
    XCTAssertFalse(state.formatterCommitPending)
    XCTAssertNil(state.formatterError)
    XCTAssertNil(state.userRevisionProvenance, "formatter is not a human correction")

    engine.formatterShouldFail = true
    let refused = expectation(description: "second formatter request was refused")
    engine.onFormatter = { refused.fulfill() }
    state.relayIntent(.format)
    await fulfillment(of: [refused], timeout: 1)
    await Task.yield()

    XCTAssertFalse(state.formatterCommitPending)
    XCTAssertEqual(state.formattedText, engine.formatterRenderedText)
    XCTAssertTrue(state.formatterError?.contains("gateway unavailable") == true)
  }

  func testCloseIsImmediateAndAgentButtonUsesControllerDelivery() async {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now })
    state.engine = engine
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("ready transcript", to: state, terminal: true)
    state.finishControllerRecording()
    var closeCount = 0
    var sentText: String?
    state.onClose = { closeCount += 1 }
    state.onSendToAgent = { sentText = $0 }
    let delivered = expectation(description: "agent button delivered")
    engine.onAssistiveSend = { delivered.fulfill() }

    state.sendToAgent()
    await fulfillment(of: [delivered], timeout: 1)
    XCTAssertEqual(sentText, "ready transcript")
    XCTAssertEqual(engine.sentAssistiveTexts, ["ready transcript"])
    XCTAssertEqual(closeCount, 1)

    state.close()
    XCTAssertEqual(closeCount, 2, "explicit close intent stays immediate")
  }

  func testAgentAutoSendDefaultsOffAndKeepsTranscriptForExplicitSend() async {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now }, autoSendEnabled: { false })
    state.engine = engine
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("kept for review", to: state, terminal: true)
    state.finishControllerRecording()
    XCTAssertNil(state.autoHideDeadline)

    clock.now = 5
    state.fireAutoHideNowForTests(armedDeadline: 5)
    await Task.yield()
    XCTAssertTrue(engine.sentAssistiveTexts.isEmpty)
    XCTAssertEqual(state.activeText, "kept for review")
    XCTAssertNil(state.autoHideDeadline)

    let delivery = state.sendToAgent()
    await delivery?.value
    XCTAssertEqual(engine.sentAssistiveTexts, ["kept for review"])
  }

  func testUntouchedAgentFinalAutoSendsAtDeadline() async {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now }, autoSendEnabled: { true })
    state.engine = engine
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("untouched final", to: state, terminal: true)
    state.finishControllerRecording()
    let delivered = expectation(description: "untouched final delivered")
    engine.onAssistiveSend = { delivered.fulfill() }

    clock.now = 5
    state.fireAutoHideNowForTests()
    await fulfillment(of: [delivered], timeout: 1)
    state.fireAutoHideNowForTests()
    XCTAssertEqual(engine.sentAssistiveTexts, ["untouched final"])
  }

  func testUntouchedRefusedAgentTakeAutoSendsOnceAtDeadline() async {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now }, autoSendEnabled: { true })
    state.engine = engine
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText(
      "usable refused words", to: state, phase: "coverage_refused",
      canSendToAgent: true, terminal: true)
    state.finishControllerRecording()
    let delivered = expectation(description: "refused Agent take delivered")
    engine.onAssistiveSend = { delivered.fulfill() }
    clock.now = 5
    state.fireAutoHideNowForTests()
    await fulfillment(of: [delivered], timeout: 1)
    state.fireAutoHideNowForTests()
    XCTAssertEqual(engine.sentAssistiveTexts, ["usable refused words"])
  }

  func testEditingRefusedAgentTakeCancelsAutoSend() async {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now }, autoSendEnabled: { true })
    state.engine = engine
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText(
      "words for review", to: state, phase: "coverage_refused",
      canSendToAgent: true, terminal: true)
    state.finishControllerRecording()
    state.beginTranscriptEdit()
    state.updateRevisionDraft("edited words")
    state.endTranscriptEdit()
    clock.now = 5
    state.fireAutoHideNowForTests()
    await Task.yield()
    XCTAssertTrue(state.isRevisionDraftDirty)
    XCTAssertTrue(engine.sentAssistiveTexts.isEmpty)
  }

  func testThreeVersionHistoryRestoresFirstAsFourthWithoutRewritingCanvas() async {
    let engine = OverlayStateTestEngine()
    engine.historyEntries = [
      CsDocumentHistoryEntry(
        revision: 1, renderedText: "first", provenance: "acoustic-ledger",
        emittedAt: "2026-09-25T00:00:01Z"),
      CsDocumentHistoryEntry(
        revision: 2, renderedText: "second", provenance: "formatter",
        emittedAt: "2026-09-25T00:00:02Z"),
      CsDocumentHistoryEntry(
        revision: 3, renderedText: "third", provenance: "retranscribe",
        emittedAt: "2026-09-25T00:00:03Z"),
    ]
    let state = OverlayState()
    state.engine = engine
    let loaded = expectation(description: "Bus history loaded")
    engine.onHistoryRead = { loaded.fulfill() }
    projectText("third", to: state, terminal: true, reducerRevision: 3)
    await fulfillment(of: [loaded], timeout: 1)
    engine.onHistoryRead = nil
    XCTAssertEqual(
      state.documentHistory.map(\.provenance),
      ["acoustic-ledger", "formatter", "retranscribe"])

    let requested = expectation(description: "selected history revision reached Rust")
    engine.onRestore = { requested.fulfill() }
    state.restoreDocumentRevision(1)
    await fulfillment(of: [requested], timeout: 1)
    XCTAssertEqual(engine.restoredSelections, [1])
    XCTAssertEqual(state.formattedText, "third", "only the reducer projection repaints")
    let refreshed = expectation(description: "fourth version loaded")
    engine.onHistoryRead = { refreshed.fulfill() }
    projectText(
      "first", to: state, terminal: true, lifecycleTerminal: false,
      sessionId: "overlay-state-tests", reducerRevision: 4,
      reducerAction: "apply_manual_edit", manualEditReceipt: "user-edit-test-4")
    state.loadDocumentHistory()
    await fulfillment(of: [refreshed], timeout: 1)
    XCTAssertEqual(state.formattedText, "first")
    XCTAssertEqual(state.documentHistory.map(\.revision), [1, 2, 3, 4])
    XCTAssertEqual(state.documentHistory.last?.provenance, "user-edit")
  }

  func testTerminalProjectionBurstReadsHistoryAtMostOnce() async {
    let engine = OverlayStateTestEngine()
    let state = OverlayState()
    state.engine = engine
    var reads = 0
    engine.onHistoryRead = { reads += 1 }
    for revision in 1...10 {
      projectText(
        "version \(revision)", to: state, terminal: true,
        lifecycleTerminal: revision == 10, reducerRevision: UInt64(revision))
    }
    try? await Task.sleep(nanoseconds: 50_000_000)
    XCTAssertEqual(reads, 1)
  }

  func testAgentDeadlineRequiresProjectedSendPermission() async {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now }, autoSendEnabled: { true })
    state.engine = engine
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("not authorized", to: state, canSendToAgent: false, terminal: true)
    state.finishControllerRecording()

    clock.now = 5
    state.fireAutoHideNowForTests(armedDeadline: 5)
    // No actor suspension between the refused deadline and explicit send:
    // an incorrect automatic send would already hold the delivery latch.
    projectText(
      "authorized revision", to: state, canSendToAgent: true, terminal: true,
      lifecycleTerminal: false)
    let delivery = state.sendToAgent()
    XCTAssertNotNil(delivery)
    await delivery?.value
    XCTAssertEqual(engine.sentAssistiveTexts, ["authorized revision"])
  }

  func testAgentReviewCancelsAutoSendAfterFocusExitButAllowsExplicitSend() async {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now }, autoSendEnabled: { true })
    state.engine = engine
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("reviewed final", to: state, terminal: true)
    state.finishControllerRecording()
    XCTAssertTrue(state.isTranscriptEditable)

    state.beginTranscriptEdit()
    state.endTranscriptEdit()
    clock.now = 5
    state.fireAutoHideNowForTests()
    // Explicit send follows on the same actor. An incorrectly queued
    // automatic send would already hold the latch and emit the old text.
    projectText("reviewed revision", to: state, terminal: true, lifecycleTerminal: false)
    let delivered = expectation(description: "reviewed transcript explicitly sent")
    engine.onAssistiveSend = { delivered.fulfill() }
    state.sendToAgent()
    await fulfillment(of: [delivered], timeout: 1)
    XCTAssertEqual(engine.sentAssistiveTexts, ["reviewed revision"])
  }

  func testAgentSendCompletionCannotDismissSuccessorCapture() async {
    enum SendFailure: Error { case refused }
    let results: [Result<Bool, Error>] = [
      .success(true), .success(false), .failure(SendFailure.refused),
    ]
    for result in results {
      let engine = OverlayStateTestEngine()
      let state = OverlayState()
      state.engine = engine
      state.applyIndicatorMode(.assistive)
      state.handleRecordingPreparing()
      state.handleRecordingStarted()
      projectText("outgoing final", to: state, terminal: true)
      state.finishControllerRecording()
      var closes = 0
      var presentations = 0
      state.onClose = { closes += 1 }
      state.onSendToAgent = { _ in presentations += 1 }
      let suspended = expectation(description: "send suspended")
      var completion: CheckedContinuation<Bool, Error>?
      engine.assistiveSendHandler = {
        try await withCheckedThrowingContinuation { continuation in
          completion = continuation
          suspended.fulfill()
        }
      }
      let deliveryTask = state.sendToAgent()
      XCTAssertNotNil(deliveryTask)
      await fulfillment(of: [suspended], timeout: 1)
      state.handleRecordingPreparing()
      state.handleRecordingStarted()
      let generation = state.captureGeneration
      completion?.resume(with: result)
      engine.assistiveSendHandler = nil
      await deliveryTask?.value
      XCTAssertEqual(state.captureGeneration, generation)
      XCTAssertEqual(closes, 0)
      XCTAssertEqual(presentations, 0)
      XCTAssertNil(state.toast)
      XCTAssertEqual(engine.sentAssistiveTexts, ["outgoing final"])
      let stopped = expectation(description: "successor remains stoppable after prior send")
      engine.onStopRecording = { stopped.fulfill() }
      state.stop()
      await fulfillment(of: [stopped], timeout: 1)
    }
  }

  func testAgentRailRequiresProjectedPermissionAndRelaysExplicitSend() async {
    let engine = OverlayStateTestEngine()
    let state = OverlayState()
    state.engine = engine
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("retained final", to: state, canSendToAgent: false, terminal: true)
    state.finishControllerRecording()
    XCTAssertFalse(OverlayIntentRail.projectedIntents(for: state).contains(.sendToAgent))
    XCTAssertNil(state.sendToAgent(), "direct invocation must also respect the producer")

    projectText(
      "accepted final", to: state, canSendToAgent: true, terminal: true,
      lifecycleTerminal: false)
    XCTAssertTrue(OverlayIntentRail.projectedIntents(for: state).contains(.sendToAgent))
    let sent = expectation(description: "rail intent reached the engine")
    engine.onAssistiveSend = { sent.fulfill() }
    state.relayIntent(.sendToAgent)
    await fulfillment(of: [sent], timeout: 1)
    XCTAssertEqual(engine.sentAssistiveTexts, ["accepted final"])
  }

  func testRustRenderedContextMarkerPassesThroughUnchanged() {
    let state = OverlayState()
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("alpha {selection_1} beta", to: state)
    XCTAssertEqual(state.activeText, "alpha {selection_1} beta")

    projectText("alpha {selection_1} beta", to: state, terminal: true)
    state.finishControllerRecording()
    XCTAssertEqual(state.formattedText, "alpha {selection_1} beta")
    XCTAssertEqual(state.activeText, "alpha {selection_1} beta")
  }

  func testLatestAgentProjectionAutoSendsAtDeadline() async {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now }, autoSendEnabled: { true })
    state.engine = engine
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("original final", to: state, terminal: true)
    state.finishControllerRecording()
    projectText("newest projected final", to: state, terminal: true, lifecycleTerminal: false)

    let delivered = expectation(description: "latest projected final delivered")
    engine.onAssistiveSend = { delivered.fulfill() }
    clock.now = 5
    state.fireAutoHideNowForTests()
    await fulfillment(of: [delivered], timeout: 1)
    XCTAssertEqual(engine.sentAssistiveTexts, ["newest projected final"])
  }

  func testNoSpeechAutoHidesAfterFiveSeconds() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    var closeCount = 0
    state.onClose = { closeCount += 1 }
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.applyNoSpeech(reason: "no_speech_detected")
    projectText("", to: state, phase: "no_speech", terminal: true)
    state.finishControllerRecording()

    XCTAssertEqual(state.mode, .noSpeech)
    clock.now = 5
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 1)
  }

  func testErrorAutoHidesAfterFiveSeconds() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    var closeCount = 0
    state.onClose = { closeCount += 1 }

    state.handleError(message: "engine unavailable")
    projectText("", to: state, phase: "error", terminal: true)
    XCTAssertEqual(state.mode, .error)
    clock.now = 5
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 1)
  }

  func testOverlayOffNeverOrdersPanelFront() {
    var factoryCount = 0
    var frontCount = 0
    let controller = OverlayController(
      state: OverlayState(),
      engine: nil,
      overlayEnabledProvider: { false },
      assistiveStatusProvider: { false },
      panelFactory: { _, _ in
        factoryCount += 1
        return NSPanel()
      },
      orderPanelFront: { _ in frontCount += 1 },
      orderPanelOut: { _ in }
    )

    controller.showForRecording()
    XCTAssertEqual(factoryCount, 0)
    XCTAssertEqual(frontCount, 0)
    XCTAssertTrue(controller.state.autoPasteControlAvailable)
  }

  func testAgentModesNeverConstructOrOrderOverlayFront() {
    for mode in ["Chat", "Selection"] {
      var frontCount = 0
      let controller = OverlayController(
        state: OverlayState(),
        engine: nil,
        overlayEnabledProvider: { true },
        assistiveStatusProvider: { true },
        panelFactory: { _, _ in NSPanel() },
        orderPanelFront: { _ in frontCount += 1 },
        orderPanelOut: { _ in }
      )

      controller.showForRecording()
      XCTAssertEqual(frontCount, 0, "\(mode) is owned by the Agent composer")
      XCTAssertFalse(controller.state.autoPasteControlAvailable)
    }
  }

  func testMidHoldAssistiveUpgradeClosesOverlayAndFlipsSemantics() {
    var frontCount = 0
    var outCount = 0
    let controller = OverlayController(
      state: OverlayState(),
      engine: nil,
      overlayEnabledProvider: { true },
      assistiveStatusProvider: { false },
      panelFactory: { _, _ in NSPanel() },
      orderPanelFront: { _ in frontCount += 1 },
      orderPanelOut: { _ in outCount += 1 }
    )

    controller.showForRecording()
    XCTAssertEqual(frontCount, 1)
    XCTAssertEqual(outCount, 0)

    controller.handleIndicatorModeChange(.assistive)
    XCTAssertEqual(outCount, 1)
    XCTAssertEqual(controller.state.indicatorMode, .assistive)
    XCTAssertFalse(controller.state.autoPasteControlAvailable)
  }

  func testFormattedReviewBlocksAssistiveHideWithoutFormatInFlight() {
    var outCount = 0
    let state = OverlayState()
    projectText("review take", to: state, terminal: true)
    let controller = OverlayController(
      state: state,
      engine: nil,
      overlayEnabledProvider: { true },
      assistiveStatusProvider: { false },
      panelFactory: { _, _ in NSPanel() },
      orderPanelFront: { _ in },
      orderPanelOut: { _ in outCount += 1 }
    )
    controller.show()
    XCTAssertTrue(state.blocksAssistiveOverlayHide)

    controller.handleIndicatorModeChange(.assistive)
    XCTAssertEqual(outCount, 0)
    XCTAssertNotEqual(state.indicatorMode, .assistive)
    XCTAssertTrue(state.autoPasteControlAvailable)
  }

  func testOverlayPanelUsesNonActivatingStyle() {
    let state = OverlayState()
    let panel = DictationOverlayWindow.make(
      state: state,
      textScale: TextScaleController(key: "OverlayStateTests.textScale")
    )

    XCTAssertTrue(panel.styleMask.contains(.nonactivatingPanel))
    XCTAssertTrue(panel.isFloatingPanel)
    XCTAssertFalse(panel.canBecomeMain)
    XCTAssertFalse(panel.canBecomeKey)
  }

  // MARK: Speech Recognition TCC error rewriting

  func testSpeechAuthNotDeterminedRewritesToActionableNotice() {
    let notice = OverlayState.speechAuthNotice(
      from: "Apple STT bridge probe failed: speech_auth_not_determined"
    )
    XCTAssertNotNil(notice)
    XCTAssertTrue(notice?.contains("Speech Recognition") == true)
    XCTAssertFalse(notice?.contains("speech_auth") == true)
  }

  func testSpeechAuthDeniedRewritesToSystemSettingsHint() {
    let notice = OverlayState.speechAuthNotice(
      from: "Couldn't start recording: speech_auth_denied: enable Speech Recognition"
    )
    XCTAssertNotNil(notice)
    XCTAssertTrue(notice?.contains("System Settings") == true)
  }

  func testNonSpeechErrorsAreNotRewritten() {
    XCTAssertNil(OverlayState.speechAuthNotice(from: "Couldn't start recording: mic busy"))
  }

  // MARK: Typed admission/calibration presentation status

  func testAdmissionRefusalProjectionBecomesRenderablePassiveOverlayState() {
    let state = OverlayState()
    var visibleCallbacks = 0
    state.onPresentationStatus = { visibleCallbacks += 1 }

    state.applyPresentationStatus(refusalStatus())

    XCTAssertEqual(state.mode, .error)
    XCTAssertEqual(state.statusText, "recording blocked")
    XCTAssertEqual(state.presentationStatus?.kind, "admission_refused")
    XCTAssertEqual(state.presentationStatus?.code, "admission_calibration_unusable")
    XCTAssertEqual(
      state.presentationStatus?.headline,
      "Stored microphone calibration cannot be used"
    )
    XCTAssertTrue(state.presentationStatus?.message.contains("Settings › Audio") == true)
    XCTAssertTrue(state.terminal)
    XCTAssertFalse(state.canPaste)
    XCTAssertFalse(state.canInsert)
    XCTAssertFalse(state.canCopy)
    XCTAssertFalse(state.canRetranscribe)
    XCTAssertFalse(state.canFormat)
    XCTAssertEqual(OverlayIntentRail.projectedIntents(for: state), [.close])
    XCTAssertEqual(visibleCallbacks, 1)
  }

  func testStatusCannotEraseTheEngineTranscript() {
    let state = OverlayState()
    let text = "  silnik\nraz raz 👩‍💻  "
    projectText(text, to: state)
    state.applyPresentationStatus(refusalStatus())
    XCTAssertEqual(Array(state.activeText.utf8), Array(text.utf8))
    XCTAssertEqual(Array(state.activeText.utf8), Array(text.utf8))
  }

  func testCalibrationSuccessProjectionCarriesNewProfileVersion() {
    let state = OverlayState()
    state.applyPresentationStatus(
      CsPresentationStatusEvent(
        schema: "codescribe.presentation-status.v1",
        emittedAt: "2026-09-05T00:00:00Z",
        sessionId: nil,
        kind: "calibration_succeeded",
        code: "calibration_succeeded",
        statusLabel: "calibrated",
        headline: "Microphone calibration saved",
        message: "Built-in Microphone at 48000 Hz is ready for recording.",
        isError: false,
        terminal: true,
        calibrationVersion: "cal2-built-in-microphone-1"
      )
    )

    XCTAssertEqual(state.statusText, "calibrated")
    XCTAssertEqual(state.presentationStatus?.kind, "calibration_succeeded")
    XCTAssertEqual(state.presentationStatus?.calibrationVersion, "cal2-built-in-microphone-1")
    XCTAssertNil(state.errorMessage)
    XCTAssertEqual(OverlayIntentRail.projectedIntents(for: state), [.close])
  }

  func testCalibrationFailureProjectionKeepsReasonPassiveAndRenderable() {
    let state = OverlayState()
    state.applyPresentationStatus(
      CsPresentationStatusEvent(
        schema: "codescribe.presentation-status.v1",
        emittedAt: "2026-09-05T00:00:00Z",
        sessionId: nil,
        kind: "calibration_failed",
        code: "calibration_failed",
        statusLabel: "calibration failed",
        headline: "Microphone calibration failed",
        message: "calibration_capture_failed: microphone disconnected",
        isError: true,
        terminal: true,
        calibrationVersion: nil
      )
    )

    XCTAssertEqual(state.mode, .error)
    XCTAssertEqual(state.presentationStatus?.kind, "calibration_failed")
    XCTAssertTrue(state.presentationStatus?.message.contains("microphone disconnected") == true)
    XCTAssertTrue(state.terminal)
    XCTAssertEqual(OverlayIntentRail.projectedIntents(for: state), [.close])
  }

  func testHandleErrorSurfacesFriendlySpeechAuthToast() {
    let state = OverlayState()
    state.handleError(
      message: "Apple STT bridge probe failed: speech_auth_not_determined (no Whisper fallback)")
    XCTAssertEqual(state.mode, .listening)
    XCTAssertTrue(state.errorMessage?.contains("Speech Recognition") == true)
    XCTAssertFalse(state.toast?.contains("speech_auth") == true)
  }

  /// Born from the 2026-08-12 Founder report: a routine
  /// `apple_final_window_overlap_normalized` warning reached `handleError`,
  /// matched none of the three literal phrases the old guard looked for, and ran
  /// `presentTerminalError` — discarding two utterances the engine log had
  /// already recorded as committed (`rendered_chars=282`). The screen said
  /// "failed" while the transcript existed.
  ///
  /// Since the bridge-side warning split, quality receipts no longer travel
  /// here at all — but the content rule this test pins is unconditional:
  /// NOTHING arriving on this channel may discard a non-empty draft.
  func testEngineWarningNeverDiscardsCommittedTranscript() {
    let state = OverlayState()
    projectText("zdanie pierwsze zdanie drugie", to: state)

    state.handleError(
      message:
        "apple_final_window_overlap_normalized: Apple final overlap removed at segment boundary")

    XCTAssertNotEqual(state.mode, .error, "an error with a draft must not discard the take")
    XCTAssertEqual(state.activeText, "zdanie pierwsze zdanie drugie")
    XCTAssertEqual(state.activeText, "zdanie pierwsze zdanie drugie")
  }

  func testTerminalFailureSidebandEndsCaptureWithoutRewritingProjection() {
    let state = OverlayState()
    var stopped = false
    state.onRecordingStopped = { stopped = true }
    state.handleRecordingStarted()
    projectText("zdanie pierwsze", to: state)

    state.handleError(message: "transcription_failed: engine gave up mid-take")

    XCTAssertEqual(state.mode, .listening, "sideband errors do not invent a projection phase")
    XCTAssertEqual(state.statusText, "listening")
    XCTAssertTrue(stopped, "stop parity must fire — no zombie Recording pill")
    XCTAssertEqual(state.activeText, "zdanie pierwsze")
    XCTAssertEqual(state.toast, "Dictation failed — transcript kept")
  }

  /// A worse retranscription must never be a one-way door (operator,
  /// 2026-09-24: "nie można zrobić back po retranscribe"). The commit replaces
  /// the rendered text on the reducer tip; the rail must retain the replaced
  /// text and Back must restore it as a NEW revision on the same session.
  func testRetranscribeRetainsReplacedTextAndBackRestoresIt() async {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    engine.lastSessionAudioPathValue = "/tmp/take-undo.wav"
    engine.transcriptionText = "gorsza wersja"
    state.engine = engine
    projectText(
      "dobra wersja", to: state, canRetranscribe: true, terminal: true,
      sessionId: "take-undo", reducerRevision: 7)

    let committed = expectation(description: "retranscribe committed")
    engine.onRevision = { committed.fulfill() }
    state.retranscribe(pass: .fullHq)
    await fulfillment(of: [committed], timeout: 2)

    XCTAssertEqual(engine.revisionRequests.first?.renderedText, "gorsza wersja")
    XCTAssertTrue(state.canUndoRetranscribe)
    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: state).first, .undoRetranscribe,
      "Back must lead the rail while the replaced text is recoverable")

    // The reducer echoes the committed retranscription as the current tip —
    // an apply_manual_edit document revision, NOT a second lifecycle terminal
    // (a replayed lifecycle line is dropped and would never repaint the tip).
    projectText(
      "gorsza wersja", to: state, canRetranscribe: true, terminal: true,
      sessionId: "take-undo", reducerRevision: 8, reducerAction: "apply_manual_edit")
    XCTAssertTrue(state.canUndoRetranscribe, "the same session keeps Back alive")

    let restored = expectation(description: "undo committed")
    engine.onRevision = { restored.fulfill() }
    state.undoRetranscribeIntent()
    await fulfillment(of: [restored], timeout: 2)

    XCTAssertEqual(engine.revisionRequests.last?.renderedText, "dobra wersja")
    XCTAssertEqual(
      engine.revisionRequests.last?.sourceRevision, 8,
      "the restore builds on the CURRENT tip, not the pre-retranscribe one")
    XCTAssertFalse(state.canUndoRetranscribe, "the slot is consumed by a landed restore")
  }

  func testRetranscribeUsesVisibleTakeAfterLaterQuietTake() async {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    engine.lastSessionAudioPathValue = "/tmp/quiet-b.wav"
    engine.audioPathsBySession["spoken-a"] = "/tmp/spoken-a.wav"
    engine.transcriptionText = "poprawione A"
    state.engine = engine
    projectText(
      "pierwotne A", to: state, canRetranscribe: true, terminal: true,
      sessionId: "spoken-a", reducerRevision: 4)
    let committed = expectation(description: "A revision committed")
    engine.onRevision = { committed.fulfill() }
    state.retranscribe(pass: .cloud)
    await fulfillment(of: [committed], timeout: 2)
    XCTAssertEqual(engine.requestedAudioSessionIds, ["spoken-a"])
    XCTAssertEqual(engine.revisionRequests.first?.sessionId, "spoken-a")
    XCTAssertEqual(state.engineChip, "cloud")
  }

  /// The rollback repaints the canvas, so it dies with its take: a new capture
  /// must never restore words over a different session's document.
  func testRetranscribeRollbackDoesNotSurviveANewCapture() async {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    engine.lastSessionAudioPathValue = "/tmp/take-undo.wav"
    engine.transcriptionText = "nowy tekst"
    state.engine = engine
    projectText(
      "stary tekst", to: state, canRetranscribe: true, terminal: true,
      sessionId: "take-a", reducerRevision: 3)

    let committed = expectation(description: "retranscribe committed")
    engine.onRevision = { committed.fulfill() }
    state.retranscribe(pass: .fullHq)
    await fulfillment(of: [committed], timeout: 2)
    XCTAssertTrue(state.canUndoRetranscribe)

    state.handleRecordingStarted()
    XCTAssertFalse(
      state.canUndoRetranscribe,
      "a new capture must clear the rollback with the rest of the transcript state")
  }

  /// Assistive hides the overlay, so "transcript kept" on a canvas the user
  /// cannot see is a drop. A terminal failure with a committed draft must hand
  /// the projection to the composer join before abort wipes capture identity —
  /// and exactly once, because the delivery slot is per session.
  func testTerminalFailureHandsTheDraftToTheComposerJoin() {
    let state = OverlayState()
    var delivered: [(text: String, sessionId: String)] = []
    state.onComposerTranscript = { text, sessionId in
      delivered.append((text: text, sessionId: sessionId))
      return .admitted(threadID: UUID())
    }
    state.handleRecordingStarted()
    projectText("zdanie pierwsze", to: state, sessionId: "failed-take-join")

    state.handleError(message: "transcription_failed: engine gave up mid-take")

    XCTAssertEqual(delivered.count, 1, "the failed take must reach the composer join")
    XCTAssertEqual(delivered.first?.text, "zdanie pierwsze")
    XCTAssertEqual(delivered.first?.sessionId, "failed-take-join")
    XCTAssertEqual(state.toast, "Dictation failed — transcript kept")

    state.handleError(message: "transcription_failed: engine gave up mid-take")
    XCTAssertEqual(delivered.count, 1, "a duplicate terminal must not insert a second copy")
  }

  func testEngineErrorSidebandPreservesProjectedPhaseOnEmptyTake() {
    let state = OverlayState()
    state.handleError(message: "layer1_lane_degraded: Layer 1 lane fell back")
    XCTAssertEqual(state.mode, .listening)
    XCTAssertEqual(state.errorMessage, "layer1_lane_degraded: Layer 1 lane fell back")
  }

  @MainActor
  func testFormattedOverlayMinimumHeightSnapshotRenders() throws {
    let state = OverlayState()
    let longTranscript = Array(
      repeating:
        "Choose Insert to paste the text where you want it and press Return. The clipboard is untouched.",
      count: 20
    ).joined(separator: "\n")
    projectText(longTranscript, to: state, terminal: true)
    let size = CGSize(
      width: 617,
      height: DictationOverlayWindow.minSize.height
    )
    let hostingView = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .environment(\.csTextScale, 0.8)
        .frame(width: size.width, height: size.height)
        .preferredColorScheme(.dark)
    )
    hostingView.frame = CGRect(origin: .zero, size: size)
    hostingView.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.03))
    guard let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
      return XCTFail("could not allocate the formatted overlay bitmap")
    }
    hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
    guard let png = bitmap.representation(using: .png, properties: [:]) else {
      XCTFail("could not render the formatted overlay")
      return
    }
    let dest = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-formatted-overlay-min-height.png")
    try png.write(to: dest)
    XCTAssertGreaterThan(png.count, 800)

    // Measure an empty right-hand footer corridor, away from the engine label,
    // the centered live-wire dock handle, and the developer-power mark. Work
    // in logical coordinates, then scale into the Retina bitmap: the previous
    // raw x=140 range accidentally crossed `local apple` at 2x. Bright glyphs
    // here mean the transcript escaped its clipped body.
    let scaleX = CGFloat(bitmap.pixelsWide) / size.width
    let scaleY = CGFloat(bitmap.pixelsHigh) / size.height
    var leakedBrightPixels = 0
    for x in Int(390 * scaleX)..<Int(520 * scaleX) {
      for y in Int(6 * scaleY)..<Int(28 * scaleY) {
        guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else {
          continue
        }
        if color.redComponent > 0.7 && color.greenComponent > 0.7
          && color.blueComponent > 0.7 && color.alphaComponent > 0.5
        {
          leakedBrightPixels += 1
        }
      }
    }
    XCTAssertLessThan(
      leakedBrightPixels, Int(20 * scaleX * scaleY),
      "formatted transcript painted into the footer band"
    )
  }

  func testProjectionFixturesMirrorEveryCanvasField() {
    let state = OverlayState()
    let rows:
      [(
        phase: String, text: String, mode: String, paste: Bool, insert: Bool,
        copy: Bool, retranscribe: Bool, format: Bool, terminal: Bool
      )] = [
        ("listening", "  exact live\ntext  ", "dictation", false, false, true, false, true, false),
        ("finalizing", "final pass", "assistive", false, false, true, false, false, false),
        ("formatted", "final text", "dictation", true, true, true, true, true, true),
        ("no_speech", "", "dictation", false, false, false, true, false, true),
        // The refused phase round-trips its raw value like any other. Before
        // the enum case existed this row would have failed on `mode.rawValue`
        // alone, because the parse fell back to the previous phase.
        ("coverage_refused", "unsealed words", "dictation", false, false, true, true, false, true),
        ("error", "kept draft", "dictation", false, false, true, true, false, true),
      ]

    for (index, row) in rows.enumerated() {
      projectText(
        row.text,
        to: state,
        mode: row.mode,
        phase: row.phase,
        canPaste: row.paste,
        canInsert: row.insert,
        canCopy: row.copy,
        canRetranscribe: row.retranscribe,
        canFormat: row.format,
        terminal: row.terminal,
        sessionId: "canvas-row-\(index)"
      )

      XCTAssertEqual(state.mode.rawValue, row.phase)
      XCTAssertEqual(state.formattedText, row.text)
      XCTAssertEqual(state.activeText, row.text)
      XCTAssertEqual(state.transcriptMode, row.mode)
      XCTAssertEqual(state.revision, UInt64(index + 1))
      XCTAssertEqual(state.canPaste, row.paste)
      XCTAssertEqual(state.canInsert, row.insert)
      XCTAssertEqual(state.canCopy, row.copy)
      XCTAssertEqual(state.canRetranscribe, row.retranscribe)
      XCTAssertEqual(state.canFormat, row.format)
      XCTAssertEqual(state.terminal, row.terminal)
    }
  }

  func testCanvasKeepsProjectedActionsAndDiscretePlacementChrome() throws {
    let macosDir = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
    let overlayDir = macosDir.appendingPathComponent("Codescribe/Screens/Overlay")
    let overlaySource = try String(
      contentsOf: overlayDir.appendingPathComponent("DictationOverlayView.swift"),
      encoding: .utf8
    )
    let splitPath = overlayDir.appendingPathComponent("OverlaySplitPrimaryAction.swift").path

    XCTAssertFalse(FileManager.default.fileExists(atPath: splitPath))
    XCTAssertTrue(overlaySource.contains("overlay-auto-paste"))
    XCTAssertTrue(overlaySource.contains("OverlayPlacementMenu"))
    XCTAssertFalse(overlaySource.contains("private var placementMenu"))
    XCTAssertFalse(overlaySource.contains("performPrimaryAction"))
    XCTAssertTrue(overlaySource.contains("OverlayIntentRail"))
    XCTAssertFalse(
      overlaySource.contains("CloseDot"),
      "the rail is the sole overlay control surface"
    )
    XCTAssertTrue(
      overlaySource.contains("chromeWaveform"),
      "waveform stays in the primary chrome"
    )
    XCTAssertFalse(
      overlaySource.contains("private var actionRow"),
      "do not restore the fat bottom Finish/Close row"
    )
  }

  @MainActor
  func testSlimOverlayListeningChromeRendersWithoutBottomActionMass() throws {
    let state = OverlayState.previewListening()
    let size = CGSize(
      width: DictationOverlayWindow.minSize.width,
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
    guard let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
      return XCTFail("could not allocate the slim listening overlay bitmap")
    }
    hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
    guard let png = bitmap.representation(using: .png, properties: [:]) else {
      XCTFail("could not render the slim listening overlay")
      return
    }
    let dest = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-slim-listening-overlay-min-width.png")
    try png.write(to: dest)
    XCTAssertGreaterThan(png.count, 800)
    XCTAssertEqual(DictationOverlayWindow.minSize.width, 320)
    XCTAssertEqual(DictationOverlayWindow.minSize.height, 260)
  }

  /// Project one event for an explicitly named session, so a test can replay
  /// two Bus sessions whose sequences both restart at 1 — the production shape
  /// `projectText` cannot express because it pins a single session id.
  private func projectSessionText(
    _ text: String,
    sessionId: String,
    sequence: UInt64,
    to state: OverlayState,
    terminal: Bool = false
  ) {
    let sampleStart = (sequence - 1) * 16_000
    let sampleEnd = sequence * 16_000
    let receipt = CsProjectedAcousticReceipt(
      acousticSerialVersion: 1,
      acousticSerial: "\(sessionId)-acoustic-\(sequence)",
      sessionId: sessionId,
      captureEpoch: 1,
      sampleStart: sampleStart,
      sampleEnd: sampleEnd,
      durationMs: 1_000,
      energyIntegral: 1,
      meanRmsDbfs: -20,
      peakDbfs: -6,
      vadOpenSample: sampleStart,
      vadCloseSample: sampleEnd,
      evidenceCalibrationVersion: "test-v1",
      wordEvidenceReceipts: ["\(sessionId)-word-\(sequence)"],
      layerDecisionReceipts: ["\(sessionId)-layer-\(sequence)"],
      sealReceipt: terminal ? "\(sessionId)-seal-\(sequence)" : nil,
      manualEditReceipt: nil,
      presentationReceipt: nil
    )
    state.applyTranscriptProjection(
      CsTranscriptProjectionEvent(
        schema: "codescribe.transcript_projection.v1",
        sequence: sequence,
        emittedAt: "2026-08-28T00:00:00Z",
        sessionId: sessionId,
        mode: "dictation",
        reducerRevision: sequence,
        reducerAction: terminal
          ? "record_ledger_terminal_seal"
          : "record_ledger_projection",
        occurrenceSessionId: sessionId,
        captureEpoch: 1,
        sampleStart: sampleStart,
        sampleEnd: sampleEnd,
        documentIndex: sequence - 1,
        label: terminal ? "terminal" : "live",
        renderedText: text,
        deliveryText: nil,
        phase: terminal ? "formatted" : "listening",
        canPaste: terminal,
        canInsert: terminal,
        canCopy: !text.isEmpty,
        canRetranscribe: terminal,
        canFormat: !terminal,
        canSendToAgent: false,
        terminal: terminal,
        lifecycleTerminal: terminal,
        delivery: .unattempted,
        acousticReceipts: [receipt],
        sealCoverage: nil,
        consultationPresentations: []
      )
    )
  }

  /// Regression, div0 2026-08-28: three takes in one app process, sessions
  /// `5567c17a` → `f15f4ad3`. Rust sealed both correctly, but the overlay kept
  /// painting the first take's text over the second and then raised
  /// "Recording ended before a sealed transcript was committed". Bus sequences
  /// restart at 1 per session, so a terminal-seal latch and a monotonic
  /// sequence guard held across sessions drop every projection of take 2.
  func testSecondSessionProjectionsAreNotLatchedByTheFirstSessionSeal() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText(
      "Panie agenci masz brzydkie pięty",
      sessionId: "5567c17a",
      sequence: 42,
      to: state,
      terminal: true
    )
    XCTAssertEqual(state.formattedText, "Panie agenci masz brzydkie pięty")

    // Take 2 in the same process: a fresh session whose sequences start at 1.
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText(
      "Jestem naprawdę na skraju",
      sessionId: "f15f4ad3",
      sequence: 1,
      to: state
    )
    projectSessionText(
      "Jestem naprawdę na skraju wytrzymałości",
      sessionId: "f15f4ad3",
      sequence: 2,
      to: state,
      terminal: true
    )

    XCTAssertEqual(state.formattedText, "Jestem naprawdę na skraju wytrzymałości")
    XCTAssertEqual(state.mode, .formatted)
    state.finishControllerRecording()
    XCTAssertNil(state.errorMessage)
  }

  /// Admission fences stale paint without reducing or sealing transcript text.
  /// The producer still owns the contents of every accepted snapshot.
  func testWithinOneSessionSwiftRejectsStalePaintWithoutReducingText() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    projectSessionText("pierwsza", sessionId: "same-session", sequence: 5, to: state)
    XCTAssertEqual(state.formattedText, "pierwsza")

    projectSessionText("spóźniona", sessionId: "same-session", sequence: 3, to: state)
    XCTAssertEqual(state.formattedText, "pierwsza")

    projectSessionText(
      "zapieczętowana",
      sessionId: "same-session",
      sequence: 6,
      to: state,
      terminal: true
    )
    XCTAssertEqual(state.formattedText, "zapieczętowana")

    projectSessionText("po pieczęci", sessionId: "same-session", sequence: 7, to: state)
    XCTAssertEqual(state.formattedText, "po pieczęci")
    XCTAssertEqual(state.mode, .listening)
    XCTAssertFalse(state.terminal)
  }

  /// Founder witness, Dragon build `3bae96614` (2026-09-09): a new take opened
  /// showing the PREVIOUS take's transcript. The lifecycle used to keep the old
  /// projection in the paint path — this test previously asserted that as the
  /// contract (`…DoesNotClearTheLastProjection`, `mode == .formatted` and the
  /// old words on a fresh capture). The witness falsified it.
  ///
  /// The real concern that test defended survives and is asserted here: the
  /// lifecycle must not DESTROY the last projection. It is retired out of the
  /// paint path and stays readable, which is a different thing from painting it
  /// as new speech.
  func testRecordingLifecycleRetiresThePriorProjectionInsteadOfPaintingIt() {
    let state = OverlayState()

    state.handleRecordingStarted()
    projectSessionText(
      "tekst poprzedniego nagrania",
      sessionId: "previous-take",
      sequence: 9,
      to: state,
      terminal: true
    )
    state.finishControllerRecording()
    XCTAssertEqual(state.activeText, "tekst poprzedniego nagrania")
    let firstGeneration = state.captureGeneration

    state.handleRecordingStarted()

    XCTAssertEqual(state.mode, .listening, "a pending capture is not a formatted take")
    XCTAssertEqual(state.activeText, "", "no old speech on a new take's canvas")
    XCTAssertEqual(state.formattedText, "")
    XCTAssertEqual(state.canvasText, "")
    XCTAssertFalse(state.terminal)
    XCTAssertFalse(state.canPaste)
    XCTAssertFalse(state.canInsert)
    XCTAssertFalse(state.canCopy)
    XCTAssertFalse(state.canRetranscribe)
    XCTAssertFalse(state.canFormat)
    XCTAssertGreaterThan(state.captureGeneration, firstGeneration)

    // Not destroyed — the prior document is still readable, and its session is
    // retired so its own late events cannot repaint this capture.
    XCTAssertEqual(state.supersededTakes.count, 1)
    XCTAssertEqual(state.pendingSupersededTake?.renderedText, "tekst poprzedniego nagrania")
    XCTAssertEqual(state.pendingSupersededTake?.sessionId, "previous-take")
    XCTAssertNil(state.pendingSupersededTake?.unsavedDraft)

    projectSessionText(
      "tekst nowego nagrania",
      sessionId: "new-take",
      sequence: 1,
      to: state
    )
    XCTAssertEqual(state.mode, .listening)
    XCTAssertEqual(state.formattedText, "tekst nowego nagrania")
  }

  // MARK: Acceptance 1 — old formatted take + dirty draft cannot reach a new one

  func testFormattedTakeWithDirtyDraftLeavesNoOldSpeechOnTheNextCapture() {
    let engine = OverlayStateTestEngine()
    let state = OverlayState()
    state.engine = engine

    projectText(
      "Tekst poprzedniej sesji.",
      to: state,
      canPaste: true,
      canInsert: true,
      canRetranscribe: true,
      terminal: true,
      sessionId: "take-1"
    )
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Tekst poprzedniej sesji z moją poprawką.")
    XCTAssertTrue(state.isRevisionDraftDirty)
    XCTAssertEqual(state.canvasText, "Tekst poprzedniej sesji z moją poprawką.")
    // A genuine focus exit schedules the commit; the next capture arrives first.
    state.endTranscriptEdit()

    state.handleRecordingPreparing()
    XCTAssertEqual(state.canvasText, "", "preparing alone must clear the canvas")
    XCTAssertEqual(state.formattedText, "")
    XCTAssertFalse(state.isRevisionDraftDirty)
    XCTAssertFalse(state.isEditingTranscript)
    XCTAssertFalse(state.isTranscriptEditable)

    state.handleRecordingStarted()
    XCTAssertEqual(state.canvasText, "")
    XCTAssertEqual(state.mode, .listening)

    // Both the sealed document and the uncommitted draft stayed recoverable,
    // and nothing was committed across the capture boundary.
    XCTAssertEqual(state.supersededTakes.count, 1)
    XCTAssertEqual(state.pendingSupersededTake?.sessionId, "take-1")
    XCTAssertEqual(state.pendingSupersededTake?.renderedText, "Tekst poprzedniej sesji.")
    XCTAssertEqual(
      state.pendingSupersededTake?.unsavedDraft, "Tekst poprzedniej sesji z moją poprawką.")
    XCTAssertEqual(
      state.pendingSupersededTake?.recoverableText,
      "Tekst poprzedniej sesji z moją poprawką.",
      "an explicit recovery hands back the user's own edit, not the ledger render")
    XCTAssertTrue(
      engine.revisionRequests.isEmpty,
      "a superseded draft is preserved for recovery, never committed mid-capture")
    XCTAssertFalse(state.revisionCommitPending)
    XCTAssertNil(state.revisionCommitError)
  }

  func testCleanDraftLeavesNoRecoverySlotBehind() {
    let state = OverlayState()
    projectText("Dokładnie ta sama treść.", to: state, terminal: true, sessionId: "clean-take")
    XCTAssertFalse(state.isRevisionDraftDirty)

    state.handleRecordingPreparing()
    XCTAssertNil(
      state.pendingSupersededTake?.unsavedDraft,
      "an unedited draft is not unsaved work and must not be offered as recovery")
    XCTAssertEqual(state.unacknowledgedSupersededEditCount, 0)
    XCTAssertEqual(state.pendingSupersededTake?.renderedText, "Dokładnie ta sama treść.")
  }

  // MARK: Recovery — retired status fencing and identity disposition

  /// The sibling status path released the capture BEFORE it ever looked at
  /// `event.sessionId`, while the projection path had been fencing retired
  /// sessions since the previous cut. A predecessor's delayed refusal therefore
  /// aborted its successor and reset the successor's transcript. The status
  /// carries the identity needed to refuse it.
  func testKnownRetiredStatusLeavesItsSuccessorsCaptureUntouched() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    var stoppedCount = 0
    var statusCallbacks = 0
    var closeCount = 0
    state.onRecordingStopped = { stoppedCount += 1 }
    state.onPresentationStatus = { statusCallbacks += 1 }
    state.onClose = { closeCount += 1 }

    // Take A runs to a terminal projection, then a successor retires it.
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText(
      "pierwsze nagranie", sessionId: "take-A", sequence: 1, to: state, terminal: true)
    state.finishControllerRecording()
    let stoppedAfterA = stoppedCount

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    clock.now += 3
    projectSessionText("drugie nagranie", sessionId: "take-B", sequence: 2, to: state)
    let generation = state.captureGeneration

    XCTAssertEqual(
      state.statusAddressing(for: refusalStatus(sessionId: "take-A")), .retiredSession)
    state.applyPresentationStatus(refusalStatus(sessionId: "take-A"))

    XCTAssertNil(
      state.presentationStatus, "a retired status must not paint the successor's card")
    XCTAssertEqual(state.formattedText, "drugie nagranie", "the successor's text survives")
    XCTAssertEqual(state.mode, .listening, "the successor's chrome is untouched")
    XCTAssertFalse(state.terminal)
    XCTAssertTrue(state.audioReady, "the successor's capture is not released")
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertEqual(state.elapsedCaptureSeconds(), 3, "the successor's clock keeps running")
    XCTAssertEqual(stoppedCount, stoppedAfterA, "no release was owed for the successor")
    XCTAssertEqual(statusCallbacks, 0)
    // No countdown was armed against the live take.
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 0)
  }

  /// Paired control for the fence above: a failure addressed to the CURRENT
  /// capture still presents its card and still releases its own capture.
  func testCurrentAddressedFailureStillPresentsAndReleasesItsOwnCapture() {
    let state = OverlayState()
    var stoppedCount = 0
    var statusCallbacks = 0
    state.onRecordingStopped = { stoppedCount += 1 }
    state.onPresentationStatus = { statusCallbacks += 1 }

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText("bieżące nagranie", sessionId: "take-live", sequence: 1, to: state)

    XCTAssertEqual(
      state.statusAddressing(for: refusalStatus(sessionId: "take-live")), .currentCapture)
    state.applyPresentationStatus(refusalStatus(sessionId: "take-live"))

    XCTAssertEqual(state.presentationStatus?.sessionId, "take-live")
    XCTAssertEqual(state.mode, .error)
    XCTAssertTrue(state.terminal)
    XCTAssertFalse(state.audioReady, "the addressed failure released its own capture")
    XCTAssertEqual(stoppedCount, 1)
    XCTAssertEqual(statusCallbacks, 1)
  }

  /// Explicit disposition for the two identity-less cases, read off the
  /// producer rather than guessed. An unobserved session id is addressed here,
  /// because the lifecycle callbacks carry no session id at all and the sole
  /// producer of `admission_refused` emits it from the start path of the take
  /// the user just asked for. A calibration outcome has NO capture identity by
  /// construction, so ending a live take on one would be minting identity.
  func testIdentitylessStatusDispositionIsExplicitAndNeverMintsIdentity() {
    let state = OverlayState()
    var stoppedCount = 0
    state.onRecordingStopped = { stoppedCount += 1 }

    XCTAssertEqual(
      state.statusAddressing(for: refusalStatus(sessionId: "never-observed")),
      .currentCapture,
      "an unobserved identity cannot be classified as a predecessor by presentation alone")
    XCTAssertEqual(
      state.statusAddressing(for: calibrationFailure()), .currentCapture,
      "with nothing in flight a calibration card is simply the current card")

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText("nagranie w toku", sessionId: "live", sequence: 1, to: state)

    XCTAssertEqual(
      state.statusAddressing(for: calibrationFailure()), .foreignToLiveCapture)
    state.applyPresentationStatus(calibrationFailure())

    XCTAssertEqual(
      state.presentationStatus?.kind, "calibration_failed",
      "the card is still product truth")
    XCTAssertEqual(
      state.formattedText, "nagranie w toku",
      "a Settings calibration result may not wipe a live take")
    XCTAssertTrue(state.audioReady)
    XCTAssertEqual(state.mode, .listening)
    XCTAssertFalse(state.terminal)
    XCTAssertEqual(stoppedCount, 0)

    // A refusal with no session id IS a capture verdict, and still lands.
    state.applyPresentationStatus(refusalStatus(sessionId: nil))
    XCTAssertEqual(state.mode, .error)
    XCTAssertTrue(state.terminal)
    XCTAssertEqual(stoppedCount, 1)
  }

  // MARK: Recovery — retained work is never implicitly evicted

  /// Edited A -> B -> clean B ends -> C. The old one-slot store replaced A's
  /// draft with `nil` at C's boundary, so an edit no user decision had ever
  /// consumed was gone. A capture boundary is not a user decision.
  func testEditedTakeSurvivesACleanSuccessorAndKeepsItsSessionAssociation() {
    let state = OverlayState()

    projectText("Zdanie A.", to: state, terminal: true, sessionId: "take-A")
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Zdanie A z moją poprawką.")
    XCTAssertTrue(state.isRevisionDraftDirty)
    state.endTranscriptEdit()

    // B is admitted, runs, and ends CLEAN — its seal seeds its own draft.
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("Zdanie B.", to: state, terminal: true, sessionId: "take-B")
    XCTAssertFalse(state.isRevisionDraftDirty)

    // C: the boundary that used to clear A's unacknowledged edit.
    state.handleRecordingPreparing()

    XCTAssertEqual(state.pendingSupersededTake?.sessionId, "take-A")
    XCTAssertEqual(state.pendingSupersededTake?.renderedText, "Zdanie A.")
    XCTAssertEqual(state.pendingSupersededTake?.unsavedDraft, "Zdanie A z moją poprawką.")
    XCTAssertEqual(state.unacknowledgedSupersededEditCount, 1)
    XCTAssertTrue(
      state.supersededTakes.contains { $0.sessionId == "take-B" },
      "B's clean document is retained too, but never at A's expense")
    XCTAssertEqual(state.canvasText, "", "and C still opens on an empty canvas")
  }

  /// A second edited outgoing take cannot silently overwrite unacknowledged A.
  func testASecondEditedTakeCannotOverwriteAnUnacknowledgedEdit() {
    let state = OverlayState()

    projectText("Zdanie A.", to: state, terminal: true, sessionId: "take-A")
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Zdanie A poprawione.")
    state.endTranscriptEdit()

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("Zdanie B.", to: state, terminal: true, sessionId: "take-B")
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Zdanie B poprawione.")
    state.endTranscriptEdit()

    state.handleRecordingPreparing()

    XCTAssertEqual(state.supersededTakes.map(\.sessionId), ["take-A", "take-B"])
    XCTAssertEqual(
      state.supersededTakes.compactMap(\.unsavedDraft),
      ["Zdanie A poprawione.", "Zdanie B poprawione."])
    XCTAssertEqual(state.unacknowledgedSupersededEditCount, 2)
    XCTAssertEqual(
      state.supersededRecoveryNotice, "2 unsaved edits to recover",
      "the number of waiting decisions is reported, never silently resolved")
  }

  /// Explicit recovery: reachable through the production intent route, hands
  /// the bytes back, and touches neither the reducer nor the current canvas.
  func testExplicitRecoveryHandsBackTheEditWithoutTouchingTheCurrentCapture() {
    let engine = OverlayStateTestEngine()
    let state = OverlayState()
    state.engine = engine
    var written: [String] = []
    state.recoveryClipboardWriter = { text in
      written.append(text)
      return true
    }

    projectText("Zdanie A.", to: state, terminal: true, sessionId: "take-A")
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Zdanie A z moją poprawką.")
    state.endTranscriptEdit()

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertEqual(state.canvasText, "", "the successor opens empty")
    XCTAssertTrue(
      OverlayIntentRail.projectedIntents(for: state).contains(.recoverSuperseded),
      "the retained edit is reachable from the sole action surface, mid-capture")

    state.relayIntent(.recoverSuperseded)

    XCTAssertEqual(state.canvasText, "", "recovery never paints old words as current speech")
    XCTAssertFalse(state.hasRecoverableSupersededWork, "the decision was consumed")
    XCTAssertTrue(engine.revisionRequests.isEmpty, "recovery commits nothing to the reducer")
    XCTAssertNil(engine.pastedText, "recovery submits nothing")
    XCTAssertTrue(engine.sentAssistiveTexts.isEmpty)
    XCTAssertFalse(state.isEditingTranscript, "recovery takes no focus")
    XCTAssertFalse(
      OverlayIntentRail.projectedIntents(for: state).contains(.recoverSuperseded))

    // The bytes themselves left through the injected writer, never the reducer
    // and never the canvas.
    XCTAssertEqual(written, ["Zdanie A z moją poprawką."])
    XCTAssertNil(state.recoveryFailure)
  }

  /// A failed clipboard write must keep the EXACT retained item and expose the
  /// failure. The item is the only copy of an unsaved edit, so consuming it on
  /// an unchecked return value would be the silent loss this owner exists to
  /// prevent — reintroduced one line below the guard that prevents it.
  /// Both outcomes are driven through the real production action, and neither
  /// touches the user's real clipboard.
  func testFailedRecoveryKeepsTheExactItemAndExposesTheFailure() {
    let state = OverlayState()
    var attempts: [String] = []
    var writeSucceeds = false
    state.recoveryClipboardWriter = { text in
      attempts.append(text)
      return writeSucceeds
    }

    projectText("Zdanie A.", to: state, terminal: true, sessionId: "take-A")
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Zdanie A z moją poprawką.")
    state.endTranscriptEdit()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    let retainedBefore = state.pendingSupersededTake
    XCTAssertNotNil(retainedBefore)

    state.relayIntent(.recoverSuperseded)

    XCTAssertEqual(attempts, ["Zdanie A z moją poprawką."], "the real write was attempted")
    XCTAssertEqual(
      state.pendingSupersededTake, retainedBefore,
      "a failed write keeps the exact retained item, identity and draft included")
    XCTAssertEqual(state.supersededTakes.count, 1)
    XCTAssertEqual(state.unacknowledgedSupersededEditCount, 1)
    XCTAssertNotNil(state.recoveryFailure, "the failure is user-visible, not swallowed")
    XCTAssertEqual(state.toast, "recover failed — kept")
    XCTAssertTrue(
      OverlayIntentRail.projectedIntents(for: state).contains(.recoverSuperseded),
      "recovery stays reachable so the user can try again")
    XCTAssertEqual(state.canvasText, "", "a failed recovery leaves the live capture alone")
    XCTAssertTrue(state.audioReady)

    // The retry consumes exactly the one selected item and clears the failure.
    writeSucceeds = true
    state.relayIntent(.recoverSuperseded)

    XCTAssertEqual(attempts.count, 2)
    XCTAssertNil(state.recoveryFailure)
    XCTAssertFalse(state.hasRecoverableSupersededWork)
    XCTAssertEqual(state.canvasText, "")
    XCTAssertTrue(state.audioReady, "the current capture was never the recovery's business")
  }

  /// An explicit discard also clears a standing failure: the decision the
  /// failure was asking for has been made, the other way.
  func testDiscardClearsAStandingRecoveryFailure() {
    let state = OverlayState()
    state.recoveryClipboardWriter = { _ in false }

    projectText("Zdanie A.", to: state, terminal: true, sessionId: "take-A")
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Zdanie A poprawione.")
    state.endTranscriptEdit()
    state.handleRecordingPreparing()

    state.relayIntent(.recoverSuperseded)
    XCTAssertNotNil(state.recoveryFailure)
    XCTAssertEqual(state.unacknowledgedSupersededEditCount, 1)

    state.relayIntent(.discardSuperseded)
    XCTAssertNil(state.recoveryFailure)
    XCTAssertFalse(state.hasRecoverableSupersededWork)
  }

  /// Discard is the only path that drops retained work.
  func testDiscardIsTheOnlyPathThatDropsRetainedWork() {
    let state = OverlayState()

    projectText("Zdanie A.", to: state, terminal: true, sessionId: "take-A")
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Zdanie A poprawione.")
    state.endTranscriptEdit()
    state.handleRecordingPreparing()
    XCTAssertEqual(state.unacknowledgedSupersededEditCount, 1)

    // Neither a further capture nor a stale watchdog may consume the decision.
    state.handleRecordingStarted()
    state.fireWarmupWatchdogForTests(generation: state.captureGeneration)
    state.finishControllerRecording()
    state.handleRecordingPreparing()
    XCTAssertEqual(
      state.unacknowledgedSupersededEditCount, 1, "only the user resolves retained work")

    state.relayIntent(.discardSuperseded)
    XCTAssertFalse(state.hasRecoverableSupersededWork)
    XCTAssertEqual(state.unacknowledgedSupersededEditCount, 0)
  }

  // MARK: Acceptance 5 — duplicate preparing/started is idempotent

  func testDuplicatePreparingAndStartedKeepAdmittedTextAndClock() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })

    state.handleRecordingPreparing()
    let generation = state.captureGeneration
    state.handleRecordingStarted()
    clock.now += 7
    projectSessionText("już przyjęte słowa", sessionId: "open-take", sequence: 1, to: state)
    XCTAssertEqual(state.formattedText, "już przyjęte słowa")
    XCTAssertEqual(state.elapsedCaptureSeconds(), 7)

    // The controller can repeat either beat for an OPEN capture.
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.handleRecordingPreparing()

    XCTAssertEqual(state.captureGeneration, generation, "one open capture, one generation")
    XCTAssertEqual(
      state.formattedText, "już przyjęte słowa", "a duplicate beat cannot erase admitted text")
    XCTAssertEqual(state.canvasText, "już przyjęte słowa")
    XCTAssertEqual(state.elapsedCaptureSeconds(), 7, "the session clock is not restarted")
    XCTAssertTrue(state.supersededTakes.isEmpty)
    XCTAssertEqual(state.latestTranscriptProjection?.sessionId, "open-take")
    // The live audio state itself must survive the duplicate beats, because it
    // is what the warmup watchdog reads. See the dedicated watchdog falsifier.
    XCTAssertTrue(state.audioReady, "a duplicate beat cannot un-confirm the recorder")
    XCTAssertFalse(state.warmingUp, "a live take is not back in warmup")
  }

  /// The counterexample the previous cut's idempotence test never drove:
  /// preparing -> started -> admitted text -> repeated preparing -> the CURRENT
  /// generation's warmup watchdog. The generation fence cannot catch this —
  /// nothing about the capture changed — so the duplicate beat itself must not
  /// put a proven take back into warmup.
  func testDuplicatePreparingCannotArmAWatchdogThatAbortsTheLiveCapture() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    var stoppedCount = 0
    var closeCount = 0
    state.onRecordingStopped = { stoppedCount += 1 }
    state.onClose = { closeCount += 1 }

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    clock.now += 5
    projectSessionText("słowa w toku", sessionId: "live-take", sequence: 1, to: state)
    XCTAssertTrue(state.audioReady)
    XCTAssertFalse(state.warmingUp)
    let generation = state.captureGeneration

    state.handleRecordingPreparing()

    XCTAssertFalse(state.warmingUp, "a duplicate beat cannot put a live take back into warmup")
    XCTAssertTrue(state.audioReady, "the recorder is still confirmed")
    XCTAssertEqual(state.captureGeneration, generation, "one open capture, one generation")

    // Fire the watchdog for the capture that is genuinely open — the exact wake
    // that used to abort it four seconds after a duplicate preparing.
    state.fireWarmupWatchdogForTests(generation: state.captureGeneration)

    XCTAssertEqual(
      state.formattedText, "słowa w toku", "the watchdog aborted a live capture")
    XCTAssertTrue(state.audioReady)
    XCTAssertFalse(state.terminal)
    XCTAssertEqual(state.elapsedCaptureSeconds(), 5, "the capture clock was not frozen")
    XCTAssertEqual(stoppedCount, 0, "no release was owed")
    XCTAssertEqual(closeCount, 0, "the overlay stayed open")
  }

  /// The paired positive: a genuine first warmup that never gets audio still
  /// dismisses itself, and a duplicated INITIAL preparing keeps that recovery.
  func testFirstWarmupTimeoutStillRecoversAcrossADuplicateInitialPreparing() {
    let state = OverlayState()
    var stoppedCount = 0
    state.onRecordingStopped = { stoppedCount += 1 }

    state.handleRecordingPreparing()
    XCTAssertTrue(state.warmingUp)
    // Repeated before any audio proved life: still un-proven, still armed.
    state.handleRecordingPreparing()
    XCTAssertTrue(state.warmingUp)
    XCTAssertFalse(state.audioReady)

    state.fireWarmupWatchdogForTests(generation: state.captureGeneration)

    XCTAssertFalse(state.warmingUp, "an orphaned starting overlay must still dismiss itself")
    XCTAssertEqual(stoppedCount, 1, "the stalled warmup still releases the capture")
  }

  // MARK: Acceptance 3 — stale async work cannot close or repaint a successor

  func testAutoHideWakeArmedByAnEarlierTakeCannotCloseTheSuccessor() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    var closes = 0
    state.onClose = { closes += 1 }

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText("pierwszy take", sessionId: "take-1", sequence: 1, to: state, terminal: true)
    let staleGeneration = state.captureGeneration

    // Take 2 opens before the first take's five-second wake resumes.
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText("drugi take", sessionId: "take-2", sequence: 1, to: state)
    clock.now += OverlayState.autoHideDelaySeconds + 1

    state.fireAutoHideForTests(generation: staleGeneration)
    XCTAssertEqual(closes, 0, "take 1's countdown has no authority over take 2")
    XCTAssertEqual(state.formattedText, "drugi take")

    // The successor's own terminal countdown still closes its own overlay.
    projectSessionText(
      "drugi take zamknięty", sessionId: "take-2", sequence: 2, to: state, terminal: true)
    clock.now += OverlayState.autoHideDelaySeconds + 1
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 1)
  }

  func testWarmupWatchdogArmedByAnEarlierTakeCannotDismissTheSuccessor() {
    let state = OverlayState()
    var closes = 0
    state.onClose = { closes += 1 }

    state.handleRecordingPreparing()
    let staleGeneration = state.captureGeneration
    state.finishControllerRecording()
    state.handleRecordingPreparing()

    XCTAssertTrue(state.warmingUp, "the successor is still in its own warmup window")
    state.fireWarmupWatchdogForTests(generation: staleGeneration)
    XCTAssertEqual(closes, 0, "a stale orphan-dismiss cannot abort a live capture")
    XCTAssertTrue(state.warmingUp)

    state.fireWarmupWatchdogForTests(generation: state.captureGeneration)
    XCTAssertEqual(closes, 1, "the successor's own watchdog still recovers a stuck start")
  }

  func testLateTerminalOfASupersededTakeCannotRepaintOrFinalizeTheSuccessor() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    var closes = 0
    state.onClose = { closes += 1 }
    var endedSessions: [String] = []
    state.onCaptureEnded = { endedSessions.append($0) }

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText("stary take", sessionId: "old", sequence: 1, to: state)

    state.finishControllerRecording()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText("nowy take", sessionId: "new", sequence: 1, to: state)

    // The predecessor's terminal seal finally arrives.
    projectSessionText(
      "stary take domknięty", sessionId: "old", sequence: 2, to: state,
      terminal: true)

    XCTAssertEqual(state.formattedText, "nowy take", "a retired seal cannot repaint")
    XCTAssertEqual(state.mode, .listening, "nor change the successor's chrome")
    XCTAssertFalse(state.terminal, "nor finalize it")
    XCTAssertEqual(
      endedSessions, ["old"],
      "the retired take still completes its own addressed capture release")

    clock.now += OverlayState.autoHideDelaySeconds + 1
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 0, "a retired terminal never armed a countdown for this capture")
  }

  // MARK: Acceptance 2 and 4 — visibility follows the capture route

  /// The lifecycle hooks are replaced with the panel calls they perform in
  /// production. `OverlayController`'s own hooks reach `AppModel.shared`, which
  /// would build the real chat/license stack inside a unit test.
  private func makeRoutedController(
    state: OverlayState,
    overlayEnabled: Bool,
    assistive: Bool,
    panel: NSPanel,
    frontCount: @escaping () -> Void,
    outCount: @escaping () -> Void
  ) -> OverlayController {
    let controller = OverlayController(
      state: state,
      engine: nil,
      overlayEnabledProvider: { overlayEnabled },
      assistiveStatusProvider: { assistive },
      panelFactory: { _, _ in panel },
      orderPanelFront: { panel in
        panel.orderFrontRegardless()
        frontCount()
      },
      orderPanelOut: { panel in
        panel.orderOut(nil)
        outCount()
      }
    )
    state.onRecordingPreparing = { [unowned controller] in controller.showForRecording() }
    state.onRecordingStarted = { [unowned controller] in controller.showForRecording() }
    state.onRecordingStopped = { [unowned controller] in controller.markStopped() }
    return controller
  }

  func testPinnedCloseHidesPanelAndNextDictationShowsItWithoutUnpinning() {
    let engine = OverlayStateTestEngine()
    let state = OverlayState()
    let panel = NSPanel()
    var shows = 0
    var hides = 0
    let controller = makeRoutedController(
      state: state, overlayEnabled: true, assistive: false, panel: panel,
      frontCount: { shows += 1 }, outCount: { hides += 1 })
    // The controller's init attaches its own (nil) engine and re-reads the pin,
    // as AppModel does with the real engine; pin only after that attach.
    state.engine = engine
    state.setKeepVisibleBetweenTakes(true)
    state.onClose = { controller.hide() }

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertTrue(panel.isVisible)
    state.relayIntent(.close)
    XCTAssertFalse(panel.isVisible)
    XCTAssertTrue(state.keepVisibleBetweenTakes)
    state.handleRecordingPreparing()
    XCTAssertTrue(panel.isVisible)
    XCTAssertGreaterThanOrEqual(shows, 2)
    XCTAssertGreaterThanOrEqual(hides, 1)
    XCTAssertEqual(engine.pinWrites, [true])
    panel.orderOut(nil)
    withExtendedLifetime(controller) {}
  }

  func testEnabledDictationStaysVisibleThroughCaptureAndSilence() {
    var fronts = 0
    var outs = 0
    let state = OverlayState()
    let panel = NSPanel()
    let controller = makeRoutedController(
      state: state, overlayEnabled: true, assistive: false, panel: panel,
      frontCount: { fronts += 1 }, outCount: { outs += 1 })

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertGreaterThan(fronts, 0)
    XCTAssertEqual(outs, 0)

    // Silence: no VAD, no measured level, no projection — and a non-assistive
    // tray tick arriving mid-capture. None of these is a hide decision.
    state.applyVad(false)
    // `AudioLevelMeter.push` takes LINEAR rms and rejects negatives, so a quiet
    // room is a tiny positive block, not a dBFS number. It measures silence.
    state.applyAudioLevel(0.0002)
    controller.handleIndicatorModeChange(.hold)
    controller.handleIndicatorModeChange(.toggle)
    controller.handleIndicatorModeChange(.processing)
    controller.showForRecording()

    XCTAssertEqual(outs, 0, "an enabled Dictation overlay survives measured silence")
    XCTAssertFalse(state.vadActive)
    XCTAssertTrue(state.hasMeasuredAudioLevel, "the mic is alive and reporting")
    XCTAssertEqual(state.levelMeter.gain, 0, "what it reports is silence")
    XCTAssertTrue(state.autoPasteControlAvailable)
    withExtendedLifetime(controller) {}
  }

  func testExplicitOffAndGenuineAssistiveHideWithoutStealingFocus() {
    var offFronts = 0
    let offState = OverlayState()
    let offPanel = NSPanel()
    let offController = makeRoutedController(
      state: offState, overlayEnabled: false, assistive: false, panel: offPanel,
      frontCount: { offFronts += 1 }, outCount: {})
    offState.handleRecordingPreparing()
    offState.handleRecordingStarted()
    XCTAssertEqual(offFronts, 0, "explicit overlay-off runs headless")
    XCTAssertFalse(offPanel.isVisible)

    var agentFronts = 0
    let agentState = OverlayState()
    let agentPanel = NSPanel()
    let agentController = makeRoutedController(
      state: agentState, overlayEnabled: true, assistive: true, panel: agentPanel,
      frontCount: { agentFronts += 1 }, outCount: {})
    agentState.handleRecordingPreparing()
    agentState.handleRecordingStarted()
    XCTAssertEqual(agentFronts, 0, "a genuine Agent/Assistive route is owned by the composer")
    XCTAssertFalse(agentState.autoPasteControlAvailable)

    // The visible Dictation route never takes key or main away from the user.
    var visibleFronts = 0
    let visibleState = OverlayState()
    let realPanel = DictationOverlayWindow.make(
      state: visibleState,
      textScale: TextScaleController(key: "OverlayStateTests.route.textScale")
    )
    let visibleController = makeRoutedController(
      state: visibleState, overlayEnabled: true, assistive: false, panel: realPanel,
      frontCount: { visibleFronts += 1 }, outCount: {})
    visibleState.handleRecordingPreparing()
    XCTAssertGreaterThan(visibleFronts, 0)
    XCTAssertTrue(realPanel.isVisible)
    XCTAssertFalse(realPanel.isKeyWindow, "showing the overlay is not a focus grab")
    XCTAssertFalse(realPanel.isMainWindow)
    realPanel.orderOut(nil)
    withExtendedLifetime([offController, agentController, visibleController]) {}
  }

  // MARK: Refusal visibility (rc-w2-refusal-visibility) — UNRUN under W2

  func testRefusedCoverageNeverArmsAutoHideAfterProjectionOrInteraction() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    var closes = 0
    var successes = 0
    state.onClose = { closes += 1 }
    state.onSuccessfulDictation = { successes += 1 }
    let words = "Zażółć — bez pieczęci.\nPowtórz. Powtórz."

    projectText(
      words, to: state, phase: "coverage_refused", canInsert: true, canCopy: true,
      canRetranscribe: true, terminal: true)
    XCTAssertNil(state.autoHideDeadline, "refusal must not arm a countdown")

    state.setPointerHovering(true)
    state.setPointerHovering(false)
    state.userDraggedOverlay()
    state.userResizedOverlay()
    XCTAssertNil(state.autoHideDeadline, "interaction must not rearm refused recovery")
    clock.now += OverlayState.autoHideDelaySeconds * 3
    state.fireAutoHideNowForTests()

    XCTAssertEqual(closes, 0)
    XCTAssertEqual(successes, 0)
    XCTAssertEqual(state.mode, .coverageRefused)
    XCTAssertEqual(Array(state.canvasText.utf8), Array(words.utf8))
    XCTAssertNotNil(state.coverageRefusalNotice)
    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: state),
      [.insertPaste, .copy, .retranscribe, .sendToAgent, .close])
  }

  func testWakeArmedBeforeRefusalCannotCloseRecoveryAtItsDeadline() throws {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    var closes = 0
    state.onClose = { closes += 1 }
    projectText("earlier verdict", to: state, terminal: true, lifecycleTerminal: false)
    let generation = state.captureGeneration
    let armedDeadline = try XCTUnwrap(state.autoHideDeadline)

    clock.now += 1
    projectText("kept words", to: state, phase: "coverage_refused", terminal: true)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertNil(state.autoHideDeadline, "refusal cancels the earlier countdown")

    // Restore the saved deadline only at the existing test seam. Without the
    // execution guard this closes the panel, even if scheduling is protected.
    clock.now = armedDeadline
    state.fireAutoHideNowForTests(armedDeadline: armedDeadline)
    XCTAssertEqual(closes, 0)
    XCTAssertNil(state.autoHideDeadline)
    XCTAssertEqual(state.activeText, "kept words")
    XCTAssertNotNil(state.coverageRefusalNotice)
  }

  func testEarlyWakeArmedBeforeRefusalCannotLeaveARecoveryDeadline() throws {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    var closes = 0
    state.onClose = { closes += 1 }
    projectText("earlier verdict", to: state, terminal: true, lifecycleTerminal: false)
    let armedDeadline = try XCTUnwrap(state.autoHideDeadline)
    projectText("kept words", to: state, phase: "coverage_refused", terminal: true)

    clock.now = armedDeadline - 1
    state.fireAutoHideNowForTests(armedDeadline: armedDeadline)
    XCTAssertNil(state.autoHideDeadline, "an early refused wake must retire its deadline")
    clock.now = armedDeadline + 1
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 0)
  }

  func testExplicitCloseStillDismissesRefusedRecoveryWithoutDiscardingWords() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    var closes = 0
    state.onClose = { closes += 1 }
    state.onComposerTranscript = { text, _ in .retained(text) }
    projectText(
      "recover these words", to: state, phase: "coverage_refused", canCopy: true,
      terminal: true, delivery: .composerPending, sessionId: "explicit-close-refused")

    XCTAssertTrue(OverlayIntentRail.projectedIntents(for: state).contains(.close))
    state.relayIntent(.close)

    XCTAssertEqual(closes, 1, "the human Close intent must reach the actual close callback")
    XCTAssertEqual(state.mode, .coverageRefused)
    XCTAssertEqual(state.activeText, "recover these words")
    XCTAssertEqual(state.retainedComposerDelivery, "recover these words")
    XCTAssertNil(state.autoHideDeadline)
    clock.now += OverlayState.autoHideDelaySeconds * 2
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 1)
  }

  func testRefusalSuccessorKeepsItsOwnSuccessfulCountdownDespitePredecessorWake() throws {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    var closes = 0
    state.onClose = { closes += 1 }
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("earlier verdict", to: state, terminal: true, sessionId: "predecessor")
    let staleGeneration = state.captureGeneration
    XCTAssertNotNil(state.autoHideDeadline)
    projectText(
      "refused predecessor", to: state, phase: "coverage_refused", terminal: true,
      sessionId: "predecessor")

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("successor live", to: state, sessionId: "successor")
    XCTAssertGreaterThan(state.captureGeneration, staleGeneration)
    state.fireAutoHideForTests(generation: staleGeneration)
    XCTAssertEqual(closes, 0)
    XCTAssertEqual(state.activeText, "successor live")
    XCTAssertNil(state.coverageRefusalNotice)

    projectText("successor complete", to: state, terminal: true, sessionId: "successor")
    let successorDeadline = try XCTUnwrap(state.autoHideDeadline)
    clock.now = successorDeadline - 0.1
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 0, "ordinary success still waits the full interval")
    clock.now = successorDeadline
    state.fireAutoHideForTests(generation: staleGeneration)
    XCTAssertEqual(closes, 0, "even an expired successor deadline is not the predecessor's")
    XCTAssertEqual(state.autoHideDeadline, successorDeadline)
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 1, "the successor's successful countdown still works")
    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(state.activeText, "successor complete")
  }

  // MARK: Refusal recovery (rc-w2-refusal-ui) — UNRUN under W2

  /// The gap this cut closes, stated as its falsifier. Before the enum case
  /// existed, `OverlayMode(rawValue:) ?? mode` kept the PREVIOUS phase, so a
  /// refused take finished its life painted as `finalizing` — a spinner over a
  /// settled document. The assertion on `formattedText` is the other half: a
  /// new terminal phase may not cost the user a single byte.
  func testRefusedCoverageIsItsOwnTerminalPhaseAndKeepsEveryWord() {
    let state = OverlayState()
    projectText("mowa w toku", to: state, phase: "finalizing")
    XCTAssertEqual(state.mode, .finalizing)

    let refused = "Zażółć gęślą jaźń — słowa bez pieczęci."
    projectText(refused, to: state, phase: "coverage_refused", canCopy: true, terminal: true)

    XCTAssertEqual(state.mode, .coverageRefused)
    XCTAssertEqual(state.statusText, "unverified coverage")
    XCTAssertEqual(Array(state.activeText.utf8), Array(refused.utf8))
    XCTAssertEqual(state.canvasText, refused)
    XCTAssertTrue(state.terminal)
    XCTAssertFalse(state.statusRippling)
  }

  /// Positive and negative in one place: the explanation is present and
  /// persistent, and the seal-success callback does not fire. Agent auto-send
  /// has its separate lifecycle and untouched-text gate.
  func testRefusedCoverageExplainsItselfWithoutMintingSuccess() {
    let state = OverlayState()
    var successes = 0
    state.onSuccessfulDictation = { successes += 1 }

    projectText("usable words", to: state, phase: "coverage_refused", canCopy: true, terminal: true)

    XCTAssertEqual(state.coverageRefusalNotice, OverlayState.defaultCoverageRefusalNotice)
    XCTAssertEqual(
      state.coverageRefusalDetail,
      "No seal was recorded for this take, so nothing here is certified complete.")
    XCTAssertEqual(successes, 0, "a refused seal fired the success callback")
    XCTAssertNil(state.errorMessage, "refusal is not an error message")
    XCTAssertTrue(state.isTranscriptEditable, "review is possible even when a seal was refused")
    XCTAssertTrue(state.blocksAssistiveOverlayHide)

    // The same document arriving as a real seal clears the notice, because the
    // notice mirrors the producer rather than latching on the first refusal.
    projectText(
      "usable words", to: state, phase: "formatted", canCopy: true, terminal: true,
      reducerAction: "apply_manual_edit", manualEditReceipt: "formatter-refusal-followup")
    XCTAssertNil(state.coverageRefusalNotice)
    XCTAssertEqual(state.mode, .formatted)
  }

  /// A refused take must survive the Assistive tray tick that calls `hide()`.
  /// Every other terminal outcome already did; the refusal is the one whose
  /// only recovery handle lives on this panel.
  func testAssistiveTickCannotTakeTheRecoveryPanelAway() {
    let state = OverlayState()
    XCTAssertFalse(state.blocksAssistiveOverlayHide, "an idle overlay yields normally")
    projectText("live", to: state, phase: "listening")
    XCTAssertFalse(state.blocksAssistiveOverlayHide, "a live capture is not post-take review")

    projectText("refused", to: state, phase: "coverage_refused", canCopy: true, terminal: true)
    XCTAssertTrue(state.blocksAssistiveOverlayHide)
  }

  /// The next capture owns its own chrome and nothing else. It clears the
  /// predecessor's notice — otherwise the panel describes the wrong take — and
  /// it must NOT clear the predecessor's retained work, which is keyed by that
  /// take's own identity and is the only copy the user has left.
  func testNextCaptureClearsTheNoticeAndKeepsThePredecessorsRetainedWork() {
    let state = OverlayState()
    state.onComposerTranscript = { text, _ in .retained(text) }

    projectText(
      "words with nowhere to go", to: state, phase: "coverage_refused", canCopy: true,
      terminal: true, delivery: .composerPending, sessionId: "refused-A",
      reducerAction: "session_ended")

    XCTAssertEqual(state.retainedComposerDelivery, "words with nowhere to go")
    XCTAssertEqual(state.toast, "kept for recovery")
    XCTAssertEqual(
      state.coverageRefusalDetail,
      "The handover came back. These words are retained here — recover them before the next take.")
    XCTAssertNotNil(state.coverageRefusalNotice)

    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    XCTAssertNil(state.coverageRefusalNotice, "the successor inherited a refusal it never had")
    XCTAssertNil(state.toast, "a persisting chip described the wrong take")
    XCTAssertEqual(
      state.retainedComposerDelivery, "words with nowhere to go",
      "the successor erased the predecessor's only remaining copy")
    XCTAssertEqual(state.mode, .listening)
  }

  /// A retained handover is standing state, not an event. The chip used to
  /// clear itself after `showFooterNotice`'s 2.6 s window, leaving words on
  /// screen with nothing saying their destination refused them.
  ///
  /// This test spends real time on purpose. There is no injected clock behind
  /// `showFooterNotice`, so the only honest falsifier is to outlive the window
  /// the transient path would have used — a shorter sleep would pass with or
  /// without the fix and prove nothing. The paired transient assertion is what
  /// makes the wait meaningful: it shows the window really does expire in this
  /// same test run.
  func testRetainedDeliveryNoticePersistsWhileOrdinaryChipsExpire() async {
    let transient = OverlayState()
    transient.showFooterNotice("copied")

    let state = OverlayState()
    state.onComposerTranscript = { text, _ in .retained(text) }
    projectText(
      "retained", to: state, phase: "coverage_refused", canCopy: true, terminal: true,
      delivery: .composerPending, sessionId: "refused-persist",
      reducerAction: "session_ended")
    XCTAssertEqual(state.toast, "kept for recovery")

    try? await Task.sleep(nanoseconds: 2_900_000_000)

    XCTAssertNil(
      transient.toast, "the ordinary toast window did not expire; the wait proves nothing")
    XCTAssertEqual(state.toast, "kept for recovery", "the recovery notice was scheduled away")
  }

  /// Empty typed refusal (`coverage_refused_empty`) reaches Swift as an Error
  /// phase with no text and no capability bits. Nothing may be invented for it.
  func testEmptyRefusalInventsNoTextAndNoRecovery() {
    let state = OverlayState()
    var deliveries: [String] = []
    state.onComposerTranscript = { text, _ in
      deliveries.append(text)
      return .admitted(threadID: UUID())
    }

    projectText(
      "", to: state, phase: "error", canCopy: false, terminal: true,
      delivery: .composerPending, sessionId: "refused-empty", reducerAction: "session_ended")

    XCTAssertEqual(state.mode, .error)
    XCTAssertEqual(state.activeText, "")
    XCTAssertTrue(deliveries.isEmpty, "an empty document was offered to the receiver")
    XCTAssertNil(state.retainedComposerDelivery)
    XCTAssertNil(state.coverageRefusalNotice, "an empty refusal is not the refused-words card")
    XCTAssertEqual(OverlayIntentRail.projectedIntents(for: state), [.close])
  }

  /// An unfamiliar phase must still behave as it did before this cut: the
  /// enum grew by one case, and that is not a licence to start rejecting the
  /// next phase the producer invents.
  func testAddingRefusedCoverageDidNotChangeUnknownPhaseHandling() {
    let state = OverlayState()
    projectText("kept", to: state, phase: "coverage_refused", canCopy: true, terminal: true)
    projectText(
      "still kept", to: state, phase: "future_engine_phase", canCopy: true, terminal: true,
      lifecycleTerminal: false)
    XCTAssertEqual(state.mode, .coverageRefused, "an unknown phase retains the current chrome")
    XCTAssertEqual(state.activeText, "still kept")
  }
  private func coverage(
    _ status: CsSealCoverageStatus,
    reason: CsCoverageUnavailableReason? = nil
  ) -> CsProjectedSealCoverageReceipt {
    CsProjectedSealCoverageReceipt(
      status: status, unavailableReason: reason, speechSamples: status == .incomplete ? 32_000 : 0,
      coveredSamples: status == .incomplete ? 16_000 : 0, uncoveredSpeechRanges: [],
      maxUncoveredSamples: status == .incomplete ? 16_000 : 0, incompleteThresholdSamples: 4_000,
      speechProducer: "capture_energy",
      availability: status == .incomplete ? "observed" : "not_observed",
      observedSamples: status == .incomplete ? 64_000 : nil,
      coverageRatio: status == .incomplete ? 0.5 : nil)
  }

  func testCompleteMeasuredCoverageWithoutTerminalSealKeepsWordsWithoutPromisingSuccess() {
    for copyAllowed in [false, true] {
      let state = OverlayState()
      var successes = 0
      var sends = 0
      state.onSuccessfulDictation = { successes += 1 }
      state.onSendToAgent = { _ in sends += 1 }
      let receipt = CsProjectedSealCoverageReceipt(
        status: .complete, unavailableReason: nil, speechSamples: 32_000,
        coveredSamples: 32_000, uncoveredSpeechRanges: [], maxUncoveredSamples: 0,
        incompleteThresholdSamples: 4_000, speechProducer: "capture_energy",
        availability: "observed", observedSamples: 64_000, coverageRatio: 1.0)
      let words = "  Zażółć — zachowane słowa.\n"
      projectText(
        words, to: state, phase: "coverage_refused", canCopy: copyAllowed,
        terminal: true, sealCoverage: receipt)

      XCTAssertEqual(state.mode, .coverageRefused)
      XCTAssertEqual(state.statusText, "unsealed transcript")
      XCTAssertEqual(
        state.coverageRefusalNotice,
        "These words were kept, but the transcript was not sealed")
      XCTAssertEqual(
        state.coverageRefusalDetail,
        "Acoustic coverage was measured as complete, but this transcript has no current terminal seal."
      )
      XCTAssertEqual(Array(state.activeText.utf8), Array(words.utf8))
      XCTAssertEqual(state.canCopy, copyAllowed, "measurement must not grant copy permission")
      XCTAssertFalse(state.canInsert)
      XCTAssertFalse(state.canRetranscribe)
      XCTAssertTrue(state.terminal)
      XCTAssertFalse(state.statusRippling)
      XCTAssertNil(state.autoHideDeadline)
      state.fireAutoHideNowForTests()
      XCTAssertEqual(successes, 0)
      XCTAssertEqual(sends, 0)
    }
  }

  func testTypedCoverageExplainsIncompleteAndEveryUnavailableReasonWithoutChangingBytes() {
    let cases: [(CsProjectedSealCoverageReceipt, String, String)] = [
      (coverage(.incomplete), "incomplete coverage", "Incomplete coverage"),
      (
        coverage(.unavailable, reason: .notObserved), "measurement unavailable",
        "No acoustic measurement"
      ),
      (
        coverage(.unavailable, reason: .identityMismatch), "measurement unavailable",
        "did not match this take"
      ),
      (
        coverage(.unavailable, reason: .invalidMeasurement), "measurement unavailable",
        "could not be used"
      ),
      (
        coverage(.unavailable, reason: .partialObservation), "measurement unavailable",
        "only part of this take"
      ),
      (
        coverage(.unavailable, reason: .unknown), "measurement unavailable",
        "measurement was unavailable"
      ),
      (coverage(.unknown), "unverified coverage", "could not be verified"),
    ]
    for (receipt, status, explanation) in cases {
      let state = OverlayState()
      var successes = 0
      var sends = 0
      state.onSuccessfulDictation = { successes += 1 }
      state.onSendToAgent = { _ in sends += 1 }
      let words = "  Zażółć — tak tak!\n"
      projectText(
        words, to: state, phase: "coverage_refused", canInsert: true, canCopy: true,
        canRetranscribe: true, terminal: true, sealCoverage: receipt)
      XCTAssertEqual(state.statusText, status)
      XCTAssertTrue(state.coverageRefusalNotice?.contains(explanation) == true)
      XCTAssertEqual(Array(state.activeText.utf8), Array(words.utf8))
      XCTAssertTrue(state.canCopy)
      XCTAssertTrue(state.canInsert)
      XCTAssertTrue(state.canRetranscribe)
      XCTAssertNil(state.autoHideDeadline)
      state.fireAutoHideNowForTests()
      XCTAssertEqual(successes, 0)
      XCTAssertEqual(sends, 0)
    }
  }

  func testStaleProjectionCannotOverwriteRefusalOrRepeatLifecycleDelivery() throws {
    for staleAxis in ["sequence", "revision", "epoch", "duplicate"] {
      let state = OverlayState()
      var deliveries = 0
      var releases = 0
      state.onComposerTranscript = { _, _ in
        deliveries += 1
        return .admitted(threadID: UUID())
      }
      state.onCaptureEnded = { _ in releases += 1 }
      projectText(
        "tak tak", to: state, phase: "coverage_refused", canCopy: true,
        terminal: true, delivery: .composerPending,
        sealCoverage: coverage(.unavailable, reason: .notObserved))
      let accepted = try XCTUnwrap(state.latestTranscriptProjection)
      var stale = accepted
      if staleAxis != "duplicate" {
        stale.sequence += 1
        stale.renderedText = "stale replacement"
        stale.phase = "formatted"
        stale.sealCoverage = coverage(.complete)
      }
      switch staleAxis {
      case "sequence": stale.sequence = accepted.sequence - 1
      case "revision": stale.reducerRevision = accepted.reducerRevision - 1
      case "epoch": stale.captureEpoch = accepted.captureEpoch - 1
      default: break
      }
      state.applyTranscriptProjection(stale)
      XCTAssertEqual(state.latestTranscriptProjection, accepted, staleAxis)
      XCTAssertEqual(state.activeText, "tak tak", staleAxis)
      XCTAssertEqual(state.statusText, "measurement unavailable", staleAxis)
      XCTAssertEqual(deliveries, 1, staleAxis)
      XCTAssertEqual(releases, 1, staleAxis)
    }
  }

  func testLaterDocumentRevisionClearsOnlyItsOwnRefusalAndPreservesRepetition() throws {
    let state = OverlayState()
    projectText(
      "tak", to: state, phase: "coverage_refused", terminal: true,
      sealCoverage: coverage(.unavailable, reason: .partialObservation))
    var revision = try XCTUnwrap(state.latestTranscriptProjection)
    revision.sequence += 1
    revision.reducerRevision += 1
    revision.lifecycleTerminal = false
    revision.phase = "formatted"
    revision.renderedText = "tak tak"
    state.applyTranscriptProjection(revision)
    XCTAssertEqual(state.activeText, "tak tak")
    XCTAssertNil(state.coverageRefusalNotice)
    state.handleRecordingPreparing()
    projectText("new take", to: state, sessionId: "successor-low-sequence")
    let successor = state.latestTranscriptProjection
    revision.sequence += 1
    revision.reducerRevision += 1
    revision.phase = "coverage_refused"
    state.applyTranscriptProjection(revision)
    XCTAssertEqual(state.latestTranscriptProjection, successor)
    XCTAssertNil(state.coverageRefusalNotice)
  }

  func testEmptyUnavailableRefusalCreatesNoDeliveryOrCapability() {
    let state = OverlayState()
    var offers = 0
    state.onComposerTranscript = { _, _ in
      offers += 1
      return .empty
    }
    projectText(
      "", to: state, phase: "error", canCopy: false, terminal: true,
      delivery: .composerPending, sealCoverage: coverage(.unavailable, reason: .invalidMeasurement))
    XCTAssertEqual(state.activeText, "")
    XCTAssertEqual(offers, 0)
    XCTAssertNil(state.coverageRefusalNotice)
    XCTAssertEqual(OverlayIntentRail.projectedIntents(for: state), [.close])
  }

  func testMultiEpochRevisionPaintsItsFinalCurrentEpochRow() throws {
    let state = OverlayState()
    projectText("first epoch", to: state)
    var row = try XCTUnwrap(state.latestTranscriptProjection)
    row.sequence += 1
    row.reducerRevision += 1
    row.captureEpoch = 2
    row.renderedText = "first epoch second epoch"
    state.applyTranscriptProjection(row)
    XCTAssertEqual(state.activeText, row.renderedText)
    // Bus republishes all sorted entries for a new revision, each carrying the
    // same full document. Its first old-epoch row cannot regress the fence;
    // the last current-epoch row still admits the complete new document.
    row.sequence += 1
    row.reducerRevision += 1
    row.captureEpoch = 1
    row.renderedText = "first epoch second epoch second epoch"
    state.applyTranscriptProjection(row)
    XCTAssertEqual(state.activeText, "first epoch second epoch")
    row.sequence += 1
    row.captureEpoch = 2
    state.applyTranscriptProjection(row)
    XCTAssertEqual(state.activeText, "first epoch second epoch second epoch")
  }

}
