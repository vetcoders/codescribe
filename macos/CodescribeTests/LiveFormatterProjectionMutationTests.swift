import XCTest

@testable import Codescribe

@MainActor
final class LiveFormatterProjectionMutationTests: XCTestCase {
  /// Kills mutant 19 (live-swift-consumes-rendered-text): the live consumer must
  /// paint deliveryText even while renderedText retains the reducer's Raw floor.
  func testLiveConsumerPaintsDeliveryTextInsteadOfRenderedText() {
    let state = OverlayState()
    let sessionId = "m4-live-consumer"
    let raw = "prosze sprawdzic codescribe kolejny fragment"
    let delivery = "Proszę sprawdzić Codescribe. kolejny fragment"
    let receipt = projectedAcousticReceipt(
      serial: "m4-live-acoustic",
      sessionId: sessionId,
      sampleStart: 0,
      sampleEnd: 16_000,
      wordEvidence: ["m4-word-1"],
      layerDecisions: ["m4-layer-1"]
    )

    state.handleRecordingPreparing()
    state.applyCompactProjection(
      CsCompactProjection(
        sessionId: sessionId,
        captureEpoch: 1,
        sequence: 1,
        text: raw,
        degraded: false,
        evidence: []
      )
    )
    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: 1,
        emittedAt: "2026-10-31T10:00:00Z",
        sessionId: sessionId,
        renderedText: raw,
        phase: "listening",
        terminal: false,
        reducerAction: "record_ledger_projection",
        reducerRevision: 1,
        deliveryText: raw,
        acousticReceipts: [receipt]
      )
    )
    XCTAssertEqual(state.canvasText, raw)
    XCTAssertFalse(state.showsLiveFormattedPresentation)

    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: 2,
        emittedAt: "2026-10-31T10:00:01Z",
        sessionId: sessionId,
        renderedText: raw,
        phase: "listening",
        terminal: false,
        reducerAction: "derived_projection",
        reducerRevision: 1,
        deliveryText: delivery,
        acousticReceipts: [receipt]
      )
    )

    XCTAssertTrue(state.showsLiveFormattedPresentation)
    XCTAssertEqual(state.latestTranscriptProjection?.renderedText, raw)
    XCTAssertEqual(state.latestTranscriptProjection?.deliveryText, delivery)
    XCTAssertEqual(state.formattedText, delivery)
    XCTAssertEqual(state.canvasText, delivery)
    XCTAssertNotEqual(state.canvasText, raw)
  }
}
