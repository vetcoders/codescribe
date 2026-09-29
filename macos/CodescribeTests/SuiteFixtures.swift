import AppKit
import XCTest

@testable import Codescribe

// Shared witnesses for the chat and overlay suites. A method lives here only
// when every previous copy had the same body. Behavioral overrides stay on
// the test that owns the scenario.

protocol ThreadsFixture: ChatThreadsProviding {}

extension ThreadsFixture {
  func listThreads() throws -> [ChatThread] { [] }
  func searchThreads(query _: String) throws -> [ChatThread] { try listThreads() }
  func loadMessages(backendId _: String) -> [ChatMessage] { [] }
  func deleteThread(backendId _: String) -> Bool { true }
  func setThreadFavorite(backendId _: String, isFavorite _: Bool) -> Bool { true }
  func renameThread(backendId _: String, title _: String) -> Bool { true }
  func setGeneratedTitle(backendId _: String, title _: String) -> Bool { true }
  func exportThreadMarkdown(backendId _: String, assistantOnly _: Bool) -> String? { nil }
  func generateThreadId() -> String { "t_generated" }
}

/// Two persisted agent threads, ids `t_a` and `t_b`, messages already loaded.
final class PairThreadsProvider: ThreadsFixture {
  func listThreads() -> [ChatThread] {
    [("t_a", "Thread A"), ("t_b", "Thread B")].map { row in
      var thread = ChatThread(title: row.1, meta: "now")
      thread.backendId = row.0
      thread.messagesLoaded = true
      return thread
    }
  }
}

protocol ChatEngineFixture: AgentChatEngine {
  func acceptReply(
    _ text: String,
    threadId: String,
    attachmentPaths: [String]
  ) async throws -> String
}

extension ChatEngineFixture {
  func isAvailable() -> Bool { true }
  func availabilityDetail() -> String? { nil }
  func generateThreadTitle(_: String) async throws -> String? { nil }
  @discardableResult
  func cancelReply(threadId _: String) -> Bool { false }
  func acceptReply(
    _: String,
    threadId _: String,
    attachmentPaths _: [String]
  ) async throws -> String { "" }

  func streamReply(
    _ text: String,
    threadId: String,
    attachmentPaths: [String],
    onDelta _: @escaping @MainActor (String) -> Void,
    onReasoning _: @escaping @MainActor (String) -> Void,
    onToolExecuting _: @escaping @MainActor (_ name: String, _ id: String) -> Void,
    onToolResult _:
      @escaping @MainActor (
        _ name: String, _ id: String, _ isError: Bool, _ reason: String
      ) -> Void
  ) async throws -> String {
    try await acceptReply(text, threadId: threadId, attachmentPaths: attachmentPaths)
  }
}

/// Records every assistive routing target and otherwise accepts an empty reply.
@MainActor
final class AssistiveTargetLogEngine: ChatEngineFixture {
  private(set) var assistiveTargets: [String?] = []

  func setAssistiveTargetThread(backendId: String?) {
    assistiveTargets.append(backendId)
  }
}

@MainActor
final class ExpectationGate {
  let entered: XCTestExpectation
  private var continuations: [CheckedContinuation<Void, Never>] = []
  private var released = false

  init(_ entered: XCTestExpectation) { self.entered = entered }

  func wait() async {
    guard !released else { return }
    await withCheckedContinuation { continuation in
      continuations.append(continuation)
      if continuations.count == 1 { entered.fulfill() }
    }
  }

  func release() {
    released = true
    let pending = continuations
    continuations.removeAll()
    for continuation in pending { continuation.resume() }
  }
}

extension XCTestCase {
  /// Refresh triggers coalesce onto the next main-queue tick, so a popover
  /// close never runs the re-read inside the notification callout (sample
  /// 2026-08-07 10:43, main thread 93/93 in
  /// `_NSPopoverCloseAndAnimate → refreshThreadsFromExternalChange`).
  /// Tests drain that tick before asserting.
  func drainMainQueue() {
    let drained = expectation(description: "main queue drained")
    DispatchQueue.main.async { drained.fulfill() }
    wait(for: [drained], timeout: 2)
  }
}

func projectedAcousticReceipt(
  serial: String,
  sessionId: String,
  sampleStart: UInt64,
  sampleEnd: UInt64,
  wordEvidence: [String],
  layerDecisions: [String],
  captureEpoch: UInt64 = 1,
  durationMs: UInt64 = 1_000,
  energyIntegral: Double = 1,
  meanRmsDbfs: Float = -20,
  peakDbfs: Float = -6,
  vadOpenSample: UInt64? = nil,
  vadCloseSample: UInt64? = nil,
  calibration: String = "test-v1",
  sealReceipt: String? = nil,
  manualEditReceipt: String? = nil,
  presentationReceipt: CsProjectedPresentationReceipt? = nil
) -> CsProjectedAcousticReceipt {
  CsProjectedAcousticReceipt(
    acousticSerialVersion: 1,
    acousticSerial: serial,
    sessionId: sessionId,
    captureEpoch: captureEpoch,
    sampleStart: sampleStart,
    sampleEnd: sampleEnd,
    durationMs: durationMs,
    energyIntegral: energyIntegral,
    meanRmsDbfs: meanRmsDbfs,
    peakDbfs: peakDbfs,
    vadOpenSample: vadOpenSample ?? sampleStart,
    vadCloseSample: vadCloseSample ?? sampleEnd,
    evidenceCalibrationVersion: calibration,
    wordEvidenceReceipts: wordEvidence,
    layerDecisionReceipts: layerDecisions,
    sealReceipt: sealReceipt,
    manualEditReceipt: manualEditReceipt,
    presentationReceipt: presentationReceipt
  )
}

func transcriptProjection(
  sequence: UInt64,
  emittedAt: String,
  sessionId: String,
  renderedText: String,
  phase: String,
  terminal: Bool,
  reducerAction: String,
  mode: String = "dictation",
  reducerRevision: UInt64? = nil,
  captureEpoch: UInt64 = 1,
  sampleStart: UInt64? = nil,
  sampleEnd: UInt64? = nil,
  documentIndex: UInt64? = nil,
  label: String? = nil,
  deliveryText: String? = nil,
  canPaste: Bool = false,
  canInsert: Bool = false,
  canCopy: Bool? = nil,
  canRetranscribe: Bool = false,
  canFormat: Bool = false,
  canSendToAgent: Bool = false,
  lifecycleTerminal: Bool? = nil,
  delivery: CsTranscriptDelivery = .unattempted,
  acousticReceipts: [CsProjectedAcousticReceipt] = [],
  sealCoverage: CsProjectedSealCoverageReceipt? = nil,
  consultationPresentations: [CsProjectedConsultationPresentation] = []
) -> CsTranscriptProjectionEvent {
  CsTranscriptProjectionEvent(
    schema: "codescribe.transcript_projection.v1",
    sequence: sequence,
    emittedAt: emittedAt,
    sessionId: sessionId,
    mode: mode,
    reducerRevision: reducerRevision ?? sequence,
    reducerAction: reducerAction,
    occurrenceSessionId: sessionId,
    captureEpoch: captureEpoch,
    sampleStart: sampleStart ?? (sequence - 1) * 16_000,
    sampleEnd: sampleEnd ?? sequence * 16_000,
    documentIndex: documentIndex ?? (sequence - 1),
    label: label ?? (terminal ? "terminal" : "live"),
    renderedText: renderedText,
    deliveryText: deliveryText,
    phase: phase,
    canPaste: canPaste,
    canInsert: canInsert,
    canCopy: canCopy ?? !renderedText.isEmpty,
    canRetranscribe: canRetranscribe,
    canFormat: canFormat,
    canSendToAgent: canSendToAgent,
    terminal: terminal,
    lifecycleTerminal: lifecycleTerminal ?? terminal,
    delivery: delivery,
    acousticReceipts: acousticReceipts,
    sealCoverage: sealCoverage,
    consultationPresentations: consultationPresentations
  )
}
