import AppKit
import Foundation
import Observation
import SwiftUI
import XCTest
import os

@testable import Codescribe

/// Falsifiers for the agents list under the Agents button (✦): rows are a
/// projection of the roster in ascending channel order without channel 0, the
/// reception marks know whether a helper receipt stands behind the policy,
/// a missing receipt is "unknown" rather than the fail-open `.on`, and the
/// list is sized to its rows.
@MainActor
final class OverlayAgentsHoverListTests: XCTestCase {

  // MARK: - Fixtures

  private func owner(
    _ channel: String, session: String, lease: String
  ) throws -> OverlayConversationOwner {
    try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session,
        "lease_id": String(repeating: lease, count: 32), "channel": channel,
        "name": "agent-\(channel)",
      ]))
  }

  private func conversation(
    _ channel: String, session: String, lease: String
  ) throws -> OverlayConversation {
    let owner = try owner(channel, session: session, lease: lease)
    return OverlayConversation(
      id: owner.id, channel: channel, name: "agent-\(channel)", owner: owner, messages: [])
  }

  private func hud(
    session: String?, alive: Bool? = true, open: Bool = false, unreadSince: Date? = nil
  ) -> OverlayChannelHudProjection {
    OverlayChannelHudProjection(
      open: open, loud: false, autosealDeadline: nil, followerAlive: alive,
      provider: session == nil ? nil : "codex", providerSessionID: session,
      unreadSince: unreadSince)
  }

  private func delivery(_ channel: String, _ agent: String) -> OverlayChannelDelivery {
    OverlayChannelDelivery(
      channel: channel, agent: agent, deliveryID: nil, stage: nil, isOpen: false)
  }

  private func row(_ digit: String, name: String) -> OverlayAgentsHoverRow {
    OverlayAgentsHoverRow(
      digit: digit, name: name, conversationID: nil, dot: .idle, status: "",
      microphoneOpen: false, disconnected: false, identity: nil, policy: .on, unread: 0)
  }

  // MARK: - Rows

  func testRowsFollowAscendingChannelsWithoutChannelZeroAndCarryUnread() throws {
    let miksa = try conversation("2", session: "s2", lease: "a")
    let kuba = try conversation("5", session: "s5", lease: "b")
    let view = OverlayChannelStatusView(
      channels: [
        delivery("5", "Kuba"), delivery("0", "All"), delivery("3", "Igor"),
        delivery("2", "Miksa"),
        // Channel 10 has no chord: no row in the list either.
        delivery("10", "Founder"),
      ],
      unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "0": hud(session: nil, alive: nil), "2": hud(session: "s2"),
        "3": hud(session: "s3", alive: false), "5": hud(session: "s5", open: true),
        "10": hud(session: "s10"),
      ],
      conversations: [miksa, kuba], unreadCounts: [kuba.id: 3])
    let rows = view.agentsHoverRows
    XCTAssertEqual(rows.map(\.digit), ["2", "3", "5"], "ascending, no 0 and no 10")
    XCTAssertEqual(rows.map(\.name), ["Miksa", "Igor", "Kuba"])
    XCTAssertEqual(rows.map(\.unread), [0, 0, 3], "unread comes from unreadCounts")
    XCTAssertEqual(rows.map(\.conversationID), [miksa.id, nil, kuba.id])
    XCTAssertEqual(rows.map(\.disconnected), [false, true, false])
    XCTAssertEqual(rows.map(\.microphoneOpen), [false, false, true])
    XCTAssertEqual(rows.map(\.id), rows.map(\.digit))
    // Every field is the roster's own projection, not a second status.
    for (row, channel) in zip(rows, ["2", "3", "5"]) {
      let source = try XCTUnwrap(view.channels.first { $0.channel == channel })
      XCTAssertEqual(row.dot, view.presenceDotKind(for: source), channel)
      XCTAssertEqual(
        row.status,
        OverlayChannelChipHoverCard.spokenStatus(
          view.channelChipStatus(for: source, current: view.currentConversations)
        ).joined(separator: ". "), channel)
      XCTAssertEqual(row.policy, source.broadcastPolicy, channel)
      XCTAssertEqual(row.identity, source.broadcastIdentity, channel)
    }
  }

  /// Monika, 2026-11-04: channels silent for hours showed green on an old
  /// receipt and replying agents amber on a newer unreceipted one. The one
  /// dot reads presence; no stage reaches it anywhere.
  func testListDotIsPresenceAloneNotTheLastUtterancesStage() throws {
    let view = OverlayChannelStatusView(
      channels: [
        OverlayChannelDelivery(
          channel: "6", agent: "Miksa", deliveryID: "d6", stage: .received, isOpen: false),
        OverlayChannelDelivery(
          channel: "8", agent: "Kaja", deliveryID: "d8", stage: .sent, isOpen: false),
        OverlayChannelDelivery(
          channel: "3", agent: "Igor", deliveryID: "d3", stage: .received, isOpen: false),
        OverlayChannelDelivery(
          channel: "5", agent: "Kuba", deliveryID: nil, stage: nil, isOpen: false),
      ],
      unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "6": hud(session: "s6", alive: false), "8": hud(session: "s8"),
        "3": hud(session: "s3", open: true), "5": hud(session: nil, alive: nil),
      ])
    let dots = Dictionary(
      uniqueKeysWithValues: view.agentsHoverRows.map { ($0.digit, $0.dot) })
    XCTAssertEqual(dots["6"], .sleeping, "an old receipt does not keep a gone follower green")
    XCTAssertEqual(dots["8"], .present, "a sent utterance without a receipt is not amber")
    XCTAssertEqual(dots["3"], .listening, "the open microphone wins")
    XCTAssertEqual(dots["5"], .idle, "no liveness claim: a question mark")
  }

  /// Monika, 2026-11-04 (decision 2): a live reader past the stall horizon
  /// keeps its olive and gains a "!"; it is neither zzz nor a greyed row.
  /// The moon comes first, since a sleeper acknowledges nothing by design.
  func testStallKeepsOliveWithABangAndTheMoonComesFirst() throws {
    let stale = Date().addingTimeInterval(-OverlayState.followerStallHorizon - 60)
    let fresh = Date().addingTimeInterval(-60)
    var moon = delivery("7", "Nela")
    moon.broadcastPolicy = .quiet
    let view = OverlayChannelStatusView(
      channels: [delivery("2", "Miksa"), delivery("4", "Igor"), moon, delivery("8", "Kaja")],
      unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "2": hud(session: "s2", unreadSince: stale),
        "4": hud(session: "s4", unreadSince: fresh),
        "7": hud(session: "s7", unreadSince: stale),
        "8": hud(session: "s8", alive: false, unreadSince: stale),
      ])
    let dots = Dictionary(
      uniqueKeysWithValues: view.agentsHoverRows.map { ($0.digit, $0.dot) })
    XCTAssertEqual(dots["2"], .stalled, "past the horizon: olive with a bang")
    XCTAssertEqual(dots["4"], .present, "a fresh backlog is no stall")
    XCTAssertEqual(dots["7"], .asleep, "the moon comes before the stall")
    XCTAssertEqual(dots["8"], .sleeping, "a gone follower is a ring, never a stall")
    XCTAssertFalse(view.hasDeadFollower(delivery("2", "Miksa")), "the stall greys no row")
  }

  /// Monika, 2026-11-04: the Agents button's dot says a message waits on
  /// channel 0 or in the inbox (10); an agent's own unread stays on its chip.
  func testAgentsButtonDotCountsChannelZeroAndTheInboxOnly() throws {
    let kuba = try conversation("5", session: "s5", lease: "b")
    let all = OverlayConversation(id: "all", channel: "0", name: "All", owner: nil, messages: [])
    let inbox = OverlayConversation(id: "10", channel: "10", name: "Inbox", owner: nil, messages: [])
    XCTAssertTrue(inbox.isHumanInbox)
    func view(_ unread: [String: Int]) -> OverlayChannelStatusView {
      OverlayChannelStatusView(
        channels: [delivery("0", "All"), delivery("5", "Kuba")], unavailable: false,
        palette: .dark, animates: false,
        hudStates: ["0": hud(session: nil, alive: nil), "5": hud(session: "s5")],
        conversations: [all, kuba, inbox], unreadCounts: unread)
    }
    XCTAssertFalse(view([kuba.id: 3]).attentionUnread, "an agent's own replies: its chip, not ✦")
    XCTAssertTrue(view([all.id: 1]).attentionUnread, "channel 0")
    XCTAssertTrue(view([inbox.id: 2]).attentionUnread, "the inbox")
    XCTAssertFalse(view([:]).attentionUnread)
  }

  func testNoAgentsMeansNoRows() {
    let view = OverlayChannelStatusView(
      channels: [delivery("0", "All")], unavailable: false, palette: .dark, animates: false,
      hudStates: ["0": hud(session: nil, alive: nil)])
    XCTAssertTrue(view.agentsHoverRows.isEmpty)
  }

  // MARK: - Ledger receipts

  func testPolicyIsUnknownUntilAReceiptIsNoted() {
    let ledger = OverlayBroadcastPolicyLedger(write: { _, _ in }, readBack: { _ in nil })
    let identity = AgentPlaybackIdentity(
      provider: "junie", session: "hover-known", bus: "/dev/null")
    let other = AgentPlaybackIdentity(provider: "junie", session: "hover-other", bus: "/dev/null")
    XCTAssertFalse(ledger.policyKnown(identity), "no poll yet: unknown, not on")
    XCTAssertFalse(ledger.policyKnown(nil), "a row without an identity is never known")
    ledger.noteReceipts([identity])
    XCTAssertTrue(ledger.policyKnown(identity))
    XCTAssertFalse(ledger.policyKnown(other), "only the identities whose receipt was read")
    XCTAssertEqual(ledger.receipted, [identity])
    ledger.noteReceipts([other])
    XCTAssertFalse(ledger.policyKnown(identity), "a later poll without its receipt forgets it")
    XCTAssertTrue(ledger.policyKnown(other))
    ledger.noteReceipts([])
    XCTAssertFalse(ledger.policyKnown(other))
  }

  /// A quiet poll writes nothing: an equal set neither bumps the revision nor
  /// invalidates a row that reads the receipts.
  func testEqualReceiptsWriteNothing() {
    let ledger = OverlayBroadcastPolicyLedger(write: { _, _ in }, readBack: { _ in nil })
    let identity = AgentPlaybackIdentity(
      provider: "junie", session: "hover-quiet", bus: "/dev/null")
    ledger.noteReceipts([identity])
    let revision = ledger.revision
    let invalidated = OSAllocatedUnfairLock(initialState: false)
    withObservationTracking {
      _ = ledger.policyKnown(identity)
    } onChange: {
      invalidated.withLock { $0 = true }
    }
    ledger.noteReceipts([identity])
    XCTAssertEqual(ledger.revision, revision, "an equal set does not bump the revision")
    XCTAssertFalse(invalidated.withLock { $0 }, "an equal set invalidates no reader")
    ledger.noteReceipts([])
    XCTAssertTrue(invalidated.withLock { $0 }, "a changed set does reach the reader")
    XCTAssertEqual(ledger.revision, revision, "receipts never bump the write revision")
  }

  /// The app's own write is read back, so its agent is known before the next
  /// poll reports.
  func testOwnWriteMakesThePolicyKnown() async throws {
    let ledger = OverlayBroadcastPolicyLedger(write: { _, _ in }, readBack: { _ in .quiet })
    let identity = AgentPlaybackIdentity(
      provider: "junie", session: "hover-write", bus: "/dev/null")
    XCTAssertFalse(ledger.policyKnown(identity))
    try await ledger.change(identity, polled: .on) { $0.moon = true }
    XCTAssertTrue(ledger.policyKnown(identity))
    XCTAssertEqual(ledger.policy(for: identity, polled: .on), .quiet)
  }

  // MARK: - Helper receipts

  /// A missing, foreign or broken receipt is a missing key, while the roster's
  /// snapshot keeps failing open to `.on` for the same agent.
  func testReceiptsHoldOnlyAgentsWhoseReceiptWasRead() async throws {
    let fixture = try ReceiptFixture()
    defer { try? FileManager.default.removeItem(at: fixture.home) }
    let identity = fixture.identity
    func receipts() async -> [AgentPlaybackIdentity: AgentBroadcastPolicy] {
      await RealAgentBridgeInstaller.broadcastPolicyReceipts(
        for: [identity], installer: fixture.installer)
    }
    func snapshot() async -> AgentBroadcastPolicy? {
      await RealAgentBridgeInstaller.broadcastPolicySnapshot(
        for: [identity], installer: fixture.installer)[identity]
    }
    let missing = await receipts()
    XCTAssertNil(missing[identity], "no receipt file: no key")
    XCTAssertTrue(missing.isEmpty)
    let failOpen = await snapshot()
    XCTAssertEqual(failOpen, .on, "the roster still fails open")

    try await RealAgentBridgeInstaller.setBroadcastPolicy(
      .off, for: identity, installer: fixture.installer)
    let written = await receipts()
    XCTAssertEqual(written[identity], .off)

    var foreign = try fixture.receipt()
    foreign["provider_session_id"] = "foreign-session"
    try JSONSerialization.data(withJSONObject: foreign).write(to: fixture.policy, options: .atomic)
    let foreignReceipts = await receipts()
    XCTAssertNil(foreignReceipts[identity], "another agent's receipt is not this one's")

    try Data("{broken".utf8).write(to: fixture.policy, options: .atomic)
    let brokenReceipts = await receipts()
    XCTAssertNil(brokenReceipts[identity], "a broken receipt is no receipt")
    let brokenSnapshot = await snapshot()
    XCTAssertEqual(brokenSnapshot, .on)
  }

  // MARK: - Size

  func testListGrowsLinearlyWithItsRowsAndKeepsItsMinimumWidth() {
    let one = OverlayAgentsHoverList.size(rows: [row("1", name: "Bo")])
    let three = OverlayAgentsHoverList.size(
      rows: [row("1", name: "Bo"), row("2", name: "Al"), row("3", name: "Ed")])
    let step = OverlayAgentsHoverList.rowHeight + OverlayAgentsHoverList.rowSpacing
    XCTAssertEqual(
      one.height,
      2 * OverlayAgentsHoverList.verticalPadding + OverlayAgentsHoverList.rowHeight)
    XCTAssertEqual(three.height - one.height, 2 * step, accuracy: 0.001, "one step per row")
    XCTAssertEqual(
      OverlayAgentsHoverList.size(rows: []).height, 2 * OverlayAgentsHoverList.verticalPadding)
    for size in [one, three] {
      XCTAssertGreaterThanOrEqual(size.width, OverlayAgentsHoverList.minimumWidth)
    }
    // Nine agents fit without a scroll view, every name whole.
    let nine = OverlayAgentsHoverList.size(
      rows: (1...9).map { row(String($0), name: "Agent\($0)") })
    XCTAssertEqual(nine.height - one.height, 8 * step, accuracy: 0.001)
    let long = OverlayAgentsHoverList.size(
      rows: [row("1", name: "Bo"), row("2", name: "Konstantynopolitańczyk")])
    let longChip = OverlayChannelPill.chipWidth(
      nameWidth: OverlayChannelPill.nameWidth("Konstantynopolitańczyk", scale: 1), form: .chip,
      scale: 1, presenceDot: true)
    XCTAssertGreaterThanOrEqual(long.width, longChip, "the widest name sets the width")
    XCTAssertGreaterThanOrEqual(long.width, one.width)
    // The list's chip carries the presence dot between the digit and the
    // name; the bar's chip does not, and is narrower by the dot and a gap.
    let bare = OverlayChannelPill.chipWidth(nameWidth: 40, form: .chip, scale: 1)
    let dotted = OverlayChannelPill.chipWidth(
      nameWidth: 40, form: .chip, scale: 1, presenceDot: true)
    XCTAssertEqual(
      dotted - bare, OverlayAgentStatusDot.diameter + OverlayChannelPill.chipGap, accuracy: 0.001)
  }

  /// A scratch home with a managed copy of the real helper, the same
  /// fixture `OverlayBroadcastPolicyTests` uses for receipts.
  private struct ReceiptFixture {
    let home: URL
    let installer: RealAgentBridgeInstaller
    let identity: AgentPlaybackIdentity
    var policy: URL {
      installer.bridgeRoot.appendingPathComponent(
        "runtime/broadcast-policy/\(identity.storageKey).json")
    }

    init() throws {
      // The helper compares the lease's bus against POSIX realpath; a scratch
      // home under the repo's ignored build/ directory crosses no symlink.
      let repo = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
      home = repo.appendingPathComponent("build/test-scratch/\(UUID().uuidString)")
      try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
      installer = RealAgentBridgeInstaller(resourceRoot: nil, homeDirectory: home, environment: [:])
      identity = AgentPlaybackIdentity(
        provider: "junie", session: "hover-receipt-fixture",
        bus: home.appendingPathComponent("fixture.jsonl").path)
      let lease = installer.bridgeRoot.appendingPathComponent("leases/\(identity.leaseID).json")
      try FileManager.default.createDirectory(
        at: lease.deletingLastPathComponent(), withIntermediateDirectories: true)
      try JSONSerialization.data(withJSONObject: [
        "schema": "codescribe.agent-bridge.lease.v1", "provider": identity.provider,
        "provider_session_id": identity.session, "lease_id": identity.leaseID, "bus": identity.bus,
      ]).write(to: lease)
      // Execute a scratch managed copy of the real helper, never the live bus.
      let source = try String(
        contentsOf: repo.appendingPathComponent("scripts/bus-demux.py"), encoding: .utf8)
      let firstNewline = try XCTUnwrap(source.firstIndex(of: "\n"))
      let command = installer.commandURL("cs-bus")
      try FileManager.default.createDirectory(
        at: command.deletingLastPathComponent(), withIntermediateDirectories: true)
      let managed =
        String(source[...firstNewline])
        + "# codescribe-managed-command: {\"managed_id\":\"hover-receipt-fixture\"}\n"
        + String(source[source.index(after: firstNewline)...])
      try managed.write(to: command, atomically: true, encoding: .utf8)
      try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: command.path)
    }

    func receipt() throws -> [String: Any] {
      try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: policy)) as? [String: Any])
    }
  }
}
