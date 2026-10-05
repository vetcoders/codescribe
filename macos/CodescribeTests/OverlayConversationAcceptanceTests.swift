import Foundation
import XCTest

@testable import Codescribe

final class OverlayConversationAcceptanceTests: XCTestCase {
  func testFollowerDeliveryIdentitySurvivesPrunedMailboxAndQuietRestart() throws {
    // Golden IDs produced by the canonical Python follower, including Unicode case folding.
    let cases: [(String, Bool, String)] = [
      ("Lena", false, "9655faaf3646b0a670acb8d8"),
      ("Lena", true, "cf7eb3fe3ae22e59c9624a0b"),
      ("Straße", false, "4c052a44a94110dc857c9df9"),
      ("Straße", true, "c24d69d2aadaf3054ebc3b6e"),
      ("*", false, "5065093fac94bf219c2b211e"),
      ("*", true, "b4433a2426097d2bd4c556a7"),
    ]
    for (audience, terminal, delivery) in cases {
      var bus = OverlayChannelDelivery.Bus()
      var row = occurrence(0, revision: 1)
      row["audience"] = audience
      if !terminal { row["reducer_action"] = "apply_manual_edit" }
      bus.consume(row)
      XCTAssertEqual(try all(bus).messages.first?.recipients.first?.deliveryID, delivery)
      bus = try JSONDecoder().decode(OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
      var ack = owner(leaseA)
      ack.merge(["schema": "codescribe.agent-ack.v1", "delivery_id": delivery]) { _, new in new }
      bus.consume(ack)
      bus.consume(reply(String(repeating: "d", count: 24), delivery: delivery))
      let messages = try all(bus).messages
      XCTAssertEqual(messages.first?.recipients.first?.acknowledged, true)
      XCTAssertEqual(messages.last?.replyTo, messages.first?.recipients.first?.deliveryID)
    }
  }

  func testTypedMessagesRetainDistinctIdentityAndCausalReceipt() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<5 {
      let identity = String(format: "%024x", index + 1)
      bus.consume([
        "schema": "codescribe.agent-user-message.v1", "kind": "agent_user_message",
        "message_id": identity, "text": "Iwo", "audience": "lena", "channel": "2",
        "recipients": [owner(leaseA)], "emitted_at": "2026-10-05T10:00:00Z",
      ])
    }
    let rows = try all(bus).messages
    XCTAssertEqual(rows.count, 5)
    XCTAssertEqual(Set(rows.map(\.id)).count, 5)
    let delivery = try XCTUnwrap(rows.first?.recipients.first?.deliveryID)
    bus.consume(reply(String(repeating: "d", count: 24), delivery: delivery))
    XCTAssertEqual(try all(bus).messages.last?.replyTo, delivery)
    bus = try JSONDecoder().decode(OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
    XCTAssertEqual(try all(bus).messages.count, 6)
  }

  private let busPath = "/fixture/transcript-events.jsonl"
  private let leaseA = String(repeating: "a", count: 32)
  private let leaseB = String(repeating: "b", count: 32)

  private func owner(_ lease: String, session: String = "agent-a", name: String = "Lena", channel: String = "2") -> [String: Any] {
    ["provider": "codex", "provider_session_id": session, "lease_id": lease,
     "name": name, "channel": channel]
  }

  private func occurrence(_ start: Int, revision: Int, text: String = "Iwo", recipients: [[String: Any]]? = nil) -> [String: Any] {
    ["schema": "codescribe.transcript-evidence.v1", "session_id": "agent-channel-2-take-a",
     "occurrence_session_id": "agent-channel-2-take-a", "capture_epoch": 1,
     "sample_start": start, "sample_end": start + 1600, "document_index": 0,
     "reducer_revision": revision, "sequence": revision,
     "reducer_action": "record_ledger_terminal_seal", "audience": "Lena",
     "rendered_text": text, "recipients": recipients ?? [owner(leaseA)]]
  }

  private func reply(_ replyID: String, delivery: String? = nil) -> [String: Any] {
    var row = owner(leaseA)
    row.merge(["schema": "codescribe.agent-reply.v1", "reply_id": replyID,
               "text": "Odpowiedź", "association": delivery == nil ? "unsolicited" : "addressed",
               "emitted_at": "2026-10-05T04:00:00Z"]) { _, new in new }
    if let delivery { row["delivery_id"] = delivery }
    return row
  }

  private func playback(_ replyID: String, ticket: String, state: String, time: String) -> [String: Any] {
    var row = owner(leaseA)
    row.merge(["schema": "codescribe.agent-reply-playback.v1", "reply_id": replyID,
               "playback_ticket": ticket, "state": state, "emitted_at": time,
               "spoken": state == "spoken"]) { _, new in new }
    return row
  }

  private func all(_ bus: OverlayChannelDelivery.Bus) throws -> OverlayConversation {
    try XCTUnwrap(bus.conversations(busPath: busPath).first { $0.channel == "0" })
  }

  func testFiveEqualLabelsInOneCaptureAndDocumentStayFiveRanges() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<5 { bus.consume(occurrence(index * 3200, revision: index + 1)) }
    let rows = try all(bus).messages
    XCTAssertEqual(rows.count, 5)
    XCTAssertEqual(Set(rows.map(\.id)).count, 5)
    XCTAssertEqual(rows.map(\.text), Array(repeating: "Iwo", count: 5))
    bus.consume(occurrence(0, revision: 10, text: "Iwo poprawione"))
    bus.consume(occurrence(0, revision: 1, text: "stara odpowiedź"))
    let revised = try all(bus).messages
    XCTAssertEqual(revised.count, 5)
    XCTAssertEqual(revised.first?.text, "Iwo poprawione")
    XCTAssertEqual(revised.map(\.id), rows.map(\.id))
  }

  func testBroadcastIsOneQuestionWithFrozenRecipientsAndNoLateInheritance() throws {
    var bus = OverlayChannelDelivery.Bus()
    let recipients = [owner(leaseA), owner(leaseB, session: "agent-b", name: "Adam", channel: "4")]
    var question = occurrence(0, revision: 1, recipients: recipients)
    question["audience"] = "*"
    bus.consume(question)
    XCTAssertEqual(try all(bus).messages.count, 1)
    XCTAssertEqual(try all(bus).messages.first?.recipients.count, 2)
    let leaseC = String(repeating: "c", count: 32)
    bus.observeLease(["schema": "codescribe.agent-bridge.lease.v1", "provider": "codex",
                      "provider_session_id": "agent-c", "lease_id": leaseC, "bus": busPath,
                      "pending": []], channel: "3",
                     binding: ["provider": "codex", "provider_session_id": "agent-c", "audience": "Astra"])
    let conversations = bus.conversations(busPath: busPath)
    XCTAssertEqual(conversations.filter { $0.owner != nil && !$0.messages.isEmpty }.count, 2)
    XCTAssertTrue(conversations.filter { $0.owner?.leaseID == leaseC }.allSatisfy { $0.messages.isEmpty })
    XCTAssertEqual(try all(bus).messages.first?.recipients.count, 2)
  }

  func testLateAcknowledgmentRetainsReplyAndForeignOwnerCannotAcknowledge() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(occurrence(0, revision: 1))
    let delivery = try XCTUnwrap(try all(bus).messages.first?.recipients.first?.deliveryID)
    bus.consume(reply(String(repeating: "d", count: 24), delivery: delivery))
    var foreign = owner(leaseB, session: "agent-b")
    foreign.merge(["schema": "codescribe.agent-ack.v1", "delivery_id": delivery]) { _, new in new }
    bus.consume(foreign)
    XCTAssertEqual(try all(bus).messages.first?.recipients.first?.acknowledged, false)
    var ack = owner(leaseA)
    ack.merge(["schema": "codescribe.agent-ack.v1", "delivery_id": delivery]) { _, new in new }
    bus.consume(ack)
    let rows = try all(bus).messages
    XCTAssertEqual(rows.count, 2)
    XCTAssertEqual(rows.first?.recipients.first?.acknowledged, true)
    XCTAssertEqual(rows.last?.text, "Odpowiedź")
    XCTAssertEqual(rows.last?.replyTo, delivery)
  }

  func testRebindingAndRenamingDoNotRelabelHistoricalReply() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(reply(String(repeating: "d", count: 24)))
    bus.observeLease(["schema": "codescribe.agent-bridge.lease.v1", "provider": "codex",
                      "provider_session_id": "agent-b", "lease_id": leaseB, "bus": busPath,
                      "pending": []], channel: "2",
                     binding: ["provider": "codex", "provider_session_id": "agent-b", "audience": "Nowy"])
    let old = try XCTUnwrap(bus.conversations(busPath: busPath).first { $0.owner?.leaseID == leaseA })
    XCTAssertEqual(old.name, "Lena")
    XCTAssertEqual(old.messages.first?.owner?.providerSessionID, "agent-a")
    XCTAssertTrue(bus.conversations(busPath: busPath).filter { $0.owner?.leaseID == leaseB }.allSatisfy { $0.messages.isEmpty })
  }

  func testPriorPlaybackTicketCannotCompleteOrRearmNewPlayback() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(reply(String(repeating: "d", count: 24)))
    bus.consume(playback(String(repeating: "d", count: 24), ticket: "old", state: "waiting", time: "2026-10-05T04:00:00Z"))
    bus.consume(playback(String(repeating: "d", count: 24), ticket: "new", state: "playing", time: "2026-10-05T04:00:02Z"))
    bus.consume(playback(String(repeating: "d", count: 24), ticket: "old", state: "spoken", time: "2026-10-05T04:00:03Z"))
    XCTAssertEqual(try all(bus).messages.last?.playback?.ticket, "new")
    bus.consume(playback(String(repeating: "d", count: 24), ticket: "old", state: "waiting", time: "2026-10-05T04:00:04Z"))
    XCTAssertEqual(try all(bus).messages.last?.playback?.ticket, "new")
    XCTAssertEqual(try all(bus).messages.last?.playback?.state, "playing")
  }

  func testProjectionRoundTripPreservesDistinctRowsAndCausalOwnership() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<5 { bus.consume(occurrence(index * 3200, revision: index + 1)) }
    let delivery = try XCTUnwrap(try all(bus).messages.last?.recipients.first?.deliveryID)
    bus.consume(reply(String(repeating: "d", count: 24), delivery: delivery))
    let restored = try JSONDecoder().decode(OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
    XCTAssertEqual(restored.conversations(busPath: busPath), bus.conversations(busPath: busPath))
  }

  func testRawObservationsCannotCreateConversationText() throws {
    var bus = OverlayChannelDelivery.Bus()
    var observation = occurrence(0, revision: 1)
    observation["schema"] = "codescribe.raw.v1"
    observation["kind"] = "correction"
    bus.consume(observation)
    XCTAssertTrue(try all(bus).messages.isEmpty)
  }

  func testHistoryRetainsOnlyTheLatestBoundedRows() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<300 { bus.consume(occurrence(index * 3200, revision: index + 1)) }
    let rows = try all(bus).messages
    XCTAssertEqual(rows.count, 256)
    XCTAssertEqual(rows.first?.order, 45)
    XCTAssertEqual(rows.last?.order, 300)
  }

  func testReplyIdentifierCannotMasqueradeAsLeaseIdentifier() throws {
    XCTAssertNil(OverlayConversationOwner(row: owner(String(repeating: "a", count: 24))))
    XCTAssertNotNil(OverlayConversationOwner(row: owner(leaseA)))
  }
}
