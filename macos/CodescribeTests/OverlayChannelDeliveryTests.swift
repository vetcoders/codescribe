import AppKit
import CryptoKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayChannelDeliveryTests: XCTestCase {
  func testPeerReplyKeepsSenderIdentityOnBroadcastDestination() throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    var projection = OverlayChannelDelivery.Bus()
    var row = fixture.recipient()
    row["schema"] = "codescribe.agent-reply.v1"
    row["reply_id"] = "peer-reply"
    row["text"] = "Reply to everyone"
    row["channel"] = "0"
    row["sender"] = fixture.recipient()
    projection.consume(row)
    let conversations = projection.conversations(busPath: fixture.bus.path)
    let conversation = try XCTUnwrap(conversations.first { $0.id == "0" })
    XCTAssertEqual(conversation.channel, "0")
    XCTAssertNil(conversation.owner, "All is the destination; its messages retain their authors")
    XCTAssertEqual(conversation.messages.first?.owner?.channel, "1")
    XCTAssertEqual(conversation.messages.first?.owner?.providerSessionID, "agent-session")
    XCTAssertEqual(conversation.messages.map(\.text), ["Reply to everyone"])
    // The previous assertion excluded this unsolicited reply from All. The
    // destination contract requires the reply itself, with no invented question.
    XCTAssertEqual(conversation.messages.map(\.kind), [.reply])
    XCTAssertTrue(
      conversations.filter { $0.owner != nil }.allSatisfy { $0.messages.isEmpty },
      "Authorship must not copy a broadcast into the sender's private channel")
  }

  func testUnsolicitedPeerBroadcastKeepsDestinationAndSenderAcrossRestart() throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    var projection = OverlayChannelDelivery.Bus()
    var recipient = fixture.recipient()
    recipient["provider_session_id"] = "recipient-session"
    recipient["lease_id"] = String(repeating: "a", count: 32)
    recipient["channel"] = "2"
    let row: [String: Any] = [
      "schema": "codescribe.agent-reply.v1", "kind": "agent_reply",
      "reply_id": Fixture.firstID, "message_id": Fixture.firstID,
      "association": "unsolicited", "channel": "2", "origin_channel": "0",
      "peer_to": "recipient", "sender": fixture.recipient(), "recipients": [recipient],
      "text": "Unsolicited broadcast", "emitted_at": "2026-10-15T04:09:10Z",
    ]
    projection.consume(row)
    projection.consume(row)
    for bus in [
      projection,
      try JSONDecoder().decode(
        OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(projection)),
    ] {
      let conversations = bus.conversations(busPath: fixture.bus.path)
      let all = try XCTUnwrap(conversations.first { $0.id == "0" })
      XCTAssertEqual(all.messages.map(\.id), ["reply:" + Fixture.firstID])
      let message = try XCTUnwrap(all.messages.first)
      XCTAssertEqual(message.sourceChannel, "0", "The original route outranks the local copy")
      XCTAssertEqual(message.owner?.channel, "1")
      XCTAssertEqual(message.owner?.leaseID, Fixture.leaseID)
      XCTAssertNil(message.replyTo)
      XCTAssertTrue(message.unsolicited)
      XCTAssertEqual(message.recipients.map(\.owner.channel), ["2"])
      XCTAssertTrue(conversations.filter { $0.owner != nil }.allSatisfy { $0.messages.isEmpty })
    }
  }

  func testPeerBroadcastReaderMergesRecipientCopiesAndAcknowledgments() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let shared = fixture.root.appendingPathComponent("a-recipient-copy.jsonl")
    var sender = fixture.recipient()
    sender["provider_session_id"] = "sender-session"
    sender["lease_id"] = String(repeating: "b", count: 32)
    sender["name"] = "leszek"
    sender["channel"] = "3"
    var other = fixture.recipient()
    other["provider_session_id"] = "other-session"
    other["lease_id"] = String(repeating: "c", count: 32)
    other["name"] = "other"
    other["channel"] = "2"
    var row: [String: Any] = [
      "schema": "codescribe.agent-reply.v1", "kind": "agent_reply",
      "reply_id": Fixture.firstID, "message_id": Fixture.firstID,
      "association": "unsolicited", "channel": "0", "peer_to": "james",
      "sender": sender, "recipients": [fixture.recipient()],
      "text": "One broadcast", "emitted_at": "2026-10-15T04:09:10Z",
    ]
    try fixture.append(row)
    // Real peer copies retain the original channel even though peer_to and
    // frozen recipients differ in each recipient journal.
    row["peer_to"] = "other"
    row["recipients"] = [other]
    row["text"] = "Updated broadcast"
    row["emitted_at"] = "2026-10-15T04:09:11Z"
    var bytes = try JSONSerialization.data(withJSONObject: row)
    bytes.append(10)
    try bytes.write(to: shared)
    let reader = OverlayChannelDeliveryReader(root: fixture.root, sharedBus: shared)
    let first = try await reader.readSnapshot()
    let message = try XCTUnwrap(first.conversations.first { $0.id == "0" }?.messages.first)
    let receipt = try XCTUnwrap(message.recipients.first { $0.owner.leaseID == Fixture.leaseID })
    let delivery = try XCTUnwrap(receipt.deliveryID)
    try fixture.append([
      "schema": "codescribe.agent-ack.v1", "delivery_id": delivery,
      "provider": "codex", "provider_session_id": "agent-session",
      "lease_id": Fixture.leaseID, "channel": "1", "agent": "james",
    ])
    for snapshot in [
      try await reader.readSnapshot(checkpointTime: .now.advanced(by: .seconds(31))),
      try await OverlayChannelDeliveryReader(root: fixture.root, sharedBus: shared).readSnapshot(),
    ] {
      let all = try XCTUnwrap(snapshot.conversations.first { $0.id == "0" })
      XCTAssertEqual(all.messages.map(\.id), ["reply:" + Fixture.firstID])
      let merged = try XCTUnwrap(all.messages.first)
      XCTAssertEqual(
        merged.text, "Updated broadcast", "A delayed recipient copy cannot rewind text")
      XCTAssertEqual(merged.sourceChannel, "0")
      XCTAssertEqual(merged.owner?.name, "leszek")
      XCTAssertEqual(merged.owner?.channel, "3")
      XCTAssertEqual(Set(merged.recipients.map(\.owner.channel)), ["1", "2"])
      XCTAssertEqual(
        merged.recipients.first { $0.owner.leaseID == Fixture.leaseID }?.acknowledged, true)
      XCTAssertEqual(merged.recipients.first { $0.owner.channel == "2" }?.acknowledged, false)
      XCTAssertTrue(
        snapshot.conversations.filter { $0.owner != nil }.allSatisfy { $0.messages.isEmpty })
    }
  }

  func testReaderRebuildsProjectionThatDroppedPeerDestinations() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append([
      "schema": "codescribe.agent-reply.v1", "reply_id": Fixture.firstID,
      "association": "unsolicited", "channel": "0", "peer_to": "all",
      "sender": fixture.recipient(), "recipients": [], "text": "Retained broadcast",
    ])
    _ = try await OverlayChannelDeliveryReader(root: fixture.root).readSnapshot()
    var marks = OverlayDeliveryCursorStore.load(root: fixture.root)
    var mark = try XCTUnwrap(marks[fixture.bus.path])
    var projection = try XCTUnwrap(
      JSONSerialization.jsonObject(with: XCTUnwrap(mark.projection)) as? [String: Any])
    projection.removeValue(forKey: "destinationVersion")
    var messages = try XCTUnwrap(projection["messages"] as? [String: [String: Any]])
    for id in messages.keys { messages[id]?.removeValue(forKey: "sourceChannel") }
    projection["messages"] = messages
    mark.projection = try JSONSerialization.data(withJSONObject: projection)
    marks[fixture.bus.path] = mark
    try OverlayDeliveryCursorStore.save(root: fixture.root, marks: marks)

    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let snapshot = try await reader.readSnapshot()
    let all = try XCTUnwrap(snapshot.conversations.first { $0.id == "0" })
    XCTAssertEqual(all.messages.map(\.id), ["reply:" + Fixture.firstID])
    XCTAssertEqual(all.messages.first?.sourceChannel, "0")
    let consumed = await reader.consumedBytes
    XCTAssertGreaterThan(
      consumed, 0, "A cursor alone cannot restore discarded destination metadata")
    let restarted = OverlayChannelDeliveryReader(root: fixture.root)
    let restored = try await restarted.readSnapshot()
    XCTAssertEqual(restored, snapshot)
    let reread = await restarted.consumedBytes
    XCTAssertEqual(reread, 0, "The repaired cache must keep the existing idle read behavior")
  }

  func testMalformedPeerSenderCannotBorrowTopLevelAuthor() throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    var projection = OverlayChannelDelivery.Bus()
    var row = fixture.recipient()
    row["schema"] = "codescribe.agent-reply.v1"
    row["reply_id"] = "malformed-peer"
    row["text"] = "Wrong attribution"
    row["sender"] = ["channel": "3"]
    projection.consume(row)
    row["sender"] = fixture.recipient()
    row.removeValue(forKey: "channel")
    projection.consume(row)
    XCTAssertTrue(
      projection.conversations(busPath: fixture.bus.path).allSatisfy {
        $0.messages.isEmpty
      }, "A peer row needs both a valid author and an explicit destination")
  }

  func testPeerReplyReachesItsFrozenRecipientAndSettlesOnThatLeasesAck() throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    var projection = OverlayChannelDelivery.Bus()
    let sender: [String: Any] = [
      "provider": "claude-code", "provider_session_id": "peer-session",
      "lease_id": "abcdefabcdefabcdefabcdefabcdefab", "name": "klaudiusz", "channel": "2",
    ]
    var row: [String: Any] = [
      "schema": "codescribe.agent-reply.v1", "kind": "agent_reply",
      "reply_id": "f5b17a3ac49b375c6fe6a182", "message_id": "f5b17a3ac49b375c6fe6a182",
      "source_event_id": "f5b17a3ac49b375c6fe6a182", "association": "unsolicited",
      "channel": "1", "peer_to": "james", "text": "Spec for james", "sender": sender,
      "recipients": [fixture.recipient()],
    ]
    projection.consume(row)
    row["reply_id"] = "0f0f0f0f0f0f0f0f0f0f0f0f"
    row["message_id"] = row["reply_id"]
    row["source_event_id"] = row["reply_id"]
    row["peer_to"] = "nobody"
    row["recipients"] = []
    row["text"] = "Spec for an unbound name"
    projection.consume(row)
    // The same name now holds a later lease; the frozen recipient keeps the row.
    let laterLease = "fedcba9876543210fedcba9876543210"
    projection.observeLease(
      [
        "schema": "codescribe.agent-bridge.lease.v1", "lease_id": laterLease,
        "provider": "codex", "provider_session_id": "agent-session", "bus": fixture.bus.path,
        "pending": [],
      ], channel: "1",
      binding: ["audience": "james", "provider": "codex", "provider_session_id": "agent-session"])
    let conversations = projection.conversations(busPath: fixture.bus.path)
    let james = try XCTUnwrap(conversations.first { $0.owner?.leaseID == Fixture.leaseID })
    XCTAssertEqual(james.messages.map(\.text), ["Spec for james"])
    XCTAssertEqual(james.messages.first?.owner?.name, "klaudiusz")
    // `_identity` in scripts/bus-demux.py for a peer message, which carries no audience.
    let delivery = "131d4e4cd70bef347dab33e7"
    XCTAssertEqual(
      james.messages.first?.recipients.map {
        "\($0.owner.name) \($0.deliveryID ?? "-") \($0.acknowledged)"
      },
      ["james \(delivery) false"], "The frozen owner lease names the demux delivery")
    XCTAssertTrue(
      conversations.first { $0.owner?.leaseID == laterLease }?.messages.isEmpty ?? false,
      "A later lease of the same name inherits neither the row nor a delivery")
    let klaudiusz = try XCTUnwrap(
      conversations.first { $0.owner?.providerSessionID == "peer-session" })
    XCTAssertTrue(klaudiusz.messages.isEmpty, "The sender is attribution, not a destination")
    XCTAssertTrue(conversations.first { $0.id == "0" }!.messages.isEmpty)
    XCTAssertEqual(
      projection.receiptCoordinates().map { "\($0.0.leaseID) \($0.1)" },
      ["\(Fixture.leaseID) \(delivery)"])
    let envelope: [String: Any] = [
      "kind": "message", "producer_schema": "codescribe.agent-reply.v1",
      "message_id": "f5b17a3ac49b375c6fe6a182", "delivery_id": delivery,
      "lease_id": Fixture.leaseID, "provider": "codex", "provider_session_id": "agent-session",
      "bus": fixture.bus.path, "channel": "1",
    ]
    projection.observeAcknowledgment(
      [
        "lease_id": Fixture.leaseID, "delivery_id": delivery, "bus": fixture.bus.path,
        "envelope": envelope,
      ],
      owner: try XCTUnwrap(james.owner), delivery: delivery, busPath: fixture.bus.path)
    // Replaying the published row keeps the settled recipient.
    row["reply_id"] = "f5b17a3ac49b375c6fe6a182"
    row["message_id"] = row["reply_id"]
    row["source_event_id"] = row["reply_id"]
    row["peer_to"] = "james"
    row["recipients"] = [fixture.recipient()]
    row["text"] = "Spec for james"
    projection.consume(row)
    let settled = try XCTUnwrap(
      projection.conversations(busPath: fixture.bus.path).first {
        $0.owner?.leaseID == Fixture.leaseID
      })
    XCTAssertEqual(settled.messages.first?.recipients.map(\.acknowledged), [true])
    XCTAssertTrue(projection.receiptCoordinates().isEmpty, "The marker settles polling")
  }

  func testTypedMessagePointersBecomeChipsAndLeaveTheBubbleBody() throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    var projection = OverlayChannelDelivery.Bus()
    let path = "/Users/tester/.codescribe/assets/inline_0549d6ff.png"
    let text =
      "Dodaj do listy\n[attachment] w prozie zostaje, to tekst Foundera\n"
      + "[attachment] inline_0549d6ff.png (image/png, 31113 bytes): \(path)"
    projection.consume([
      "schema": "codescribe.agent-user-message.v1", "kind": "agent_user_message",
      "message_id": Fixture.firstID, "source_event_id": Fixture.firstID, "source": "typed",
      "audience": "james", "text": text, "recipients": [fixture.recipient()],
      "attachments": [
        [
          "name": "inline_0549d6ff.png", "path": path, "media_type": "image/png",
          "bytes": 31113, "sha256": "0549d6ff",
        ],
        ["name": "relative.png", "path": "relative.png"],
      ],
    ])
    let message = try XCTUnwrap(
      projection.conversations(busPath: fixture.bus.path).first {
        $0.owner?.providerSessionID == "agent-session"
      }?.messages.first)
    XCTAssertEqual(
      message.attachments,
      [
        OverlayMessageAttachment(
          name: "inline_0549d6ff.png", path: path, mediaType: "image/png", bytes: 31113)
      ], "Only absolute pointers become chips")
    XCTAssertEqual(
      message.bodyText, "Dodaj do listy\n[attachment] w prozie zostaje, to tekst Foundera",
      "Only the generated pointer line leaves the body; typed text stays")
    XCTAssertEqual(message.text, text, "The published text stays intact for copying and agents")
    var plain = projection
    plain.consume([
      "schema": "codescribe.agent-user-message.v1", "kind": "agent_user_message",
      "message_id": "0a0a0a0a0a0a0a0a0a0a0a0a", "source_event_id": "0a0a0a0a0a0a0a0a0a0a0a0a",
      "source": "typed", "audience": "james", "text": "[attachment] in prose stays",
      "recipients": [fixture.recipient()],
    ])
    let prose = try XCTUnwrap(
      plain.conversations(busPath: fixture.bus.path).first {
        $0.owner?.providerSessionID == "agent-session"
      }?.messages.last)
    XCTAssertNil(prose.attachments)
    XCTAssertEqual(prose.bodyText, prose.text, "Without pointers no line is removed")
  }

  func testRestoredBroadcastOwnerRecoversCurrentThreadWithoutReplayingHistory() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    let peer = fixture.root.appendingPathComponent("000-peer.jsonl")
    var reply = fixture.recipient()
    reply["schema"] = "codescribe.agent-reply.v1"
    reply["reply_id"] = "peer-current"
    reply["text"] = "Current reply"
    reply["channel"] = "0"
    reply["sender"] = fixture.recipient()
    var previous = fixture.recipient()
    previous["provider_session_id"] = "previous-session"
    previous["lease_id"] = String(repeating: "a", count: 32)
    previous["schema"] = "codescribe.agent-reply.v1"
    previous["reply_id"] = "peer-previous"
    previous["text"] = "Previous reply"
    var bytes = try JSONSerialization.data(withJSONObject: reply)
    bytes.append(10)
    bytes.append(try JSONSerialization.data(withJSONObject: previous))
    bytes.append(10)
    try bytes.write(to: peer)
    _ = try await OverlayChannelDeliveryReader(root: fixture.root, sharedBus: peer).readSnapshot()

    // Simulate the shipped cache: the same exact owner was first observed on
    // a lexicographically earlier peer bus with the broadcast route as its slot.
    var marks = OverlayDeliveryCursorStore.load(root: fixture.root)
    for path in marks.keys {
      var mark = try XCTUnwrap(marks[path])
      var projection = try XCTUnwrap(
        JSONSerialization.jsonObject(with: XCTUnwrap(mark.projection)) as? [String: Any])
      var owners = try XCTUnwrap(projection["historicalOwners"] as? [String: [String: Any]])
      for id in owners.keys where owners[id]?["providerSessionID"] as? String == "agent-session" {
        owners[id]?["channel"] = "0"
      }
      projection["historicalOwners"] = owners
      mark.projection = try JSONSerialization.data(withJSONObject: projection)
      marks[path] = mark
    }
    try OverlayDeliveryCursorStore.save(root: fixture.root, marks: marks)

    let reader = OverlayChannelDeliveryReader(root: fixture.root, sharedBus: peer)
    let snapshot = try await reader.readSnapshot()
    let current = try XCTUnwrap(
      snapshot.conversations.first {
        $0.owner?.providerSessionID == "agent-session"
      })
    XCTAssertEqual(current.channel, "1")
    XCTAssertEqual(current.owner?.channel, "1")
    XCTAssertEqual(current.messages.map(\.text), ["Iwo"])
    XCTAssertEqual(
      snapshot.conversations.first { $0.id == "0" }?.messages.map(\.text), ["Current reply"],
      "Repairing the author's roster slot must not change a peer reply's destination")
    let old = try XCTUnwrap(
      snapshot.conversations.first {
        $0.owner?.providerSessionID == "previous-session"
      })
    XCTAssertEqual(old.messages.map(\.text), ["Previous reply"])
    XCTAssertEqual(old.owner?.leaseID, String(repeating: "a", count: 32))
    let consumed = await reader.consumedBytes
    XCTAssertEqual(
      consumed, 0, "Repair metadata at the existing cursor; do not replay or delete history")
    let hud = OverlayChannelHudProjection(
      open: false, loud: false, autosealDeadline: nil, followerAlive: true,
      provider: "codex", providerSessionID: "agent-session")
    let view = OverlayChannelStatusView(
      channels: snapshot.deliveries, unavailable: false, palette: .dark, animates: false,
      hudStates: ["1": hud], conversations: snapshot.conversations)
    XCTAssertEqual(view.conversation(for: try XCTUnwrap(snapshot.deliveries.first))?.id, current.id)
    XCTAssertTrue(view.savedConversations.contains { $0.id == old.id })

    _ = try await reader.readSnapshot(checkpointTime: .now.advanced(by: .seconds(31)))
    let saved = OverlayDeliveryCursorStore.load(root: fixture.root)
    for mark in saved.values {
      let projection = try XCTUnwrap(
        JSONSerialization.jsonObject(with: XCTUnwrap(mark.projection)) as? [String: Any])
      let owners = try XCTUnwrap(projection["historicalOwners"] as? [String: [String: Any]])
      for owner in owners.values where owner["providerSessionID"] as? String == "agent-session" {
        XCTAssertEqual(
          owner["channel"] as? String, "1", "Persist the repaired metadata on every mirror")
      }
    }
  }

  func testActiveSnapshotsUpdateTheViewWithoutRewritingTheWholeCache() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let start = ContinuousClock.now
    let first = try await reader.readSnapshot(checkpointTime: start)
    var previousID = first.deliveries.first?.deliveryID
    let cache = OverlayDeliveryCursorStore.url(root: fixture.root)
    let initial = try Data(contentsOf: cache)
    for sequence in 2...21 {
      try fixture.append(fixture.seal(sequence))
      let snapshot = try await reader.readSnapshot(checkpointTime: start)
      XCTAssertNotEqual(snapshot.deliveries.first?.deliveryID, previousID)
      previousID = snapshot.deliveries.first?.deliveryID
    }
    XCTAssertEqual(
      try Data(contentsOf: cache), initial,
      "Active polls paint new evidence without rewriting every cached projection")
    let current = try await reader.readSnapshot(checkpointTime: start)
    let resumed = try await OverlayChannelDeliveryReader(root: fixture.root).readSnapshot()
    XCTAssertEqual(resumed, current, "Restart replays the suffix since the last checkpoint")
  }

  func testCheckpointBatchesAllChangedBusesIntoOneAtomicWrite() async throws {
    let fixture = try Fixture()
    let other = try Fixture()
    defer {
      fixture.remove()
      other.remove()
    }
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    try other.append(other.seal(2))
    let reader = OverlayChannelDeliveryReader(root: fixture.root, sharedBus: other.bus)
    let snapshot = try await reader.readSnapshot()
    let writes = await reader.checkpointWrites
    XCTAssertEqual(writes, 1, "All buses and derived receipts share one checkpoint")
    XCTAssertEqual(
      try fixture.cursorOffsets(),
      [
        fixture.bus.path: try fixture.busSize(), other.bus.path: try other.busSize(),
      ])
    let restarted = OverlayChannelDeliveryReader(root: fixture.root, sharedBus: other.bus)
    let restored = try await restarted.readSnapshot()
    XCTAssertEqual(restored, snapshot)
    let replayed = await restarted.consumedBytes
    XCTAssertEqual(replayed, 0)
  }

  func testContinuousChangesCannotPostponeTheCheckpointDeadline() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let start = ContinuousClock.now
    _ = try await reader.readSnapshot(checkpointTime: start)
    for second in 1...29 {
      try fixture.append(fixture.seal(second + 1))
      _ = try await reader.readSnapshot(checkpointTime: start.advanced(by: .seconds(second)))
    }
    let before = await reader.checkpointWrites
    XCTAssertEqual(before, 1)
    try fixture.append(fixture.seal(31))
    let latest = try await reader.readSnapshot(checkpointTime: start.advanced(by: .seconds(30)))
    let after = await reader.checkpointWrites
    XCTAssertEqual(after, 2, "The deadline measures from successful save, not newest change")
    let restored = try await OverlayChannelDeliveryReader(root: fixture.root).readSnapshot()
    XCTAssertEqual(restored, latest)
  }

  func testReceiptOnlyChangeCheckpointsOnAnIdlePollAndThenStopsWriting() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let start = ContinuousClock.now
    let initial = try await reader.readSnapshot(checkpointTime: start)
    XCTAssertEqual(initial.deliveries.first?.stage, .sent)
    let delivery = try XCTUnwrap(initial.deliveries.first?.deliveryID)
    let acknowledgments = fixture.root.appendingPathComponent("acknowledgments/\(Fixture.leaseID)")
    try FileManager.default.createDirectory(at: acknowledgments, withIntermediateDirectories: true)
    var envelope = fixture.recipient()
    envelope.merge([
      "kind": "seal", "delivery_id": delivery, "session_id": "take-a",
      "utterance_id": "u1", "sequence": 1,
    ]) { _, new in new }
    try fixture.write(
      [
        "lease_id": Fixture.leaseID, "delivery_id": delivery, "bus": fixture.bus.path,
        "envelope": envelope,
      ],
      to: acknowledgments.appendingPathComponent("\(delivery).json"))
    let acknowledged = try await reader.readSnapshot(
      checkpointTime: start.advanced(by: .seconds(1)))
    XCTAssertEqual(acknowledged.deliveries.first?.stage, .received)
    let earlyWrites = await reader.checkpointWrites
    XCTAssertEqual(earlyWrites, 1)
    _ = try await reader.readSnapshot(checkpointTime: start.advanced(by: .seconds(30)))
    let checkpointWrites = await reader.checkpointWrites
    XCTAssertEqual(checkpointWrites, 2, "No new bus bytes are needed to flush derived receipts")
    for second in [60, 90, 120] {
      let snapshot = try await reader.readSnapshot(
        checkpointTime: start.advanced(by: .seconds(second)))
      XCTAssertEqual(snapshot, acknowledged)
    }
    let idleWrites = await reader.checkpointWrites
    XCTAssertEqual(idleWrites, 2)
    let restored = try await OverlayChannelDeliveryReader(root: fixture.root).readSnapshot()
    XCTAssertEqual(restored, acknowledged)
  }

  func testLargeByteLagCheckpointsBeforeTheTimeDeadline() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let start = ContinuousClock.now
    _ = try await reader.readSnapshot(checkpointTime: start)
    try fixture.fillBusWithEvidence(bytes: (8 << 20) + 10000)
    try fixture.append(fixture.seal(2))
    let latest = try await reader.readSnapshot(checkpointTime: start.advanced(by: .seconds(1)))
    let writes = await reader.checkpointWrites
    XCTAssertEqual(writes, 2, "Heavy input checkpoints without waiting thirty seconds")
    XCTAssertEqual(try fixture.cursorOffsets()[fixture.bus.path], try fixture.busSize())
    let restarted = OverlayChannelDeliveryReader(root: fixture.root)
    let restored = try await restarted.readSnapshot()
    XCTAssertEqual(restored, latest)
    let replayed = await restarted.consumedBytes
    XCTAssertEqual(replayed, 0)
  }

  func testCheckpointFailureRetainsThePreviousFileAndRetriesDirtyState() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append(fixture.seal(1))
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let start = ContinuousClock.now
    _ = try await reader.readSnapshot(checkpointTime: start)
    let cache = OverlayDeliveryCursorStore.url(root: fixture.root)
    let saved = try Data(contentsOf: cache)
    let directory = cache.deletingLastPathComponent()
    try FileManager.default.setAttributes([.posixPermissions: 0o500], ofItemAtPath: directory.path)
    defer {
      try? FileManager.default.setAttributes(
        [.posixPermissions: 0o700], ofItemAtPath: directory.path)
    }
    try fixture.append(fixture.seal(2))
    do {
      _ = try await reader.readSnapshot(checkpointTime: start.advanced(by: .seconds(30)))
      XCTFail("Read-only cache directory must refuse checkpoint creation")
    } catch {}
    XCTAssertEqual(try Data(contentsOf: cache), saved)
    let failedWrites = await reader.checkpointWrites
    XCTAssertEqual(failedWrites, 1)
    try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path)
    let retried = try await reader.readSnapshot(checkpointTime: start.advanced(by: .seconds(31)))
    let writes = await reader.checkpointWrites
    XCTAssertEqual(writes, 2)
    let restored = try await OverlayChannelDeliveryReader(root: fixture.root).readSnapshot()
    XCTAssertEqual(restored, retried)
  }

  func testPartialRowNeverAdvancesTheCheckpointPastItsSafeBoundary() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append(fixture.seal(1))
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let start = ContinuousClock.now
    _ = try await reader.readSnapshot(checkpointTime: start)
    let safeOffset = try fixture.busSize()
    try fixture.appendBytes(try JSONSerialization.data(withJSONObject: fixture.seal(2)))
    _ = try await reader.readSnapshot(checkpointTime: start.advanced(by: .seconds(30)))
    XCTAssertEqual(try fixture.cursorOffsets()[fixture.bus.path], safeOffset)
    try fixture.appendBytes(Data([10]))
    let complete = try await reader.readSnapshot(checkpointTime: start.advanced(by: .seconds(31)))
    XCTAssertEqual(try fixture.cursorOffsets()[fixture.bus.path], try fixture.busSize())
    let restored = try await OverlayChannelDeliveryReader(root: fixture.root).readSnapshot()
    XCTAssertEqual(restored, complete)
  }

  func testIncompleteStorageTransactionRestartsBeforeItsFirstChunk() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append(fixture.seal(1))
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let start = ContinuousClock.now
    let initial = try await reader.readSnapshot(checkpointTime: start)
    let safeOffset = try fixture.busSize()
    var document = fixture.seal(2)
    document["diagnostic_padding"] = String(repeating: "x", count: 40_000)
    let bytes = try JSONSerialization.data(withJSONObject: document, options: [.sortedKeys])
    let digest = SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
    func chunk(_ part: Int) -> [String: Any] {
      let begin = part * 32768
      return [
        "schema": "codescribe.bus-chunk.v1", "part": part, "parts": 2,
        "length": bytes.count, "id": digest, "event": ["session_id": "take-a"],
        "payload": bytes.subdata(in: begin..<min(begin + 32768, bytes.count)).base64EncodedString(),
      ]
    }
    try fixture.append(chunk(0))
    let incomplete = try await reader.readSnapshot(checkpointTime: start.advanced(by: .seconds(30)))
    XCTAssertEqual(incomplete, initial, "Incomplete storage cannot publish its document")
    XCTAssertEqual(try fixture.cursorOffsets()[fixture.bus.path], safeOffset)
    let restarted = OverlayChannelDeliveryReader(root: fixture.root)
    let restoredIncomplete = try await restarted.readSnapshot()
    XCTAssertEqual(restoredIncomplete, initial)
    XCTAssertEqual(try fixture.cursorOffsets()[fixture.bus.path], safeOffset)
    try fixture.append(chunk(1))
    let completed = try await reader.readSnapshot(checkpointTime: start.advanced(by: .seconds(31)))
    let restoredCompleted = try await restarted.readSnapshot()
    XCTAssertEqual(restoredCompleted, completed)
    XCTAssertNotEqual(completed.deliveries.first?.deliveryID, initial.deliveries.first?.deliveryID)
    XCTAssertEqual(try fixture.cursorOffsets()[fixture.bus.path], try fixture.busSize())
  }

  func testIdleSnapshotsDoNotRereadMegabyteLeaseMetadata() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let leaseURL = fixture.root.appendingPathComponent("leases/\(Fixture.leaseID).json")
    var lease = try XCTUnwrap(
      JSONSerialization.jsonObject(with: Data(contentsOf: leaseURL)) as? [String: Any])
    lease["diagnostic_padding"] = String(repeating: "x", count: 1 << 20)
    try fixture.write(lease, to: leaseURL)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let initial = try await reader.readSnapshot()
    let coldBytes = await reader.consumedMetadataBytes
    let coldOpens = await reader.metadataFileOpens
    XCTAssertGreaterThan(coldBytes, 1 << 20)
    for _ in 0..<20 {
      let snapshot = try await reader.readSnapshot()
      XCTAssertEqual(snapshot, initial)
    }
    let steadyBytes = await reader.consumedMetadataBytes
    let steadyOpens = await reader.metadataFileOpens
    XCTAssertEqual(steadyOpens, coldOpens, "Unchanged and absent metadata needs no new descriptor")
    XCTAssertEqual(steadyBytes, coldBytes, "Idle polls must not repeatedly parse the same lease")
    try fixture.lease(pending: [fixture.envelope(Fixture.firstID)])
    let changed = try await reader.readSnapshot()
    XCTAssertEqual(changed.deliveries.first?.stage, .queued)
    XCTAssertEqual(
      changed.conversations.first?.messages.map(\.text),
      initial.conversations.first?.messages.map(\.text))
  }

  /// Screen recording 2026-10-13 09:46: a test host reading the live agent
  /// bridge drew a second overlay with the real conversations during a take.
  func testTestHostNeverResolvesTheLiveRuntime() throws {
    let live = FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent(".codescribe", isDirectory: true).standardizedFileURL.path
    let environment = ProcessInfo.processInfo.environment
    let bridge = try XCTUnwrap(environment[QualityCaptureHost.agentBridgeHomeVariable])
    let data = try XCTUnwrap(environment[QualityCaptureHost.dataDirectoryVariable])
    for path in [
      bridge, data, OverlayChannelDeliveryReader.productionRoot().path,
      agentConversationBusPath(),
    ] {
      let resolved = URL(fileURLWithPath: path).standardizedFileURL.path
      XCTAssertFalse(
        resolved == live || resolved.hasPrefix(live + "/"), "\(resolved) is the live runtime")
    }
  }

  func testIsolationFallsBackToAPerProcessRootWithoutTheRunner() throws {
    let dataVariable = QualityCaptureHost.dataDirectoryVariable
    let bridgeVariable = QualityCaptureHost.agentBridgeHomeVariable
    let temp = FileManager.default.temporaryDirectory
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    defer { try? FileManager.default.removeItem(at: temp) }
    var assigned: [String: String] = [:]
    XCTAssertNil(
      QualityCaptureHost.isolateFromLiveRuntime(
        runningTests: false, environment: [:], temporaryDirectory: temp, processID: 42,
        setVariable: { assigned[$0] = $1 }))
    XCTAssertTrue(assigned.isEmpty, "The product never rewrites its environment")

    let homes = try XCTUnwrap(
      QualityCaptureHost.isolateFromLiveRuntime(
        runningTests: true, environment: [:], temporaryDirectory: temp, processID: 42,
        setVariable: { assigned[$0] = $1 }))
    XCTAssertEqual(homes.data.path, temp.appendingPathComponent("codescribe-test-host-42").path)
    XCTAssertEqual(homes.agentBridge.path, homes.data.appendingPathComponent("agent-bridge").path)
    XCTAssertEqual(
      assigned, [dataVariable: homes.data.path, bridgeVariable: homes.agentBridge.path])
    XCTAssertTrue(FileManager.default.fileExists(atPath: homes.agentBridge.path))

    assigned = [:]
    let runnerData = temp.appendingPathComponent("runner-data").path
    let runner = try XCTUnwrap(
      QualityCaptureHost.isolateFromLiveRuntime(
        runningTests: true, environment: [dataVariable: runnerData],
        temporaryDirectory: temp, processID: 42, setVariable: { assigned[$0] = $1 }))
    XCTAssertEqual(runner.data.path, runnerData)
    XCTAssertEqual(runner.agentBridge.path, runnerData + "/agent-bridge")
    XCTAssertEqual(
      assigned, [bridgeVariable: runnerData + "/agent-bridge"], "A runner's data dir is kept")
  }

  /// A sample of build 1004 (2026-10-14) had two reader threads at ~55 % of a
  /// core rebuilding and re-sorting unchanged conversations every 500 ms.
  func testIdlePollsReuseTheConversationListUntilAProjectionChanges() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let initial = try await reader.readSnapshot()
    for _ in 0..<20 {
      let snapshot = try await reader.readSnapshot()
      XCTAssertEqual(snapshot, initial)
    }
    let idleBuilds = await reader.conversationBuilds
    XCTAssertEqual(idleBuilds, 1, "Unchanged buses build their conversations once")
    let idle = try await reader.readSnapshot()
    XCTAssertEqual(
      idle.conversations.withUnsafeBufferPointer { $0.baseAddress },
      initial.conversations.withUnsafeBufferPointer { $0.baseAddress },
      "The reused list keeps its storage, so the main actor's comparison ends at identity")

    try fixture.append(fixture.seal(2))
    let grown = try await reader.readSnapshot()
    let grownBuilds = await reader.conversationBuilds
    XCTAssertEqual(grownBuilds, 2, "A new bus row rebuilds the list")
    XCTAssertNotEqual(grown.conversations, initial.conversations)

    // A receipt alone, with no new bus bytes, still changes the projection.
    let delivery = try XCTUnwrap(grown.deliveries.first?.deliveryID)
    let acknowledgments = fixture.root.appendingPathComponent("acknowledgments/\(Fixture.leaseID)")
    try FileManager.default.createDirectory(at: acknowledgments, withIntermediateDirectories: true)
    var envelope = fixture.recipient()
    envelope.merge([
      "kind": "seal", "delivery_id": delivery, "session_id": "take-a",
      "utterance_id": "u2", "sequence": 2,
    ]) { _, new in new }
    try fixture.write(
      [
        "lease_id": Fixture.leaseID, "delivery_id": delivery, "bus": fixture.bus.path,
        "envelope": envelope,
      ],
      to: acknowledgments.appendingPathComponent("\(delivery).json"))
    let acknowledged = try await reader.readSnapshot()
    let acknowledgedBuilds = await reader.conversationBuilds
    XCTAssertEqual(acknowledged.deliveries.first?.stage, .received)
    XCTAssertEqual(acknowledgedBuilds, 3, "An acknowledgment receipt rebuilds the list")
    XCTAssertNotEqual(acknowledged.conversations, grown.conversations)
    _ = try await reader.readSnapshot()
    let settledBuilds = await reader.conversationBuilds
    XCTAssertEqual(settledBuilds, 3)
    let restored = try await OverlayChannelDeliveryReader(root: fixture.root).readSnapshot()
    XCTAssertEqual(restored, acknowledged)
  }

  func testReaderRetainsTheExactBroadcastSkipAcrossCheckpointRestore() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    var row = fixture.seal(1)
    row["audience"] = "*"
    row["channel"] = "0"
    try fixture.append(row)
    let reader = OverlayChannelDeliveryReader(root: fixture.root, sharedBus: nil)
    let initial = try await reader.readSnapshot()
    let recipient = try XCTUnwrap(
      initial.conversations.first { $0.id == "0" }?.messages.first?.recipients.first)
    let delivery = try XCTUnwrap(recipient.deliveryID)
    let marker = fixture.root.appendingPathComponent(
      "broadcast-skipped/\(Fixture.leaseID)/\(delivery).json")
    try FileManager.default.createDirectory(
      at: marker.deletingLastPathComponent(), withIntermediateDirectories: true)
    var skip = fixture.recipient()
    skip.merge([
      "schema": "codescribe.broadcast-policy.v1", "kind": "broadcast_skipped",
      "delivery_id": delivery, "origin_channel": "0", "policy": "off",
    ]) { _, new in new }
    try fixture.write(skip, to: marker)
    for snapshot in [
      try await reader.readSnapshot(checkpointTime: .now.advanced(by: .seconds(31))),
      try await OverlayChannelDeliveryReader(root: fixture.root, sharedBus: nil).readSnapshot(),
    ] {
      let observed = try XCTUnwrap(
        snapshot.conversations.first { $0.id == "0" }?.messages.first?.recipients.first)
      XCTAssertEqual(observed.broadcastSkipped, true)
      XCTAssertFalse(observed.acknowledged, "Skipped is not an agent read receipt")
      XCTAssertEqual(
        OverlayConversationView.receiptStatusText(for: observed), "Skipped: off channel 0")
    }
  }

  func testForeignSkipFileCannotHideAValidAcknowledgment() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    var row = fixture.seal(1)
    row["audience"] = "*"
    row["channel"] = "0"
    try fixture.append(row)
    let reader = OverlayChannelDeliveryReader(root: fixture.root, sharedBus: nil)
    let initial = try await reader.readSnapshot()
    let message = try XCTUnwrap(initial.conversations.first { $0.id == "0" }?.messages.first)
    let recipient = try XCTUnwrap(message.recipients.first)
    let delivery = try XCTUnwrap(recipient.deliveryID)
    let skip = fixture.root.appendingPathComponent(
      "broadcast-skipped/\(Fixture.leaseID)/\(delivery).json")
    try FileManager.default.createDirectory(
      at: skip.deletingLastPathComponent(), withIntermediateDirectories: true)
    try fixture.write(
      [
        "schema": "codescribe.broadcast-policy.v1", "kind": "broadcast_skipped",
        "delivery_id": delivery, "lease_id": String(repeating: "f", count: 32),
        "provider": "codex", "provider_session_id": "foreign-session",
        "origin_channel": "0", "policy": "off", "bus": fixture.bus.path,
      ], to: skip)
    let acknowledgment = fixture.root.appendingPathComponent(
      "acknowledgments/\(Fixture.leaseID)/\(delivery).json")
    try FileManager.default.createDirectory(
      at: acknowledgment.deletingLastPathComponent(), withIntermediateDirectories: true)
    var envelope = fixture.recipient()
    envelope.merge([
      "kind": "seal", "delivery_id": delivery, "session_id": "take-a",
      "utterance_id": "u1", "sequence": 1, "audience": "*",
    ]) { _, new in new }
    try fixture.write(
      [
        "lease_id": Fixture.leaseID, "delivery_id": delivery,
        "bus": fixture.bus.path, "envelope": envelope,
      ], to: acknowledgment)
    let withForeignSkip = try await reader.readSnapshot()
    let observed = try XCTUnwrap(
      withForeignSkip.conversations.first { $0.id == "0" }?.messages.first?.recipients.first)
    XCTAssertNotEqual(observed.broadcastSkipped, true)
    XCTAssertTrue(observed.acknowledged, "An ignored foreign skip cannot suppress a valid ACK")
    // Positive control: the same reader and same valid ACK with only the
    // foreign skip removed must have the identical read disposition.
    try FileManager.default.removeItem(at: skip)
    let withoutForeignSkip = try await reader.readSnapshot()
    XCTAssertEqual(
      withoutForeignSkip.conversations.first { $0.id == "0" }?
        .messages.first?.recipients.first?.acknowledged, true)
  }

  func testAcknowledgedDeliveryStopsPollingWithoutAProviderAcceptanceReceipt() throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    var projection = OverlayChannelDelivery.Bus()
    projection.consume(fixture.seal(1))
    let coordinate = try XCTUnwrap(projection.receiptCoordinates().first)
    let owner = coordinate.0
    let delivery = coordinate.1
    XCTAssertEqual(coordinate.2, false, "No acceptance receipt observed yet")
    var envelope = fixture.recipient()
    envelope.merge([
      "kind": "seal", "delivery_id": delivery, "session_id": "take-a",
      "utterance_id": "u1", "sequence": 1,
    ]) { _, new in new }
    var receipt: [String: Any] = [
      "lease_id": owner.leaseID, "delivery_id": delivery,
      "bus": fixture.bus.path, "envelope": envelope,
    ]
    receipt["bus"] = fixture.root.appendingPathComponent("foreign.jsonl").path
    projection.observeAcknowledgment(
      receipt, owner: owner, delivery: delivery, busPath: fixture.bus.path)
    XCTAssertEqual(
      projection.receiptCoordinates().count, 1, "Foreign receipt cannot settle polling")
    projection.observeAcceptance([
      "schema": "codescribe.native-queue.receipt.v1", "disposition": "provider_accepted",
      "delivery_id": delivery, "lease_id": owner.leaseID,
      "provider": owner.provider, "provider_session_id": owner.providerSessionID,
      "channel": owner.channel, "name": owner.name,
    ])
    XCTAssertEqual(projection.receiptCoordinates().count, 1, "Acceptance alone is not reading")
    XCTAssertEqual(
      projection.receiptCoordinates().first?.2, true,
      "An accepted recipient tells the reader to poll only the acknowledgment")
    // Recreate monitor-only delivery: there is no provider-acceptance file.
    projection = OverlayChannelDelivery.Bus()
    projection.consume(fixture.seal(1))
    receipt["bus"] = fixture.bus.path
    projection.observeAcknowledgment(
      receipt, owner: owner, delivery: delivery, busPath: fixture.bus.path)
    let question = try XCTUnwrap(
      projection.conversations(busPath: fixture.bus.path).flatMap(\.messages).first {
        $0.recipients.contains { $0.owner.id == owner.id && $0.deliveryID == delivery }
      })
    XCTAssertEqual(question.recipients.first?.acknowledged, true)
    XCTAssertEqual(question.recipients.first?.accepted, false)
    XCTAssertTrue(projection.receiptCoordinates().isEmpty)
    let restored = try JSONDecoder().decode(
      OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(projection))
    var resumed = restored
    XCTAssertTrue(resumed.receiptCoordinates().isEmpty, "Restart must retain settled receipt")
  }

  func testMetadataReplacementWithSameSizeAndTimestampChangesTheDisplayedOwner() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let binding = fixture.root.appendingPathComponent("vc.agent-audience-binding.v1.json")
    let initialBytes = try Data(contentsOf: binding)
    let originalDate = try XCTUnwrap(
      FileManager.default.attributesOfItem(atPath: binding.path)[.modificationDate] as? Date)
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    try fixture.append(fixture.seal(1))
    let initial = try await reader.read()
    XCTAssertEqual(initial.first?.agent, "james")
    let changedBytes = Data(
      try XCTUnwrap(String(data: initialBytes, encoding: .utf8))
        .replacingOccurrences(of: "james", with: "jamie").utf8)
    XCTAssertEqual(changedBytes.count, initialBytes.count)
    try changedBytes.write(to: binding, options: .atomic)
    try FileManager.default.setAttributes(
      [.modificationDate: originalDate], ofItemAtPath: binding.path)
    let changed = try await reader.read()
    XCTAssertEqual(changed.first?.agent, "jamie")
  }

  func testInPlaceMetadataWriteWithRestoredTimestampDoesNotReuseOldOwner() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let binding = fixture.root.appendingPathComponent("vc.agent-audience-binding.v1.json")
    let bytes = try Data(contentsOf: binding)
    let attributes = try FileManager.default.attributesOfItem(atPath: binding.path)
    let originalDate = try XCTUnwrap(attributes[.modificationDate] as? Date)
    let inode = try XCTUnwrap(attributes[.systemFileNumber] as? NSNumber)
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    _ = try await reader.read()
    let changed = Data(
      try XCTUnwrap(String(data: bytes, encoding: .utf8))
        .replacingOccurrences(of: "james", with: "jamie").utf8)
    let handle = try FileHandle(forWritingTo: binding)
    try handle.write(contentsOf: changed)
    try handle.close()
    try FileManager.default.setAttributes(
      [.modificationDate: originalDate], ofItemAtPath: binding.path)
    XCTAssertEqual(
      try FileManager.default.attributesOfItem(atPath: binding.path)[.systemFileNumber]
        as? NSNumber,
      inode)
    let snapshot = try await reader.read()
    XCTAssertEqual(snapshot.first?.agent, "jamie")
  }

  func testBrokenAndDeletedMetadataCannotKeepACachedBindingAlive() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let binding = fixture.root.appendingPathComponent("vc.agent-audience-binding.v1.json")
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    _ = try await reader.read()
    try Data("{".utf8).write(to: binding, options: .atomic)
    do {
      _ = try await reader.read()
      XCTFail("Malformed metadata must not serve its previously valid parse")
    } catch {}
    try FileManager.default.removeItem(at: binding)
    let removed = try await reader.read()
    XCTAssertTrue(removed.isEmpty)
    try fixture.bind(session: "another-session")
    let rebound = try await reader.read()
    XCTAssertTrue(rebound.isEmpty, "A cached previous owner cannot satisfy a new binding")
  }

  func testMetadataLargerThanCacheBudgetRemainsReadableWithoutBeingRetained() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let leaseURL = fixture.root.appendingPathComponent("leases/\(Fixture.leaseID).json")
    var lease = try XCTUnwrap(
      JSONSerialization.jsonObject(with: Data(contentsOf: leaseURL)) as? [String: Any])
    lease["diagnostic_padding"] = String(repeating: "x", count: 9 << 20)
    try fixture.write(lease, to: leaseURL)
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let initial = try await reader.readSnapshot()
    let before = await reader.consumedMetadataBytes
    let repeated = try await reader.readSnapshot()
    let after = await reader.consumedMetadataBytes
    XCTAssertEqual(repeated, initial)
    XCTAssertGreaterThan(
      after - before, 9 << 20, "Large metadata is not retained in the bounded cache")
  }

  func testMultipleLargeLeasesCannotGrowTheMetadataCacheWithoutBound() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    for digit in 1...4 {
      let identity = String(repeating: String(digit), count: 32)
      try fixture.write(
        [
          "schema": "codescribe.agent-bridge.lease.v1", "lease_id": identity,
          "provider": "codex", "provider_session_id": "other-\(digit)",
          "bus": fixture.bus.path, "pending": [],
          "diagnostic_padding": String(repeating: "x", count: 3 << 20),
        ], to: fixture.root.appendingPathComponent("leases/\(identity).json"))
    }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let initial = try await reader.readSnapshot()
    let before = await reader.consumedMetadataBytes
    let repeated = try await reader.readSnapshot()
    let after = await reader.consumedMetadataBytes
    XCTAssertEqual(repeated, initial)
    XCTAssertGreaterThan(
      after - before, 3 << 20, "Twelve MiB of metadata must not fit in the eight MiB cache")
  }

  func testReplySpeechCapabilitySurvivesMirrorsAndRebuildsOlderCachedProjection() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let shared = fixture.root.appendingPathComponent("a-shared.jsonl")
    var speech = fixture.recipient()
    speech.merge([
      "schema": "codescribe.agent-reply.v1", "kind": "agent_reply",
      "reply_id": String(repeating: "a", count: 24), "text": "Taki sam tekst",
      "emitted_at": "2026-10-07T14:00:00Z", "spoken": false,
      "tts_vendor": "xai", "voice": "ara", "speed": 1.25,
    ]) { _, new in new }
    try fixture.append(speech)
    var mirror = speech
    mirror.removeValue(forKey: "tts_vendor")
    mirror.removeValue(forKey: "voice")
    var sharedBytes = try JSONSerialization.data(withJSONObject: mirror)
    sharedBytes.append(10)
    try sharedBytes.write(to: shared)
    var text = mirror
    text["reply_id"] = String(repeating: "b", count: 24)
    try fixture.append(text)
    let sourceBytes = try Data(contentsOf: fixture.bus)
    func messages(_ snapshot: OverlayChannelDeliverySnapshot) throws -> [OverlayConversationMessage]
    {
      try XCTUnwrap(snapshot.conversations.first { $0.owner?.leaseID == Fixture.leaseID }).messages
    }
    let initial = try await OverlayChannelDeliveryReader(root: fixture.root, sharedBus: shared)
      .readSnapshot()
    let replies = try messages(initial)
    XCTAssertEqual(replies.map(\.text), ["Taki sam tekst", "Taki sam tekst"])
    XCTAssertEqual(replies.map(\.supportsSpeechPlayback), [true, false])
    let restarted = OverlayChannelDeliveryReader(root: fixture.root, sharedBus: shared)
    let restored = try await restarted.readSnapshot()
    XCTAssertEqual(try messages(restored), replies)
    let consumed = await restarted.consumedBytes
    XCTAssertEqual(consumed, 0)

    let cacheURL = OverlayDeliveryCursorStore.url(root: fixture.root)
    var cache = try XCTUnwrap(
      JSONSerialization.jsonObject(with: Data(contentsOf: cacheURL))
        as? [String: Any])
    var buses = try XCTUnwrap(cache["buses"] as? [String: [String: Any]])
    for path in Array(buses.keys) {
      var entry = try XCTUnwrap(buses[path])
      let encoded = try XCTUnwrap(entry["projection"] as? String)
      var projection = try XCTUnwrap(
        JSONSerialization.jsonObject(
          with: XCTUnwrap(Data(base64Encoded: encoded))) as? [String: Any])
      var cachedMessages = try XCTUnwrap(projection["messages"] as? [String: [String: Any]])
      for id in Array(cachedMessages.keys) {
        cachedMessages[id]?.removeValue(forKey: "supportsSpeechPlayback")
      }
      projection["messages"] = cachedMessages
      entry["projection"] = try JSONSerialization.data(withJSONObject: projection)
        .base64EncodedString()
      buses[path] = entry
    }
    cache["buses"] = buses
    try fixture.write(cache, to: cacheURL)
    let upgraded = OverlayChannelDeliveryReader(root: fixture.root, sharedBus: shared)
    let rebuilt = try await upgraded.readSnapshot()
    XCTAssertEqual(try messages(rebuilt), replies)
    let rebuiltBytes = await upgraded.consumedBytes
    XCTAssertGreaterThan(rebuiltBytes, 0)
    XCTAssertLessThanOrEqual(rebuiltBytes, 2 * OverlayChannelDeliveryReader.tailWindow)
    XCTAssertEqual(try Data(contentsOf: fixture.bus), sourceBytes)
  }

  func testCaptureReceiptsWaitForTerminalAndSurviveLateRevisions() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let session = "agent-channel-0-lifecycle"
    func row(_ revision: Int, _ text: String) -> [String: Any] {
      [
        "schema": "codescribe.transcript-evidence.v1", "session_id": session,
        "audience": "*", "sequence": revision, "reducer_revision": revision,
        "reducer_action": "commit_delta", "rendered_text": text,
        "occurrence_session_id": session, "capture_epoch": 1,
        "sample_start": 3200, "sample_end": 9600, "document_index": 0,
        "recipients": [fixture.recipient()],
      ]
    }
    try fixture.append(row(1, "Pierwsze"))
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    var snapshot = try await reader.readSnapshot()
    var message = try XCTUnwrap(snapshot.conversations.first { $0.id == "0" }?.messages.first)
    XCTAssertFalse(message.showsDeliveryReceipts)
    XCTAssertFalse(message.recipients.isEmpty)
    var occurrenceTerminal = row(2, "Pierwsza zamknięta fraza")
    occurrenceTerminal["status"] = "transcript_sealed"
    try fixture.append(occurrenceTerminal)
    snapshot = try await reader.readSnapshot()
    message = try XCTUnwrap(snapshot.conversations.first { $0.id == "0" }?.messages.first)
    XCTAssertFalse(message.showsDeliveryReceipts, "A phrase terminal does not finish the take")
    try fixture.append([
      "schema": "codescribe.channel-session.v1", "channel": "0",
      "session_id": session, "state": "sealed",
    ])
    snapshot = try await reader.readSnapshot()
    message = try XCTUnwrap(snapshot.conversations.first { $0.id == "0" }?.messages.first)
    XCTAssertTrue(message.showsDeliveryReceipts)
    XCTAssertEqual(message.captureState, .finished)
    try fixture.append(row(3, "Pierwsze i dalsze"))
    snapshot = try await reader.readSnapshot()
    message = try XCTUnwrap(snapshot.conversations.first { $0.id == "0" }?.messages.first)
    XCTAssertEqual(message.text, "Pierwsze i dalsze")
    XCTAssertTrue(message.showsDeliveryReceipts)
    let restored = try await OverlayChannelDeliveryReader(root: fixture.root).readSnapshot()
    XCTAssertEqual(restored, snapshot)
  }

  func testBroadcastStreamingKeepsNewestReducerRevisionAcrossDelayedBusCopies() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let shared = fixture.root.appendingPathComponent("a-shared.jsonl")
    func row(_ revision: Int, _ text: String) -> [String: Any] {
      [
        "schema": "codescribe.transcript-evidence.v1",
        "session_id": "agent-channel-0-streaming-fixture", "audience": "*",
        "sequence": revision, "reducer_revision": revision,
        "reducer_action": "commit_delta", "rendered_text": text,
        "occurrence_session_id": "agent-channel-0-streaming-fixture",
        "capture_epoch": 1, "sample_start": 3200, "sample_end": 9600,
        "document_index": 0, "recipients": [fixture.recipient()],
        "emitted_at": "2026-10-07T10:00:0\(revision)Z",
      ]
    }
    func appendShared(_ value: [String: Any]) throws {
      var bytes = try JSONSerialization.data(withJSONObject: value)
      bytes.append(10)
      if !FileManager.default.fileExists(atPath: shared.path) {
        try bytes.write(to: shared)
      } else {
        let handle = try FileHandle(forWritingTo: shared)
        defer { try? handle.close() }
        try handle.seekToEnd()
        try handle.write(contentsOf: bytes)
      }
    }
    try fixture.append(row(1, "Pierwsze słowo"))
    try appendShared(row(1, "Pierwsze słowo"))
    try appendShared(row(4, "Pierwsze słowo i cała dalsza wypowiedź"))
    let reader = OverlayChannelDeliveryReader(root: fixture.root, sharedBus: shared)
    var snapshot = try await reader.readSnapshot()
    var message = try XCTUnwrap(snapshot.conversations.first { $0.id == "0" }?.messages.first)
    XCTAssertEqual(message.text, "Pierwsze słowo i cała dalsza wypowiedź")
    XCTAssertEqual(snapshot.conversations.first { $0.id == "0" }?.messages.count, 1)
    let identity = message.id

    // A delayed mirror must not rewind the visible document. A later source
    // correction may shorten it: neither string length nor opening time wins.
    try fixture.append(row(2, "Pierwsze słowo i"))
    try appendShared(row(5, "Poprawiona wypowiedź"))
    snapshot = try await reader.readSnapshot()
    message = try XCTUnwrap(snapshot.conversations.first { $0.id == "0" }?.messages.first)
    XCTAssertEqual(message.text, "Poprawiona wypowiedź")
    XCTAssertEqual(message.id, identity)
    XCTAssertEqual(snapshot.conversations.first { $0.id == "0" }?.messages.count, 1)

    let restarted = try await OverlayChannelDeliveryReader(root: fixture.root, sharedBus: shared)
      .readSnapshot()
    XCTAssertEqual(
      restarted.conversations.first { $0.id == "0" }?.messages.first?.text,
      "Poprawiona wypowiedź")
  }

  func testBroadcastMirrorMergeRetainsFivePCMEntriesAndSeparateCaptures() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let shared = fixture.root.appendingPathComponent("a-shared.jsonl")
    var sharedBytes = Data()
    for capture in ["one", "two"] {
      for index in 0..<5 {
        let row: [String: Any] = [
          "schema": "codescribe.transcript-evidence.v1",
          "session_id": "agent-channel-0-\(capture)", "audience": "*",
          "sequence": index + 1, "reducer_revision": index == 4 ? 3 : 4,
          "reducer_action": "commit_delta", "rendered_text": "Iwo Iwo Iwo Iwo Iwo",
          "occurrence_session_id": "agent-channel-0-\(capture)", "capture_epoch": 1,
          "sample_start": 3200 + index * 1280, "sample_end": 4480 + index * 1280,
          "document_index": 0, "recipients": [fixture.recipient()],
          "emitted_at": "2026-10-07T10:00:00Z",
        ]
        if index == 4 {
          try fixture.append(row)
        } else {
          sharedBytes.append(try JSONSerialization.data(withJSONObject: row))
          sharedBytes.append(10)
        }
      }
    }
    try sharedBytes.write(to: shared)
    let snapshot = try await OverlayChannelDeliveryReader(root: fixture.root, sharedBus: shared)
      .readSnapshot()
    let messages = try XCTUnwrap(snapshot.conversations.first { $0.id == "0" }).messages
    XCTAssertEqual(messages.count, 2, "equal words in separate captures keep separate identity")
    XCTAssertEqual(Set(messages.map(\.id)).count, 2)
    for message in messages {
      XCTAssertEqual(message.text, "Iwo Iwo Iwo Iwo Iwo")
      XCTAssertEqual(Set(message.occurrenceIDs ?? []).count, 5)
    }
  }

  func testRestartRestoresVisibleReceiptWithoutNeedingAnotherBusEvent() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    try fixture.append(fixture.ack(Fixture.firstID))
    let first = OverlayChannelDeliveryReader(root: fixture.root)
    let beforeRestart = try await first.read()
    XCTAssertEqual(beforeRestart.first?.stage, .received)
    XCTAssertEqual(beforeRestart.first?.isOpen, true)

    // Restart without appending another seal, heartbeat or receipt. A saved
    // cursor cannot replace the presentation state it has already consumed.
    let restarted = OverlayChannelDeliveryReader(root: fixture.root)
    let afterRestart = try await restarted.read()
    XCTAssertEqual(afterRestart, beforeRestart)
    let newBytes = await restarted.consumedBytes
    XCTAssertEqual(newBytes, 0, "restored state does not need a history rescan")
  }

  func testFixtureTriadRequiresNewestSealPendingEnvelopeAndMatchingBusAck() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    var statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .sent)
    XCTAssertEqual(statuses.first?.deliveryID, Fixture.firstID)
    XCTAssertEqual(statuses.first?.isOpen, true)

    try fixture.lease(pending: [fixture.envelope(Fixture.firstID)])
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .queued)

    // The durable marker can precede the watcher row. It is not that row.
    try fixture.marker(Fixture.firstID)
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .queued)
    try fixture.append(fixture.ack(Fixture.firstID, channel: "2"))
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .queued, "another channel cannot acknowledge this one")
    try fixture.append(fixture.ack(Fixture.firstID))
    try fixture.lease(pending: [])
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .received)
    XCTAssertEqual(statuses.first?.isOpen, true, "receipt does not close the microphone")

    try fixture.append(fixture.seal(2))
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.deliveryID, "d8281fc3cceedc07762ee89e")
    XCTAssertEqual(statuses.first?.stage, .sent, "new words must not inherit the old receipt")
    try fixture.append(fixture.ack(Fixture.firstID))
    try fixture.append(fixture.seal(1))
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .sent)
  }

  func testFiveIdenticalLabelsRemainFiveDistinctOutboundDeliveries() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let expected = [
      Fixture.firstID, "d8281fc3cceedc07762ee89e", "73aaf65a701a5d96129ef750",
      "8796735e3c8ab23e9dbf319c", "769a770d54e99f754fd91423",
    ]
    for sequence in 1...5 {
      try fixture.append(fixture.seal(sequence))
      let statuses = try await reader.read()
      XCTAssertEqual(statuses.first?.deliveryID, expected[sequence - 1])
      XCTAssertEqual(statuses.first?.stage, .sent)
    }
    XCTAssertEqual(Set(expected).count, 5)
  }

  func testTerminalEvidenceUsesPythonPhaseIdentityAcrossDocumentRowsAndRestart() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let id = "82da0959f6f76c5ca5eff8d5"
    for index in 0...4 {
      try fixture.append([
        "schema": "codescribe.transcript-evidence.v1", "session_id": "take-a",
        "audience": "james", "sequence": index + 10, "document_index": index,
        "reducer_revision": 7, "reducer_action": "record_ledger_terminal_seal",
        "rendered_text": "Iwo Iwo Iwo Iwo Iwo", "recipients": [fixture.recipient()],
      ])
    }
    try fixture.lease(pending: [fixture.envelope(id)])
    var statuses = try await OverlayChannelDeliveryReader(root: fixture.root).read()
    XCTAssertEqual(statuses.first?.deliveryID, id)
    XCTAssertEqual(statuses.first?.stage, .queued)
    try fixture.append(fixture.ack(id))
    try fixture.lease(pending: [])
    // A restarted reader resumes from the durable cursor instead of replaying
    // history. The terminal phase repeats at the tail and yields the same
    // Python-computed identity, so the ack written before the restart lands.
    try fixture.append([
      "schema": "codescribe.transcript-evidence.v1", "session_id": "take-a",
      "audience": "james", "sequence": 15, "document_index": 5,
      "reducer_revision": 7, "reducer_action": "record_ledger_terminal_seal",
      "rendered_text": "Iwo Iwo Iwo Iwo Iwo", "recipients": [fixture.recipient()],
    ])
    statuses = try await OverlayChannelDeliveryReader(root: fixture.root).read()
    XCTAssertEqual(statuses.first?.deliveryID, id)
    XCTAssertEqual(statuses.first?.stage, .received)
  }

  func testOpenStateSurvivesSealAndOnlyMatchingLifecycleClosesIt() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    try fixture.append([
      "schema": "codescribe.transcript.v1", "session_id": "another-take", "status": "session_ended",
    ])
    var statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.isOpen, true)
    var oldClose = fixture.open()
    oldClose["state"] = "sealed"
    oldClose["opened_at"] = "earlier"
    try fixture.append(oldClose)
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.isOpen, true)
    try fixture.append([
      "schema": "codescribe.transcript.v1", "session_id": "take-a", "status": "session_ended",
    ])
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.isOpen, false)
    XCTAssertEqual(statuses.first?.stage, .sent)
  }

  func testLeaseRebindingAndBusRotationCannotRetainReceivedOrOpenState() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    try fixture.append(fixture.ack(Fixture.firstID))
    var statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .received)
    try fixture.bind(session: "new-agent-session")
    statuses = try await reader.read()
    XCTAssertTrue(statuses.isEmpty, "the previous owner's lease is not attached")
    try fixture.bind()
    try Data().write(to: fixture.bus, options: .atomic)
    statuses = try await reader.read()
    XCTAssertNil(statuses.first?.stage)
    XCTAssertEqual(statuses.first?.isOpen, false)
  }

  func testPartialBusLineWaitsForNewlineAndCorruptRowIsSkipped() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let data = try JSONSerialization.data(withJSONObject: fixture.seal(1))
    try data.write(to: fixture.bus)
    var statuses = try await reader.read()
    XCTAssertNil(statuses.first?.stage)
    try fixture.appendBytes(Data([10]))
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .sent)
    try fixture.appendBytes(Data("not-json\n".utf8))
    statuses = try await reader.read()
    XCTAssertEqual(
      statuses.first?.stage, .sent,
      "one corrupt row is skipped with a log; it cannot fail or rewind the read")
  }

  func testCorruptRowIsSkippedAndNeverRewindsTheCursor() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    try fixture.append(fixture.open())
    try fixture.appendBytes(Data("{\"schema\": broken\n".utf8))
    try fixture.append(fixture.seal(1))
    let statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.isOpen, true)
    XCTAssertEqual(
      statuses.first?.stage, .sent, "a corrupt row cannot hide the rows behind it")
    let consumed = await reader.consumedBytes
    XCTAssertEqual(
      consumed, try fixture.busSize(),
      "the cursor advanced past the corrupt row in one pass")
    try fixture.append(fixture.ack(Fixture.firstID))
    let after = try await reader.read()
    XCTAssertEqual(after.first?.stage, .received, "later reads continue past the corruption")
  }

  func testRestartResumesFromTheDurableCursorAndReadsOnlyNewBytes() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    let first = OverlayChannelDeliveryReader(root: fixture.root)
    _ = try await first.read()
    var size = try fixture.busSize()
    var consumed = await first.consumedBytes
    XCTAssertEqual(consumed, size, "a bus smaller than the window is read from byte zero")

    let cursorURL = OverlayDeliveryCursorStore.url(root: fixture.root)
    XCTAssertTrue(FileManager.default.fileExists(atPath: cursorURL.path))
    let attributes = try FileManager.default.attributesOfItem(atPath: cursorURL.path)
    let permissions = try XCTUnwrap(attributes[.posixPermissions] as? NSNumber)
    XCTAssertEqual(permissions.uint16Value & 0o777, 0o600, "the cursor is owner-only")

    try fixture.append(fixture.seal(2))
    try fixture.append(fixture.ack("d8281fc3cceedc07762ee89e"))
    let appended = try fixture.busSize() - size
    size = try fixture.busSize()

    let second = OverlayChannelDeliveryReader(root: fixture.root)
    let statuses = try await second.read()
    consumed = await second.consumedBytes
    XCTAssertEqual(
      consumed, appended, "the restart consumes only the bytes written since the cursor")
    XCTAssertEqual(statuses.first?.stage, .received)
    let offsets = try fixture.cursorOffsets()
    XCTAssertEqual(Array(offsets.values), [size], "the durable cursor ends at the bus EOF")
  }

  func testBusRotationResetsToTheTailWindowOfTheNewFile() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    var statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .sent)

    let window = OverlayChannelDeliveryReader.tailWindow
    try fixture.replaceBusWithEvidence(bytes: Int(window) + 6_000_000, trailing: [fixture.open()])
    let before = await reader.consumedBytes
    statuses = try await reader.read()
    let after = await reader.consumedBytes
    let replayed = after - before
    XCTAssertLessThanOrEqual(
      replayed, window, "rotation replays the tail window, never the whole file")
    XCTAssertGreaterThan(replayed, window - 100_000)
    XCTAssertEqual(
      statuses.first?.isOpen, true, "the rotated file's tail carries the live session")
    XCTAssertNil(statuses.first?.stage, "the old file's receipt cannot survive rotation")
  }

  func testTailWindowReadOnAHundredMegabyteBusStaysFastAndBounded() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.fillBusWithEvidence(bytes: 100 << 20)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    let before = Self.residentBytes()
    let started = Date()
    let statuses = try await OverlayChannelDeliveryReader(root: fixture.root).read()
    let elapsed = Date().timeIntervalSince(started)
    let growth = Int64(bitPattern: Self.residentBytes()) &- Int64(bitPattern: before)
    print("PERF100MB elapsed=\(elapsed)s rss_delta=\(growth)B")
    XCTAssertLessThan(elapsed, 1, "the first read parses only the 64 MiB tail window")
    XCTAssertLessThan(
      growth, 200 << 20, "the chunked autoreleasepool keeps the peak bounded")
    XCTAssertEqual(statuses.first?.isOpen, true)
    XCTAssertEqual(statuses.first?.stage, .sent)
  }

  private static func residentBytes() -> UInt64 {
    var info = mach_task_basic_info()
    var count = mach_msg_type_number_t(
      MemoryLayout<mach_task_basic_info>.stride / MemoryLayout<integer_t>.stride)
    let result = withUnsafeMutablePointer(to: &info) { pointer in
      pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { rebound in
        task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), rebound, &count)
      }
    }
    return result == KERN_SUCCESS ? info.resident_size : 0
  }

  func testLoudOpenStateShowsPanelAndRefusesHideWithoutChangingTranscript() throws {
    let state = OverlayState.previewFormatted()
    let transcript = state.activeText
    var shown = 0
    var hidden = 0
    let panel = NSPanel(
      contentRect: NSRect(x: 0, y: 0, width: 500, height: 300),
      styleMask: [.borderless], backing: .buffered, defer: false)
    let controller = OverlayController(
      state: state, overlayEnabledProvider: { false }, assistiveStatusProvider: { false },
      panelFactory: { _, _ in panel }, orderPanelFront: { _ in shown += 1 },
      orderPanelOut: { _ in hidden += 1 })
    let open = OverlayChannelDelivery(
      channel: "1", agent: "james", deliveryID: nil, stage: nil, isOpen: true)
    state.applyChannelDelivery([open])
    XCTAssertEqual(shown, 1, "open channel overrides the ordinary dictation visibility preference")
    controller.hide()
    controller.hideForAgentHandoff()
    XCTAssertEqual(hidden, 0)
    XCTAssertEqual(state.activeText, transcript)
    state.applyChannelDelivery([])
    controller.hide()
    XCTAssertEqual(hidden, 1)
    // Automatic hides yield to the open channel; the human Close intent does not.
    state.applyChannelDelivery([open])
    state.relayIntent(.close)
    XCTAssertEqual(hidden, 2, "an open channel must never veto the brand-dot close")
    XCTAssertTrue(state.hasOpenChannel, "closing the panel does not rewrite channel evidence")
  }

  func testChannelDetailsDoNotIncreaseCollapsedPanelHeight() {
    let state = OverlayState.previewFormatted()
    let panel = DictationOverlayWindow.make(
      state: state, textScale: TextScaleController(key: "ChannelCompactChromeTests"))
    defer { (panel as? FloatingOverlayPanel)?.invalidatePresence() }
    state.toggleCollapsed()
    let before = panel.frame.height
    state.applyChannelDelivery(
      (1...9).map {
        OverlayChannelDelivery(
          channel: String($0), agent: "Agent \($0)", deliveryID: nil,
          stage: nil, isOpen: true)
      })
    panel.contentView?.layoutSubtreeIfNeeded()
    XCTAssertEqual(panel.frame.height, before, accuracy: 0.5)
    XCTAssertEqual(panel.frame.height, DictationOverlayWindow.collapsedHeight, accuracy: 0.5)
  }

  func testChannelChromeIsOutsideCollapseGateAndDragStillPrecedesGlass() throws {
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent()
    let view = try String(
      contentsOf: root.appendingPathComponent(
        "Codescribe/Screens/Overlay/DictationOverlayView.swift"), encoding: .utf8)
    let start = try XCTUnwrap(view.range(of: "private var header: some View"))
    let end = try XCTUnwrap(
      view.range(of: "private var closeButton", range: start.upperBound..<view.endIndex))
    let header = String(view[start.lowerBound..<end.lowerBound])
    XCTAssertFalse(header.contains("OverlayChannelStatusView("))
    let controls = try XCTUnwrap(
      view.range(of: "private func justifiedHeader(density: OverlayHeaderDensity)"))
    XCTAssertTrue(view[controls.lowerBound...].contains("OverlayChannelStatusView("))
    XCTAssertFalse(header.contains("!state.isCollapsed"))
    let drag = try XCTUnwrap(header.range(of: "OverlayWindowDragRegion"))
    let glass = try XCTUnwrap(
      header.range(of: ".modifier(OverlayControlGlass())")
    )
    XCTAssertLessThan(drag.lowerBound, glass.lowerBound)
    let status = try String(
      contentsOf: root.appendingPathComponent(
        "Codescribe/Screens/Overlay/OverlayChannelStatusView.swift"), encoding: .utf8)
    XCTAssertTrue(status.contains("Microphone active"))
    XCTAssertFalse(status.contains(".popover("))
    XCTAssertTrue(view.contains("channelStatusView.monitorBody"))
    XCTAssertTrue(status.contains("overlay-channel-delivery-"))
    XCTAssertTrue(status.contains("Divider("), "saved histories retain their own section")
    XCTAssertFalse(status.contains("glassEffect("))
    // The header shows one glyph, never the microphone: mic = recording only.
    // The antenna belongs to the roster's 0 · All dot alone (Maciej,
    // 2026-10-17: "wyróżnić bardziej kanał ogólny"), never to the header.
    XCTAssertEqual(
      status.components(separatedBy: "antenna.radiowaves").count - 1, 1,
      "one antenna in the status view: the shared channel's dot")
    let statusDot = try XCTUnwrap(status.range(of: "struct OverlayAgentStatusDot: View"))
    let pillStack = try XCTUnwrap(status.range(of: "struct OverlayAgentPillStack: View"))
    let antenna = try XCTUnwrap(status.range(of: "antenna.radiowaves"))
    XCTAssertTrue(
      statusDot.lowerBound < antenna.lowerBound && antenna.lowerBound < pillStack.lowerBound,
      "the antenna is drawn by OverlayAgentStatusDot, not by the header")
    XCTAssertFalse(status.contains("hasOpenChannel ? \"mic.fill\""))
    // Passive navigation and capture ownership are exercised by
    // testNotificationNavigationRetainsOwnerAndUnreadCountWithoutOpeningMicrophone.
  }

  func testRosterToggleAcceptsOnlyChannelDigitsAndForwardsEachClickOnce() {
    XCTAssertEqual(OverlayChannelStatusView.toggleDigit(for: "1"), 1)
    XCTAssertEqual(OverlayChannelStatusView.toggleDigit(for: "9"), 9)
    XCTAssertEqual(OverlayChannelStatusView.toggleDigit(for: "0"), 0)
    for invalid in ["10", "agent-1", ""] {
      XCTAssertNil(OverlayChannelStatusView.toggleDigit(for: invalid))
    }

    var calls: [UInt8] = []
    let view = OverlayChannelStatusView(
      channels: [], unavailable: false, palette: .dark, animates: false,
      onToggleChannel: { calls.append($0) })
    let closed = OverlayChannelDelivery(
      channel: "1", agent: "klaudiusz", deliveryID: nil, stage: nil, isOpen: false)
    let open = OverlayChannelDelivery(
      channel: "3", agent: "roman", deliveryID: nil, stage: nil, isOpen: true)
    view.toggle(closed)
    view.toggle(open)
    view.toggle(.init(channel: "10", agent: "invalid", deliveryID: nil, stage: nil, isOpen: false))
    XCTAssertEqual(calls, [1, 3], "both directions use the same per-digit toggle intent")
  }

  func testSavedConversationsShowNewestMessageFirstWithUnknownDatesLast() {
    func conversation(_ id: String, dates: [String]) -> OverlayConversation {
      OverlayConversation(
        id: id, channel: "1", name: id, owner: nil,
        messages: dates.enumerated().map { index, stamp in
          .init(
            id: "\(id)-\(index)", kind: .reply, text: "Retained reply", order: UInt64(index),
            emittedAt: stamp, owner: nil, recipients: [], deliveryID: nil, replyTo: nil,
            unsolicited: true, playback: nil, busPath: "")
        })
    }
    let old = conversation("old", dates: ["2026-10-05T15:26:00Z"])
    let newest = conversation(
      "newest", dates: ["2026-10-01T08:00:00Z", "2026-10-12T10:00:00.250Z", "invalid"])
    let offset = conversation("offset", dates: ["2026-10-12T12:00:00+02:00"])
    let tieB = conversation("tie-b", dates: ["2026-10-07T21:19:00Z"])
    let tieA = conversation("tie-a", dates: ["2026-10-07T21:19:00Z"])
    var unloaded = conversation("unloaded", dates: [])
    unloaded.historyLoaded = false
    let invalid = conversation("invalid", dates: ["not-a-date"])
    let broadcast = OverlayConversation(
      id: "0", channel: "0", name: "All", owner: nil, messages: newest.messages)
    let input = [old, unloaded, tieB, invalid, newest, broadcast, offset, tieA]
    let view = OverlayChannelStatusView(
      channels: [], unavailable: false, palette: .dark, animates: false, conversations: input)

    XCTAssertEqual(
      view.savedConversations.map(\.id),
      [newest.id, offset.id, tieA.id, tieB.id, old.id, invalid.id, unloaded.id])
    XCTAssertEqual(view.currentConversations.map(\.id), [broadcast.id])
    XCTAssertEqual(view.conversations, input, "Sorting is a passive presentation projection")
    XCTAssertFalse(unloaded.historyLoaded, "Sorting must not load archives or invent their dates")
  }

  func testMonitorSeparatesCurrentOwnerFromSameNamedSavedConversation() throws {
    func conversation(session: String, lease: String) throws -> OverlayConversation {
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": session, "lease_id": lease,
          "channel": "3", "name": "astra",
        ]))
      return OverlayConversation(
        id: owner.id, channel: "3", name: "astra", owner: owner, messages: [])
    }
    let old = try conversation(session: "previous", lease: String(repeating: "a", count: 32))
    let current = try conversation(session: "current", lease: String(repeating: "b", count: 32))
    let hud = OverlayChannelHudProjection(
      open: false, loud: false,
      autosealDeadline: nil, followerAlive: true, provider: "codex", providerSessionID: "current")
    let view = OverlayChannelStatusView(
      channels: [], unavailable: false,
      palette: .dark, animates: false,
      hudStates: ["3": hud], conversations: [old, current])
    XCTAssertEqual(view.currentConversations.map(\.id), [current.id])
    XCTAssertEqual(view.savedConversations.map(\.id), [old.id])
    XCTAssertEqual(view.conversations.count, 2, "Rendering does not merge histories by name")
    let sameSession = try conversation(session: "current", lease: String(repeating: "c", count: 32))
    let ambiguous = OverlayChannelStatusView(
      channels: [], unavailable: false,
      palette: .dark, animates: false,
      hudStates: ["3": hud], conversations: [current, sameSession])
    XCTAssertTrue(
      ambiguous.currentConversations.isEmpty, "The roster has no lease authority to choose")
    XCTAssertEqual(ambiguous.savedConversations.count, 2)
  }

  func testRosterClickWithoutBridgeActionDoesNotStartAnyAgent() {
    let channel = OverlayChannelDelivery(
      channel: "2", agent: "miron", deliveryID: nil, stage: nil, isOpen: false)
    let view = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .light, animates: false)
    XCTAssertNil(view.onToggleChannel)
    view.toggle(channel)
    XCTAssertFalse(view.isOpen(channel), "a click cannot optimistically open the channel")
  }

  func testUnifiedAgentRowKeepsViewingAndRecordingSeparate() throws {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "current",
        "lease_id": String(repeating: "b", count: 32),
        "channel": "2", "name": "lena",
      ]))
    let current = OverlayConversation(
      id: owner.id, channel: "2", name: "lena", owner: owner, messages: [])
    let channel = OverlayChannelDelivery(
      channel: "2", agent: "lena", deliveryID: nil, stage: nil, isOpen: false)
    var selections: [String?] = []
    var toggles: [UInt8] = []
    let view = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "2": .init(
          open: false, loud: false, autosealDeadline: nil, followerAlive: true,
          provider: "codex", providerSessionID: "current")
      ],
      onToggleChannel: { toggles.append($0) }, conversations: [current],
      onSelectConversation: { selections.append($0) })
    view.viewConversation(channel)
    XCTAssertEqual(selections, [current.id])
    XCTAssertTrue(toggles.isEmpty, "passive viewing never opens the microphone")
    view.toggle(channel)
    XCTAssertEqual(toggles, [2])
    XCTAssertEqual(selections, [current.id], "capture uses its own controller intent")
    XCTAssertFalse(view.isOpen(channel), "the controller, not a click, owns open state")
  }

  func testNotificationNavigationRetainsOwnerAndUnreadCountWithoutOpeningMicrophone() throws {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "current",
        "lease_id": String(repeating: "b", count: 32), "channel": "2", "name": "lena",
      ]))
    let conversation = OverlayConversation(
      id: owner.id, channel: "2", name: "lena", owner: owner, messages: [])
    let channel = OverlayChannelDelivery(
      channel: "2", agent: "lena", deliveryID: "confirmed", stage: .received, isOpen: false)
    var selections: [String?] = []
    var toggles: [UInt8] = []
    var monitors = 0
    let view = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "2": .init(
          open: false, loud: false, autosealDeadline: nil,
          followerAlive: true, provider: "codex", providerSessionID: "current")
      ],
      onToggleChannel: { toggles.append($0) }, conversations: [conversation],
      unreadCounts: [owner.id: 3], onSelectConversation: { selections.append($0) },
      onShowMonitor: { monitors += 1 })
    XCTAssertEqual(view.notificationTitle(for: channel), "2 · lena (3)")
    XCTAssertEqual(view.glyph, .acknowledged, "unread replies are separate from receipt state")
    view.viewConversation(channel)
    view.showMonitor()
    XCTAssertEqual(selections, [owner.id])
    XCTAssertEqual(monitors, 1)
    XCTAssertTrue(toggles.isEmpty, "notification navigation never opens a microphone")
    XCTAssertFalse(view.isOpen(channel))
  }

  func testHeaderAndChannelMicrophoneGlyphKeepOneCompactCircleInBothAppearances() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      for symbol in ["mic", "mic.fill", "stop.fill"] {
        let host = NSHostingView(
          rootView:
            OverlayMicrophoneGlyph(symbol: symbol, tint: palette.listeningStatus.color))
        let size = host.fittingSize
        XCTAssertEqual(size.width, OverlayRecordingControls.controlDiameter, accuracy: 0.5)
        XCTAssertEqual(size.height, OverlayRecordingControls.controlDiameter, accuracy: 0.5)
      }
    }
  }

  func testUnifiedAgentRowDoesNotChooseAnAmbiguousLease() throws {
    let owners = try ["b", "c"].map { lease in
      try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": "current",
          "lease_id": String(repeating: lease, count: 32),
          "channel": "2", "name": "lena",
        ]))
    }
    let channel = OverlayChannelDelivery(
      channel: "2", agent: "lena", deliveryID: nil, stage: nil, isOpen: false)
    var selections: [String?] = []
    let view = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "2": .init(
          open: false, loud: false, autosealDeadline: nil, followerAlive: true,
          provider: "codex", providerSessionID: "current")
      ],
      conversations: owners.map {
        .init(id: $0.id, channel: "2", name: "lena", owner: $0, messages: [])
      },
      onSelectConversation: { selections.append($0) })
    view.viewConversation(channel)
    XCTAssertTrue(selections.isEmpty)
    XCTAssertEqual(view.savedConversations.map(\.id), owners.map(\.id))
  }

  func testRenderedUnifiedMonitorFitsCurrentAndSavedRowsWithoutDuplicatingCaptureList() throws {
    func conversation(session: String, lease: String) throws -> OverlayConversation {
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": session,
          "lease_id": String(repeating: lease, count: 32),
          "channel": "2", "name": "lena",
        ]))
      return .init(id: owner.id, channel: "2", name: "lena", owner: owner, messages: [])
    }
    let current = try conversation(session: "current", lease: "b")
    let saved = try conversation(session: "previous", lease: "a")
    for palette in [OverlayAppearancePalette.light, .dark] {
      var selections: [String?] = []
      var toggles: [UInt8] = []
      let view = OverlayChannelStatusView(
        channels: [
          .init(channel: "2", agent: "lena", deliveryID: "receipt", stage: .received, isOpen: true)
        ],
        unavailable: false, palette: palette, animates: false,
        hudStates: [
          "2": .init(
            open: true, loud: false, autosealDeadline: nil, followerAlive: true,
            provider: "codex", providerSessionID: "current")
        ],
        onToggleChannel: { toggles.append($0) }, conversations: [saved, current],
        unreadCounts: [current.id: 3], onSelectConversation: { selections.append($0) })
      let host = NSHostingView(rootView: view.monitorBody.frame(width: 440))
      let window = NSWindow(
        contentRect: .init(x: 0, y: 0, width: 440, height: 320),
        styleMask: [.titled], backing: .buffered, defer: false)
      window.contentView = host
      window.orderFrontRegardless()
      defer { window.orderOut(nil) }
      host.layoutSubtreeIfNeeded()
      window.displayIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.05))
      host.layoutSubtreeIfNeeded()

      XCTAssertEqual(view.currentConversations.map(\.id), [current.id])
      XCTAssertEqual(view.savedConversations.map(\.id), [saved.id])
      XCTAssertEqual(host.fittingSize.width, 440, accuracy: 1)
      // One compact row plus the saved section measured 118 pt once the
      // Transcription row left the column (Maciej, 2026-10-17); without the
      // saved section the column would stop short of 100 pt.
      XCTAssertGreaterThan(host.fittingSize.height, 100, "saved history is still visible")
      XCTAssertTrue(selections.isEmpty, "rendering does not navigate")
      XCTAssertTrue(toggles.isEmpty, "rendering does not start recording")
      XCTAssertLessThan(host.fittingSize.height, 220, "one compact row plus saved history")
    }
  }

  func testFourAgentMonitorScrollsWithinShortCanvasAndHugsTallCanvas() throws {
    let channels = ["1", "2", "4", "7"].map {
      OverlayChannelDelivery(
        channel: $0, agent: "agent-\($0)", deliveryID: "receipt-\($0)", stage: .received,
        isOpen: true)
    }
    for maximumHeight: CGFloat in [174, 588] {
      let view = OverlayChannelStatusView(
        channels: channels, unavailable: false, palette: .dark, animates: false)
      let host = NSHostingView(
        rootView: view.monitorBody(maximumHeight: maximumHeight).frame(width: 336))
      let window = NSWindow(
        contentRect: .init(x: 0, y: 0, width: 336, height: maximumHeight), styleMask: [.titled],
        backing: .buffered, defer: false)
      window.contentView = host
      window.orderFrontRegardless()
      defer { window.orderOut(nil) }
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.15))
      host.layoutSubtreeIfNeeded()
      XCTAssertLessThanOrEqual(host.fittingSize.height, maximumHeight + 1)
      XCTAssertGreaterThan(host.fittingSize.height, 100)
      if maximumHeight > 500 {
        XCTAssertLessThan(
          host.fittingSize.height, 400, "four agents do not need a screen-height drawer")
      }
    }
  }

  func testDeadFollowerIsVisibleWithoutRewritingDeliveryOrOpenState() {
    let channel = OverlayChannelDelivery(
      channel: "3", agent: "roman", deliveryID: "delivery-3", stage: .queued, isOpen: true)
    let view = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .dark, animates: false,
      hudStates: ["3": .init(open: false, loud: false, autosealDeadline: nil, followerAlive: false)]
    )
    XCTAssertFalse(view.isOpen(channel), "controller HUD state wins over older mailbox state")
    XCTAssertTrue(view.hasDeadFollower(channel))
    XCTAssertEqual(view.statusSymbol(for: channel), "xmark.circle")
    XCTAssertTrue(view.statusHelp(for: channel).contains(String(localized: "Disconnected")))
    XCTAssertEqual(view.detail(for: channel), "queued · waiting for receipt · nobody listening")
    XCTAssertEqual(channel.stage, .queued, "liveness does not reinterpret delivery evidence")
  }

  func testCompactRosterUsesOnlySuppliedRuntimeDescriptorAndKeepsUnknownModelsAbsent() {
    let channel = OverlayChannelDelivery(
      channel: "3", agent: "astra", deliveryID: nil, stage: nil, isOpen: false)
    let hud = OverlayChannelHudProjection(
      open: false, loud: false, autosealDeadline: nil, followerAlive: true,
      provider: "codex", providerSessionID: "astra-session")
    let named = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .light, animates: false,
      hudStates: ["3": hud], agentDescriptors: ["3": "Codex · gpt-6.1-sol"])
    XCTAssertEqual(named.agentDescriptor(for: channel), "Codex · gpt-6.1-sol")
    let providerOnly = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .light, animates: false,
      hudStates: ["3": hud])
    XCTAssertEqual(providerOnly.agentDescriptor(for: channel), "codex")
    let unknown = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .light, animates: false)
    XCTAssertNil(unknown.agentDescriptor(for: channel))
    XCTAssertEqual(providerOnly.statusSymbol(for: channel), "circle")
  }

  func testLiveAndUnknownFollowerKeepTheExistingDeliveryCopy() {
    let channel = OverlayChannelDelivery(
      channel: "1", agent: "klaudiusz", deliveryID: "delivery-1", stage: .received,
      isOpen: false)
    let live = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .light, animates: false,
      hudStates: ["1": .init(open: true, loud: true, autosealDeadline: nil, followerAlive: true)])
    let unknown = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .light, animates: false)
    XCTAssertTrue(live.isOpen(channel))
    XCTAssertFalse(live.hasDeadFollower(channel))
    XCTAssertEqual(live.detail(for: channel), "receipt confirmed by the agent")
    XCTAssertFalse(unknown.hasDeadFollower(channel), "missing HUD evidence must not claim death")
    XCTAssertEqual(unknown.detail(for: channel), "receipt confirmed by the agent")
  }

  func testHeaderNativeMenuKeepsFullAgentGlyphForEveryStateAndAppearance() throws {
    for palette in [OverlayAppearancePalette.light, .dark] {
      var attachedMenuPixels: Data?
      for glyph in OverlayAgentGlyph.allCases {
        let channel = OverlayChannelDelivery(
          channel: "2", agent: "lena",
          deliveryID: glyph == .attached || glyph == .open ? nil : "d1",
          stage: glyph == .awaitingReceipt ? .queued : glyph == .acknowledged ? .received : nil,
          isOpen: glyph == .open)
        let mark = OverlayAgentStatusMark(
          reduceMotion: true, glyph: glyph, palette: palette, animates: false, fontSize: 13)
        let standalone = try glyphInk(mark, palette: palette, name: "standalone-\(glyph)")
        let oldButton = try glyphInk(
          Button {
          } label: {
            mark
          }.buttonStyle(.plain),
          palette: palette, name: "old-button-\(glyph)")
        let header = OverlayChannelStatusView(
          channels: [channel], unavailable: glyph == .unavailable,
          palette: palette, animates: true)
        XCTAssertEqual(header.glyph, glyph, "visual art must not replace projected receipt truth")
        let menu = try glyphInk(header, palette: palette, name: "native-menu-\(glyph)")
        if glyph == .attached { attachedMenuPixels = menu.pixels }
        XCTAssertEqual(
          menu.pixels, try XCTUnwrap(attachedMenuPixels),
          "header must retain the full muted agent glyph rather than a spinner arc: \(glyph)")
        print(
          "GLYPH_INK \(palette.appearance) \(glyph) standalone=\(standalone.ink) oldButton=\(oldButton.ink) menu=\(menu.ink)"
        )
        XCTAssertGreaterThan(standalone.ink, 5, "standalone positive rendering control")
        XCTAssertGreaterThan(oldButton.ink, 5, "original plain button positive rendering control")
        XCTAssertGreaterThan(
          menu.ink, 5,
          "native Menu must paint the glyph, not merely reserve its slot: \(glyph), \(palette.appearance)"
        )
      }
    }
  }

  private func glyphInk<V: View>(
    _ view: V, palette: OverlayAppearancePalette, name: String
  ) throws -> (ink: Int, pixels: Data) {
    let host = NSHostingView(
      rootView: ZStack {
        palette.desktopBackground.color
        view
      }
      .frame(width: 64, height: 64)
      .environment(\.colorScheme, palette.appearance == .dark ? .dark : .light))
    host.frame = NSRect(x: 0, y: 0, width: 64, height: 64)
    let window = NSWindow(
      contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.appearance = NSAppearance(named: palette.appearance == .dark ? .darkAqua : .aqua)
    window.contentView = host
    defer {
      window.contentView = nil
      window.close()
    }
    host.layoutSubtreeIfNeeded()
    host.displayIfNeeded()
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.03))
    host.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let background = try XCTUnwrap(bitmap.colorAt(x: 2, y: 2)?.usingColorSpace(.deviceRGB))
    XCTAssertGreaterThan(background.alphaComponent, 0.95, "opaque test surface must render")
    var ink = 0
    var strongestContrast: CGFloat = 0
    var strongestColor = background
    let sx = CGFloat(bitmap.pixelsWide) / 64
    let sy = CGFloat(bitmap.pixelsHigh) / 64
    for y in Int(21 * sy)..<Int(43 * sy) {
      for x in Int(23 * sx)..<Int(41 * sx) {
        let color = try XCTUnwrap(bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB))
        let contrast = max(
          abs(color.redComponent - background.redComponent),
          abs(color.greenComponent - background.greenComponent),
          abs(color.blueComponent - background.blueComponent))
        if contrast > 0.08 { ink += 1 }
        if contrast > strongestContrast {
          strongestContrast = contrast
          strongestColor = color
        }
      }
    }
    if name.hasPrefix("native-menu-") {
      let expected = try XCTUnwrap(palette.mutedText.nsColor.usingColorSpace(.deviceRGB))
      XCTAssertLessThan(
        max(
          abs(strongestColor.redComponent - expected.redComponent),
          abs(strongestColor.greenComponent - expected.greenComponent),
          abs(strongestColor.blueComponent - expected.blueComponent)),
        0.08, "header glyph must retain its muted palette for every projected state")
    }
    let attachment = XCTAttachment(
      data: try XCTUnwrap(bitmap.representation(using: .png, properties: [:])),
      uniformTypeIdentifier: "public.png")
    attachment.name = "\(palette.appearance)-\(name)"
    attachment.lifetime = .keepAlways
    add(attachment)
    func nativeLabels(_ node: NSView) -> [String] {
      let own: [String]
      if let button = node as? NSButton {
        own = [
          "\(type(of: button)): title=\(button.title.debugDescription) image=\(button.image != nil) frame=\(button.frame)"
        ]
      } else {
        own = []
      }
      return own + node.subviews.flatMap(nativeLabels)
    }
    print("GLYPH_NATIVE \(palette.appearance) \(name) \(nativeLabels(host))")
    if name == "native-menu-awaitingReceipt" {
      func buttons(_ node: NSView) -> [NSButton] {
        ((node as? NSButton).map { [$0] } ?? []) + node.subviews.flatMap(buttons)
      }
      let before = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.08))
      host.cacheDisplay(in: host.bounds, to: bitmap)
      XCTAssertEqual(
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])), before,
        "the header receipt icon stays stationary even while overlay animation is enabled")
    }
    return (ink, try XCTUnwrap(bitmap.representation(using: .png, properties: [:])))
  }

  // MARK: Agent glyph (Annex A3/A4 — the state table is the Codex root's proposal)

  func testAgentGlyphUsesSpinnerOrOneCharacterWithOneLabelPerState() {
    let table: [(OverlayAgentGlyph, String?, String)] = [
      (.attached, "\u{2756}", "Agent attached"),
      (.open, "\u{2756}", "Agent channel open"),
      (.awaitingReceipt, nil, "Waiting for the agent to confirm receipt"),
      (.acknowledged, "\u{2406}", "Agent confirmed receipt"),
      (.unavailable, "\u{26A0}\u{FE0E}", "Agent channel status unavailable"),
    ]
    XCTAssertEqual(table.map(\.0), OverlayAgentGlyph.allCases, "every state is named once")
    for (glyph, character, label) in table {
      XCTAssertEqual(glyph.character, character)
      XCTAssertEqual(
        glyph.character?.count, character == nil ? nil : 1, "\(glyph) spends exactly one character")
      XCTAssertEqual(glyph.label, label)
    }
    XCTAssertEqual(Set(table.map(\.2)).count, table.count, "labels tell every state apart")
    XCTAssertEqual(
      OverlayAgentGlyph.unavailable.character?.unicodeScalars.last, "\u{FE0E}",
      "the warning sign is the monochrome text form, never the colour emoji")
    XCTAssertNil(OverlayAgentGlyph.awaitingReceipt.character)
  }

  func testWaitingSpinnerRotatesContinuouslyUnlessMotionIsReducedOrHidden() {
    for glyph in OverlayAgentGlyph.allCases {
      for animates in [true, false] {
        for reduceMotion in [true, false] {
          let mark = OverlayAgentStatusMark(
            reduceMotion: reduceMotion, glyph: glyph, palette: .dark, animates: animates,
            fontSize: 13)
          let shouldRotate = glyph == .awaitingReceipt && animates && !reduceMotion
          XCTAssertEqual(mark.showsSpinner, glyph == .awaitingReceipt)
          XCTAssertEqual(mark.rotates, shouldRotate)
          XCTAssertEqual(mark.rotation(at: 0.25).degrees, shouldRotate ? 90 : 0)
          XCTAssertEqual(mark.rotation(at: 0.75).degrees, shouldRotate ? 270 : 0)
        }
      }
    }
  }

  func testRosterSpinnerKeepsItsSlotWithReduceMotion() {
    for reduceMotion in [true, false] {
      for glyph in OverlayAgentGlyph.allCases {
        let mark = OverlayAgentStatusMark(
          reduceMotion: reduceMotion, glyph: glyph, palette: .light, animates: true, fontSize: 11)
        let host = NSHostingView(rootView: mark)
        host.layoutSubtreeIfNeeded()
        XCTAssertEqual(host.fittingSize.width, 18, accuracy: 0.5)
        XCTAssertEqual(host.fittingSize.height, 22, accuracy: 0.5)
      }
    }
  }

  func testAgentGlyphResolvesWorstNewsFirstFromProjectionOnly() {
    func channel(_ id: String, _ stage: OverlayChannelDelivery.Stage?, open: Bool = false)
      -> OverlayChannelDelivery
    {
      OverlayChannelDelivery(
        channel: id, agent: "Agent \(id)", deliveryID: stage == nil ? nil : "d\(id)",
        stage: stage, isOpen: open)
    }
    XCTAssertNil(OverlayAgentGlyph.resolve(channels: [], unavailable: false), "no agent, no slot")
    XCTAssertEqual(OverlayAgentGlyph.resolve(channels: [], unavailable: true), .unavailable)
    XCTAssertEqual(
      OverlayAgentGlyph.resolve(channels: [channel("1", nil)], unavailable: false), .attached)
    XCTAssertEqual(
      OverlayAgentGlyph.resolve(channels: [channel("1", nil, open: true)], unavailable: false),
      .open)
    for stage in [OverlayChannelDelivery.Stage.sent, .queued] {
      XCTAssertEqual(
        OverlayAgentGlyph.resolve(channels: [channel("1", stage, open: true)], unavailable: false),
        .awaitingReceipt)
    }
    XCTAssertEqual(
      OverlayAgentGlyph.resolve(
        channels: [channel("1", .received, open: true)], unavailable: false),
      .acknowledged)
    XCTAssertEqual(
      OverlayAgentGlyph.resolve(
        channels: [channel("1", .received), channel("2", .sent)], unavailable: false),
      .awaitingReceipt, "one unconfirmed delivery keeps the slot waiting")
    XCTAssertEqual(
      OverlayAgentGlyph.resolve(channels: [channel("1", .received)], unavailable: true),
      .unavailable, "an unreadable status never shows a stale receipt")
  }

  /// ␆ comes from the agent's own `codescribe.agent-ack.v1` row for this
  /// delivery, channel and agent — not from the durable marker, another
  /// channel, re-reading over time, or an earlier take's receipt.
  func testAcknowledgedGlyphLightsOnlyFromTheMatchingAgentAckRow() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    func glyph() async throws -> OverlayAgentGlyph? {
      OverlayAgentGlyph.resolve(channels: try await reader.read(), unavailable: false)
    }
    var current = try await glyph()
    XCTAssertEqual(current, .attached)
    try fixture.append(fixture.open())
    current = try await glyph()
    XCTAssertEqual(current, .open)
    try fixture.append(fixture.seal(1))
    current = try await glyph()
    XCTAssertEqual(current, .awaitingReceipt)
    try fixture.lease(pending: [fixture.envelope(Fixture.firstID)])
    try fixture.marker(Fixture.firstID)
    for _ in 0..<3 {
      current = try await glyph()
      XCTAssertEqual(current, .awaitingReceipt, "marker and elapsed reads are not the agent's row")
    }
    try fixture.append(fixture.ack(Fixture.firstID, channel: "2"))
    current = try await glyph()
    XCTAssertEqual(current, .awaitingReceipt, "another channel cannot acknowledge this one")
    try fixture.append(fixture.ack(Fixture.firstID))
    current = try await glyph()
    XCTAssertEqual(current, .acknowledged)
    try fixture.append(fixture.seal(2))
    current = try await glyph()
    XCTAssertEqual(current, .awaitingReceipt, "new words must not inherit the old receipt")
  }

  func testAgentGlyphKeepsOneFixedSlotSoNeighboursNeverMove() {
    let inputs: [(OverlayAgentGlyph, [OverlayChannelDelivery], Bool)] = [
      (
        .attached, [.init(channel: "1", agent: "a", deliveryID: nil, stage: nil, isOpen: false)],
        false
      ),
      (.open, [.init(channel: "1", agent: "a", deliveryID: nil, stage: nil, isOpen: true)], false),
      (
        .awaitingReceipt,
        [.init(channel: "1", agent: "a", deliveryID: "d", stage: .queued, isOpen: false)], false
      ),
      (
        .acknowledged,
        [.init(channel: "1", agent: "a", deliveryID: "d", stage: .received, isOpen: false)], false
      ),
      (
        .unavailable, [.init(channel: "1", agent: "a", deliveryID: nil, stage: nil, isOpen: false)],
        true
      ),
    ]
    var sizes: [CGSize] = []
    for (expected, channels, unavailable) in inputs {
      for palette in [OverlayAppearancePalette.light, .dark] {
        let view = OverlayChannelStatusView(
          channels: channels, unavailable: unavailable, palette: palette, animates: false)
        XCTAssertEqual(view.glyph, expected)
        let host = NSHostingView(rootView: view)
        host.layoutSubtreeIfNeeded()
        sizes.append(host.fittingSize)
      }
    }
    for size in sizes {
      XCTAssertEqual(size.width, OverlayRecordingControls.controlDiameter, accuracy: 0.5)
      XCTAssertEqual(size.height, OverlayAgentGlyph.slotSize.height, accuracy: 0.5)
    }
  }

  func testEmbeddedRosterUsesOverlayAppearanceAndReadableTokens() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      let monitor = ChannelRosterContent(palette: palette) {
        Text("Channel receipt")
      }
      let style = monitor.style
      XCTAssertEqual(style.surface, palette.desktopBackground)
      XCTAssertEqual(style.border, palette.border)
      XCTAssertEqual(style.colorScheme, palette.appearance == .dark ? .dark : .light)
      XCTAssertEqual(style.primaryText, palette.primaryText)
      XCTAssertEqual(style.bodyText, palette.bodyText)
      XCTAssertEqual(style.mutedText, palette.mutedText)

      for (role, foreground, minimum) in [
        ("channel name", style.primaryText, 4.5),
        ("receipt and dead follower", style.bodyText, 4.5),
        ("secondary text", style.mutedText, 3.0),
        ("open microphone", palette.listeningStatus, 4.5),
        ("queued receipt", palette.processingStatus, 4.5),
        ("confirmed receipt", palette.successStatus, 4.5),
        ("toggle failure", palette.errorStatus, 4.5),
      ] {
        let ratio = OverlayColorToken.contrastRatio(
          foreground: foreground, surface: style.surface, background: style.surface)
        XCTAssertGreaterThanOrEqual(
          ratio, minimum, "\(palette.appearance) \(role) contrast was \(ratio):1")
      }
    }
  }

  func testManagedRolloverKeepsOpenReceiptAndConsumesOnlyTheNewSuffix() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    let before = try await reader.read()
    XCTAssertEqual(before.first?.stage, .sent)
    XCTAssertEqual(before.first?.isOpen, true)
    let consumedBefore = await reader.consumedBytes
    try pinSyntheticGeneration(fixture)
    try fixture.append(fixture.ack(Fixture.firstID))
    let suffix = try fixture.busSize()
    let after = try await reader.read()
    XCTAssertEqual(after.first?.stage, .received)
    XCTAssertEqual(after.first?.isOpen, true, "storage rollover cannot close a microphone")
    let consumedAfter = await reader.consumedBytes
    XCTAssertEqual(
      consumedAfter - consumedBefore, suffix, "active observer must not replay archived bytes")
    let restarted = try await OverlayChannelDeliveryReader(root: fixture.root).read()
    XCTAssertEqual(restarted.first?.stage, .received)
    XCTAssertEqual(restarted.first?.isOpen, true)
  }

  func testManagedColdMonitorSeesNewestSpeechWithinTheExistingTailBudget() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.fillBusWithEvidence(bytes: Int(OverlayChannelDeliveryReader.tailWindow) + 6_000_000)
    try pinSyntheticGeneration(fixture)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let statuses = try await reader.read()
    XCTAssertEqual(
      statuses.first?.isOpen, true, "first monitor observation must include the newest capture")
    XCTAssertEqual(
      statuses.first?.stage, .sent, "cold archived prefix cannot delay the current utterance")
    let consumed = await reader.consumedBytes
    XCTAssertLessThanOrEqual(
      consumed, OverlayChannelDeliveryReader.tailWindow,
      "archive admission does not remove the established cold-monitor scan budget")
  }

  func testMalformedSharedInventoryCannotPublishItsFirstTerminalOccurrence() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append([
      "schema": "codescribe.transcript-evidence.v1", "session_id": "take-a",
      "audience": "james", "sequence": 10, "document_index": 0,
      "reducer_revision": 7, "reducer_action": "record_ledger_terminal_seal",
      "rendered_text": "Iwo Iwo", "persistence_encoding": "shared-revision.v1",
      "occurrence_rows": [["sequence": 11, "label": "Iwo"]],
    ])
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    do {
      _ = try await reader.read()
      XCTFail("an invalid shared inventory must be refused before publishing any occurrence")
    } catch {
      XCTAssertFalse(
        FileManager.default.fileExists(
          atPath: OverlayDeliveryCursorStore.url(root: fixture.root).path),
        "invalid storage cannot advance a durable cursor")
    }
  }

  private func pinSyntheticGeneration(
    _ fixture: Fixture, day: String? = "2026_1002", volumeUUID: Any? = nil
  ) throws {
    let attributes = try FileManager.default.attributesOfItem(atPath: fixture.bus.path)
    let inode = try XCTUnwrap(attributes[.systemFileNumber] as? NSNumber)
    let dev = try XCTUnwrap(attributes[.systemNumber] as? NSNumber)
    let bytes = try fixture.busSize()
    let archive = fixture.root.appendingPathComponent("events/2026_1002/closed.jsonl")
    try FileManager.default.createDirectory(
      at: archive.deletingLastPathComponent(), withIntermediateDirectories: true)
    try FileManager.default.moveItem(at: fixture.bus, to: archive)
    try Data().write(to: fixture.bus, options: .atomic)
    let current = try FileManager.default.attributesOfItem(atPath: fixture.bus.path)
    let closed: [String: Any] = [
      "id": "closed", "path": archive.path, "start": 0, "length": bytes,
      "dev": dev, "ino": inode, "day": day.map { $0 as Any } ?? NSNull(), "compressed": false,
      "sha256": NSNull(), "superseded": NSNull(),
    ]
    let active: [String: Any] = [
      "id": "active", "path": fixture.bus.path, "start": bytes, "length": 0,
      "dev": try XCTUnwrap(current[.systemNumber] as? NSNumber),
      "ino": try XCTUnwrap(current[.systemFileNumber] as? NSNumber), "day": "2026_1003",
      "compressed": false, "sha256": NSNull(), "superseded": NSNull(),
    ]
    var manifest: [String: Any] = [
      "schema": "codescribe.bus-generations.v1", "root": fixture.bus.path,
      "stream_id": "synthetic-stream", "stream_inode": inode, "stream_dev": dev,
      "stream_birthtime": NSNull(), "segments": [closed], "active": active, "pending": NSNull(),
    ]
    if let volumeUUID { manifest["volume_uuid"] = volumeUUID }
    try fixture.write(
      manifest,
      to: URL(fileURLWithPath: fixture.bus.path + ".generations.json"))
  }

  func testManagedReceiptAcceptsOptionalVolumeUUIDAndRetainsClosedTranscript() async throws {
    for volume in [NSNull(), "41b7ff38-c1d7-48cd-bf70-373435ecf99a"] as [Any] {
      let fixture = try Fixture()
      defer { fixture.remove() }
      try fixture.append(fixture.open())
      try fixture.append(fixture.seal(1))
      let before = try Data(contentsOf: fixture.bus)
      try pinSyntheticGeneration(fixture, volumeUUID: volume)
      let reader = OverlayChannelDeliveryReader(root: fixture.root)
      let observed = try await reader.read()
      XCTAssertEqual(observed.first?.stage, .sent)
      let archive = fixture.root.appendingPathComponent("events/2026_1002/closed.jsonl")
      XCTAssertEqual(try Data(contentsOf: archive), before)
      XCTAssertEqual(try fixture.busSize(), 0)
    }
  }

  func testManagedReceiptRefusesMalformedVolumeUUID() async throws {
    for volume in ["not-a-uuid", 23, true, ["uuid": "invalid"]] as [Any] {
      let fixture = try Fixture()
      defer { fixture.remove() }
      try fixture.append(fixture.open())
      try pinSyntheticGeneration(fixture, volumeUUID: volume)
      let reader = OverlayChannelDeliveryReader(root: fixture.root)
      do {
        _ = try await reader.read()
        XCTFail("a malformed volume identity must not admit the storage receipt")
      } catch {
        XCTAssertFalse(
          FileManager.default.fileExists(
            atPath: OverlayDeliveryCursorStore.url(root: fixture.root).path))
      }
    }
  }

  func testManagedReceiptStillRefusesUnknownFieldsWithVolumeUUID() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.append(fixture.open())
    try pinSyntheticGeneration(fixture, volumeUUID: "41b7ff38-c1d7-48cd-bf70-373435ecf99a")
    let receipt = URL(fileURLWithPath: fixture.bus.path + ".generations.json")
    var manifest = try XCTUnwrap(
      JSONSerialization.jsonObject(with: Data(contentsOf: receipt)) as? [String: Any])
    manifest["unrecognized"] = true
    try fixture.write(manifest, to: receipt)
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    do {
      _ = try await reader.read()
      XCTFail("an optional volume UUID must not loosen unknown-field validation")
    } catch {
      XCTAssertFalse(
        FileManager.default.fileExists(
          atPath: OverlayDeliveryCursorStore.url(root: fixture.root).path))
    }
  }

  func testManagedColdMonitorSeesNewestSpeechAfterAnUndatedPrefix() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.fillBusWithEvidence(bytes: Int(OverlayChannelDeliveryReader.tailWindow) + 6_000_000)
    try pinSyntheticGeneration(fixture, day: nil)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let first = try await reader.read()
    XCTAssertEqual(first.first?.stage, .sent)
    XCTAssertEqual(first.first?.isOpen, true)
    let bytes = await reader.consumedBytes
    XCTAssertLessThanOrEqual(bytes, OverlayChannelDeliveryReader.tailWindow)
  }

  func testManagedColdMonitorFindsTheNewestTakeInAClosedGenerationWithEmptyHotFile() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    try fixture.fillBusWithEvidence(bytes: Int(OverlayChannelDeliveryReader.tailWindow) + 6_000_000)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    try pinSyntheticGeneration(fixture)
    XCTAssertEqual(try fixture.busSize(), 0)
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let first = try await reader.read()
    XCTAssertEqual(first.first?.stage, .sent)
    XCTAssertEqual(first.first?.isOpen, true)
    let bytes = await reader.consumedBytes
    XCTAssertLessThanOrEqual(bytes, OverlayChannelDeliveryReader.tailWindow)
  }

  // MARK: - Saved Conversation Search & Sorting Tests

  func testSavedConversationSearchNarrowsLiveAndSortsNewestOldestName() async throws {
    func makeConversation(
      name: String, provider: String, session: String, lease: String, dates: [String]
    ) -> OverlayConversation {
      let owner = OverlayConversationOwner(row: [
        "provider": provider,
        "provider_session_id": session,
        "lease_id": lease,
        "channel": "1",
        "name": name,
      ])!
      return OverlayConversation(
        id: owner.id, channel: "1", name: name, owner: owner,
        messages: dates.enumerated().map { index, stamp in
          .init(
            id: "\(name)-\(index)", kind: .reply, text: "Message \(name)", order: UInt64(index),
            emittedAt: stamp, owner: owner, recipients: [], deliveryID: nil, replyTo: nil,
            unsolicited: true, playback: nil, busPath: ""
          )
        }
      )
    }

    let alpha = makeConversation(
      name: "Alpha Project", provider: "claude", session: "sess-1",
      lease: String(repeating: "a", count: 32), dates: ["2026-10-01T10:00:00Z"])
    let beta = makeConversation(
      name: "Beta Architecture", provider: "codex", session: "sess-2",
      lease: String(repeating: "b", count: 32), dates: ["2026-10-10T12:00:00Z"])
    let gamma = makeConversation(
      name: "Gamma Research", provider: "gemini", session: "sess-3",
      lease: String(repeating: "c", count: 32), dates: ["2026-10-05T08:00:00Z"])

    struct MockClient: OverlayConversationSearchClient {
      let results: [OverlayConversationSearchResult]
      func search(rootPath: String, query: String) async throws -> OverlayConversationSearchResponse
      {
        OverlayConversationSearchResponse(results: results, complete: true, errorCount: 0)
      }
    }

    let client = MockClient(results: [
      OverlayConversationSearchResult(
        conversationID: alpha.id, lastActivity: "2026-10-01T10:00:00Z"),
      OverlayConversationSearchResult(
        conversationID: beta.id, lastActivity: "2026-10-10T12:00:00Z"),
    ])

    let searchModel = OverlayConversationSearchModel(client: client, debounceDuration: .zero)
    let view = OverlayChannelStatusView(
      channels: [], unavailable: false, palette: .dark, animates: false,
      conversations: [alpha, beta, gamma], searchModel: searchModel
    )

    // 1. Initial state (empty query): sort defaults to newest.
    XCTAssertEqual(view.savedConversations.map(\.id), [beta.id, gamma.id, alpha.id])
    XCTAssertEqual(searchModel.sortOrder, .newest)

    // 2. Sort by oldest:
    searchModel.sortOrder = .oldest
    XCTAssertEqual(view.savedConversations.map(\.id), [alpha.id, gamma.id, beta.id])

    // 3. Sort by name:
    searchModel.sortOrder = .name
    XCTAssertEqual(view.savedConversations.map(\.id), [alpha.id, beta.id, gamma.id])

    // 4. Perform search query narrowing live:
    await searchModel.searchImmediately(query: "Project")
    // Intersects exact saved owner IDs, narrowing to alpha and beta:
    searchModel.sortOrder = .newest
    XCTAssertEqual(view.savedConversations.map(\.id), [beta.id, alpha.id])

    searchModel.sortOrder = .oldest
    XCTAssertEqual(view.savedConversations.map(\.id), [alpha.id, beta.id])

    searchModel.sortOrder = .name
    XCTAssertEqual(view.savedConversations.map(\.id), [alpha.id, beta.id])
  }

  actor ControlledConversationSearchClient: OverlayConversationSearchClient {
    private var continuations:
      [String: [CheckedContinuation<OverlayConversationSearchResponse, any Error>]] = [:]
    private var cannedResponses: [String: Result<OverlayConversationSearchResponse, any Error>] =
      [:]
    private(set) var callLog: [String] = []

    func setResponse(for query: String, response: OverlayConversationSearchResponse) {
      cannedResponses[query] = .success(response)
    }

    func setError(for query: String, error: any Error) {
      cannedResponses[query] = .failure(error)
    }

    func search(rootPath: String, query: String) async throws -> OverlayConversationSearchResponse {
      callLog.append(query)
      if let canned = cannedResponses[query] {
        switch canned {
        case .success(let resp): return resp
        case .failure(let err): throw err
        }
      }
      return try await withCheckedThrowingContinuation { continuation in
        continuations[query, default: []].append(continuation)
      }
    }

    func hasPendingCall(for query: String) -> Bool {
      !(continuations[query]?.isEmpty ?? true)
    }

    func resumeFirst(for query: String, returning response: OverlayConversationSearchResponse) {
      if var list = continuations[query], !list.isEmpty {
        let cont = list.removeFirst()
        continuations[query] = list
        cont.resume(returning: response)
      }
    }

    func resumeFirst(for query: String, throwing error: any Error) {
      if var list = continuations[query], !list.isEmpty {
        let cont = list.removeFirst()
        continuations[query] = list
        cont.resume(throwing: error)
      }
    }

    func resumeLast(for query: String, returning response: OverlayConversationSearchResponse) {
      if var list = continuations[query], !list.isEmpty {
        let cont = list.removeLast()
        continuations[query] = list
        cont.resume(returning: response)
      }
    }

    func queryCount(_ query: String) -> Int {
      callLog.filter { $0 == query }.count
    }
  }

  func testSavedConversationSearchStaleCompletionIsDiscarded() async throws {
    let convA = OverlayConversation(
      id: "prov\0s1\0" + String(repeating: "1", count: 32), channel: "1", name: "Alpha", owner: nil,
      messages: [])
    let convB = OverlayConversation(
      id: "prov\0s2\0" + String(repeating: "2", count: 32), channel: "1", name: "Beta", owner: nil,
      messages: [])

    let client = ControlledConversationSearchClient()
    let searchModel = OverlayConversationSearchModel(client: client, debounceDuration: .zero)
    let view = OverlayChannelStatusView(
      channels: [], unavailable: false, palette: .dark, animates: false,
      conversations: [convA, convB], searchModel: searchModel
    )

    // Trigger slow search query first:
    let slowTask = Task { @MainActor in
      await searchModel.searchImmediately(query: "slow")
    }
    while await !client.hasPendingCall(for: "slow") {
      await Task.yield()
    }

    // Trigger fast search query immediately afterwards:
    await client.setResponse(
      for: "fast",
      response: OverlayConversationSearchResponse(
        results: [OverlayConversationSearchResult(conversationID: convB.id)],
        complete: true, errorCount: 0
      )
    )
    await searchModel.searchImmediately(query: "fast")
    XCTAssertEqual(view.savedConversations.map(\.id), [convB.id])

    // Now resume the slow query (which is stale):
    await client.resumeFirst(
      for: "slow",
      returning: OverlayConversationSearchResponse(
        results: [OverlayConversationSearchResult(conversationID: convA.id)],
        complete: true, errorCount: 0
      )
    )
    _ = await slowTask.value

    // Stale result for "slow" MUST be discarded; only latest "fast" completion paints:
    XCTAssertEqual(
      view.savedConversations.map(\.id), [convB.id],
      "Stale query completion must not overwrite latest query")
    XCTAssertEqual(searchModel.query, "fast")
  }

  func testSavedConversationSearchClearingRestoresListPreservingSelection() async throws {
    let leaseA = String(repeating: "a", count: 32)
    let leaseB = String(repeating: "b", count: 32)
    let ownerA = OverlayConversationOwner(row: [
      "provider": "codex", "provider_session_id": "sA", "lease_id": leaseA, "channel": "1",
      "name": "A",
    ])!
    let ownerB = OverlayConversationOwner(row: [
      "provider": "codex", "provider_session_id": "sB", "lease_id": leaseB, "channel": "2",
      "name": "B",
    ])!
    let convA = OverlayConversation(
      id: ownerA.id, channel: "1", name: "A", owner: ownerA, messages: [])
    let convB = OverlayConversation(
      id: ownerB.id, channel: "2", name: "B", owner: ownerB, messages: [])

    struct SingleMatchClient: OverlayConversationSearchClient {
      let matchID: String
      func search(rootPath: String, query: String) async throws -> OverlayConversationSearchResponse
      {
        OverlayConversationSearchResponse(
          results: [OverlayConversationSearchResult(conversationID: matchID)],
          complete: true, errorCount: 0
        )
      }
    }

    let searchModel = OverlayConversationSearchModel(
      client: SingleMatchClient(matchID: convB.id), debounceDuration: .zero)
    let view = OverlayChannelStatusView(
      channels: [], unavailable: false, palette: .dark, animates: false,
      conversations: [convA, convB], selectedConversationID: convA.id,
      searchModel: searchModel
    )

    XCTAssertEqual(view.selectedConversationID, convA.id)
    await searchModel.searchImmediately(query: "B")
    XCTAssertEqual(view.savedConversations.map(\.id), [convB.id])
    XCTAssertEqual(
      view.selectedConversationID, convA.id, "Searching does not auto-open or change selection")

    // Clear search
    searchModel.clear()
    XCTAssertEqual(searchModel.query, "")
    XCTAssertEqual(
      Set(view.savedConversations.map(\.id)), Set([convA.id, convB.id]),
      "Clearing restores full list")
    XCTAssertEqual(
      view.selectedConversationID, convA.id, "Clearing restores list without changing selection")
  }

  func testSavedConversationSearchTransitionsThroughPendingErrorPartialZeroAndCompleteZero()
    async throws
  {
    let convA = OverlayConversation(
      id: "prov\0s1\0" + String(repeating: "1", count: 32), channel: "1", name: "Alpha", owner: nil,
      messages: [])
    let convB = OverlayConversation(
      id: "prov\0s2\0" + String(repeating: "2", count: 32), channel: "1", name: "Beta", owner: nil,
      messages: [])

    let client = ControlledConversationSearchClient()
    let searchModel = OverlayConversationSearchModel(client: client, debounceDuration: .zero)
    let view = OverlayChannelStatusView(
      channels: [], unavailable: false, palette: .dark, animates: false,
      conversations: [convA, convB], searchModel: searchModel
    )

    // Initial state: empty query, full list rendered, no empty notice
    XCTAssertEqual(view.savedConversations.map(\.id), [convA.id, convB.id])
    XCTAssertFalse(searchModel.hasExhaustiveNoMatch(in: view.savedConversations))

    // Transition 1: Pending (loading)
    let pendingTask = Task { @MainActor in
      await searchModel.searchImmediately(query: "probe")
    }
    while await !client.hasPendingCall(for: "probe") {
      await Task.yield()
    }
    XCTAssertTrue(searchModel.isLoading)
    XCTAssertNil(searchModel.searchError)
    XCTAssertFalse(searchModel.isPartial)
    XCTAssertFalse(
      searchModel.hasExhaustiveNoMatch(in: view.savedConversations),
      "Pending/loading state must NEVER report exhaustive no-match"
    )

    // Transition 2: Thrown error
    await client.resumeFirst(for: "probe", throwing: CocoaError(.fileReadCorruptFile))
    _ = await pendingTask.value
    XCTAssertFalse(searchModel.isLoading)
    XCTAssertNotNil(searchModel.searchError)
    XCTAssertNotNil(searchModel.effectiveSearchError)
    XCTAssertFalse(searchModel.isPartial)
    XCTAssertTrue(view.savedConversations.isEmpty)
    XCTAssertFalse(
      searchModel.hasExhaustiveNoMatch(in: view.savedConversations),
      "Thrown error state must NEVER report exhaustive no-match"
    )

    // Transition 3: Partial zero (incomplete query returning 0 matches)
    let partialTask = Task { @MainActor in
      await searchModel.searchImmediately(query: "partial-zero")
    }
    while await !client.hasPendingCall(for: "partial-zero") {
      await Task.yield()
    }
    await client.resumeFirst(
      for: "partial-zero",
      returning: OverlayConversationSearchResponse(results: [], complete: false, errorCount: 1)
    )
    _ = await partialTask.value
    XCTAssertFalse(searchModel.isLoading)
    XCTAssertNil(searchModel.searchError)
    XCTAssertTrue(searchModel.isPartial)
    XCTAssertTrue(searchModel.isEffectivePartial)
    XCTAssertTrue(view.savedConversations.isEmpty)
    XCTAssertFalse(
      searchModel.hasExhaustiveNoMatch(in: view.savedConversations),
      "Partial zero-hit results must NEVER report exhaustive no-match"
    )

    // Transition 4: Complete zero (exhaustive zero-hit result)
    let completeTask = Task { @MainActor in
      await searchModel.searchImmediately(query: "complete-zero")
    }
    while await !client.hasPendingCall(for: "complete-zero") {
      await Task.yield()
    }
    await client.resumeFirst(
      for: "complete-zero",
      returning: OverlayConversationSearchResponse(results: [], complete: true, errorCount: 0)
    )
    _ = await completeTask.value
    XCTAssertFalse(searchModel.isLoading)
    XCTAssertNil(searchModel.searchError)
    XCTAssertFalse(searchModel.isPartial)
    XCTAssertFalse(searchModel.isEffectivePartial)
    XCTAssertTrue(view.savedConversations.isEmpty)
    XCTAssertTrue(
      searchModel.hasExhaustiveNoMatch(in: view.savedConversations),
      "Successful complete query with zero hits reports definitive no-match"
    )
    XCTAssertTrue(searchModel.isExhaustiveZeroMatch)

    // Transition 5: Clear restores list immediately
    searchModel.clear()
    XCTAssertEqual(searchModel.query, "")
    XCTAssertFalse(searchModel.isLoading)
    XCTAssertNil(searchModel.searchError)
    XCTAssertFalse(searchModel.isPartial)
    XCTAssertEqual(view.savedConversations.map(\.id), [convA.id, convB.id])
    XCTAssertFalse(searchModel.hasExhaustiveNoMatch(in: view.savedConversations))
  }

  func testSavedConversationSearchRetainsVerifiedPartialHits() async throws {
    let convA = OverlayConversation(
      id: "prov\0s1\0" + String(repeating: "1", count: 32), channel: "1", name: "Alpha", owner: nil,
      messages: [])
    let convB = OverlayConversation(
      id: "prov\0s2\0" + String(repeating: "2", count: 32), channel: "1", name: "Beta", owner: nil,
      messages: [])

    let client = ControlledConversationSearchClient()
    await client.setResponse(
      for: "alpha",
      response: OverlayConversationSearchResponse(
        results: [OverlayConversationSearchResult(conversationID: convA.id)],
        complete: false,
        errorCount: 2
      )
    )
    let searchModel = OverlayConversationSearchModel(client: client, debounceDuration: .zero)
    let view = OverlayChannelStatusView(
      channels: [], unavailable: false, palette: .dark, animates: false,
      conversations: [convA, convB], searchModel: searchModel
    )

    await searchModel.searchImmediately(query: "alpha")
    XCTAssertTrue(searchModel.isPartial)
    XCTAssertTrue(searchModel.isEffectivePartial)
    XCTAssertEqual(searchModel.errorCount, 2)
    XCTAssertEqual(
      view.savedConversations.map(\.id), [convA.id],
      "Verified partial hits must be retained and rendered"
    )
    XCTAssertFalse(
      searchModel.hasExhaustiveNoMatch(in: view.savedConversations),
      "Partial results with hits must not show empty matches"
    )
  }

  func testSavedConversationSearchMetadataSurfacesPartialAndErrorStatus() async throws {
    let convA = OverlayConversation(
      id: "prov\0s1\0" + String(repeating: "1", count: 32), channel: "1", name: "Alpha", owner: nil,
      messages: [])
    let convB = OverlayConversation(
      id: "prov\0s2\0" + String(repeating: "2", count: 32), channel: "1", name: "Beta", owner: nil,
      messages: [])

    let client = ControlledConversationSearchClient()
    let searchModel = OverlayConversationSearchModel(client: client, debounceDuration: .zero)
    let view = OverlayChannelStatusView(
      channels: [], unavailable: false, palette: .dark, animates: false,
      conversations: [convA, convB], searchModel: searchModel
    )

    // 1. Incomplete / partial metadata response
    await client.setResponse(
      for: "",
      response: OverlayConversationSearchResponse(
        results: [
          OverlayConversationSearchResult(
            conversationID: convA.id,
            lastActivity: "2026-10-10T12:00:00Z"
          )
        ],
        complete: false,
        errorCount: 2
      )
    )
    await searchModel.loadArchiveMetadata()

    XCTAssertTrue(searchModel.isMetadataPartial)
    XCTAssertEqual(searchModel.metadataErrorCount, 2)
    XCTAssertNil(searchModel.metadataError)
    XCTAssertNil(searchModel.searchError)
    XCTAssertFalse(searchModel.isPartial)
    // When query is empty, effective partial surfaces metadata partial status:
    XCTAssertTrue(searchModel.isEffectivePartial)
    XCTAssertNil(searchModel.effectiveSearchError)
    XCTAssertNil(
      searchModel.matchedConversationIDs,
      "Metadata load must never clobber active query filter"
    )
    XCTAssertEqual(view.savedConversations.count, 2)
    XCTAssertFalse(searchModel.hasExhaustiveNoMatch(in: view.savedConversations))

    // 2. Failed metadata response (thrown error)
    await client.setError(for: "", error: CocoaError(.fileReadNoSuchFile))
    await searchModel.loadArchiveMetadata()

    XCTAssertFalse(searchModel.isMetadataPartial)
    XCTAssertNotNil(searchModel.metadataError)
    XCTAssertNil(searchModel.searchError)
    // When query is empty, effective error surfaces metadata error:
    XCTAssertNotNil(searchModel.effectiveSearchError)
    XCTAssertFalse(searchModel.isEffectivePartial)
    XCTAssertNil(
      searchModel.matchedConversationIDs,
      "Metadata error must never empty saved conversations list"
    )
    XCTAssertEqual(view.savedConversations.count, 2)
    XCTAssertFalse(searchModel.hasExhaustiveNoMatch(in: view.savedConversations))

    // 3. Active query takes precedence over background metadata status
    await client.setResponse(
      for: "beta",
      response: OverlayConversationSearchResponse(
        results: [OverlayConversationSearchResult(conversationID: convB.id)],
        complete: true,
        errorCount: 0
      )
    )
    await searchModel.searchImmediately(query: "beta")
    XCTAssertNil(searchModel.searchError)
    XCTAssertFalse(searchModel.isPartial)
    // Active query is complete and error-free: effective status reflects active query:
    XCTAssertNil(searchModel.effectiveSearchError)
    XCTAssertFalse(searchModel.isEffectivePartial)
    XCTAssertEqual(view.savedConversations.map(\.id), [convB.id])

    // 4. Clear search restores full list immediately without losing selection
    searchModel.clear()
    XCTAssertEqual(searchModel.query, "")
    XCTAssertNil(searchModel.matchedConversationIDs)
    XCTAssertEqual(view.savedConversations.count, 2)
  }

  func testSavedConversationSearchStaleMetadataCompletionIsDiscarded() async throws {
    let conv = OverlayConversation(
      id: "prov\0s1\0" + String(repeating: "1", count: 32), channel: "1", name: "Alpha", owner: nil,
      messages: [])

    let client = ControlledConversationSearchClient()
    let searchModel = OverlayConversationSearchModel(client: client, debounceDuration: .zero)

    // Trigger metadata load 1 (slow)
    let task1 = Task { @MainActor in
      await searchModel.loadArchiveMetadata()
    }
    while await !client.hasPendingCall(for: "") {
      await Task.yield()
    }

    // Trigger metadata load 2 (fast/newer)
    let task2 = Task { @MainActor in
      await searchModel.loadArchiveMetadata()
    }
    while await client.queryCount("") < 2 {
      await Task.yield()
    }

    // Resume load 2 with newer date (Oct 25)
    await client.resumeLast(
      for: "",
      returning: OverlayConversationSearchResponse(
        results: [
          OverlayConversationSearchResult(
            conversationID: conv.id,
            lastActivity: "2026-10-25T12:00:00Z"
          )
        ],
        complete: true,
        errorCount: 0
      )
    )
    _ = await task2.value
    let dateAfterTask2 = searchModel.backendLastActivityDates[conv.id]
    XCTAssertNotNil(dateAfterTask2)

    // Now resume load 1 (stale) with older date (Oct 01)
    await client.resumeFirst(
      for: "",
      returning: OverlayConversationSearchResponse(
        results: [
          OverlayConversationSearchResult(
            conversationID: conv.id,
            lastActivity: "2026-10-01T12:00:00Z"
          )
        ],
        complete: true,
        errorCount: 0
      )
    )
    _ = await task1.value

    // Date from newer load 2 must NOT be overwritten by stale load 1:
    XCTAssertEqual(
      searchModel.backendLastActivityDates[conv.id], dateAfterTask2,
      "Stale metadata completion must not overwrite date cache from newer metadata load"
    )
  }

  func testOlderMetadataCannotOverwriteDateFromNewerSearchSnapshot() async throws {
    let client = ControlledConversationSearchClient()
    let model = OverlayConversationSearchModel(client: client, debounceDuration: .zero)

    // 1. Metadata search starts first (snapshot sequence 1)
    let metadataTask = Task { @MainActor in
      await model.loadArchiveMetadata()
    }
    while await !client.hasPendingCall(for: "") {
      await Task.yield()
    }

    // 2. Query search starts second (snapshot sequence 2)
    let searchTask = Task { @MainActor in
      await model.searchImmediately(query: "needle")
    }
    while await !client.hasPendingCall(for: "needle") {
      await Task.yield()
    }

    // 3. Complete newer query search first with Oct 12 date
    await client.resumeFirst(
      for: "needle",
      returning: OverlayConversationSearchResponse(
        results: [
          OverlayConversationSearchResult(
            conversationID: "owner",
            lastActivity: "2026-10-12T08:00:00Z"
          )
        ],
        complete: true,
        errorCount: 0
      )
    )
    _ = await searchTask.value
    XCTAssertEqual(
      model.backendLastActivityDates["owner"],
      MessageTimestamp.date(fromISO8601: "2026-10-12T08:00:00Z")
    )
    XCTAssertEqual(model.matchedConversationIDs, Set(["owner"]))

    // 4. Complete older metadata search second with Oct 01 for "owner" and Oct 05 for "nonmatching"
    await client.resumeFirst(
      for: "",
      returning: OverlayConversationSearchResponse(
        results: [
          OverlayConversationSearchResult(
            conversationID: "owner",
            lastActivity: "2026-10-01T08:00:00Z"
          ),
          OverlayConversationSearchResult(
            conversationID: "nonmatching",
            lastActivity: "2026-10-05T08:00:00Z"
          ),
        ],
        complete: true,
        errorCount: 0
      )
    )
    _ = await metadataTask.value

    // Older metadata must NOT overwrite date for "owner" from newer search snapshot:
    XCTAssertEqual(
      model.backendLastActivityDates["owner"],
      MessageTimestamp.date(fromISO8601: "2026-10-12T08:00:00Z"),
      "An older background snapshot must not replace a newer query's verified activity date"
    )
    // Nonmatching owner from metadata snapshot MUST be preserved:
    XCTAssertEqual(
      model.backendLastActivityDates["nonmatching"],
      MessageTimestamp.date(fromISO8601: "2026-10-05T08:00:00Z"),
      "Nonmatching owners from background metadata must be preserved"
    )
    // Active query filter remains intact:
    XCTAssertEqual(model.matchedConversationIDs, Set(["owner"]))

    // 5. Subsequent newer metadata search (snapshot sequence 3) CAN update the date earlier
    let newerMetadataTask = Task { @MainActor in
      await model.loadArchiveMetadata()
    }
    while await !client.hasPendingCall(for: "") {
      await Task.yield()
    }
    await client.resumeFirst(
      for: "",
      returning: OverlayConversationSearchResponse(
        results: [
          OverlayConversationSearchResult(
            conversationID: "owner",
            lastActivity: "2026-10-08T08:00:00Z"
          )
        ],
        complete: true,
        errorCount: 0
      )
    )
    _ = await newerMetadataTask.value
    XCTAssertEqual(
      model.backendLastActivityDates["owner"],
      MessageTimestamp.date(fromISO8601: "2026-10-08T08:00:00Z"),
      "A newer authoritative metadata snapshot may update date earlier"
    )
  }

  func testSavedConversationSearchUnloadedHistoriesUseBackendDatesForSorting() async throws {
    func makeConv(id: String, name: String, dates: [String], loaded: Bool) -> OverlayConversation {
      var conv = OverlayConversation(
        id: id, channel: "1", name: name, owner: nil,
        messages: dates.enumerated().map { index, stamp in
          .init(
            id: "\(id)-\(index)", kind: .reply, text: "Text", order: UInt64(index),
            emittedAt: stamp, owner: nil, recipients: [], deliveryID: nil, replyTo: nil,
            unsolicited: true, playback: nil, busPath: ""
          )
        }
      )
      conv.historyLoaded = loaded
      return conv
    }

    let loadedOld = makeConv(
      id: "loaded-old", name: "Loaded Old", dates: ["2026-10-02T10:00:00Z"], loaded: true)
    let loadedNew = makeConv(
      id: "loaded-new", name: "Loaded New", dates: ["2026-10-12T10:00:00Z"], loaded: true)
    let unloadedArchive = makeConv(
      id: "unloaded-archive", name: "Unloaded Archive", dates: [], loaded: false)

    struct DateProvidingClient: OverlayConversationSearchClient {
      func search(rootPath: String, query: String) async throws -> OverlayConversationSearchResponse
      {
        OverlayConversationSearchResponse(
          results: [
            OverlayConversationSearchResult(
              conversationID: "loaded-old", lastActivity: "2026-10-02T10:00:00Z"),
            OverlayConversationSearchResult(
              conversationID: "unloaded-archive", lastActivity: "2026-10-08T15:00:00Z"),
            OverlayConversationSearchResult(
              conversationID: "loaded-new", lastActivity: "2026-10-12T10:00:00Z"),
          ],
          complete: true, errorCount: 0
        )
      }
    }

    let searchModel = OverlayConversationSearchModel(
      client: DateProvidingClient(), debounceDuration: .zero)
    let view = OverlayChannelStatusView(
      channels: [], unavailable: false, palette: .dark, animates: false,
      conversations: [loadedOld, unloadedArchive, loadedNew], searchModel: searchModel
    )

    // Before search response: unloaded archive has no date, so it sorts last:
    XCTAssertEqual(
      view.savedConversations.map(\.id), [loadedNew.id, loadedOld.id, unloadedArchive.id])

    // After search provides verified backend last_activity date:
    await searchModel.searchImmediately(query: "test")
    // Now unloaded archive (Oct 08) sorts between Oct 12 and Oct 02!
    XCTAssertEqual(
      view.savedConversations.map(\.id),
      [loadedNew.id, unloadedArchive.id, loadedOld.id],
      "Source response dates fix ordering of unloaded histories"
    )
  }

  func testSavedConversationSearchExactOwnerNavigationWithoutMutatingComposer() {
    let lease = "0123456789abcdef0123456789abcdef"
    let owner = OverlayConversationOwner(row: [
      "provider": "codex", "provider_session_id": "ses-live", "lease_id": lease,
      "channel": "4", "name": "Astra",
    ])!
    let conv = OverlayConversation(
      id: owner.id, channel: "4", name: "Astra", owner: owner, messages: [])

    var selectedID: String? = nil
    let view = OverlayChannelStatusView(
      channels: [], unavailable: false, palette: .dark, animates: false,
      conversations: [conv],
      onSelectConversation: { selectedID = $0 }
    )

    view.onSelectConversation?(conv.id)
    XCTAssertEqual(selectedID, owner.id)
    XCTAssertEqual(selectedID, "codex\0ses-live\0\(lease)")
  }

  func testSavedConversationSearchLocalizationCatalogsContainAllKeysInEnAndPl() {
    let keys = [
      "Search conversations…",
      "Newest first",
      "Oldest first",
      "By name",
      "Sort order",
      "No matching conversations",
      "Search failed",
      "Partial results",
      "Clear search",
    ]
    let bundle = Bundle.main
    if let polishPath = bundle.path(forResource: "pl", ofType: "lproj"),
      let polishBundle = Bundle(path: polishPath)
    {
      for key in keys {
        let localized = polishBundle.localizedString(forKey: key, value: nil, table: nil)
        XCTAssertNotEqual(localized, key, "Key '\(key)' should be translated into Polish")
        XCTAssertFalse(localized.isEmpty)
      }
    }
    for key in keys {
      let localized = bundle.localizedString(forKey: key, value: nil, table: nil)
      XCTAssertFalse(localized.isEmpty)
    }
  }

  private struct Fixture {
    static let leaseID = "0123456789abcdef0123456789abcdef"
    static let firstID = "b67e9660afb01eb53ab2de50"
    let root: URL
    var bus: URL { root.appendingPathComponent("fixture.jsonl") }
    init() throws {
      root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      try FileManager.default.createDirectory(
        at: root.appendingPathComponent("leases"), withIntermediateDirectories: true)
      try Data().write(to: bus)
      try bind()
      try lease(pending: [])
    }
    func remove() { try? FileManager.default.removeItem(at: root) }
    func write(_ value: [String: Any], to url: URL) throws {
      try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]).write(
        to: url, options: .atomic)
    }
    func bind(session: String = "agent-session") throws {
      try write(
        [
          "schema": "vc.agent-audience-binding.v1",
          "bindings": [
            "1": [
              "audience": "james", "provider": "codex", "provider_session_id": session,
            ]
          ],
        ], to: root.appendingPathComponent("vc.agent-audience-binding.v1.json"))
    }
    func lease(pending: [[String: Any]]) throws {
      try write(
        [
          "schema": "codescribe.agent-bridge.lease.v1", "lease_id": Self.leaseID,
          "provider": "codex", "provider_session_id": "agent-session", "bus": bus.path,
          "pending": pending,
        ], to: root.appendingPathComponent("leases/\(Self.leaseID).json"))
    }
    func marker(_ id: String) throws {
      let directory = root.appendingPathComponent("acknowledgments/\(Self.leaseID)")
      try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      try write(
        ["lease_id": Self.leaseID, "delivery_id": id],
        to: directory.appendingPathComponent("\(id).json"))
    }
    func append(_ row: [String: Any]) throws {
      var bytes = try JSONSerialization.data(withJSONObject: row)
      bytes.append(10)
      try appendBytes(bytes)
    }
    func appendBytes(_ bytes: Data) throws {
      let file = try FileHandle(forWritingTo: bus)
      defer { try? file.close() }
      try file.seekToEnd()
      try file.write(contentsOf: bytes)
    }
    func busSize() throws -> UInt64 {
      let attributes = try FileManager.default.attributesOfItem(atPath: bus.path)
      return (attributes[.size] as? NSNumber)?.uint64Value ?? 0
    }
    func cursorOffsets() throws -> [String: UInt64] {
      let data = try Data(contentsOf: OverlayDeliveryCursorStore.url(root: root))
      guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
        let buses = object["buses"] as? [String: [String: Any]]
      else { return [:] }
      return buses.compactMapValues { ($0["offset"] as? NSNumber)?.uint64Value }
    }
    /// Bulk evidence rows with a 30 KB pseudo-random payload each, the same
    /// shape that filled the Founder's bus, so the tail-window tests measure
    /// the real parse path instead of toy rows.
    func fillBusWithEvidence(bytes target: Int) throws {
      let file = try FileHandle(forWritingTo: bus)
      defer { try? file.close() }
      try file.seekToEnd()
      var total = 0
      var sequence = 0
      while total < target {
        sequence += 1
        let row = Self.evidenceRow(sequence: sequence)
        try file.write(contentsOf: row)
        total += row.count
      }
    }
    /// Atomic write replaces the file (new inode), exactly what a bus rotation does.
    func replaceBusWithEvidence(bytes target: Int, trailing rows: [[String: Any]]) throws {
      var data = Data()
      data.reserveCapacity(target + 1_000_000)
      var sequence = 0
      while data.count < target {
        sequence += 1
        data.append(Self.evidenceRow(sequence: sequence))
      }
      for row in rows {
        var bytes = try JSONSerialization.data(withJSONObject: row)
        bytes.append(10)
        data.append(bytes)
      }
      try data.write(to: bus, options: .atomic)
    }
    static let evidenceText: String = {
      let alphabet = Array("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789")
      var state: UInt64 = 0x243F_6A88_85A3_08D3
      var text = ""
      text.reserveCapacity(30_000)
      while text.count < 30_000 {
        state = state &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
        text.append(alphabet[Int(state >> 33) % alphabet.count])
      }
      return text
    }()
    static func evidenceRow(sequence: Int) -> Data {
      // A multiline literal excludes the line break before its closing
      // delimiter, so the row's newline is appended explicitly.
      let row = #"""
        {"schema":"codescribe.transcript-evidence.v1","session_id":"bulk","audience":"bulk-agent","sequence":\#(sequence),"document_index":0,"reducer_revision":7,"reducer_action":"record_ledger_terminal_seal","rendered_text":"\#(evidenceText)"}
        """#
      return Data((row + "\n").utf8)
    }
    func envelope(_ id: String) -> [String: Any] { ["kind": "seal", "delivery_id": id] }
    func recipient() -> [String: Any] {
      [
        "provider": "codex", "provider_session_id": "agent-session",
        "lease_id": Self.leaseID, "bus": bus.path, "channel": "1", "name": "james",
      ]
    }
    func seal(_ sequence: Int) -> [String: Any] {
      [
        "schema": "codescribe.transcript.v1", "session_id": "take-a", "sequence": sequence,
        "utterance_id": "u\(sequence)", "audience": "james", "status": "transcript_sealed",
        "text": "Iwo", "recipients": [recipient()],
      ]
    }
    func ack(_ id: String, channel: String = "1") -> [String: Any] {
      [
        "schema": "codescribe.agent-ack.v1", "kind": "agent_ack", "channel": channel,
        "agent": "james", "delivery_id": id,
      ]
    }
    func open() -> [String: Any] {
      [
        "schema": "codescribe.channel-session.v1", "kind": "channel_session", "channel": "1",
        "agent": "james", "state": "open", "loud": true, "session_id": "take-a",
        "provider": "codex", "provider_session_id": "agent-session",
        "opened_at": "2026-09-29T08:00:00Z",
      ]
    }
  }
}
