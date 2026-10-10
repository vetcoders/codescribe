import XCTest

@testable import Codescribe

/// Pins the cloud-privacy settings copy to the contract the Rust core
/// enforces (`core/config/cloud_asr.rs`, `core/asr_session/consent.rs`,
/// `core/asr_session/cloud.rs`, `core/config/keychain.rs`,
/// `bridge/src/recording.rs`). The copy is a promise surface: these tests
/// guard the promises, not the sentences, so the wording can be rewritten
/// while the enforced behavior stays described truthfully.
@MainActor
final class CloudPrivacyCopyTests: XCTestCase {
  /// Every detail subsection is headed and non-empty, and collapsing the
  /// details did not drop a safeguard: all four enforced promises still render.
  func testDetailBlocksAreHeadedAndKeepEverySafeguard() {
    XCTAssertFalse(CloudPrivacyCopy.detailBlocks.isEmpty)
    for block in CloudPrivacyCopy.detailBlocks {
      XCTAssertFalse(block.heading.isEmpty, "block \(block.id) needs a heading")
      XCTAssertFalse(block.lines.isEmpty, "block \(block.id) needs at least one line")
      for line in block.lines {
        XCTAssertFalse(line.isEmpty, "block \(block.id) carries an empty line")
      }
    }
    let rendered = CloudPrivacyCopy.detailBlocks.flatMap(\.lines)
    for safeguard in [
      CloudPrivacyCopy.diagnostics, CloudPrivacyCopy.apiKeys,
      CloudPrivacyCopy.withoutConsent, CloudPrivacyCopy.localPowerInstall,
    ] {
      XCTAssertTrue(
        rendered.contains(safeguard),
        "hiding the details behind a disclosure must not remove a safeguard")
    }
    // Round 14: the details caption is gone — the expanded subsection
    // headings carry that information now.
    XCTAssertFalse(CloudPrivacyCopy.configureCloudServices.isEmpty)
  }

  /// Cloud audio egress is consent-gated, and the copy says the consent is
  /// explicit rather than implied by picking a mode. The arming condition
  /// reads on the Audio egress row.
  func testCopyStatesConsentGatesCloudAudioEgress() {
    XCTAssertTrue(
      CloudPrivacyCopy.withoutConsent.localizedCaseInsensitiveContains("without your consent"),
      "the refusal line must name the missing consent as the cause")
    XCTAssertTrue(
      CloudPrivacyCopy.egressAudioDetail.localizedCaseInsensitiveContains("cloud mode"),
      "the egress condition must name the mode that arms it")
  }

  /// The stored consent record is a row of its own, and the copy denies the
  /// reading that a stored grant means audio is leaving now.
  func testConsentIsShownSeparatelyFromTheSelectedMode() {
    XCTAssertNotEqual(CloudPrivacyCopy.currentModeLabel, CloudPrivacyCopy.savedConsentLabel)
    XCTAssertFalse(CloudPrivacyCopy.currentModeLabel.isEmpty)
    XCTAssertFalse(CloudPrivacyCopy.savedConsentLabel.isEmpty)
    XCTAssertNotEqual(CloudPrivacyCopy.consentGranted, CloudPrivacyCopy.consentNotGranted)
    XCTAssertTrue(
      CloudPrivacyCopy.consentIsNotLiveEgress.localizedCaseInsensitiveContains("does not mean"),
      "a saved grant must be separated from live egress in words, not only in layout")
  }

  /// A refusal resolves to Apple on-device, and the copy rules out a local
  /// model loaded as a silent substitute (round 14: one sentence).
  func testRefusalResolvesToAppleOnDeviceWithoutLoadingLocalWeights() {
    let line = CloudPrivacyCopy.withoutConsent
    XCTAssertTrue(line.localizedCaseInsensitiveContains("does not use the cloud"))
    XCTAssertTrue(line.localizedCaseInsensitiveContains("Apple on-device"))
    XCTAssertTrue(
      line.localizedCaseInsensitiveContains("no local model is loaded"),
      "the copy must deny the hidden local substitution")
  }

  /// Choosing the local lane is not an install, so the copy must not promise
  /// that picking Local power downloads weights.
  func testLocalPowerSelectionIsNotDescribedAsADownload() {
    XCTAssertTrue(
      CloudPrivacyCopy.localPowerInstall.localizedCaseInsensitiveContains("does not download"),
      "the copy must separate choosing the lane from installing the model")
  }

  /// Both egress surfaces are named: the live Cloud mode session and the
  /// explicit re-transcription of a finished recording, which does not run
  /// through the mode picker.
  func testEgressNamesBothCloudModeAndExplicitRetranscription() {
    let audio = CloudPrivacyCopy.egressAudioDetail
    XCTAssertTrue(audio.localizedCaseInsensitiveContains("Cloud mode"))
    XCTAssertTrue(
      audio.localizedCaseInsensitiveContains("re-transcription"),
      "the second audio egress must be named, not folded into Cloud mode")
    XCTAssertTrue(
      CloudPrivacyCopy.egressTextDetail.localizedCaseInsensitiveContains("AI requests"),
      "text egress belongs to configured AI providers")
    XCTAssertNotEqual(CloudPrivacyCopy.egressAudioTitle, CloudPrivacyCopy.egressTextTitle)
  }

  /// Telemetry is bounded exactly like the typed core telemetry: identifiers
  /// and counters, never audio or transcript content.
  func testTelemetryCopyExcludesAudioAndTranscriptText() {
    let line = CloudPrivacyCopy.diagnostics
    XCTAssertTrue(line.localizedCaseInsensitiveContains("Never audio"))
    XCTAssertTrue(line.localizedCaseInsensitiveContains("never transcript text"))
    for field in ["identifiers", "session statistics", "error"] {
      XCTAssertTrue(
        line.localizedCaseInsensitiveContains(field),
        "diagnostics copy must enumerate the recorded field: \(field)")
    }
  }

  /// Keys the user configures are Keychain items, and the no-vendor-key
  /// promise stays restricted to the gateway architecture.
  func testKeyCopyNamesTheKeychainAndBoundsTheGatewayPromise() {
    let line = CloudPrivacyCopy.apiKeys
    XCTAssertTrue(line.localizedCaseInsensitiveContains("macOS Keychain"))
    // Round 14: the Founder's sentence says "through Libraxis"; the
    // session-token clause carries the gateway promise.
    XCTAssertTrue(line.localizedCaseInsensitiveContains("Libraxis"))
    XCTAssertTrue(line.localizedCaseInsensitiveContains("short-lived session token"))
    if let keyFree = line.range(of: "no vendor key", options: .caseInsensitive) {
      XCTAssertTrue(
        line[keyFree.upperBound...].localizedCaseInsensitiveContains("Libraxis"),
        "a no-vendor-key claim must be bounded to the gateway lane that earns it")
    }
  }

  /// The copy stays provider-neutral: the vendor lives behind the gateway.
  func testCopyNeverNamesACloudVendor() {
    let vendors = ["OpenAI", "Deepgram", "AssemblyAI", "Google", "Azure", "Speechmatics", "Groq"]
    var lines = CloudPrivacyCopy.detailBlocks.flatMap(\.lines)
    lines.append(contentsOf: [
      CloudPrivacyCopy.consentIsNotLiveEgress,
      CloudPrivacyCopy.egressAudioDetail,
      CloudPrivacyCopy.egressTextDetail,
      CloudPrivacyCopy.configureCloudServices,
    ])
    for line in lines {
      for vendor in vendors {
        XCTAssertFalse(
          line.contains(vendor), "privacy copy must not name a cloud vendor: \(vendor)")
      }
    }
  }

  /// Selecting Cloud on the Engine picker is the explicit grant: it writes the
  /// consent record and the mode together. The copy above describes this write.
  func testPickerPersistsCloudOnlyAfterExplicitGrant() {
    var writes: [(String, String)] = []
    var persisted = CsSettings.sample
    persisted.asrMode = nil
    persisted.cloudConsent = nil
    let applyWrite: (String, String) -> Void = { key, value in
      writes.append((key, value))
      if key == "CODESCRIBE_ASR_MODE" { persisted.asrMode = value }
      if key == "CODESCRIBE_CLOUD_CONSENT" { persisted.cloudConsent = value }
    }
    let model = SettingsViewModel(
      engine: MockSettingsEngine(
        settingsLoader: { persisted },
        updateConfigManyObserver: { entries in
          for entry in entries { applyWrite(entry.key, entry.value) }
        },
        updateConfigObserver: applyWrite
      ),
      permissionProbe: MockPermissionProbe()
    )
    XCTAssertEqual(model.asrModeId, "apple_only")
    model.setAsrMode("cloud")
    XCTAssertEqual(writes.map(\.0), ["CODESCRIBE_CLOUD_CONSENT", "CODESCRIBE_ASR_MODE"])
    XCTAssertEqual(writes.map(\.1), ["granted", "cloud"])
    XCTAssertEqual(model.asrModeId, "cloud")
  }

  /// The status rows render the live mode label and a consent state that
  /// tracks the stored record, so neither row can go stale against the other.
  func testStatusRowValuesTrackTheStoredRecord() {
    var withoutRecord = CsSettings.sample
    withoutRecord.asrMode = nil
    withoutRecord.cloudConsent = nil
    let plain = SettingsViewModel(
      engine: MockSettingsEngine(settingsLoader: { withoutRecord }),
      permissionProbe: MockPermissionProbe()
    )
    XCTAssertFalse(plain.cloudConsentGranted)
    XCTAssertFalse(plain.asrModeLabel.isEmpty)

    var withRecord = withoutRecord
    withRecord.cloudConsent = "granted"
    let granted = SettingsViewModel(
      engine: MockSettingsEngine(settingsLoader: { withRecord }),
      permissionProbe: MockPermissionProbe()
    )
    XCTAssertTrue(granted.cloudConsentGranted)
    XCTAssertEqual(granted.asrModeId, "apple_only")
    XCTAssertEqual(
      granted.asrModeLabel, plain.asrModeLabel,
      "a granted record alone does not move the lane to Cloud")
  }
}
