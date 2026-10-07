import AppKit
import Foundation
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayAgentArchiveTests: XCTestCase {
  func testOnlyDisconnectedClosedOwnersOfferArchiveAndStaleClickDoesNothing() async throws {
    let state = OverlayState()
    for (alive, open) in [(true, false), (nil, false), (false, true)] {
      state.applyChannelRoster([roster(alive: alive, open: open)])
      XCTAssertNil(state.archiveCandidate(for: "4"))
    }
    state.applyChannelRoster([roster()])
    let owner = try XCTUnwrap(state.archiveCandidate(for: "4"))
    var invocations = 0
    state.archiveAgentCommand = { _ in invocations += 1 }
    state.applyChannelRoster([roster(session: "replacement", alive: true)])
    await state.archiveAgent(owner)
    XCTAssertEqual(invocations, 0)
    XCTAssertEqual(state.visibleChannelRows.map(\.agent), ["adam"])
    XCTAssertTrue(state.archivedAgentOwners.isEmpty)
  }

  func testRefusalKeepsListConversationAndDraftAndSuccessMovesItToSaved() async throws {
    let state = OverlayState()
    state.applyChannelRoster([roster()])
    let owner = try XCTUnwrap(state.archiveCandidate(for: "4"))
    let conversation = OverlayConversation(
      id: owner.id, channel: "4", name: "adam", owner: owner, messages: [])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
    state.selectConversation(owner.id)
    state.conversationDrafts[owner.id] = "Unsent draft"
    state.archiveAgentCommand = { _ in throw CocoaError(.fileWriteUnknown) }
    await state.archiveAgent(owner)
    XCTAssertNotNil(state.agentArchiveError)
    XCTAssertEqual(state.visibleChannelRows.count, 1)
    XCTAssertEqual(state.selectedConversationID, owner.id)
    XCTAssertEqual(state.conversationDrafts[owner.id], "Unsent draft")
    state.archiveAgentCommand = { _ in }
    await state.archiveAgent(owner)
    XCTAssertNil(state.agentArchiveError)
    XCTAssertTrue(state.visibleChannelRows.isEmpty)
    XCTAssertFalse(state.canToggleConversationMicrophone(conversation))
    XCTAssertEqual(state.conversationDrafts[owner.id], "Unsent draft")
    var drawer = OverlayChannelStatusView(
      channels: state.visibleChannelRows, unavailable: false, palette: .light, animates: false,
      hudStates: state.channelHudStates, conversations: state.conversations)
    drawer.archivedOwners = state.archivedAgentOwners
    XCTAssertTrue(drawer.currentConversations.isEmpty)
    XCTAssertEqual(drawer.savedConversations.map(\.id), [owner.id])
    state.applyChannelRoster([roster(session: "replacement", alive: true)])
    XCTAssertEqual(state.visibleChannelRows.count, 1, "reusing a digit does not hide its new owner")
  }

  func testNativeFacadeRunsCanonicalArchiveAndReaderRetainsConversationAcrossRestart() async throws
  {
    let fixture = try ArchiveFixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let before: OverlayChannelDeliverySnapshot
    do { before = try await reader.readSnapshot() } catch {
      XCTFail("Initial history read: \(error)")
      return
    }
    let conversation = try XCTUnwrap(before.conversations.first { $0.id == fixture.owner.id })
    XCTAssertEqual(conversation.messages.first?.text, "Iwo Iwo Iwo Iwo Iwo")
    let leaseBytes = try Data(contentsOf: fixture.lease)
    let busBytes = try Data(contentsOf: fixture.bus)
    do {
      try await RealAgentBridgeInstaller.archiveBusAgent(
        owner: fixture.owner, installer: fixture.installer)
    } catch {
      XCTFail("Managed archive command: \(error)")
      return
    }
    let after: OverlayChannelDeliverySnapshot
    do { after = try await reader.readSnapshot() } catch {
      XCTFail("Archived history read: \(error)")
      return
    }
    XCTAssertTrue(after.deliveries.isEmpty)
    XCTAssertTrue(after.archivedOwners.contains(fixture.owner))
    XCTAssertEqual(after.conversations.first { $0.id == fixture.owner.id }, conversation)
    let restarted = try await OverlayChannelDeliveryReader(root: fixture.root).readSnapshot()
    XCTAssertEqual(restarted.conversations.first { $0.id == fixture.owner.id }, conversation)
    XCTAssertEqual(try Data(contentsOf: fixture.lease), leaseBytes)
    XCTAssertEqual(try Data(contentsOf: fixture.bus), busBytes)
    XCTAssertFalse(
      FileManager.default.fileExists(
        atPath: fixture.root.appendingPathComponent("acknowledgments").path))
  }

  func testArchiveMetadataCannotHideABoundOwnerAndEmptyHistoryIsStillSaved() async throws {
    let fixture = try ArchiveFixture(includeMessage: false)
    defer { fixture.remove() }
    try fixture.writeArchive()
    var snapshot = try await OverlayChannelDeliveryReader(root: fixture.root).readSnapshot()
    XCTAssertTrue(
      snapshot.archivedOwners.isEmpty, "a failed binding publication must keep its owner active")
    try fixture.writeBinding(session: "replacement")
    snapshot = try await OverlayChannelDeliveryReader(root: fixture.root).readSnapshot()
    XCTAssertEqual(snapshot.archivedOwners, [fixture.owner])
    XCTAssertNotNil(snapshot.conversations.first { $0.id == fixture.owner.id })
  }

  func testFacadeRefusesReboundOwnerAndPreservesBothArtifacts() async throws {
    let fixture = try ArchiveFixture()
    defer { fixture.remove() }
    try fixture.writeBinding(session: "replacement")
    let binding = try Data(contentsOf: fixture.binding)
    let lease = try Data(contentsOf: fixture.lease)
    do {
      try await RealAgentBridgeInstaller.archiveBusAgent(
        owner: fixture.owner, installer: fixture.installer)
      XCTFail("a stale owner must be refused")
    } catch {}
    XCTAssertEqual(try Data(contentsOf: fixture.binding), binding)
    XCTAssertEqual(try Data(contentsOf: fixture.lease), lease)
    XCTAssertFalse(
      FileManager.default.fileExists(atPath: fixture.root.appendingPathComponent("archives").path))
  }

  func testSelectingAnArchiveOutsideThePollingBudgetLoadsItsHistoryAfterColdRestart() async throws {
    let fixture = try ArchiveFixture(includeMessage: false)
    defer { fixture.remove() }
    try FileManager.default.removeItem(at: fixture.binding)
    var expected: [String: String] = [:]
    for index in 0..<17 {
      let session = "saved-session-\(index)"
      let lease = AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: session)
      let row: [String: Any] = [
        "provider": "codex", "provider_session_id": session, "lease_id": lease,
        "channel": "4", "name": "saved-\(index)",
      ]
      let owner = try XCTUnwrap(OverlayConversationOwner(row: row))
      let bus = fixture.home.appendingPathComponent("saved-\(index).jsonl")
      let text = "Retained archive \(index)"
      expected[owner.id] = text
      try fixture.write(
        [
          "schema": "codescribe.transcript.v1", "status": "transcript_sealed", "sequence": 1,
          "session_id": "saved-take-\(index)", "utterance_id": "u1", "audience": owner.name,
          "text": text, "recipients": [row.merging(["bus": bus.path]) { _, value in value }],
        ], to: bus, newline: true)
      try fixture.write(
        row.merging([
          "schema": "codescribe.agent-archive.v1", "released": true, "bus": bus.path,
        ]) { _, value in value },
        to: fixture.root.appendingPathComponent("archives/\(lease)-4.json"))
    }
    for _ in 0..<2 {
      let reader = OverlayChannelDeliveryReader(root: fixture.root)
      let snapshot = try await reader.readSnapshot()
      XCTAssertEqual(snapshot.archivedOwners.count, 17)
      let saved = snapshot.conversations.filter { expected[$0.id] != nil }
      XCTAssertEqual(saved.count, 17, "Every saved owner remains discoverable")
      XCTAssertLessThanOrEqual(
        saved.filter { !$0.messages.isEmpty }.count, 16, "Background polling remains bounded")
      let unloaded = try XCTUnwrap(saved.first { $0.messages.isEmpty })
      let state = OverlayState()
      state.applyConversationSnapshot(snapshot)
      state.observeChannelDelivery(using: reader)
      state.selectConversation(unloaded.id)
      let deadline = ContinuousClock.now.advanced(by: .seconds(2))
      while state.selectedConversation?.messages.isEmpty == true && ContinuousClock.now < deadline {
        try await Task.sleep(for: .milliseconds(20))
      }
      XCTAssertEqual(state.selectedConversation?.messages.first?.text, expected[unloaded.id])
      XCTAssertEqual(state.selectedConversationID, unloaded.id)
    }
  }

  private func roster(session: String = "archive-session", alive: Bool? = false, open: Bool = false)
    -> CsChannelRosterState
  {
    .init(
      channel: "4", audience: "adam", provider: "codex", providerSessionId: session,
      open: open, loud: false, autosealDeadlineUnixMs: nil, followerAlive: alive)
  }

  private struct ArchiveFixture {
    let home: URL
    let installer: RealAgentBridgeInstaller
    let owner: OverlayConversationOwner
    var root: URL { installer.bridgeRoot }
    var bus: URL { home.appendingPathComponent("archive.jsonl") }
    var lease: URL { root.appendingPathComponent("leases/\(owner.leaseID).json") }
    var binding: URL { root.appendingPathComponent("vc.agent-audience-binding.v1.json") }

    init(includeMessage: Bool = true) throws {
      home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        .resolvingSymlinksInPath()
      installer = RealAgentBridgeInstaller(resourceRoot: nil, homeDirectory: home, environment: [:])
      owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": "archive-session", "name": "adam",
          "channel": "4",
          "lease_id": AgentPlaybackIdentity.leaseIdentifier(
            provider: "codex", session: "archive-session"),
        ]))
      try FileManager.default.createDirectory(
        at: lease.deletingLastPathComponent(), withIntermediateDirectories: true)
      try writeBinding(session: owner.providerSessionID)
      try write(
        [
          "schema": "codescribe.agent-bridge.lease.v1", "lease_id": owner.leaseID,
          "provider": owner.provider, "provider_session_id": owner.providerSessionID,
          "name": owner.name, "bus": bus.path, "cursor": 0,
          "pending": [
            ["kind": "seal", "delivery_id": String(repeating: "1", count: 24), "text": "Unread"]
          ],
        ], to: lease)
      if includeMessage {
        try write(
          [
            "schema": "codescribe.transcript.v1", "status": "transcript_sealed", "sequence": 1,
            "session_id": "archive-take", "utterance_id": "u1", "audience": "adam",
            "text": "Iwo Iwo Iwo Iwo Iwo",
            "recipients": [
              [
                "provider": owner.provider, "provider_session_id": owner.providerSessionID,
                "lease_id": owner.leaseID, "name": owner.name, "channel": owner.channel,
                "bus": bus.path,
              ]
            ],
          ], to: bus, newline: true)
      } else {
        try Data().write(to: bus)
      }
      // A scratch managed executable runs the real helper, never the live bus.
      let repo = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
      let source = try String(
        contentsOf: repo.appendingPathComponent("scripts/bus-demux.py"), encoding: .utf8)
      let firstNewline = try XCTUnwrap(source.firstIndex(of: "\n"))
      let command = installer.commandURL("cs-bus")
      try FileManager.default.createDirectory(
        at: command.deletingLastPathComponent(), withIntermediateDirectories: true)
      let managed =
        String(source[...firstNewline])
        + "# codescribe-managed-command: {\"managed_id\":\"archive-fixture\"}\n"
        + String(source[source.index(after: firstNewline)...])
      try managed.write(to: command, atomically: true, encoding: .utf8)
      try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: command.path)
    }

    func writeBinding(session: String) throws {
      try write(
        [
          "schema": "vc.agent-audience-binding.v1",
          "bindings": [
            "4": ["audience": "adam", "provider": "codex", "provider_session_id": session]
          ],
        ], to: binding)
    }
    func writeArchive() throws {
      try write(
        [
          "schema": "codescribe.agent-archive.v1", "released": true,
          "provider": owner.provider, "provider_session_id": owner.providerSessionID,
          "lease_id": owner.leaseID, "channel": owner.channel, "name": owner.name, "bus": bus.path,
        ], to: root.appendingPathComponent("archives/\(owner.leaseID)-4.json"))
    }
    func write(_ row: [String: Any], to path: URL, newline: Bool = false) throws {
      try FileManager.default.createDirectory(
        at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
      var data = try JSONSerialization.data(withJSONObject: row, options: [.sortedKeys])
      if newline { data.append(10) }
      try data.write(to: path, options: .atomic)
    }
    func remove() { try? FileManager.default.removeItem(at: home) }
  }
}
