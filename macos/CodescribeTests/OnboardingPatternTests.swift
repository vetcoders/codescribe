import XCTest

@testable import Codescribe

/// Round 3 of the Founder's review (2026-10-20, UI-47…UI-56): the nine wizard
/// steps share one pattern — the frame owns the step's question and category
/// label, the content sits in one column — and the captions tell the truth
/// about what each step does.
@MainActor
final class OnboardingPatternTests: XCTestCase {
  private func onboardingSources() throws -> [String: String] {
    let directory = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens/Onboarding")
    let files = try FileManager.default.contentsOfDirectory(
      at: directory, includingPropertiesForKeys: nil
    ).filter { $0.pathExtension == "swift" }
    var sources: [String: String] = [:]
    for file in files {
      sources[file.lastPathComponent] = try String(contentsOf: file, encoding: .utf8)
    }
    return sources
  }

  private func polishCatalog() throws -> [String: String] {
    let url = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Resources/Localization/Localizable.xcstrings")
    let data = try Data(contentsOf: url)
    let root = try XCTUnwrap(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    let strings = try XCTUnwrap(root["strings"] as? [String: Any])
    var polish: [String: String] = [:]
    for (key, value) in strings {
      guard let entry = value as? [String: Any],
        let localizations = entry["localizations"] as? [String: Any],
        let pl = localizations["pl"] as? [String: Any],
        let unit = pl["stringUnit"] as? [String: Any],
        let text = unit["value"] as? String
      else { continue }
      polish[key] = text
    }
    return polish
  }

  /// One column for every step, narrower than the window leaves at its
  /// minimum width, owned by the frame rather than by nine step views.
  func testTheFrameOwnsOneContentColumn() throws {
    XCTAssertEqual(OnboardingView.columnMaxWidth, 560)
    let sources = try onboardingSources()
    let frame = try XCTUnwrap(sources["OnboardingView.swift"])
    let steps = try XCTUnwrap(sources["OnboardingSteps.swift"])
    XCTAssertTrue(frame.contains("minWidth: 680"))
    XCTAssertLessThan(
      OnboardingView.columnMaxWidth, 680 - 2 * 28, "the column fits the minimum window")
    XCTAssertTrue(frame.contains("columnMaxWidth"))
    XCTAssertFalse(steps.contains("struct OnboardingStepHeader"), "no per-step headline component")
    // The slogans that competed with the step's own question are gone.
    for retired in [
      "Your language. Your shortcuts. Your way of working.",
      "Choose how you want to use Codescribe.",
      "Connect to the active session",
      "When you speak more than one language.",
      "You can continue without granting these permissions.",
    ] {
      for (name, source) in sources {
        XCTAssertFalse(source.contains("\"\(retired)\""), "\(name) still says: \(retired)")
      }
    }
  }

  /// Steps 2, 3, 8 and 9: captions that match what the code does.
  func testCaptionsMatchWhatTheStepsDo() throws {
    let sources = try onboardingSources()
    func pinned(_ text: String) -> Bool {
      sources.values.contains { $0.contains("\"\(text)\"") }
    }
    XCTAssertTrue(pinned("Dictate, or talk to an AI agent."))
    XCTAssertTrue(pinned("You can do this later in Settings."))
    // Step 8 installs a skill for later use; nothing attaches to a session.
    XCTAssertTrue(pinned("Install the Codescribe skill"))
    XCTAssertTrue(pinned("Codescribe skill installed"))
    // Step 9 never promises the agent when only dictation is ready.
    XCTAssertTrue(pinned("Dictation is ready."))
    let frame = try XCTUnwrap(sources["OnboardingView.swift"])
    XCTAssertTrue(frame.contains("!model.agentBridgeReadyToGo"))

    let polish = try polishCatalog()
    let expected: [String: String] = [
      "Dictate, or talk to an AI agent.": "Możesz dyktować albo rozmawiać z agentem AI.",
      "You can do this later in Settings.": "Możesz to zrobić później w Ustawieniach.",
      "Install the Codescribe skill": "Zainstaluj skill Codescribe",
      "Codescribe skill installed": "Skill Codescribe zainstalowany",
      "Dictation is ready.": "Dyktowanie jest gotowe.",
      "Dictation language": "Język dyktowania",
      "Local transcription": "Transkrypcja lokalna",
      "AI provider": "Dostawca AI",
      "Do you want to transcribe on your Mac?": "Chcesz transkrybować na swoim Macu?",
    ]
    for (key, value) in expected {
      XCTAssertEqual(polish[key], value, key)
    }
    for retired in [
      "Choose how you want to use Codescribe.", "Use both methods.",
      "Connect to the active session", "When you speak more than one language.",
      "You can continue without granting these permissions.",
    ] {
      XCTAssertNil(polish[retired], "retired key still in the catalog: \(retired)")
    }
  }

  /// D1 (2026-10-26): the wizard speaks in one type family. Status labels,
  /// field labels and sentences use the system face in the E3 roles; the
  /// success mark is a glyph, not a character inside the sentence.
  func testWizardUsesOneTypeFamilyAndAGlyphForReady() throws {
    let sources = try onboardingSources()
    let steps = try XCTUnwrap(sources["OnboardingSteps.swift"])
    let whisperURL = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens/Settings/WhisperDownloadView.swift")
    let whisper = try String(contentsOf: whisperURL, encoding: .utf8)
    let checked = [("OnboardingSteps.swift", steps), ("WhisperDownloadView.swift", whisper)]
    for (name, source) in checked {
      XCTAssertFalse(source.contains("CSFont.mono("), "\(name) still sets a mono face")
      XCTAssertFalse(source.contains(".font(.caption"), "\(name) still uses the caption style")
    }
    XCTAssertFalse(steps.contains("\"Ready to go ✓\""), "the check mark left the sentence")
    XCTAssertTrue(steps.contains("\"Ready to go\""))
    let frame = try XCTUnwrap(sources["OnboardingView.swift"])
    let eyebrow =
      "EyebrowLabel(text: current.label, color: CSColor.textMuted, font: CSFont.ui(11, .semibold))"
    XCTAssertTrue(frame.contains(eyebrow), "the category label: system face, muted grey")
    // The step heading sits on the 18 pt display role of the one text scale,
    // not on a system text style (W0-17).
    XCTAssertFalse(frame.contains(".title2"), "the step heading left the system title style")
    XCTAssertTrue(frame.contains(".font(CSFont.ui(18, .semibold))"), "the step heading role")
    XCTAssertTrue(whisper.contains(".truncationMode(.middle)"), "the model path keeps both ends")

    let polish = try polishCatalog()
    XCTAssertNil(polish["Ready to go ✓"], "retired key still in the catalog")
    let expected: [String: String] = [
      "Ready to go": "Gotowe do pracy",
      "Source": "Źródło",
      "Size": "Rozmiar",
      "Model location on this Mac": "Lokalizacja modelu na komputerze",
    ]
    for (key, value) in expected {
      XCTAssertEqual(polish[key], value, key)
    }
  }

  /// Founder decision of 2026-10-26: the brand terracotta marks state in the
  /// wizard (progress, the selected choice, the step icon) while the macOS
  /// accent keeps the controls; verified statuses are an olive capsule with a
  /// thin check, stored identity the empty circle, on step 6 and step 9 alike.
  func testStateWearsTheBrandAccentAndStatusesTheOlive() throws {
    let sources = try onboardingSources()
    let steps = try XCTUnwrap(sources["OnboardingSteps.swift"])
    let frame = try XCTUnwrap(sources["OnboardingView.swift"])
    let componentsURL = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/DesignSystem/Components.swift")
    let components = try String(contentsOf: componentsURL, encoding: .utf8)

    XCTAssertTrue(components.contains("func csChoice(selected: Bool)"), "a choice has a style")
    XCTAssertTrue(
      components.contains("CSColor.brandAccent.opacity(isEnabled ? CSAlpha.selectionWash"))
    XCTAssertTrue(steps.contains(".csChoice(selected: isSelected)"))
    XCTAssertTrue(steps.contains("isSelected ? CSColor.brandAccent : CSColor.secondaryInk"))
    XCTAssertFalse(steps.contains(".csAction(prominent: isSelected)"), "a selection is no action")
    XCTAssertTrue(frame.contains("CSProgressBar(value: model.progressFraction)"), "progress")
    XCTAssertTrue(components.contains("struct CSProgressBar"), "the brand bar lives in the system")
    XCTAssertFalse(frame.contains("ProgressView("), "the native bar ignores a brand tint")
    XCTAssertTrue(frame.contains(".foregroundStyle(CSColor.brandAccent)"), "the step icon")
    for source in [steps, frame] {
      XCTAssertFalse(source.contains(".tint(CSColor.chromeAccent)"), "no chrome-accent progress")
    }

    XCTAssertTrue(components.contains("struct CSStatusBadge"), "one badge for wizard and Settings")
    XCTAssertTrue(steps.contains("CSProgressBar(value: SettingsViewModel.calibrationProgress("))
    // The capsule takes the mark's tint (W0-03): olive for saved and verified,
    // terracotta only for a state that blocks ("red only when blocking").
    XCTAssertTrue(components.contains("Capsule().fill(tint.opacity(0.14))"))
    XCTAssertTrue(components.contains("case .saved, .verified: CSColor.oliveLight"))
    XCTAssertTrue(components.contains("case .blocked: CSColor.terracottaLight"))
    XCTAssertTrue(
      components.contains("CSIconView(icon: .success, size: 9, weight: .semibold,"),
      "a verified status is the thin check of the glyph canon, never a filled badge")
    XCTAssertFalse(steps.contains("struct OnboardingStatusBadge"), "the wizard owns no badge")
    XCTAssertTrue(steps.contains("mark: state.isGranted ? .verified : .none"), "granted scope")
    XCTAssertTrue(steps.contains("mark: .verified)"), "ready to go")
    XCTAssertTrue(steps.contains("mark: keySaved ? .saved : .none"), "saved key")
    XCTAssertTrue(
      steps.contains("mark: done ? (presenceOnly ? .saved : .verified) : .none)"),
      "step 9 draws stored identity with the empty circle of step 6, in the capsule")
    XCTAssertTrue(
      steps.contains("CSStatusBadge(text: state.status, mark: .verified)"),
      "a calibrated microphone is a verified state")
    XCTAssertTrue(
      steps.contains("CSStatusBadge(text: state.status, mark: state.blocked ? .blocked : .none)"),
      "step 9 marks blocked recording readiness with a sign, not colour alone (W0-03)")
    XCTAssertFalse(steps.contains("private func statusBadge("), "one badge, not two")
    XCTAssertEqual(
      steps.components(separatedBy: "CSStatusBadge(").count, 8, "seven wizard statuses")
  }
}
