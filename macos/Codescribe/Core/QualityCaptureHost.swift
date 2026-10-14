import Foundation

/// Are we running inside the XCTest host rather than the product?
///
/// The Swift suite runs *inside the app bundle*, so anything that reaches the
/// filesystem through a free FFI function — rather than through an injected
/// collaborator a test can mock — writes to the operator's REAL data directory.
/// That is not theoretical: `OverlayState.captureQualityIfEdited` calls the free
/// `commitOverlayQualityRecord`, and every `make test-swift` run appended two
/// synthetic corrections to `~/.codescribe/quality/corrections.jsonl`. By the
/// time it was caught, 276 of 501 rows in the live store were test residue and
/// they were rendering in Settings › Dictionary as the user's own corrections.
///
/// `core/config/keychain.rs::in_xctest_host` is the Rust half of the same idea
/// (it keys on the XCTest environment markers, pinned by a test). This is the
/// Swift half: one definition, so a new write path has an obvious thing to ask.
///
/// Fails CLOSED for the user's data: when the marker is absent we assume
/// production and write, which is the safe direction for a real dictation.
enum QualityCaptureHost {
  /// `XCTestCase` only exists in the process once the test bundle is loaded.
  static let isRunningTests = NSClassFromString("XCTestCase") != nil

  static let dataDirectoryVariable = "CODESCRIBE_DATA_DIR"
  static let agentBridgeHomeVariable = "CODESCRIBE_AGENT_BRIDGE_HOME"

  /// Where the XCTest host keeps its state, so it never observes the live app.
  struct IsolatedHomes: Equatable {
    let data: URL
    let agentBridge: URL
  }

  /// The XCTest host never observes the account's live runtime.
  ///
  /// `AppModel()` in a test attaches the production overlay, whose delivery
  /// reader polls the agent bridge and the shared transcript bus every 500 ms,
  /// and the in-process Rust core answers the channel roster from the same
  /// bridge. Against the real `~/.codescribe`, a test run during a take drew a
  /// second, English-language overlay with the live conversations (screen
  /// recording 2026-10-13 09:46, review 2026-10-14).
  ///
  /// `make test-swift` exports isolated homes through `TEST_RUNNER_`; any other
  /// launch (Xcode, a bare `xcodebuild test`) falls back to a per-process
  /// temporary root. Swift and Rust both resolve these variables, so setting
  /// them before the first engine exists isolates both sides.
  @discardableResult
  static func isolateFromLiveRuntime(
    runningTests: Bool = isRunningTests,
    environment: [String: String] = ProcessInfo.processInfo.environment,
    temporaryDirectory: URL = FileManager.default.temporaryDirectory,
    processID: Int32 = ProcessInfo.processInfo.processIdentifier,
    setVariable: (String, String) -> Void = { setenv($0, $1, 1) }
  ) -> IsolatedHomes? {
    guard runningTests else { return nil }
    func configured(_ name: String) -> URL? {
      guard let path = environment[name]?.trimmingCharacters(in: .whitespacesAndNewlines),
        !path.isEmpty
      else { return nil }
      return URL(fileURLWithPath: (path as NSString).expandingTildeInPath, isDirectory: true)
    }
    let data =
      configured(dataDirectoryVariable)
      ?? temporaryDirectory.appendingPathComponent(
        "codescribe-test-host-\(processID)", isDirectory: true)
    let agentBridge =
      configured(agentBridgeHomeVariable)
      ?? data.appendingPathComponent("agent-bridge", isDirectory: true)
    for (name, home) in [(dataDirectoryVariable, data), (agentBridgeHomeVariable, agentBridge)]
    where configured(name) == nil {
      try? FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
      setVariable(name, home.path)
    }
    return IsolatedHomes(data: data, agentBridge: agentBridge)
  }
}
