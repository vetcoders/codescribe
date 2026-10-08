import AppKit
import XCTest

@testable import Codescribe

/// The status light and the warning copy are the overlay's legend: every
/// colour has a name, and every warning says whose fact it is.
@MainActor
final class OverlayRecordingLightTests: XCTestCase {
  func testProductionFooterDoesNotExposeSealDiagnostics() {
    XCTAssertFalse(
      DeveloperSurface.isEnabled(), "The XCTest host is the production-surface control")
    let slots = OverlayBottomChromeSlots(
      mode: .coverageRefused, hasPresentationStatus: false, isCollapsed: false)
    XCTAssertEqual(
      slots.ordered, [.rail],
      "Unverified seal diagnostics are power-mode details, not a production footer")
  }

  func testQuietMicrophoneAdvisoryRemainsUserFacing() {
    let slots = OverlayBottomChromeSlots(
      mode: .listening, hasPresentationStatus: false, isCollapsed: false,
      hasLowInputSignal: true)
    XCTAssertEqual(slots.ordered, [.rail, .coverageWarning])
    XCTAssertEqual(OverlayWarningCopy.quietInput.owner, .microphone)
  }

  func testCoverageDiagnosticsCannotDisplaceAnActionableStatus() {
    for mode in [OverlayMode.coverageRefused, .error, .noSpeech] {
      let slots = OverlayBottomChromeSlots(
        mode: mode, hasPresentationStatus: true, isCollapsed: false)
      XCTAssertEqual(slots.ordered, [.rail])
    }
    XCTAssertEqual(
      OverlayBottomChromeSlots(
        mode: .coverageRefused, hasPresentationStatus: false, isCollapsed: true
      ).ordered, [])
  }

  func testTechnicalCoverageRequiresBothDeveloperBuildAndLabToggle() {
    for developerBuild in [false, true] {
      for labEnabled in [false, true] {
        let power = DeveloperSurface.isPowerModeEnabled(
          labMode: labEnabled, surfaceEnabled: developerBuild)
        let slots = OverlayBottomChromeSlots(
          mode: .coverageRefused, hasPresentationStatus: false, isCollapsed: false,
          showsDiagnostics: power)
        XCTAssertEqual(
          slots.ordered,
          developerBuild && labEnabled ? [.rail, .coverageWarning] : [.rail],
          "developer build=\(developerBuild), lab=\(labEnabled)")
        let quiet = OverlayBottomChromeSlots(
          mode: .listening, hasPresentationStatus: false, isCollapsed: false,
          hasLowInputSignal: true, showsDiagnostics: power)
        XCTAssertEqual(quiet.ordered, [.rail, .coverageWarning])
      }
    }
  }

  func testDeveloperCoverageDiagnosticsRespectCollapseAndActionableStatus() {
    for collapsed in [false, true] {
      for actionableStatus in [false, true] {
        let slots = OverlayBottomChromeSlots(
          mode: .coverageRefused, hasPresentationStatus: actionableStatus,
          isCollapsed: collapsed, showsDiagnostics: true)
        XCTAssertEqual(
          slots.ordered,
          collapsed ? [] : actionableStatus ? [.rail] : [.rail, .coverageWarning])
      }
    }
  }

  func testDiagnosticVisibilityDoesNotChangeTranscriptAuthorityOrRecoveryActions() {
    let state = OverlayState()
    let acoustic = projectedAcousticReceipt(
      serial: "diagnostic-visibility-1", sessionId: "diagnostic-visibility", sampleStart: 0,
      sampleEnd: 16000, wordEvidence: ["diagnostic-word"], layerDecisions: ["diagnostic-layer"])
    let projection = transcriptProjection(
      sequence: 1, emittedAt: "2026-10-03T00:00:00Z", sessionId: "diagnostic-visibility",
      renderedText: "Retained words remain editable", phase: "coverage_refused", terminal: true,
      reducerAction: "session_ended", canCopy: true, acousticReceipts: [acoustic],
      sealCoverage: coverage(.incomplete, speech: 32000, covered: 16000))
    state.applyTranscriptProjection(projection)
    let text = state.activeText
    let revision = state.revision
    let notice = state.coverageRefusalNotice
    let intents = OverlayIntentRail.projectedIntents(for: state)
    XCTAssertFalse(text.isEmpty)
    XCTAssertTrue(state.isTranscriptEditable)
    XCTAssertNotNil(notice)
    for power in [false, true] {
      _ = OverlayBottomChromeSlots(
        mode: state.mode, hasPresentationStatus: false,
        isCollapsed: false, showsDiagnostics: power)
      XCTAssertEqual(state.activeText, text)
      XCTAssertEqual(state.revision, revision)
      XCTAssertEqual(state.coverageRefusalNotice, notice)
      XCTAssertEqual(OverlayIntentRail.projectedIntents(for: state), intents)
      XCTAssertTrue(state.isTranscriptEditable)
    }
  }

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

  func testBusCaptureUsesAgentLightOverSettledOrdinaryTakeAndQuietMeter() {
    let state = OverlayState()
    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: 1, emittedAt: "2026-10-07T21:00:00Z", sessionId: "settled-ordinary",
        renderedText: "Saved dictation", phase: "formatted", terminal: true,
        reducerAction: "session_ended", canCopy: true))
    XCTAssertTrue(state.terminal)
    XCTAssertFalse(state.recording)
    XCTAssertNil(state.recordingLight)
    let text = state.activeText
    let revision = state.revision
    state.applyChannelRoster([
      .init(
        channel: "4", audience: "bruno", provider: "codex", providerSessionId: "bus-agent",
        open: true, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true)
    ])
    XCTAssertTrue(state.audioCaptureActive)
    XCTAssertTrue(state.usesAgentAccent)
    XCTAssertEqual(state.recordingLight, .agent)
    feed(state, db: -60, from: 0, seconds: 2)
    XCTAssertTrue(state.levelMeter.isSilent)
    XCTAssertEqual(state.recordingLight, .agent)
    XCTAssertEqual(state.activeText, text)
    XCTAssertEqual(state.revision, revision)
    state.applyChannelRoster([])
    XCTAssertFalse(state.audioCaptureActive)
    XCTAssertFalse(state.usesAgentAccent)
    XCTAssertNil(state.recordingLight)
    XCTAssertEqual(state.activeText, text)
  }

  func testBroadcastAndNamedBusStayVioletDuringOrdinaryFinalization() {
    for channel in ["0", "4"] {
      let state = OverlayState()
      state.applyTranscriptProjection(
        transcriptProjection(
          sequence: 1, emittedAt: "2026-10-07T22:00:00Z", sessionId: "ordinary-finalizing",
          renderedText: "Retained text", phase: "finalizing", terminal: false,
          reducerAction: "session_ended", canCopy: true))
      XCTAssertEqual(state.mode, .finalizing)
      XCTAssertEqual(state.recordingLight, .processing)
      let text = state.activeText
      let revision = state.revision
      state.applyChannelRoster([
        .init(
          channel: channel, audience: channel == "0" ? "*" : "bruno", provider: "codex",
          providerSessionId: "bus-agent", open: true, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: true)
      ])
      XCTAssertTrue(state.channelAudioCaptureActive)
      XCTAssertTrue(state.usesAgentAccent)
      XCTAssertEqual(state.recordingLight, .agent)
      XCTAssertEqual(state.activeText, text)
      XCTAssertEqual(state.revision, revision)
      state.applyChannelRoster([])
      XCTAssertFalse(state.usesAgentAccent)
      XCTAssertEqual(state.recordingLight, .processing)
      XCTAssertEqual(state.activeText, text)
      XCTAssertEqual(state.revision, revision)
    }
  }

  func testClosedBusRosterKeepsOrdinaryAccentAndBuiltInAgentKeepsViolet() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.applyIndicatorMode(.hold)
    state.applyChannelRoster([
      .init(
        channel: "4", audience: "bruno", provider: "codex", providerSessionId: "bus-agent",
        open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true)
    ])
    XCTAssertFalse(state.channelAudioCaptureActive)
    XCTAssertFalse(state.usesAgentAccent)
    XCTAssertEqual(state.recordingLight, .holdToTalk)
    state.applyIndicatorMode(.toggle)
    XCTAssertFalse(state.usesAgentAccent)
    XCTAssertEqual(state.recordingLight, .handsFree)
    state.applyIndicatorMode(.assistive)
    XCTAssertTrue(state.usesAgentAccent)
    XCTAssertEqual(state.recordingLight, .agent)
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

  /// Founder 2026-10-01: say what happened, not "engine missed … text kept".
  func testSealCoverageDescribesTheObservationWithoutBlameOrDeliveryClaims() {
    let cases: [(CsProjectedSealCoverageReceipt?, String, String)] = [
      (
        nil, "No coverage measurement for this take",
        "No coverage measurement was recorded for this take."
      ),
      (
        coverage(.incomplete, speech: 32_000, covered: 29_440), "Text verification incomplete",
        "Text verification is incomplete; this receipt provides no usable uncovered speech interval. You can review and recover the available text."
      ),
      (
        coverage(.unavailable, reason: .notObserved), "Speech coverage not measured",
        "Speech coverage was not measured because no acoustic measurement was taken."
      ),
      (
        coverage(.complete, speech: 32_000, covered: 32_000), "Take not sealed yet",
        "Speech coverage was measured as complete, but this take has no terminal seal."
      ),
      (
        coverage(.unknown), "No coverage measurement for this take",
        "No coverage measurement was recorded for this take."
      ),
    ]
    for (receipt, chip, sentence) in cases {
      let copy = OverlayWarningCopy.sealRefused(receipt)
      XCTAssertEqual(copy.owner, .coverage)
      XCTAssertEqual(copy.chip, chip)
      XCTAssertEqual(copy.sentence, sentence)
      for text in [copy.chip, copy.sentence] {
        for blame in ["engine", "missed", "kept", "calibration", "quality"] {
          XCTAssertFalse(text.localizedCaseInsensitiveContains(blame), text)
        }
      }
    }
  }

  func testUncoveredSpeechIsPlacedOnlyWithTheMeasuredClock() {
    func gaps(_ ranges: [(UInt64, UInt64)]) -> CsProjectedSealCoverageReceipt {
      var receipt = coverage(.incomplete, speech: 1_000_000, covered: 900_000)
      receipt.uncoveredSpeechRanges = ranges.map {
        CsProjectedSealCoverageRange(sampleStart: $0.0, sampleEnd: $0.1)
      }
      return receipt
    }
    let one = OverlayWarningCopy.sealRefused(gaps([(576_000, 638_400)]), sampleRateHz: 48_000)
    XCTAssertEqual(one.chip, "Ledger: 1.3 s to verify · ranges: 1")
    XCTAssertEqual(
      one.sentence,
      "Unresolved ledger ranges: 0:12–0:14. This measures alignment coverage, not missing words. Review these audio intervals in Voice Lab."
    )
    let two = OverlayWarningCopy.sealRefused(
      gaps([(928_000, 939_200), (192_000, 208_000)]), sampleRateHz: 16_000)
    XCTAssertEqual(two.chip, "Ledger: 1.7 s to verify · ranges: 2")
    let long = OverlayWarningCopy.sealRefused(gaps([(1_040_000, 1_056_000)]), sampleRateHz: 16_000)
    XCTAssertEqual(long.chip, "Ledger: 1.0 s to verify · ranges: 1")
    let tiny = OverlayWarningCopy.sealRefused(gaps([(0, 1_000)]), sampleRateHz: 48_000)
    XCTAssertEqual(tiny.chip, "Ledger: <0.1 s to verify · ranges: 1")
    for rate in [nil, UInt32(0)] {
      XCTAssertEqual(
        OverlayWarningCopy.sealRefused(gaps([(0, 16_000)]), sampleRateHz: rate).chip,
        "Speech coverage incomplete", "no clock, no invented position")
    }
    XCTAssertEqual(
      OverlayWarningCopy.sealRefused(gaps([]), sampleRateHz: 16_000).chip,
      "Text verification incomplete")
  }

  /// The live chip reads the take's own clock from the receipt it shows.
  func testRefusedTakeChipPlacesSpeechWithTheReceiptClock() {
    let state = OverlayState()
    var receipt = coverage(.incomplete, speech: 96_000, covered: 33_600)
    receipt.sampleRateHz = 48_000
    receipt.uncoveredSpeechRanges = [
      CsProjectedSealCoverageRange(sampleStart: 576_000, sampleEnd: 638_400)
    ]
    let acoustic = projectedAcousticReceipt(
      serial: "light-clock-1", sessionId: "light-clock", sampleStart: 0,
      sampleEnd: 16_000, wordEvidence: ["light-clock-word"],
      layerDecisions: ["light-clock-layer"])
    let projection = transcriptProjection(
      sequence: 1, emittedAt: "2026-10-01T00:00:00Z", sessionId: "light-clock",
      renderedText: "kept words", phase: "coverage_refused", terminal: true,
      reducerAction: "session_ended", canCopy: true, acousticReceipts: [acoustic],
      sealCoverage: receipt)
    state.applyTranscriptProjection(projection)
    XCTAssertEqual(state.footerWarning?.chip, "Ledger: 1.3 s to verify · ranges: 1")
    XCTAssertEqual(state.coverageRefusalNotice, state.footerWarning?.sentence)
    XCTAssertTrue(state.coverageRefusalNotice?.contains("0:12–0:14") == true)
  }

  func testOverlappingLedgerRangesAreCountedOnceAndStayOutOfTheChip() {
    var receipt = coverage(.incomplete, speech: 160_000, covered: 0)
    receipt.uncoveredSpeechRanges = [
      CsProjectedSealCoverageRange(sampleStart: 80_000, sampleEnd: 96_000),
      CsProjectedSealCoverageRange(sampleStart: 16_000, sampleEnd: 48_000),
      CsProjectedSealCoverageRange(sampleStart: 32_000, sampleEnd: 64_000),
      CsProjectedSealCoverageRange(sampleStart: 16_000, sampleEnd: 48_000),
    ]
    let copy = OverlayWarningCopy.sealRefused(receipt, sampleRateHz: 16_000)
    XCTAssertEqual(copy.chip, "Ledger: 4.0 s to verify · ranges: 2")
    XCTAssertFalse(copy.chip.contains("0:"), "intervals belong in the opened detail")
    XCTAssertTrue(copy.sentence.contains("0:01–0:04, 0:05–0:06"))
    XCTAssertTrue(copy.sentence.contains("not missing words"))
  }

  func testRefusedTakeShowsTheCoverageSentenceNotAMicrophoneHint() {
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

  func testLiveDiagnosticDoesNotInventMissingWordsOrRecoveryWork() {
    let copy = OverlayWarningCopy.liveTranscriptBehind
    XCTAssertEqual(copy.owner, .engine)
    XCTAssertTrue(copy.sentence.contains("does not measure missing words"))
    XCTAssertTrue(copy.sentence.contains("or prove that a recovery pass is running"))
  }

  /// The header icon's tooltip and VoiceOver label are the same sentence.
  func testHeaderWarningIconCarriesTheSentenceAsTooltip() throws {
    let source = try String(
      contentsOf: URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Codescribe/Screens/Overlay/DictationOverlayView.swift"),
      encoding: .utf8)
    XCTAssertTrue(source.contains(".help(transcriptPreviewHelp)"))
    XCTAssertTrue(
      source.contains(".accessibilityValue(transcriptPreviewHelp)"))
    XCTAssertTrue(source.contains("OverlayWarningCopy.liveTranscriptBehind.sentence"))
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
    let start = try XCTUnwrap(source.range(of: "private func recordingControls"))
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
        canFinish: light != .processing, presentationMode: .expanded, compact: false,
        palette: .dark, onIntent: { intents.append($0) }, onPreviewToggle: {},
        isFinalizing: light == .processing, recordingLight: light)
      XCTAssertEqual(control.recordingSymbol, "stop.fill", "\(light)")
      XCTAssertEqual(control.recordingStatusValue, light.name)
      XCTAssertEqual(control.recordingDisabled, light == .processing)
      XCTAssertEqual(
        control.recordingTint,
        light == .holdToTalk || light == .handsFree
          ? OverlayAppearancePalette.dark.errorStatus.color : light.color)
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
      status: status, sampleRateHz: nil, unavailableReason: reason, speechSamples: speech,
      coveredSamples: covered, uncoveredSpeechRanges: [],
      maxUncoveredSamples: speech > covered ? speech - covered : 0,
      incompleteThresholdSamples: 4_000, speechProducer: "capture_energy",
      availability: "observed", observedSamples: nil, coverageRatio: nil)
  }
}
