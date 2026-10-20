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
    XCTAssertLessThan(OnboardingView.columnMaxWidth, 680 - 2 * 28, "the column fits the minimum window")
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

  /// Step 7: the hybrid preset no longer repeats itself under its own name;
  /// the other two keep their gesture instructions.
  func testHybridPresetHasNoSummaryAndTheOthersKeepTheirs() {
    let locale = Locale(identifier: "en")
    XCTAssertNil(HotkeyModeChoice.both.summary(locale: locale))
    XCTAssertNotNil(HotkeyModeChoice.hold.summary(locale: locale))
    XCTAssertNotNil(HotkeyModeChoice.toggle.summary(locale: locale))
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
}
