import AppKit
import Foundation
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayAgentArchiveTests: XCTestCase {
  func testDisconnectingLastAgentShowsLocalizedHintAlongsideBroadcastAndSavedHistory() async throws
  {
    let state = OverlayState()
    state.applyChannelRoster([roster()])
    let owner = try XCTUnwrap(state.archiveCandidate(for: "4"))
    let agent = OverlayConversation(
      id: owner.id, channel: "4", name: "adam", owner: owner, messages: [])
    let broadcast = OverlayConversation(
      id: "0", channel: "0", name: "All", owner: nil, messages: [])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, agent]))

    func drawer() -> OverlayChannelStatusView {
      var view = OverlayChannelStatusView(
        channels: state.visibleChannelRows, unavailable: false, palette: .dark, animates: false,
        hudStates: state.channelHudStates, conversations: state.conversations,
        onSelectConversation: { _ in })
      view.archivedOwners = state.archivedAgentOwners
      return view
    }

    XCTAssertFalse(drawer().showsConnectionHint, "An agent is still listed")
    state.archiveAgentCommand = { _, _ in }
    await state.archiveAgent(owner)
    XCTAssertTrue(state.visibleChannelRows.isEmpty)
    XCTAssertEqual(state.conversations.map(\.id), [broadcast.id, agent.id])
    let emptyDrawer = drawer()
    XCTAssertTrue(emptyDrawer.showsConnectionHint)
    XCTAssertEqual(emptyDrawer.currentConversations.map(\.id), [broadcast.id])
    XCTAssertEqual(emptyDrawer.savedConversations.map(\.id), [agent.id])

    let key =
      "No agents connected. To connect an agent to Codescribe, run the `codescribe` skill in its session."
    let polish = try XCTUnwrap(Bundle.main.path(forResource: "pl", ofType: "lproj"))
    XCTAssertEqual(
      try XCTUnwrap(Bundle(path: polish)).localizedString(forKey: key, value: nil, table: nil),
      "Brak podłączonych agentów. Aby połączyć agenta z Codescribe, uruchom skill `codescribe` w jego sesji."
    )
    for locale in ["en", "pl"] {
      let host = NSHostingView(
        rootView: emptyDrawer.monitorBody.frame(width: 336)
          .environment(\.locale, Locale(identifier: locale)))
      let window = NSWindow(
        contentRect: .init(x: -10000, y: -10000, width: 336, height: 480),
        styleMask: [.titled], backing: .buffered, defer: false)
      window.contentView = host
      window.orderFrontRegardless()
      defer { window.orderOut(nil) }
      host.layoutSubtreeIfNeeded()
      try await Task.sleep(for: .milliseconds(50))
      host.layoutSubtreeIfNeeded()
      XCTAssertEqual(host.fittingSize.width, 336, accuracy: 1)
      XCTAssertGreaterThan(host.fittingSize.height, 200, "The hint occupies space between the rows")
      XCTAssertLessThan(host.fittingSize.height, 480, "The hint fits the sidebar")
    }
    state.applyChannelRoster([roster(session: "replacement", alive: true)])
    XCTAssertFalse(drawer().showsConnectionHint, "A newly attached agent hides the hint")
  }

  /// Founder, 2026-10-14: the cross is on every listed agent. A dead reader is
  /// released, a live one is stopped first, an open channel is refused with
  /// a reason, and a stale click on a replaced owner does nothing.
  func testEveryBoundOwnerOffersArchiveAndTheClickCarriesTheReaderDecision() async throws {
    let state = OverlayState()
    for (alive, open) in [(true, false), (nil, false), (false, false), (false, true)] {
      state.applyChannelRoster([roster(alive: alive, open: open)])
      XCTAssertNotNil(
        state.archiveCandidate(for: "4"), "alive=\(String(describing: alive)) open=\(open)")
    }
    var releases: [Bool] = []
    state.archiveAgentCommand = { _, release in releases.append(release) }

    state.applyChannelRoster([roster(alive: false, open: true)])
    await state.archiveAgent(try XCTUnwrap(state.archiveCandidate(for: "4")))
    XCTAssertEqual(releases, [], "an open channel never reaches the helper")
    XCTAssertEqual(
      state.agentArchiveError,
      "Couldn't archive adam: its channel is open. Hang up first; the conversation is retained.")

    state.applyChannelRoster([roster(alive: true)])
    await state.archiveAgent(try XCTUnwrap(state.archiveCandidate(for: "4")))
    XCTAssertEqual(releases, [true], "a live reader is stopped before the digit is released")
    XCTAssertNil(state.agentArchiveError)
    XCTAssertTrue(state.visibleChannelRows.isEmpty)

    state.applyChannelRoster([roster(session: "second", alive: false)])
    await state.archiveAgent(try XCTUnwrap(state.archiveCandidate(for: "4")))
    XCTAssertEqual(releases, [true, false], "a dead reader is simply released")

    state.applyChannelRoster([roster(session: "third")])
    let owner = try XCTUnwrap(state.archiveCandidate(for: "4"))
    state.applyChannelRoster([roster(session: "replacement", alive: true)])
    await state.archiveAgent(owner)
    XCTAssertEqual(releases, [true, false], "a stale click on a replaced owner does nothing")
    XCTAssertEqual(state.visibleChannelRows.map(\.agent), ["adam"])
  }

  /// Liveness has two layers: the follower process (heartbeat) and the
  /// conversation behind it (acknowledgements). Alive with a backlog older
  /// than the horizon reads as "not reading" with the minutes; younger
  /// backlog, no backlog or a dead follower do not.
  func testStalledFollowerIsAliveWithAnOldUnreadBacklog() throws {
    let state = OverlayState()
    let old = Date().addingTimeInterval(-OverlayState.followerStallHorizon - 120)
    let fresh = Date().addingTimeInterval(-60)
    func drawer() -> OverlayChannelStatusView {
      OverlayChannelStatusView(
        channels: state.visibleChannelRows, unavailable: false, palette: .dark, animates: false,
        hudStates: state.channelHudStates, conversations: [])
    }
    for (alive, since, stalled) in [
      (true, old, true), (true, fresh, false), (true, nil, false), (false, old, false),
      (nil, old, false),
    ] as [(Bool?, Date?, Bool)] {
      state.applyChannelRoster([roster(alive: alive, unreadSince: since)])
      let hud = try XCTUnwrap(state.channelHudStates["4"])
      XCTAssertEqual(
        hud.isStalled(), stalled,
        "alive=\(String(describing: alive)) since=\(String(describing: since))")
      let row = try XCTUnwrap(state.visibleChannelRows.first)
      XCTAssertEqual(drawer().hasStalledFollower(row), stalled)
      XCTAssertEqual(drawer().statusSymbol(for: row) == "zzz", stalled)
    }
    state.applyChannelRoster([roster(alive: true, unreadSince: old)])
    let row = try XCTUnwrap(state.visibleChannelRows.first)
    XCTAssertEqual(drawer().stalledMinutes(row), 12)
    XCTAssertTrue(drawer().detail(for: row).hasSuffix("nothing read for 12 min"))
    XCTAssertEqual(
      drawer().archiveHelp(for: row),
      "Stop the agent's reader, remove from list and move to archive")
  }

  func testArchiveFailureReasonNamesTheHelperRefusalOrThePreflightFailure() {
    XCTAssertEqual(
      OverlayState.archiveFailureReason(
        CsError.Recording(msg: "agent archive refused: agent reader is alive; channel retained\n")),
      "agent archive refused: agent reader is alive; channel retained")
    XCTAssertEqual(
      OverlayState.archiveFailureReason(CocoaError(.fileNoSuchFile)),
      "the cs-bus helper is not installed")
    XCTAssertEqual(
      OverlayState.archiveFailureReason(CocoaError(.fileReadCorruptFile)),
      "the agent's lease no longer matches this owner")
    XCTAssertEqual(
      OverlayState.archiveFailureReason(CocoaError(.fileWriteUnknown)),
      "the helper returned no valid receipt")
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
    state.archiveAgentCommand = { _, _ in throw CocoaError(.fileWriteUnknown) }
    await state.archiveAgent(owner)
    XCTAssertEqual(
      state.agentArchiveError,
      "Couldn't archive adam: the helper returned no valid receipt. The channel and conversation are retained."
    )
    XCTAssertEqual(state.visibleChannelRows.count, 1)
    XCTAssertEqual(state.selectedConversationID, owner.id)
    XCTAssertEqual(state.conversationDrafts[owner.id], "Unsent draft")
    state.archiveAgentCommand = { _, _ in }
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
        owner: fixture.owner, installer: fixture.installer, archive: fixture.execute)
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
        owner: fixture.owner, installer: fixture.installer, archive: fixture.execute)
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
    var archiveBuses: [String: URL] = [:]
    let otherRow: [String: Any] = [
      "provider": "codex", "provider_session_id": "other-session", "channel": "7",
      "lease_id": AgentPlaybackIdentity.leaseIdentifier(
        provider: "codex", session: "other-session"),
      "name": "other-agent",
    ]
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
      archiveBuses[owner.id] = bus
      try fixture.write(
        [
          "schema": "codescribe.transcript.v1", "status": "transcript_sealed", "sequence": 1,
          "session_id": "saved-take-\(index)", "utterance_id": "u1", "audience": owner.name,
          "text": text, "recipients": [row.merging(["bus": bus.path]) { _, value in value }],
        ], to: bus, newline: true)
      try fixture.append(
        otherRow.merging([
          "schema": "codescribe.agent-reply.v1", "reply_id": String(format: "%024x", index + 1001),
          "text": "Historical mirrored reply \(index)", "association": "unsolicited",
          "emitted_at": "2026-10-05T04:00:00Z",
        ]) { _, value in value }, to: bus)
      try fixture.write(
        row.merging([
          "schema": "codescribe.agent-archive.v1", "released": true, "bus": bus.path,
          "name": "Renamed saved label \(index)",
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
      XCTAssertFalse(unloaded.historyLoaded)
      let state = OverlayState()
      state.applyConversationSnapshot(snapshot)
      var unsolicitedPresentations = 0
      state.onChannelPresentationChanged = { [weak state] in
        if state?.freshReplyPresentationRequested == true { unsolicitedPresentations += 1 }
      }
      state.observeChannelDelivery(using: reader)
      state.selectConversation(unloaded.id)
      let deadline = ContinuousClock.now.advanced(by: .seconds(2))
      while state.selectedConversation?.messages.isEmpty == true && ContinuousClock.now < deadline {
        try await Task.sleep(for: .milliseconds(20))
      }
      XCTAssertEqual(state.selectedConversation?.messages.first?.text, expected[unloaded.id])
      XCTAssertTrue(try XCTUnwrap(state.selectedConversation).historyLoaded)
      XCTAssertEqual(state.selectedConversationID, unloaded.id)
      XCTAssertEqual(unsolicitedPresentations, 0, "Loading archive mirrors is passive history")
      let bytesBefore = await reader.consumedBytes
      _ = try await reader.readSnapshot(selectedOwner: unloaded.owner)
      let bytesAfter = await reader.consumedBytes
      XCTAssertEqual(bytesAfter, bytesBefore, "Loaded archive polls must not replay its bytes")
      let newReplyID = UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(24)
        .lowercased()
      try fixture.append(
        otherRow.merging([
          "schema": "codescribe.agent-reply.v1", "reply_id": newReplyID,
          "text": "A newly appended reply", "association": "unsolicited",
          "emitted_at": "2026-10-07T04:00:00Z",
        ]) { _, value in value }, to: try XCTUnwrap(archiveBuses[unloaded.id]))
      state.applyConversationSnapshot(try await reader.readSnapshot(selectedOwner: unloaded.owner))
      XCTAssertEqual(state.selectedConversation?.owner?.providerSessionID, "other-session")
      XCTAssertEqual(unsolicitedPresentations, 1, "Later fresh replies retain existing navigation")
    }
  }

  private func roster(
    session: String = "archive-session", alive: Bool? = false, open: Bool = false,
    unreadSince: Date? = nil
  ) -> CsChannelRosterState {
    .init(
      channel: "4", audience: "adam", provider: "codex", providerSessionId: session,
      open: open, loud: false, autosealDeadlineUnixMs: nil, followerAlive: alive,
      unreadSinceUnixMs: unreadSince.map { Int64($0.timeIntervalSince1970 * 1_000) })
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

    @MainActor
    func execute(_ request: CsAgentArchiveRequest) async throws -> String {
      // Scratch helper execution verifies the native request facade. The real
      // RecordingController lifecycle and cancellation are covered in Rust.
      try await Task.detached {
        let process = Process()
        let output = Pipe()
        process.executableURL = URL(fileURLWithPath: request.executable)
        process.arguments =
          [
            "--archive-agent", String(request.channel), "--provider", request.provider,
            "--session", request.providerSessionId, "--lease", request.leaseId,
            "--bus", request.bus, "--bridge-home", request.bridgeHome,
          ] + (request.releaseReader ? ["--release-reader"] : [])
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0,
          let bytes = try output.fileHandleForReading.read(upToCount: 65537), bytes.count <= 65536,
          let receipt = String(data: bytes, encoding: .utf8)
        else { throw CocoaError(.fileWriteUnknown) }
        return receipt
      }.value
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
    func append(_ row: [String: Any], to path: URL) throws {
      var data = try JSONSerialization.data(withJSONObject: row, options: [.sortedKeys])
      data.append(10)
      let handle = try FileHandle(forWritingTo: path)
      defer { try? handle.close() }
      try handle.seekToEnd()
      try handle.write(contentsOf: data)
    }
    func remove() { try? FileManager.default.removeItem(at: home) }
  }
}
