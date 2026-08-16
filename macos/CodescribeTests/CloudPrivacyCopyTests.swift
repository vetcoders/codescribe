import XCTest

@testable import Codescribe

/// Pins the cloud-privacy settings copy to the contract the Rust core
/// enforces (config::cloud_asr + asr_session::consent). The copy is a promise
/// surface: if it drifts from the enforced behavior — consent-gated egress,
/// Apple-only fallback, no vendor keys, content-free telemetry — these tests
/// are the tripwire.
@MainActor
final class CloudPrivacyCopyTests: XCTestCase {
  /// The copy states the explicit-consent contract for audio egress.
  func testCopyStatesExplicitConsentBeforeEgress() {
    XCTAssertTrue(CloudPrivacyCopy.intro.contains("only after you explicitly allow it"))
    XCTAssertTrue(CloudPrivacyCopy.modeCloud.contains("explicit consent"))
  }

  /// All three product modes are named, in the canonical order.
  func testCopyNamesAllThreeModes() {
    XCTAssertTrue(CloudPrivacyCopy.modeAppleOnly.hasPrefix("Apple only"))
    XCTAssertTrue(CloudPrivacyCopy.modeCloud.hasPrefix("Cloud"))
    XCTAssertTrue(CloudPrivacyCopy.modeLocalPower.hasPrefix("Local power"))
    XCTAssertEqual(CloudPrivacyCopy.lines.count, 6)
    XCTAssertEqual(
      CloudPrivacyCopy.lines[1...3],
      [
        CloudPrivacyCopy.modeAppleOnly,
        CloudPrivacyCopy.modeCloud,
        CloudPrivacyCopy.modeLocalPower,
      ]
    )
  }

  /// The refusal shape is spelled out: Apple + dictionary, and explicitly
  /// no local model as a hidden substitute.
  func testCopyStatesAppleOnlyFallbackWithoutHiddenLocalLoad() {
    XCTAssertTrue(CloudPrivacyCopy.consentFallback.contains("Cloud never arms"))
    XCTAssertTrue(CloudPrivacyCopy.consentFallback.contains("no local model is loaded"))
  }

  /// Cloud copy stays provider-neutral (the vendor lives behind the
  /// Libraxis gateway) and states that no vendor keys are stored.
  func testCloudCopyIsProviderNeutralAndKeyFree() {
    XCTAssertTrue(CloudPrivacyCopy.modeCloud.contains("Libraxis gateway"))
    XCTAssertTrue(CloudPrivacyCopy.modeCloud.contains("no vendor keys"))
    let vendors = [
      "OpenAI", "Deepgram", "AssemblyAI", "Google", "Azure", "Speechmatics", "Groq",
    ]
    for line in CloudPrivacyCopy.lines {
      for vendor in vendors {
        XCTAssertFalse(
          line.contains(vendor),
          "privacy copy must not name a cloud vendor: \(vendor)"
        )
      }
    }
  }

  /// The telemetry promise is bounded exactly like the typed core telemetry:
  /// identifiers and counters, never audio or transcript content.
  /// The picker is clickable and writes the promoted consent + mode keys.
  /// Copy stays; this pins that Cloud is not display-only prose.
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
      )
    )
    XCTAssertEqual(model.asrModeId, "apple_only")
    model.setAsrMode("cloud")
    XCTAssertTrue(writes.contains { $0 == ("CODESCRIBE_CLOUD_CONSENT", "granted") })
    XCTAssertTrue(writes.contains { $0 == ("CODESCRIBE_ASR_MODE", "cloud") })
    XCTAssertEqual(model.asrModeId, "cloud")
  }

  func testTelemetryCopyExcludesContent() {
    XCTAssertTrue(CloudPrivacyCopy.telemetry.contains("Never audio"))
    XCTAssertTrue(CloudPrivacyCopy.telemetry.contains("never transcript text"))
    for field in ["latency", "bytes", "error", "model"] {
      XCTAssertTrue(
        CloudPrivacyCopy.telemetry.contains(field),
        "telemetry copy must enumerate the allowed field: \(field)"
      )
    }
  }
}
