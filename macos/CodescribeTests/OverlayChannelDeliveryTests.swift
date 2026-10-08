import AppKit
import CryptoKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayChannelDeliveryTests: XCTestCase {
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

  func testAcknowledgedDeliveryStopsPollingWithoutAProviderAcceptanceReceipt() throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    var projection = OverlayChannelDelivery.Bus()
    projection.consume(fixture.seal(1))
    let coordinate = try XCTUnwrap(projection.receiptCoordinates().first)
    let owner = coordinate.0
    let delivery = coordinate.1
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
      view.range(of: "private var fullHeader", range: start.upperBound..<view.endIndex))
    let header = String(view[start.lowerBound..<end.lowerBound])
    XCTAssertFalse(header.contains("OverlayChannelStatusView("))
    let controls = try XCTUnwrap(view.range(of: "private func justifiedHeader(compact: Bool)"))
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
    XCTAssertFalse(status.contains("antenna.radiowaves"))
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
      XCTAssertGreaterThan(host.fittingSize.height, 120, "saved history is still visible")
      XCTAssertTrue(selections.isEmpty, "rendering does not navigate")
      XCTAssertTrue(toggles.isEmpty, "rendering does not start recording")
      XCTAssertLessThan(host.fittingSize.height, 220, "one compact row plus saved history")
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

  private func pinSyntheticGeneration(_ fixture: Fixture, day: String? = "2026_1002") throws {
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
    try fixture.write(
      [
        "schema": "codescribe.bus-generations.v1", "root": fixture.bus.path,
        "stream_id": "synthetic-stream", "stream_inode": inode, "stream_dev": dev,
        "stream_birthtime": NSNull(), "segments": [closed], "active": active, "pending": NSNull(),
      ],
      to: URL(fileURLWithPath: fixture.bus.path + ".generations.json"))
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
