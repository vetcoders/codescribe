import XCTest

@testable import Codescribe

@MainActor
final class OverlayFormattingAcceptanceTests: XCTestCase {
  func testLiveDerivedProjectionPaintsFormattedCanvasDuringActiveTakeAndYieldsToNewerRawRevision() {
    let state = OverlayState()
    let sessionId = "live-derived-session"

    state.handleRecordingPreparing()
    state.applyCompactProjection(
      CsCompactProjection(
        sessionId: sessionId,
        captureEpoch: 1,
        sequence: 1,
        text: "prosze sprawdzic codescribe dzisiaj",
        degraded: false,
        evidence: []
      )
    )

    let rawReceipt = projectedAcousticReceipt(
      serial: "acoustic-1",
      sessionId: sessionId,
      sampleStart: 0,
      sampleEnd: 16_000,
      wordEvidence: ["word-1"],
      layerDecisions: ["layer-1"]
    )
    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: 1,
        emittedAt: "2026-10-31T10:00:00Z",
        sessionId: sessionId,
        renderedText: "prosze sprawdzic codescribe dzisiaj",
        phase: "listening",
        terminal: false,
        reducerAction: "record_ledger_projection",
        reducerRevision: 1,
        deliveryText: "prosze sprawdzic codescribe dzisiaj",
        acousticReceipts: [rawReceipt]
      )
    )

    XCTAssertFalse(state.showsLiveFormattedPresentation)
    XCTAssertEqual(state.canvasText, "prosze sprawdzic codescribe dzisiaj")

    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: 2,
        emittedAt: "2026-10-31T10:00:01Z",
        sessionId: sessionId,
        renderedText: "prosze sprawdzic codescribe dzisiaj",
        phase: "listening",
        terminal: false,
        reducerAction: "derived_projection",
        reducerRevision: 1,
        deliveryText: "Proszę sprawdzić Codescribe dzisiaj.",
        acousticReceipts: [rawReceipt]
      )
    )

    XCTAssertTrue(state.showsLiveFormattedPresentation)
    XCTAssertEqual(state.canvasText, "Proszę sprawdzić Codescribe dzisiaj.")

    state.applyCompactProjection(
      CsCompactProjection(
        sessionId: sessionId,
        captureEpoch: 1,
        sequence: 2,
        text: "prosze sprawdzic codescribe dzisiaj kolejne zdanie",
        degraded: false,
        evidence: []
      )
    )
    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: 3,
        emittedAt: "2026-10-31T10:00:02Z",
        sessionId: sessionId,
        renderedText: "prosze sprawdzic codescribe dzisiaj kolejne zdanie",
        phase: "listening",
        terminal: false,
        reducerAction: "record_ledger_projection",
        reducerRevision: 2,
        deliveryText: "prosze sprawdzic codescribe dzisiaj kolejne zdanie",
        acousticReceipts: [rawReceipt]
      )
    )

    XCTAssertFalse(state.showsLiveFormattedPresentation)
    XCTAssertEqual(
      state.canvasText,
      "prosze sprawdzic codescribe dzisiaj kolejne zdanie"
    )
  }

  func testLateNonTerminalLiveDerivedProjectionAfterSessionEndIsIgnored() {
    let state = OverlayState()
    let sessionId = "ended-live-session"
    let receipt = projectedAcousticReceipt(
      serial: "acoustic-final",
      sessionId: sessionId,
      sampleStart: 0,
      sampleEnd: 16_000,
      wordEvidence: ["word-1"],
      layerDecisions: ["layer-1"],
      sealReceipt: "seal-1"
    )

    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: 1,
        emittedAt: "2026-10-31T10:00:00Z",
        sessionId: sessionId,
        renderedText: "surowy tekst koncowy",
        phase: "formatted",
        terminal: true,
        reducerAction: "record_ledger_terminal_seal",
        reducerRevision: 1,
        deliveryText: "Surowy tekst końcowy.",
        canCopy: true,
        canFormat: true,
        lifecycleTerminal: true,
        acousticReceipts: [receipt]
      )
    )
    XCTAssertEqual(state.formattedText, "surowy tekst koncowy")

    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: 2,
        emittedAt: "2026-10-31T10:00:01Z",
        sessionId: sessionId,
        renderedText: "surowy tekst koncowy",
        phase: "listening",
        terminal: false,
        reducerAction: "derived_projection",
        reducerRevision: 1,
        deliveryText: "Spóźniony live widok, który nie może nadpisać końca.",
        lifecycleTerminal: false,
        acousticReceipts: [receipt]
      )
    )

    XCTAssertEqual(state.formattedText, "surowy tekst koncowy")
    XCTAssertFalse(state.showsLiveFormattedPresentation)
  }

  func testAutomaticFormattingChoicesExcludeMaxWhileManualFormatMenuKeepsMax() {
    XCTAssertEqual(
      FormattingPolicyOption.automaticCases,
      [.off, .correction, .smart]
    )
    XCTAssertEqual(FormattingPolicyOption.max.automaticLevel, .smart)
    XCTAssertEqual(FormattingPolicyOption.smart.automaticLevel, .smart)
    XCTAssertEqual(FormattingPolicyOption.correction.automaticLevel, .correction)
    XCTAssertEqual(FormattingPolicyOption.off.automaticLevel, .off)

    XCTAssertEqual(
      OverlayIntentRail.formatOnceLevels,
      [.correction, .smart, .max]
    )
  }

  func testVersionsPresentationKeepsRawFloorAccessibleAndTracksQualityProvenance() {
    let versions = [
      CsDocumentVersion(
        step: 0,
        provenance: "raw",
        detail: nil,
        renderedText: "surowy tekst z nagrania",
        emittedAt: "2026-10-31T10:00:00Z",
        receiptId: "v0"
      ),
      CsDocumentVersion(
        step: 1,
        provenance: "formatter",
        detail: "smart",
        renderedText: "Surowy tekst z nagrania.",
        emittedAt: "2026-10-31T10:00:01Z",
        receiptId: "v1"
      ),
      CsDocumentVersion(
        step: 2,
        provenance: "formatter",
        detail: "max",
        renderedText: "Wersja Max po konsultacji.",
        emittedAt: "2026-10-31T10:00:02Z",
        receiptId: "v2"
      ),
    ]

    let atMax = OverlayTranscriptVersionsPresentation.make(
      versions: versions,
      cursor: 2,
      blockedReason: nil
    )
    XCTAssertTrue(atMax.canUndo)
    XCTAssertEqual(atMax.selectableSteps, [0, 1])
    XCTAssertEqual(
      atMax.qualityProvenance,
      OverlayQualityProvenance(rawText: "surowy tekst z nagrania", formattingLevel: .max)
    )

    let atRaw = OverlayTranscriptVersionsPresentation.make(
      versions: versions,
      cursor: 0,
      blockedReason: nil
    )
    XCTAssertTrue(atRaw.canRedo)
    XCTAssertEqual(atRaw.current?.text, "surowy tekst z nagrania")
    XCTAssertEqual(
      atRaw.qualityProvenance,
      OverlayQualityProvenance(rawText: "surowy tekst z nagrania", formattingLevel: .off)
    )
  }
}
