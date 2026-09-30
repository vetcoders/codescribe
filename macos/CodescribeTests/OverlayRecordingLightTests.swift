import AppKit
import XCTest

@testable import Codescribe

/// The status light and the warning copy are the overlay's legend: every
/// colour has a name, and every warning says whose fact it is.
@MainActor
final class OverlayRecordingLightTests: XCTestCase {

  // MARK: One table: state → colour → name

  func testEveryLightHasItsOwnLookAndAName() {
    let lights = OverlayRecordingLight.allCases
    XCTAssertEqual(Set(lights.map(\.name)).count, lights.count)
    XCTAssertEqual(Set(lights.map(\.colorName)).count, lights.count)
    XCTAssertEqual(
      Set(lights.map { "\(ObjectIdentifier($0.nsColor))|\($0.pulses)" }).count, lights.count,
      "two states would render identically")
    for light in lights {
      XCTAssertFalse(light.meaning.isEmpty)
      XCTAssertTrue(light.tooltip.hasPrefix(light.colorName), "\(light)")
      XCTAssertTrue(light.tooltip.contains(light.name), "\(light)")
      XCTAssertTrue(light.tooltip.contains(light.meaning), "\(light)")
    }
  }

  /// The overlay names the hues the cursor badge and the menu-bar dot use.
  func testLightsSpeakTheBadgeAndTrayColours() {
    XCTAssertTrue(OverlayRecordingLight.holdToTalk.nsColor === CSPalette.indicatorRecording)
    XCTAssertTrue(OverlayRecordingLight.handsFree.nsColor === CSPalette.indicatorRecording)
    XCTAssertTrue(OverlayRecordingLight.processing.nsColor === CSPalette.modeProcessing)
    XCTAssertTrue(OverlayRecordingLight.agent.nsColor === CSPalette.assistive)
    XCTAssertTrue(OverlayRecordingLight.silence.nsColor === CSPalette.indicatorSilence)
    XCTAssertEqual(OverlayRecordingLight.allCases.filter(\.pulses), [.handsFree, .processing])
  }

  func testLightGrowsWithTheOverlayTextScale() {
    XCTAssertGreaterThan(OverlayRecordingLight.diameter(textScale: 1), 7, "the close dot is 7 pt")
    XCTAssertEqual(
      OverlayRecordingLight.diameter(textScale: TextScaleController.maxScale),
      OverlayRecordingLight.baseDiameter * TextScaleController.maxScale)
    XCTAssertLessThan(
      OverlayRecordingLight.diameter(textScale: TextScaleController.minScale),
      OverlayRecordingLight.diameter(textScale: 1))
  }

  func testPulseStaysVisibleAndNeverBlinksOut() {
    for step in 0..<120 {
      let opacity = OverlayRecordingLight.pulseOpacity(at: Double(step) / 100)
      XCTAssertGreaterThanOrEqual(opacity, 0.45 - 1e-9)
      XCTAssertLessThanOrEqual(opacity, 1 + 1e-9)
    }
  }

  func testResolveFollowsCanonicalRecordingSemantics() {
    func light(
      mode: OverlayMode = .listening, terminal: Bool = false, recording: Bool = true,
      transcribing: Bool = false, indicator: CsIndicatorMode = .hold, silent: Bool = false
    ) -> OverlayRecordingLight? {
      OverlayRecordingLight.resolve(
        mode: mode, terminal: terminal, recording: recording, transcribing: transcribing,
        indicatorMode: indicator, silent: silent)
    }
    XCTAssertEqual(light(), .holdToTalk)
    XCTAssertEqual(light(indicator: .toggle), .handsFree)
    XCTAssertEqual(light(silent: true), .silence)
    XCTAssertEqual(light(indicator: .toggle, silent: true), .silence)
    XCTAssertEqual(
      light(indicator: .assistive, silent: true), .agent, "silence never hides the agent")
    XCTAssertEqual(light(indicator: .processing), .processing)
    XCTAssertEqual(light(mode: .finalizing, recording: false), .processing)
    XCTAssertEqual(light(recording: false, transcribing: true), .processing)
    XCTAssertNil(light(recording: false), "no live take, no light")
    for mode in [OverlayMode.formatted, .coverageRefused, .noSpeech, .error] {
      XCTAssertNil(
        light(mode: mode, terminal: true, recording: false, indicator: .processing),
        "a settled \(mode) take speaks through its body, not the light")
    }
  }

  // MARK: Silence from measured capture level

  func testLiveTakeTurnsYellowOnlyAfterSustainedQuietAndBackOnSpeech() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.applyIndicatorMode(.toggle)
    XCTAssertEqual(state.recordingLight, .handsFree)

    feed(state, db: -60, from: 0, seconds: 1.0)
    XCTAssertEqual(state.recordingLight, .handsFree, "a pause shorter than the hold is not silence")
    feed(state, db: -60, from: 1.02, seconds: 0.4)
    XCTAssertEqual(state.recordingLight, .silence)

    state.applyAudioLevel(rms(-30), now: 1.44)
    XCTAssertEqual(state.recordingLight, .handsFree, "speech ends silence at once")

    feed(state, db: -60, from: 1.46, seconds: 1.4)
    XCTAssertEqual(state.recordingLight, .silence)
    state.finishControllerRecording()
    XCTAssertFalse(state.levelMeter.isSilent, "a stopped capture carries no silence verdict")
  }

  func testSilenceNeedsUnbrokenMeasuredBlocks() {
    let meter = AudioLevelMeter()
    // Blocks arriving 0.5 s apart are missing audio, not measured silence.
    for tick in 0...10 {
      meter.push(rms: rms(-70), now: Double(tick) * 0.5)
    }
    XCTAssertFalse(meter.isSilent)

    // A soft level between the floor and speaking level breaks a quiet run.
    meter.reset()
    for tick in 0...50 {
      let db = tick == 40 ? -50.0 : -60.0
      meter.push(rms: rms(db), now: Double(tick) / 50)
    }
    XCTAssertFalse(meter.isSilent, "the run restarted at the soft block")

    // A VAD speech verdict outranks the energy reading.
    meter.reset()
    for tick in 0...100 {
      meter.push(rms: rms(-60), speechActive: true, now: Double(tick) / 50)
    }
    XCTAssertFalse(meter.isSilent)
  }

  // MARK: Warnings say whose fact it is

  func testRefusedSealIsTheEnginesFactAndNeverSendsTheUserToCalibration() {
    let cases: [CsProjectedSealCoverageReceipt?] = [
      nil,
      coverage(.incomplete, speech: 32_000, covered: 29_440),
      coverage(.unavailable, reason: .notObserved),
      coverage(.complete, speech: 32_000, covered: 32_000),
      coverage(.unknown),
    ]
    for receipt in cases {
      let copy = OverlayWarningCopy.sealRefused(receipt)
      XCTAssertEqual(copy.owner, .engine)
      XCTAssertTrue(copy.sentence.hasPrefix("The engine"), copy.sentence)
      XCTAssertTrue(copy.chip.hasPrefix("Engine"), copy.chip)
      XCTAssertFalse(copy.sentence.localizedCaseInsensitiveContains("calibration"))
      XCTAssertFalse(copy.sentence.localizedCaseInsensitiveContains("quality"))
    }
    let incomplete = OverlayWarningCopy.sealRefused(
      coverage(.incomplete, speech: 32_000, covered: 29_440))
    XCTAssertEqual(incomplete.chip, "Engine missed 8% of your speech — text kept")
    XCTAssertEqual(
      incomplete.sentence,
      "The engine found no words for 8% of the speech it detected, so it kept this text without sealing the take."
    )
  }

  func testMissedShareIsHonestAtTheEdges() {
    XCTAssertEqual(
      OverlayWarningCopy.missedShare(coverage(.incomplete, speech: 100_000, covered: 99_900)),
      "under 1%")
    XCTAssertEqual(
      OverlayWarningCopy.missedShare(coverage(.incomplete, speech: 0, covered: 0)), "part")
    XCTAssertEqual(
      OverlayWarningCopy.missedShare(coverage(.incomplete, speech: 1_000, covered: 2_000)), "part")
  }

  func testRefusedTakeShowsTheEngineSentenceNotAMicrophoneHint() {
    let state = OverlayState()
    let receipt = coverage(.incomplete, speech: 32_000, covered: 16_000)
    let acoustic = projectedAcousticReceipt(
      serial: "light-refusal-1", sessionId: "light-refusal", sampleStart: 0,
      sampleEnd: 16_000, wordEvidence: ["light-refusal-word"],
      layerDecisions: ["light-refusal-layer"])
    let projection = transcriptProjection(
      sequence: 1, emittedAt: "2026-09-29T00:00:00Z", sessionId: "light-refusal",
      renderedText: "kept words", phase: "coverage_refused", terminal: true,
      reducerAction: "session_ended", canCopy: true, acousticReceipts: [acoustic],
      sealCoverage: receipt)
    state.applyTranscriptProjection(projection)
    XCTAssertEqual(state.mode, .coverageRefused)
    XCTAssertEqual(state.footerWarning, OverlayWarningCopy.sealRefused(receipt))
    XCTAssertEqual(state.coverageRefusalNotice, state.footerWarning?.sentence)
    XCTAssertNil(state.recordingLight, "a settled take has no status light")
  }

  func testLiveTranscriptWarningBlamesTheEngine() {
    let copy = OverlayWarningCopy.liveTranscriptBehind
    XCTAssertEqual(copy.owner, .engine)
    XCTAssertTrue(copy.sentence.contains("not your microphone"))
  }

  /// The header icon's tooltip and VoiceOver label are the same sentence.
  func testHeaderWarningIconCarriesTheSentenceAsTooltip() throws {
    let source = try String(
      contentsOf: URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Codescribe/Screens/Overlay/DictationOverlayView.swift"),
      encoding: .utf8)
    XCTAssertTrue(source.contains(".help(OverlayWarningCopy.liveTranscriptBehind.sentence)"))
    XCTAssertTrue(
      source.contains(".accessibilityLabel(OverlayWarningCopy.liveTranscriptBehind.sentence)"))
    XCTAssertFalse(source.contains("OverlayRecordingLightView("))
    XCTAssertTrue(source.contains("recordingLight: state.recordingLight"))
  }

  // MARK: Helpers

  func testHeaderControlCarriesEveryLightWithoutASeparateDot() throws {
    let source = try String(
      contentsOf: URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Codescribe/Screens/Overlay/DictationOverlayView.swift"),
      encoding: .utf8)
    let start = try XCTUnwrap(source.range(of: "private func justifiedHeader"))
    let end = try XCTUnwrap(source.range(of: "private func chromeWaveform"))
    let header = String(source[start.lowerBound..<end.lowerBound])
    XCTAssertFalse(header.contains("OverlayRecordingLightView("))
    XCTAssertEqual(header.components(separatedBy: "OverlayRecordingControls(").count - 1, 1)
    XCTAssertTrue(header.contains("recordingLight: state.recordingLight, animates: overlayVisible"))
    XCTAssertTrue(source.contains("recordingLight?.pulses == true && animates && !reduceMotion"))
    XCTAssertTrue(source.contains("OverlayRecordingLight.pulseOpacity(at:"))
    XCTAssertTrue(source.contains(".accessibilityValue(recordingStatusValue)"))
    for light in OverlayRecordingLight.allCases {
      var intents: [OverlayIntent] = []
      let control = OverlayRecordingControls(
        canFinish: light != .processing, isPreviewCollapsed: false, compact: false,
        palette: .dark, onIntent: { intents.append($0) }, onPreviewToggle: {},
        isFinalizing: light == .processing, recordingLight: light)
      XCTAssertEqual(control.recordingSymbol, "stop.fill", "\(light)")
      XCTAssertEqual(control.recordingStatusValue, light.name)
      XCTAssertEqual(control.recordingDisabled, light == .processing)
      XCTAssertEqual(control.recordingTint,
        light == .holdToTalk || light == .handsFree ? OverlayAppearancePalette.dark.errorStatus.color : light.color)
      control.activateRecordingControl()
      XCTAssertEqual(intents, light == .processing ? [] : [.finish])
      XCTAssertEqual(OverlayRecordingControls.controlDiameter, 22)
    }
  }

  private func rms(_ db: Double) -> Float {
    Float(pow(10, db / 20))
  }

  private func feed(_ state: OverlayState, db: Double, from start: Double, seconds: Double) {
    for tick in 0...Int((seconds * 50).rounded()) {
      state.applyAudioLevel(rms(db), now: start + Double(tick) / 50)
    }
  }

  private func coverage(
    _ status: CsSealCoverageStatus,
    reason: CsCoverageUnavailableReason? = nil,
    speech: UInt64 = 0,
    covered: UInt64 = 0
  ) -> CsProjectedSealCoverageReceipt {
    CsProjectedSealCoverageReceipt(
      status: status, unavailableReason: reason, speechSamples: speech,
      coveredSamples: covered, uncoveredSpeechRanges: [],
      maxUncoveredSamples: speech > covered ? speech - covered : 0,
      incompleteThresholdSamples: 4_000, speechProducer: "capture_energy",
      availability: "observed", observedSamples: nil, coverageRatio: nil)
  }
}
