import CryptoKit
import Foundation
import XCTest

@testable import Codescribe

/// Exercises the generated Swift → Rust bridge with canonical archive files,
/// rather than a mock response or the overlay's bounded in-memory history.
@MainActor
final class OverlayBusSearchIntegrationTests: XCTestCase {
  func testOlderMetadataCannotOverwriteDateFromNewerSearchSnapshot() async {
    let client = SnapshotRaceClient()
    let model = OverlayConversationSearchModel(client: client, debounceDuration: .zero)
    let metadata = Task { await model.loadArchiveMetadata() }
    await client.waitForRequest("")
    let search = Task { await model.searchImmediately(query: "needle") }
    await client.waitForRequest("needle")
    await client.complete("needle", date: "2026-10-12T08:00:00Z")
    await search.value
    await client.complete("", date: "2026-10-01T08:00:00Z")
    await metadata.value
    XCTAssertEqual(
      model.backendLastActivityDates["owner"],
      MessageTimestamp.date(fromISO8601: "2026-10-12T08:00:00Z"),
      "An older background snapshot must not replace a newer query's verified activity date")
    XCTAssertEqual(model.matchedConversationIDs, Set(["owner"]))
  }

  func testBridgeFindsColdArchiveBeyondAllOverlayHistoryWindowsWithoutWriting() throws {
    let fixture = try ArchiveFixture()
    defer { fixture.remove() }
    var owners: [ArchiveFixture.Owner] = []
    for index in 0..<18 {
      let owner = fixture.owner("saved-\(index)")
      owners.append(owner)
      var rows: [[String: Any]] = []
      if index == 0 {
        rows.append(
          fixture.message(
            1, text: String(repeating: "x", count: 20_000) + " ŻÓŁW marker",
            owner: owner, date: "2026-10-01T08:00:00Z"))
        for number in 2...302 {
          rows.append(
            fixture.message(
              number, text: "Ordinary tail \(number)", owner: owner,
              date: "2026-10-02T08:00:00Z"))
        }
        rows.append(
          fixture.message(
            303, text: "foreign-owner-secret", owner: fixture.owner("foreign"),
            date: "2026-10-30T08:00:00Z"))
      } else {
        rows.append(
          fixture.message(
            1, text: "Archive body \(index)", owner: owner,
            date: String(format: "2026-10-%02dT08:00:00Z", index + 2)))
      }
      try fixture.archive(owner, rows: rows)
    }
    let before = try fixture.snapshot()
    let bridge = CodescribeThreads()
    let hits = try bridge.searchBusConversations(rootPath: fixture.root.path, query: "żółw MARKER")
    XCTAssertTrue(hits.complete)
    XCTAssertEqual(hits.errorCount, 0)
    XCTAssertEqual(hits.results.map(\.conversationId), [owners[0].id])
    XCTAssertEqual(
      hits.results.first.flatMap { MessageTimestamp.date(fromISO8601: $0.lastActivity) },
      MessageTimestamp.date(fromISO8601: "2026-10-02T08:00:00Z"))
    XCTAssertTrue(
      try bridge.searchBusConversations(
        rootPath: fixture.root.path, query: "foreign-owner-secret"
      ).results.isEmpty, "A mixed bus must not leak another owner's text into its archive")
    XCTAssertTrue(
      try bridge.searchBusConversations(rootPath: fixture.root.path, query: "zolw marker")
        .results.isEmpty, "Accents remain meaningful in the shared matcher")
    let all = try bridge.searchBusConversations(rootPath: fixture.root.path, query: "")
    XCTAssertTrue(all.complete)
    XCTAssertEqual(all.results.count, 18)
    XCTAssertEqual(all.results.first?.conversationId, owners[17].id)
    XCTAssertEqual(all.results.last?.conversationId, owners[0].id)
    XCTAssertEqual(try fixture.snapshot(), before, "Search must not ACK or mutate canonical files")
  }

  func testBridgeRetainsVerifiedHitsAndReportsIncompleteHistory() throws {
    let fixture = try ArchiveFixture()
    defer { fixture.remove() }
    let readable = fixture.owner("readable")
    let bus = try fixture.archive(
      readable,
      rows: [
        fixture.message(1, text: "retained hit", owner: readable, date: "2026-10-12T08:00:00Z")
      ])
    let handle = try FileHandle(forWritingTo: bus)
    try handle.seekToEnd()
    try handle.write(contentsOf: Data("{incomplete-json\n".utf8))
    try handle.close()
    let missing = fixture.owner("missing-history")
    try fixture.writeArchive(missing, bus: fixture.root.appendingPathComponent("missing.jsonl"))
    let before = try fixture.snapshot()
    let response = try CodescribeThreads().searchBusConversations(
      rootPath: fixture.root.path, query: "retained HIT")
    XCTAssertFalse(response.complete)
    XCTAssertGreaterThanOrEqual(response.errorCount, 2)
    XCTAssertEqual(response.results.map(\.conversationId), [readable.id])
    XCTAssertEqual(try fixture.snapshot(), before)
  }

  func testBridgeSearchesCurrentDocumentRevisionDespiteStaleReplay() throws {
    let fixture = try ArchiveFixture()
    defer { fixture.remove() }
    let owner = fixture.owner("withdrawn")
    func revision(_ number: Int, text: String) -> [String: Any] {
      [
        "schema": "codescribe.transcript-evidence.v1", "session_id": "agent-channel-1-roundtrip",
        "audience": "fixture", "reducer_revision": number, "reducer_action": "revise",
        "rendered_text": text, "occurrence_session_id": "agent-channel-1-roundtrip",
        "capture_epoch": 1, "sample_start": 0, "sample_end": 20,
        "recipients": [owner.row], "emitted_at": "2026-10-12T08:00:00Z",
      ]
    }
    try fixture.archive(
      owner,
      rows: [
        revision(1, text: "withdrawn-only-phrase"), revision(3, text: "current-document-phrase"),
        revision(2, text: "withdrawn-only-phrase"),
      ])
    let before = try fixture.snapshot()
    let response = try CodescribeThreads().searchBusConversations(
      rootPath: fixture.root.path, query: "withdrawn-only-phrase")
    XCTAssertTrue(response.complete)
    XCTAssertTrue(response.results.isEmpty)
    let current = try CodescribeThreads().searchBusConversations(
      rootPath: fixture.root.path, query: "current-document-phrase")
    XCTAssertTrue(current.complete)
    XCTAssertEqual(current.results.map(\.conversationId), [owner.id])
    XCTAssertEqual(try fixture.snapshot(), before)
  }

  private actor SnapshotRaceClient: OverlayConversationSearchClient {
    private var pending:
      [String: CheckedContinuation<OverlayConversationSearchResponse, any Error>] = [:]
    private var arrivals: [String: CheckedContinuation<Void, Never>] = [:]

    func search(rootPath: String, query: String) async throws -> OverlayConversationSearchResponse {
      try await withCheckedThrowingContinuation { continuation in
        pending[query] = continuation
        arrivals.removeValue(forKey: query)?.resume()
      }
    }

    func waitForRequest(_ query: String) async {
      guard pending[query] == nil else { return }
      await withCheckedContinuation { arrivals[query] = $0 }
    }

    func complete(_ query: String, date: String) {
      pending.removeValue(forKey: query)?.resume(
        returning: OverlayConversationSearchResponse(
          results: [OverlayConversationSearchResult(conversationID: "owner", lastActivity: date)]))
    }
  }

  private struct ArchiveFixture {
    struct Owner {
      let session: String
      let lease: String
      var id: String { "codex\0\(session)\0\(lease)" }
      var row: [String: Any] {
        [
          "provider": "codex", "provider_session_id": session, "lease_id": lease,
          "channel": "1", "name": "Fixture \(session)",
        ]
      }
    }

    let root: URL

    init() throws {
      root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      try FileManager.default.createDirectory(
        at: root.appendingPathComponent("archives"), withIntermediateDirectories: true)
    }

    func remove() { try? FileManager.default.removeItem(at: root) }

    func owner(_ session: String) -> Owner {
      let digest = SHA256.hash(data: Data("codex\0\(session)".utf8))
      return Owner(
        session: session, lease: digest.prefix(16).map { String(format: "%02x", $0) }.joined())
    }

    func message(_ number: Int, text: String, owner: Owner, date: String) -> [String: Any] {
      [
        "schema": "codescribe.agent-user-message.v1", "kind": "agent_user_message",
        "message_id": String(format: "%024x", number), "text": text, "audience": "fixture",
        "recipients": [owner.row], "emitted_at": date,
      ]
    }

    @discardableResult
    func archive(_ owner: Owner, rows: [[String: Any]]) throws -> URL {
      let bus = root.appendingPathComponent("\(owner.lease).jsonl")
      var bytes = Data()
      for row in rows {
        bytes.append(try JSONSerialization.data(withJSONObject: row, options: [.sortedKeys]))
        bytes.append(10)
      }
      try bytes.write(to: bus)
      try writeArchive(owner, bus: bus)
      return bus
    }

    func writeArchive(_ owner: Owner, bus: URL) throws {
      var row = owner.row
      row["schema"] = "codescribe.agent-archive.v1"
      row["released"] = true
      row["bus"] = bus.path
      try JSONSerialization.data(withJSONObject: row, options: [.sortedKeys]).write(
        to: root.appendingPathComponent("archives/\(owner.lease)-1.json"))
    }

    func snapshot() throws -> [String: Data] {
      let paths = try FileManager.default.subpathsOfDirectory(atPath: root.path)
      var result: [String: Data] = [:]
      for path in paths {
        let url = root.appendingPathComponent(path)
        if try url.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile == true {
          result[path] = try Data(contentsOf: url)
        }
      }
      return result
    }
  }
}
