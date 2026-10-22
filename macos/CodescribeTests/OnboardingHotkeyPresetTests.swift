import XCTest

@testable import Codescribe

/// The wizard's hotkey step writes the three mode bindings only when the user
/// picks a preset in this run. Rerunning the wizard over custom shortcuts and
/// pressing Continue leaves them alone (Founder decision, 2026-10-20, UI-02).
@MainActor
final class OnboardingHotkeyPresetTests: XCTestCase {
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

  /// Dictation on Hold Ctrl, Formatting unbound, Agent on Double Ctrl: no
  /// preset's triple, the shape a user reaches through Settings › Shortcuts.
  private static let customBindings: [CsModeBinding] = CsModeBinding.sampleBindings.map { row in
    var row = row
    switch row.mode {
    case .dictation: row.binding = .holdCtrl
    case .formatting: row.binding = .disabled
    case .assistive: row.binding = .doubleCtrl
    }
    return row
  }

  private func wizardOnHotkeyStep(bindings: [CsModeBinding]) throws -> (
    OnboardingViewModel, RecordingHotkeysEngine
  ) {
    let engine = MockOnboardingEngine()
    engine.progress = UInt32(try XCTUnwrap(OnboardingStep.flow.firstIndex(of: .hotkeyMode)))
    let hotkeys = RecordingHotkeysEngine(persisted: bindings)
    let suite = "codescribe-hotkey-preset-tests-\(UUID().uuidString)"
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
    let (model, hotkeys) = try wizardOnHotkeyStep(bindings: Self.customBindings)
    XCTAssertTrue(model.hotkeyBindingsAreCustom, "No preset matches Hold Ctrl / – / Double Ctrl")

    model.advance()

    XCTAssertTrue(hotkeys.writes.isEmpty, "Continue must not rewrite shortcuts the user never touched")
    XCTAssertNotEqual(model.step, .hotkeyMode, "Continue still advances")
  }

  func testPickingAPresetWritesAllThreeBindingsOnce() throws {
    let (model, hotkeys) = try wizardOnHotkeyStep(bindings: Self.customBindings)

    model.selectHotkeyMode(.hold)
    XCTAssertEqual(hotkeys.writes.map(\.mode), [.dictation, .formatting, .assistive])
    XCTAssertEqual(hotkeys.writes.map(\.binding), [.holdFn, .disabled, .disabled])

    // Continue after a pick persists the same preset again; it never reverts
    // to the derived one and never writes a different triple.
    model.advance()
    XCTAssertEqual(hotkeys.writes.count, 6)
    XCTAssertEqual(Set(hotkeys.writes.suffix(3).map(\.binding)), [.holdFn, .disabled])
  }

  func testDefaultBindingsAreNotCustom() throws {
    let (model, _) = try wizardOnHotkeyStep(bindings: CsModeBinding.sampleBindings)
    XCTAssertFalse(model.hotkeyBindingsAreCustom, "The core default is the Both preset")
    XCTAssertEqual(model.hotkeyMode, .both)
  }
}
