import AppKit
import Darwin
import XCTest

@testable import Codescribe

@MainActor
final class OverlayPlaybackOwnerTests: XCTestCase {
  func testBroadcastMirrorCannotOwnMuteAndReusedChannelCannotBorrowThePriorSession() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    let previousRoot = ProcessInfo.processInfo.environment["CODESCRIBE_AGENT_BRIDGE_HOME"]
    setenv("CODESCRIBE_AGENT_BRIDGE_HOME", root.path, 1)
    defer {
      if let previousRoot {
        setenv("CODESCRIBE_AGENT_BRIDGE_HOME", previousRoot, 1)
      } else {
        unsetenv("CODESCRIBE_AGENT_BRIDGE_HOME")
      }
      try? FileManager.default.removeItem(at: root)
    }
    let old = AgentPlaybackIdentity(
      provider: "codex", session: "mute-old",
      bus: root.appendingPathComponent("channel-2.jsonl").path)
    let fresh = AgentPlaybackIdentity(
      provider: "codex", session: "mute-new", bus: old.bus)
    let foreignBus = root.appendingPathComponent("transcript-events.jsonl").path
    func write(_ row: [String: Any], at file: URL) throws {
      try FileManager.default.createDirectory(
        at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
      try JSONSerialization.data(withJSONObject: row).write(to: file)
    }
    func lease(_ identity: AgentPlaybackIdentity, model: String, muted: Bool) throws {
      try write(
        [
          "schema": "codescribe.agent-bridge.lease.v1", "lease_id": identity.leaseID,
          "provider": identity.provider, "provider_session_id": identity.session,
          "bus": identity.bus, "model": model,
        ],
        at: root.appendingPathComponent("leases/\(identity.leaseID).json"))
      try write(
        [
          "schema": "codescribe.agent-playback-mute.v1", "lease_id": identity.leaseID,
          "provider": identity.provider, "provider_session_id": identity.session,
          "bus": identity.bus, "muted": muted,
        ],
        at: root.appendingPathComponent("runtime/playback-mutes/\(identity.storageKey).json"))
    }
    try lease(old, model: "old-model", muted: true)
    try lease(fresh, model: "new-model", muted: false)
    func conversation(_ identity: AgentPlaybackIdentity) throws -> OverlayConversation {
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": identity.provider, "provider_session_id": identity.session,
          "lease_id": identity.leaseID, "channel": "2", "name": "Same display name",
        ]))
      let message = OverlayConversationMessage(
        id: "broadcast-\(identity.session)", kind: .user, text: "Iwo Iwo Iwo Iwo Iwo", order: 1,
        emittedAt: "2026-10-11T19:00:00Z", owner: owner, recipients: [], deliveryID: nil,
        replyTo: nil, unsolicited: false, playback: nil, busPath: foreignBus)
      return OverlayConversation(
        id: owner.id, channel: "2", name: owner.name, owner: owner, messages: [message])
    }
    func roster(_ identity: AgentPlaybackIdentity) -> CsChannelRosterState {
      .init(
        channel: "2", audience: "Same display name", provider: identity.provider,
        providerSessionId: identity.session, open: false, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: true)
    }
    var state: OverlayState? = OverlayState()
    defer { state = nil }
    weak let retainedState = state
    state?.applyChannelRoster([roster(old)])
    state?.observeChannelDelivery(using: OverlayChannelDeliveryReader(root: root))
    func waitForDescriptor(_ descriptor: String) async throws {
      let deadline = ContinuousClock.now.advanced(by: .seconds(2))
      while state?.channelAgentDescriptors["2"] != descriptor && ContinuousClock.now < deadline {
        try await Task.sleep(for: .milliseconds(10))
      }
      XCTAssertEqual(state?.channelAgentDescriptors["2"], descriptor)
    }
    try await waitForDescriptor("Codex · old-model")
    let oldConversation = try conversation(old)
    XCTAssertEqual(oldConversation.messages.first?.busPath, foreignBus)
    XCTAssertEqual(state?.channelPlaybackMuted["2"], true)
    XCTAssertEqual(
      state?.conversationPlaybackMuted(oldConversation), true,
      "A broadcast mirror's provenance must not override the verified lease bus")
    state?.applyChannelRoster([roster(fresh)])
    try await waitForDescriptor("Codex · new-model")
    XCTAssertEqual(state?.channelPlaybackMuted["2"], false)
    XCTAssertNil(
      state?.conversationPlaybackMuted(oldConversation),
      "A reused channel and matching display name cannot lend mute to a historical session")
    let freshConversation = try conversation(fresh)
    XCTAssertEqual(state?.conversationPlaybackMuted(freshConversation), false)
    let otherProvider = AgentPlaybackIdentity(
      provider: "claude-code", session: fresh.session, bus: fresh.bus)
    XCTAssertNil(
      state?.conversationPlaybackMuted(try conversation(otherProvider)),
      "Matching session text in another provider does not identify the bound owner")
    let globalMuteURL = root.appendingPathComponent("runtime/playback-mutes/global.json")
    func globalMute(_ muted: Bool) async throws {
      try write(
        ["schema": "codescribe.agent-playback-mute.v1", "scope": "all", "muted": muted],
        at: globalMuteURL)
      let deadline = ContinuousClock.now.advanced(by: .seconds(2))
      while state?.globalPlaybackMuted != muted && ContinuousClock.now < deadline {
        try await Task.sleep(for: .milliseconds(10))
      }
      XCTAssertEqual(state?.globalPlaybackMuted, muted)
    }
    try await globalMute(true)
    XCTAssertEqual(
      state?.channelPlaybackMuted["2"], true,
      "The roster must reflect global mute even when this agent is locally audible")
    XCTAssertEqual(
      state?.conversationPlaybackMuted(freshConversation), true,
      "Every agent conversation must reflect the same global playback block")
    try await globalMute(false)
    XCTAssertEqual(state?.channelPlaybackMuted["2"], false)
    XCTAssertEqual(state?.conversationPlaybackMuted(freshConversation), false)
    let localMute = try XCTUnwrap(
      JSONSerialization.jsonObject(
        with: Data(
          contentsOf: root.appendingPathComponent(
            "runtime/playback-mutes/\(fresh.storageKey).json"))) as? [String: Any])
    XCTAssertEqual(
      localMute["muted"] as? Bool, false, "Global mute must not rewrite local preferences")
    state = nil
    let released = ContinuousClock.now.advanced(by: .seconds(2))
    while retainedState != nil && ContinuousClock.now < released {
      try await Task.sleep(for: .milliseconds(10))
    }
    XCTAssertNil(retainedState, "Release the observer before restoring the process-only test root")
  }
}
