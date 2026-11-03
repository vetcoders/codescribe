import XCTest

@testable import Codescribe

/// The wizard's hotkey step explains how recording starts and writes no
/// binding: Formatting and the Agent have no gesture (founder decision
/// 2026-10-26), and rerunning the wizard over custom shortcuts leaves them
/// alone (Founder decision, 2026-10-20, UI-02).
@MainActor
final class OnboardingHotkeyStepTests: XCTestCase {
  /// Records every write instead of persisting it; the wizard never sees a
  /// store it did not write to itself here.
  private final class RecordingHotkeysEngine: HotkeysEngine {
    private(set) var writes: [(mode: CsWorkMode, binding: CsShortcutBinding)] = []
    private let persisted: [CsModeBinding]

    init(persisted: [CsModeBinding]) { self.persisted = persisted }

    func modeBindings() -> [CsModeBinding] { persisted }
    func availableBindings() -> [CsBindingOption] { CsBindingOption.sampleOptions }
    func setModeBinding(mode: CsWorkMode, binding: CsShortcutBinding) throws {
      writes.append((mode, binding))
    }
    func resetToDefaults() throws {}
    func rearmAfterPermissionGrant() {}
    func channelModifier() -> String { "ctrl" }
    func setChannelModifier(_ value: String) throws {}
    func setWidgetShortcut(_ wireId: String) throws {}
    func fnTapTogglesDictation() -> Bool { false }
    func setFnTapTogglesDictation(_ enabled: Bool) throws {}
    func middleMouseActsAsFn() -> Bool { false }
    func setMiddleMouseActsAsFn(_ enabled: Bool) throws {}
    func middleMouseAgentGesture() -> Bool { false }
    func setMiddleMouseAgentGesture(_ enabled: Bool) throws {}
    func validate(candidate: [CsModeBinding]) -> [CsHotkeyConflict] { [] }
  }

  private struct NoBridge: AgentBridgeInstalling {
    func status() -> AgentBridgeInstallationStatus {
      .init(
        payloadAvailable: false, bundleVersion: nil, installedClients: [], installedPaths: [],
        detail: "")
    }
    func install(selectedClients: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
      throw AgentBridgeInstallationError.payloadUnavailable
    }
    func adoptManualSkill(client: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
      throw AgentBridgeInstallationError.payloadUnavailable
    }
  }

  /// The default bindings with Dictation moved to `binding`.
  private static func bindings(dictation binding: CsShortcutBinding) -> [CsModeBinding] {
    CsModeBinding.sampleBindings.map { row in
      var row = row
      if row.mode == .dictation { row.binding = binding }
      return row
    }
  }

  private func wizardOnHotkeyStep(bindings: [CsModeBinding]) throws -> (
    OnboardingViewModel, RecordingHotkeysEngine
  ) {
    let engine = MockOnboardingEngine()
    engine.progress = UInt32(try XCTUnwrap(OnboardingStep.flow.firstIndex(of: .hotkeyMode)))
    let hotkeys = RecordingHotkeysEngine(persisted: bindings)
    let suite = "codescribe-hotkey-step-tests-\(UUID().uuidString)"
    let preferences = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { preferences.removePersistentDomain(forName: suite) }
    let model = OnboardingViewModel(
      engine: engine, hotkeys: hotkeys, agentStatus: MockAgentStatusEngine(),
      agentBridge: NoBridge(), probe: MockPermissionProbe(.allGranted),
      languagePreferences: preferences, processInterfaceLanguage: .english)
    XCTAssertEqual(model.step, .hotkeyMode)
    return (model, hotkeys)
  }

  func testContinueOverCustomShortcutsWritesNothing() throws {
    let (model, hotkeys) = try wizardOnHotkeyStep(bindings: Self.bindings(dictation: .holdCtrl))

    model.advance()

    XCTAssertTrue(hotkeys.writes.isEmpty, "the hotkey step never writes a binding")
    XCTAssertNotEqual(model.step, .hotkeyMode, "Continue still advances")
  }

  func testDefaultBindingsSayHoldFn() throws {
    let (model, _) = try wizardOnHotkeyStep(bindings: CsModeBinding.sampleBindings)
    XCTAssertEqual(model.dictationStartLine, "Hold Fn to dictate.")
  }

  /// A rerun over a custom dictation shortcut names that shortcut instead of
  /// promising Fn, and a removed one says so.
  func testCustomDictationShortcutIsNamed() throws {
    let (custom, _) = try wizardOnHotkeyStep(bindings: Self.bindings(dictation: .holdCtrl))
    XCTAssertTrue(
      custom.dictationStartLine.contains(CsShortcutBinding.holdCtrl.visibleName),
      custom.dictationStartLine)

    let (removed, _) = try wizardOnHotkeyStep(bindings: Self.bindings(dictation: .disabled))
    XCTAssertEqual(removed.dictationStartLine, "Dictation has no shortcut.")
  }
}
