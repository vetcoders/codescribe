import Darwin
import XCTest

@testable import Codescribe

/// A real two-process crash trial for unsent composer drafts, run on request
/// only. `write` types into a private directory, waits out the save window and
/// SIGKILLs its own test host, so no shutdown path runs; `verify` is a new
/// process that restores from the same file. Both leave a JSON receipt.
///
///   TEST_RUNNER_CODESCRIBE_DRAFT_CRASH_TRIAL=write|verify
///   TEST_RUNNER_CODESCRIBE_DRAFT_CRASH_TRIAL_ROOT=/private/dir
@MainActor
final class OverlayComposerDraftCrashTrialTests: XCTestCase {
  private static let allDraft = String(
    repeating: "Każde słowo tego szkicu ma przetrwać pad aplikacji. ", count: 30)
  private static let agentDraft = "Lena, sprawdź to po restarcie"
  private static let inboxDraft = "Do skrzynki, też po padzie"

  private struct Trial {
    let phase: String
    let root: URL
  }

  private func trial() throws -> Trial {
    let environment = ProcessInfo.processInfo.environment
    guard let phase = environment["CODESCRIBE_DRAFT_CRASH_TRIAL"],
      let path = environment["CODESCRIBE_DRAFT_CRASH_TRIAL_ROOT"], path.hasPrefix("/")
    else { throw XCTSkip("crash trial runs only on request") }
    return Trial(phase: phase, root: URL(fileURLWithPath: path, isDirectory: true))
  }

  private func snapshot() -> OverlayChannelDeliverySnapshot {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume([
      "schema": "codescribe.agent-reply.v1", "kind": "agent_reply",
      "reply_id": String(repeating: "1", count: 24), "text": "Gotowe",
      "provider": "codex", "provider_session_id": "lena-trial",
      "lease_id": String(repeating: "a", count: 32), "channel": "2", "name": "Lena",
      "association": "unsolicited",
    ])
    return .init(deliveries: [], conversations: bus.conversations(busPath: "/trial/bus"))
  }

  private func writeReceipt(_ name: String, _ fields: [String: Any], root: URL) throws {
    var fields = fields
    fields["pid"] = Int(getpid())
    fields["at"] = ISO8601DateFormatter().string(from: Date())
    let data = try JSONSerialization.data(withJSONObject: fields, options: [.sortedKeys])
    try data.write(to: root.appendingPathComponent(name), options: .atomic)
  }

  func testCrashTrial() async throws {
    let trial = try trial()
    try FileManager.default.createDirectory(at: trial.root, withIntermediateDirectories: true)
    let lenaID = try XCTUnwrap(snapshot().conversations.first { $0.channel == "2" }).id
    switch trial.phase {
    case "write":
      let state = OverlayState()
      state.restoreComposerDrafts(cacheRoot: trial.root)
      state.applyConversationSnapshot(snapshot())
      state.setConversationDraft(Self.allDraft, for: "0")
      state.setConversationDraft(Self.inboxDraft, for: "10")
      state.setConversationDraft(Self.agentDraft, for: lenaID)
      // No explicit save: the app's own coalescing window must carry the words.
      try await Task.sleep(for: .milliseconds(700))
      try writeReceipt(
        "write-receipt.json",
        ["phase": "write", "signal": "SIGKILL", "drafts": 3,
         "all_characters": Self.allDraft.count],
        root: trial.root)
      kill(getpid(), SIGKILL)
      XCTFail("the host survived SIGKILL")
    case "verify":
      let state = OverlayState()
      state.restoreComposerDrafts(cacheRoot: trial.root)
      state.applyConversationSnapshot(snapshot())
      let restored = [
        "0": state.conversationDraft(for: "0") == Self.allDraft,
        "10": state.conversationDraft(for: "10") == Self.inboxDraft,
        "lena": state.conversationDraft(for: lenaID) == Self.agentDraft,
      ]
      let mode =
        (try FileManager.default.attributesOfItem(
          atPath: OverlayComposerDraftStore.url(cacheRoot: trial.root).path)[.posixPermissions]
          as? NSNumber)?.intValue ?? -1
      try writeReceipt(
        "verify-receipt.json",
        ["phase": "verify", "restored": restored, "file_mode": String(mode, radix: 8),
         "composing": state.founderComposing],
        root: trial.root)
      XCTAssertEqual(restored, ["0": true, "10": true, "lena": true])
      XCTAssertEqual(mode, 0o600)
      XCTAssertTrue(state.founderComposing)
    default:
      XCTFail("unknown crash trial phase \(trial.phase)")
    }
  }
}
