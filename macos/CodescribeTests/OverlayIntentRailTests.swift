import XCTest

@testable import Codescribe

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
        ("error", "draft", true, true, true, true, true, true, [.close]),
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

  func testDispatchEmitsDeferredIntentAndLeavesProjectionUntouched() {
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
    var emitted: [OverlayIntent] = []
    state.onDeferredBackendIntent = { emitted.append($0) }
    let rail = OverlayIntentRail(
      phase: state.statusText,
      intents: OverlayIntentRail.projectedIntents(for: state),
      palette: .dark,
      onIntent: state.relayIntent
    )

    rail.dispatch(.retranscribe)
    rail.dispatch(.format)

    XCTAssertEqual(emitted, [.retranscribe, .format])
    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(state.formattedText, "final")
    XCTAssertEqual(state.revision, 1)
    XCTAssertTrue(state.canPaste)
    XCTAssertTrue(state.canInsert)
    XCTAssertTrue(state.canCopy)
    XCTAssertTrue(state.canRetranscribe)
    XCTAssertTrue(state.canFormat)
    XCTAssertTrue(state.terminal)
    XCTAssertNil(state.toast)
  }

  func testEveryIntentHasVoiceOverCopyAndRailReportsProjectedPhase() {
    let intents: [OverlayIntent] = [
      .finish, .copy, .insertPaste, .retranscribe, .format, .close,
    ]

    XCTAssertEqual(
      intents.map(\.accessibilityLabel),
      [
        "Finish recording",
        "Copy transcript",
        "Insert transcript",
        "Retranscribe recording",
        "Format transcript",
        "Close overlay",
      ]
    )
    XCTAssertTrue(intents.allSatisfy { !$0.accessibilityHint.isEmpty })
    XCTAssertEqual(OverlayIntentRail.accessibilityValue(for: "no speech"), "no speech")
  }

  func testReduceMotionDisablesRailAnimation() {
    XCTAssertNil(OverlayIntentRail.revealAnimation(reduceMotion: true))
    XCTAssertNotNil(OverlayIntentRail.revealAnimation(reduceMotion: false))
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
        phase: phase,
        canPaste: canPaste,
        canInsert: canInsert,
        canCopy: canCopy,
        canRetranscribe: canRetranscribe,
        canFormat: canFormat,
        terminal: terminal,
        acousticReceipts: []
      )
    )
    return state
  }
}
