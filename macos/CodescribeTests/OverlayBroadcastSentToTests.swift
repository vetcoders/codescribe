import Foundation
import XCTest

@testable import Codescribe

/// "Read X/Y" from `runtime/receipts/`: the overlay is the second
/// implementation of the receipts contract (`docs/TRANSCRIPT_BUS.md`, "One
/// contract, two implementations") and is pinned to the same model
/// directories as the helper, `scripts/tests/fixtures/receipts/<case>/`.
@MainActor
final class OverlayBroadcastSentToTests: XCTestCase {
  static let repoRoot = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
  static let modelDirectories = repoRoot.appendingPathComponent(
    "scripts/tests/fixtures/receipts", isDirectory: true)

  private static let schema = "codescribe.message-receipt.v1"
  private static let typedID = "0123456789abcdef00000201"
  private static let agentTextID = "0123456789abcdef00000202"
  private static let listedID = "0123456789abcdef00000203"
  private static let unstampedID = "0123456789abcdef00000204"

  /// A bridge home with four bound agents on one bus.
  private struct Fixture {
    let root: URL
    var cacheRoot: URL { root.appendingPathComponent("data.noindex", isDirectory: true) }
    var bus: URL { root.appendingPathComponent("fixture.jsonl") }
    let owners: [String: [String: Any]]

    init() throws {
      root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      try FileManager.default.createDirectory(
        at: root.appendingPathComponent("leases"), withIntermediateDirectories: true)
      try Data().write(to: bus)
      var owners: [String: [String: Any]] = [:]
      var bindings: [String: [String: Any]] = [:]
      for (name, channel, lease) in [
        ("lena", "3", String(repeating: "a", count: 32)),
        ("igor", "7", String(repeating: "b", count: 32)),
        ("kuba", "5", String(repeating: "c", count: 32)),
        ("nina", "2", String(repeating: "d", count: 32)),
      ] {
        let session = "\(name)-session"
        owners[name] = [
          "provider": "codex", "provider_session_id": session, "lease_id": lease,
          "bus": bus.path, "channel": channel, "name": name,
        ]
        bindings[channel] = ["audience": name, "provider": "codex", "provider_session_id": session]
        try Self.write(
          [
            "schema": "codescribe.agent-bridge.lease.v1", "lease_id": lease,
            "provider": "codex", "provider_session_id": session, "bus": bus.path,
            "pending": [],
          ], to: root.appendingPathComponent("leases/\(lease).json"))
      }
      self.owners = owners
      try Self.write(
        ["schema": "vc.agent-audience-binding.v1", "bindings": bindings],
        to: root.appendingPathComponent("vc.agent-audience-binding.v1.json"))
    }

    func remove() { try? FileManager.default.removeItem(at: root) }

    static func write(_ value: [String: Any], to url: URL) throws {
      try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]).write(
        to: url, options: .atomic)
    }

    func append(_ row: [String: Any]) throws {
      var bytes = try JSONSerialization.data(withJSONObject: row)
      bytes.append(10)
      let file = try FileHandle(forWritingTo: bus)
      defer { try? file.close() }
      try file.seekToEnd()
      try file.write(contentsOf: bytes)
    }

    func reach(_ names: [String]) -> [[String: Any]] {
      names.map { ["name": $0, "channel": owners[$0]?["channel"] ?? "9"] }
    }

    /// One channel-0 copy of a typed text, as `cs-bus --send-text --channel 0`
    /// publishes it per admitted recipient, with the frozen list stamped.
    func typedCopy(_ messageID: String, to name: String, sentTo: [String]?) -> [String: Any] {
      var row: [String: Any] = [
        "schema": "codescribe.agent-user-message.v1", "kind": "agent_user_message",
        "source": "typed", "message_id": messageID, "text": "Everyone, stop",
        "channel": "0", "origin_channel": "0", "audience": name,
        "recipients": [owners[name] ?? [:]], "emitted_at": "2026-10-26T02:06:00Z",
      ]
      if let sentTo {
        row["target"] = ["mode": "broadcast", "channel": "0", "recipients": reach(sentTo)]
      }
      return row
    }

    /// An agent's text to channel 0 (`cs-bus --send … --to 0`): one row, the
    /// recipients frozen at publication, the admitted list stamped.
    func agentText(_ messageID: String, from author: String, to names: [String], sentTo: [String])
      -> [String: Any]
    {
      [
        "schema": "codescribe.agent-reply.v1", "kind": "agent_reply", "reply_id": messageID,
        "text": "On my way", "sender": owners[author] ?? [:], "origin_channel": "0",
        "channel": "0", "peer_to": "0", "recipients": names.map { owners[$0] ?? [:] },
        "target": ["mode": "broadcast", "channel": "0", "recipients": reach(sentTo)],
        "emitted_at": "2026-10-26T04:14:00Z",
      ]
    }

    func directory(_ messageID: String) -> URL {
      root.appendingPathComponent("runtime/receipts/\(messageID)", isDirectory: true)
    }

    /// One receipt file as `record_message_receipt` writes it.
    func receipt(
      _ kind: String, messageID: String, delivery: String, owner name: String,
      lease: String? = nil, session: String? = nil, fileName: String? = nil
    ) throws {
      try FileManager.default.createDirectory(
        at: directory(messageID), withIntermediateDirectories: true)
      var row = owners[name] ?? [:]
      row["schema"] = OverlayBroadcastSentToTests.schema
      row["kind"] = kind
      row["message_id"] = messageID
      row["delivery_id"] = delivery
      if let lease { row["lease_id"] = lease }
      if let session { row["provider_session_id"] = session }
      switch kind {
      case "read": row["read_at"] = "2026-10-26T04:19:00Z"
      case "delivered":
        row["wake"] = "on"
        row["at"] = "2026-10-26T02:06:01Z"
      default:
        row["policy"] = "off"
        row["at"] = "2026-10-26T02:06:02Z"
      }
      try Self.write(row, to: directory(messageID).appendingPathComponent(fileName ?? "\(delivery).\(kind).json"))
    }

    func sent(_ messageID: String, to names: [String]) throws {
      try FileManager.default.createDirectory(
        at: directory(messageID), withIntermediateDirectories: true)
      try Self.write(
        [
          "schema": OverlayBroadcastSentToTests.schema, "kind": "sent", "message_id": messageID,
          "sent_to": reach(names), "bus": NSNull(), "at": "2026-10-26T02:06:00Z",
        ], to: directory(messageID).appendingPathComponent("sent.json"))
    }

    func playback(_ messageID: String, ticket: String, state: String, by author: String) throws {
      try FileManager.default.createDirectory(
        at: directory(messageID), withIntermediateDirectories: true)
      var row = owners[author] ?? [:]
      row.removeValue(forKey: "name")
      row.removeValue(forKey: "channel")
      row.removeValue(forKey: "bus")
      row["schema"] = OverlayBroadcastSentToTests.schema
      row["kind"] = "playback"
      row["message_id"] = messageID
      row["playback_ticket"] = ticket
      row["state"] = state
      row["reason"] = NSNull()
      row["spoken"] = state == "spoken"
      row["at"] = "2026-10-26T04:14:30Z"
      try Self.write(row, to: directory(messageID).appendingPathComponent("playback.\(ticket).json"))
    }
  }

  private func conversationMessage(
    _ id: String, in snapshot: OverlayChannelDeliverySnapshot
  ) throws -> OverlayConversationMessage {
    try XCTUnwrap(
      snapshot.conversations.first { $0.id == "0" }?.messages.first { $0.id == id },
      "channel 0 shows \(id)")
  }

  private func deliveries(of message: OverlayConversationMessage) throws -> [String: String] {
    var result: [String: String] = [:]
    for recipient in message.recipients {
      result[recipient.owner.name] = try XCTUnwrap(recipient.deliveryID)
    }
    return result
  }

  // MARK: Model directories

  func testTheModelDirectoriesPinTheExactStateOfBothImplementations() throws {
    let cases = try FileManager.default.contentsOfDirectory(
      at: Self.modelDirectories, includingPropertiesForKeys: [.isDirectoryKey]
    ).filter { (try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true }
      .sorted { $0.lastPathComponent < $1.lastPathComponent }
    XCTAssertGreaterThanOrEqual(cases.count, 7)
    XCTAssertTrue(
      Set(cases.map(\.lastPathComponent)).isSuperset(of: [
        "skipped-on-sent-to", "legacy-delivered-unread",
      ]))
    for directory in cases {
      let expected = try XCTUnwrap(
        JSONSerialization.jsonObject(
          with: Data(contentsOf: directory.appendingPathComponent("expected.json")))
          as? [String: Any], directory.lastPathComponent)
      let messageID = try XCTUnwrap(expected["message_id"] as? String)
      let receipts = directory.appendingPathComponent("runtime/receipts/\(messageID)")
      let files = try FileManager.default.contentsOfDirectory(atPath: receipts.path).map { name in
        (
          name: name,
          row: (try? JSONSerialization.jsonObject(
            with: Data(contentsOf: receipts.appendingPathComponent(name))) as? [String: Any])
            ?? [:]
        )
      }
      let composed = OverlayReceiptState.compose(files: files, messageID: messageID)
      XCTAssertTrue(
        NSDictionary(dictionary: composed).isEqual(to: expected),
        "\(directory.lastPathComponent): \(composed) != \(expected)")
      for (_, row) in files {
        XCTAssertNil(row["text"], "\(directory.lastPathComponent) carries no message text")
      }
    }
  }

  func testTheComposerSkipsWritesInProgressAndForeignFiles() {
    let messageID = String(repeating: "e", count: 24)
    let delivery = String(repeating: "1", count: 24)
    let read: [String: Any] = [
      "schema": Self.schema, "kind": "read", "message_id": messageID, "delivery_id": delivery,
      "name": "lena", "channel": "3", "read_at": "2026-10-26T04:19:00Z",
    ]
    let composed = OverlayReceiptState.compose(
      files: [
        (name: ".\(delivery).delivered.json.tmp", row: read),
        (name: "\(delivery).delivered.json", row: ["schema": "other", "kind": "delivered"]),
        (name: "\(delivery).read.json", row: read),
        (name: "sent.json", row: ["schema": Self.schema, "kind": "sent", "message_id": String(repeating: "f", count: 24)]),
      ], messageID: messageID)
    let recipients = composed["recipients"] as? [[String: Any]]
    XCTAssertEqual(recipients?.count, 1)
    XCTAssertEqual(recipients?.first?["state"] as? String, "read")
    XCTAssertTrue(composed["sent_to"] is NSNull, "a sent file of another message is ignored")
    XCTAssertEqual(composed["denominator"] as? Int, 1)
  }

  // MARK: Projection through the reader

  func testFourCopiesWithTwoAdmittedAgentsReadOneOfTwo() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    for name in ["lena", "igor", "kuba", "nina"] {
      try fixture.append(fixture.typedCopy(Self.typedID, to: name, sentTo: ["lena", "igor"]))
    }
    try fixture.append(
      fixture.agentText(
        Self.agentTextID, from: "igor", to: ["lena", "kuba", "nina"], sentTo: ["lena", "igor"]))
    let reader = OverlayChannelDeliveryReader(root: fixture.root, cacheRoot: fixture.cacheRoot)
    let initial = try await reader.readSnapshot()
    for id in ["typed:" + Self.typedID, "reply:" + Self.agentTextID] {
      let message = try message(id, in: initial)
      XCTAssertEqual(
        message.sentTo, [OverlayConversationReach(name: "lena", channel: "3"), OverlayConversationReach(name: "igor", channel: "7")],
        "the publisher's stamp is the admitted list")
      XCTAssertEqual(message.readReceipt, OverlayReadReceipt(read: 0, denominator: 2, unreceipted: []))
    }
    // Two delivered, one read, two skipped: the files the followers wrote.
    let typed = try deliveries(of: try conversationMessage("typed:" + Self.typedID, in: initial))
    XCTAssertEqual(typed.count, 4, "four copies, four recipients")
    try fixture.receipt("delivered", messageID: Self.typedID, delivery: typed["lena"]!, owner: "lena")
    try fixture.receipt("read", messageID: Self.typedID, delivery: typed["lena"]!, owner: "lena")
    try fixture.receipt("delivered", messageID: Self.typedID, delivery: typed["igor"]!, owner: "igor")
    try fixture.receipt("skipped", messageID: Self.typedID, delivery: typed["kuba"]!, owner: "kuba")
    try fixture.receipt("skipped", messageID: Self.typedID, delivery: typed["nina"]!, owner: "nina")
    let agent = try deliveries(of: try conversationMessage("reply:" + Self.agentTextID, in: initial))
    XCTAssertEqual(Set(agent.keys), ["lena", "kuba", "nina"], "the author is not a recipient")
    try fixture.receipt("read", messageID: Self.agentTextID, delivery: agent["lena"]!, owner: "lena")
    try fixture.receipt("skipped", messageID: Self.agentTextID, delivery: agent["kuba"]!, owner: "kuba")
    try fixture.receipt("skipped", messageID: Self.agentTextID, delivery: agent["nina"]!, owner: "nina")
    try fixture.playback(Self.agentTextID, ticket: String(repeating: "f", count: 24), state: "spoken", by: "igor")
    var snapshot = initial
    for _ in 0..<3 { snapshot = try await reader.readSnapshot() }

    let message = try conversationMessage("typed:" + Self.typedID, in: snapshot)
    XCTAssertEqual(message.readReceipt, OverlayReadReceipt(read: 1, denominator: 2, unreceipted: []))
    let states = Dictionary(
      uniqueKeysWithValues: message.recipients.map {
        ($0.owner.name, OverlayConversationView.receiptStatusText(for: $0))
      })
    XCTAssertEqual(
      states,
      ["lena": "Read", "igor": "Queued", "kuba": "Skipped: off channel 0", "nina": "Skipped: off channel 0"])
    let text = try conversationMessage("reply:" + Self.agentTextID, in: snapshot)
    XCTAssertEqual(
      text.readReceipt, OverlayReadReceipt(read: 1, denominator: 2, unreceipted: []),
      "igor is admitted and the author: listed, not a copy, never unread")
    XCTAssertEqual(text.playback?.state, "spoken")
    XCTAssertEqual(text.playback?.spoken, true)
    XCTAssertEqual(ReplyPlaybackCopy.label(state: "spoken", reason: nil), "Spoken")
  }

  func testAnAdmittedAgentWithoutAnyFileHasNoReceiptAndCounts() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    // Three admitted, two copies: hania's reader never ran.
    let hania = OverlayConversationReach(name: "hania", channel: "9")
    for name in ["lena", "igor"] {
      var row = fixture.typedCopy(Self.listedID, to: name, sentTo: ["lena", "igor"])
      row["target"] = [
        "mode": "broadcast", "channel": "0",
        "recipients": fixture.reach(["lena", "igor"]) + [["name": "hania", "channel": "9"]],
      ]
      try fixture.append(row)
    }
    let reader = OverlayChannelDeliveryReader(root: fixture.root, cacheRoot: fixture.cacheRoot)
    let initial = try await reader.readSnapshot()
    let copies = try deliveries(of: try conversationMessage("typed:" + Self.listedID, in: initial))
    try fixture.receipt("delivered", messageID: Self.listedID, delivery: copies["lena"]!, owner: "lena")
    try fixture.receipt("read", messageID: Self.listedID, delivery: copies["lena"]!, owner: "lena")
    try fixture.receipt("delivered", messageID: Self.listedID, delivery: copies["igor"]!, owner: "igor")
    var snapshot = initial
    for _ in 0..<3 { snapshot = try await reader.readSnapshot() }
    let message = try conversationMessage("typed:" + Self.listedID, in: snapshot)
    XCTAssertEqual(message.readReceipt, OverlayReadReceipt(read: 1, denominator: 3, unreceipted: [hania]))
    XCTAssertEqual(message.recipients.count, 2, "no copy is invented for hania")
    XCTAssertEqual(OverlayConversationView.noReceiptText, "No receipt")
  }

  func testReadComesFromTheReadFileNotTheMarkerAndAHandoverIsOneRecipient() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    for name in ["lena", "igor"] {
      try fixture.append(fixture.typedCopy(Self.typedID, to: name, sentTo: ["lena", "igor"]))
    }
    let reader = OverlayChannelDeliveryReader(root: fixture.root, cacheRoot: fixture.cacheRoot)
    let initial = try await reader.readSnapshot()
    let copies = try deliveries(of: try conversationMessage("typed:" + Self.typedID, in: initial))
    let lena = try XCTUnwrap(fixture.owners["lena"])
    // The old ACK marker, complete with its envelope, is not a source.
    let markers = fixture.root.appendingPathComponent(
      "acknowledgments/\(lena["lease_id"] as? String ?? "")")
    try FileManager.default.createDirectory(at: markers, withIntermediateDirectories: true)
    try Fixture.write(
      [
        "lease_id": lena["lease_id"] ?? "", "delivery_id": copies["lena"]!, "bus": fixture.bus.path,
        "provider": "codex", "provider_session_id": "lena-session",
        "read_at": "2026-10-26T04:19:00Z", "message_id": Self.typedID,
        "envelope": fixture.typedCopy(Self.typedID, to: "lena", sentTo: ["lena", "igor"]),
      ], to: markers.appendingPathComponent("\(copies["lena"]!).json"))
    // A write in progress at the receipt's own name is not a receipt either.
    try fixture.receipt(
      "read", messageID: Self.typedID, delivery: copies["lena"]!, owner: "lena",
      fileName: ".\(copies["lena"]!).read.json.tmp")
    var snapshot = initial
    for _ in 0..<3 { snapshot = try await reader.readSnapshot() }
    var message = try conversationMessage("typed:" + Self.typedID, in: snapshot)
    XCTAssertEqual(message.readReceipt, OverlayReadReceipt(read: 0, denominator: 2, unreceipted: []))
    XCTAssertFalse(message.recipients.contains(where: \.acknowledged))
    // The successor lease of the same name and channel acknowledges the
    // delivery its predecessor received: one file keyed by delivery, one
    // recipient, read.
    try fixture.receipt(
      "read", messageID: Self.typedID, delivery: copies["lena"]!, owner: "lena",
      lease: String(repeating: "e", count: 32), session: "lena-successor")
    for _ in 0..<3 { snapshot = try await reader.readSnapshot() }
    message = try conversationMessage("typed:" + Self.typedID, in: snapshot)
    XCTAssertEqual(message.recipients.count, 2)
    XCTAssertEqual(
      message.recipients.first { $0.owner.name == "lena" }?.acknowledged, true,
      "read.json settles the delivery whichever lease of the agent wrote it")
    XCTAssertEqual(message.readReceipt, OverlayReadReceipt(read: 1, denominator: 2, unreceipted: []))
  }

  func testACopyWithoutAStampKeepsTheCopiesAsTheDenominatorUntilSentJSON() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    for name in ["lena", "igor", "kuba"] {
      try fixture.append(fixture.typedCopy(Self.unstampedID, to: name, sentTo: nil))
    }
    let reader = OverlayChannelDeliveryReader(root: fixture.root, cacheRoot: fixture.cacheRoot)
    let initial = try await reader.readSnapshot()
    var message = try conversationMessage("typed:" + Self.unstampedID, in: initial)
    XCTAssertNil(message.sentTo, "an older row without the stamp has an unknown reach")
    XCTAssertEqual(message.readReceipt, OverlayReadReceipt(read: 0, denominator: 3, unreceipted: []))
    let copies = try deliveries(of: message)
    try fixture.receipt("skipped", messageID: Self.unstampedID, delivery: copies["kuba"]!, owner: "kuba")
    var snapshot = initial
    for _ in 0..<3 { snapshot = try await reader.readSnapshot() }
    message = try conversationMessage("typed:" + Self.unstampedID, in: snapshot)
    XCTAssertEqual(
      message.readReceipt, OverlayReadReceipt(read: 0, denominator: 2, unreceipted: []),
      "a skipped copy leaves the count even without a list")
    // The publisher's sent.json, read once, is the list.
    try fixture.sent(Self.unstampedID, to: ["lena", "igor"])
    for _ in 0..<3 { snapshot = try await reader.readSnapshot() }
    message = try conversationMessage("typed:" + Self.unstampedID, in: snapshot)
    XCTAssertEqual(
      message.sentTo, [OverlayConversationReach(name: "lena", channel: "3"), OverlayConversationReach(name: "igor", channel: "7")])
    XCTAssertEqual(message.readReceipt, OverlayReadReceipt(read: 0, denominator: 2, unreceipted: []))
  }
}
