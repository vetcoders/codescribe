import XCTest

@testable import Codescribe

/// The diagnostic log is the persisted twin of the unified-log ledger: one
/// line per entry, UTC with milliseconds, created on first write, rolled once.
final class DiagnosticLogTests: XCTestCase {
  private var directory: URL!

  override func setUpWithError() throws {
    try super.setUpWithError()
    directory = FileManager.default.temporaryDirectory
      .appendingPathComponent("DiagnosticLogTests-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
  }

  override func tearDownWithError() throws {
    try? FileManager.default.removeItem(at: directory)
    try super.tearDownWithError()
  }

  func testAppendWritesOneLineWithUTCTimestampAndCategory() throws {
    let log = DiagnosticLog(url: DiagnosticLog.url(dataDirectory: directory.path))
    log.append(
      category: "overlay-presentation", "frame.completed midi gen=7",
      at: Date(timeIntervalSince1970: 1_000_000_000.5))
    let text = try String(contentsOf: log.url, encoding: .utf8)
    XCTAssertEqual(
      text, "2001-09-09T01:46:40.500Z overlay-presentation frame.completed midi gen=7\n")
  }

  func testURLIsLogsDiagnosticLogAndFirstAppendCreatesTheDirectory() {
    let url = DiagnosticLog.url(dataDirectory: directory.path)
    XCTAssertEqual(url.path, directory.appendingPathComponent("logs/diagnostic.log").path)
    let logs = directory.appendingPathComponent("logs").path
    XCTAssertFalse(FileManager.default.fileExists(atPath: logs))
    DiagnosticLog(url: url).append(category: "launch", "first")
    XCTAssertTrue(FileManager.default.fileExists(atPath: url.path))
  }

  func testTwoAppendsLandInOrderOnOneFile() throws {
    let log = DiagnosticLog(url: DiagnosticLog.url(dataDirectory: directory.path))
    log.append(category: "a", "one")
    log.append(category: "b", "two")
    let lines = try log.lines()
    XCTAssertEqual(lines.count, 2)
    XCTAssertTrue(lines[0].hasSuffix(" a one"))
    XCTAssertTrue(lines[1].hasSuffix(" b two"))
  }

  func testOversizedFileRollsToGenerationOneAtOpen() throws {
    let url = DiagnosticLog.url(dataDirectory: directory.path)
    try FileManager.default.createDirectory(
      at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    let old = String(repeating: "x", count: 200) + "\n"
    try old.write(to: url, atomically: true, encoding: .utf8)
    let log = DiagnosticLog(url: url, rollBytes: 64)
    log.append(category: "launch", "fresh")
    let rolled = url.appendingPathExtension("1")
    XCTAssertEqual(try String(contentsOf: rolled, encoding: .utf8), old)
    let lines = try log.lines()
    XCTAssertEqual(lines.count, 1)
    XCTAssertTrue(lines[0].hasSuffix(" launch fresh"))
  }

  func testInteractionJoinsActualCompletionToItsStart() throws {
    let log = DiagnosticLog(url: DiagnosticLog.url(dataDirectory: directory.path))
    let action = log.beginInteraction(action: "widget.show", surface: "tray")
    XCTAssertEqual(try interactionRows(log).map { $0["phase"] as? String }, ["started"])
    XCTAssertTrue(action.finish(.succeeded, effect: .stateChanged, cleanup: .notRequired))
    let rows = try interactionRows(log)
    XCTAssertEqual(rows.count, 2)
    XCTAssertEqual(rows[0]["actionID"] as? String, rows[1]["actionID"] as? String)
    XCTAssertEqual(rows[0]["runID"] as? String, rows[1]["runID"] as? String)
    XCTAssertEqual(rows[1]["outcome"] as? String, "succeeded")
    XCTAssertEqual(rows[1]["effect"] as? String, "stateChanged")
    XCTAssertEqual(rows[1]["cleanup"] as? String, "notRequired")
    XCTAssertGreaterThanOrEqual(try XCTUnwrap(rows[1]["durationMilliseconds"] as? Double), 0)
  }

  func testRacingCancelAndResultProduceExactlyOneTerminal() throws {
    let log = DiagnosticLog(url: DiagnosticLog.url(dataDirectory: directory.path))
    let action = log.beginInteraction(action: "agent.send", surface: "composer")
    DispatchQueue.concurrentPerform(iterations: 100) { index in
      action.finish(
        index.isMultiple(of: 2) ? .cancelled : .succeeded,
        effect: .callbackCompleted, cleanup: .complete)
    }
    let rows = try interactionRows(log)
    XCTAssertEqual(rows.count, 2)
    XCTAssertEqual(rows.filter { $0["phase"] as? String == "completed" }.count, 1)
    XCTAssertFalse(action.finish(.failed, effect: .unverified, cleanup: .unverified))
  }

  func testDroppedUnfinishedWorkIsNeverReportedAsSuccess() throws {
    let log = DiagnosticLog(url: DiagnosticLog.url(dataDirectory: directory.path))
    do {
      let action = log.beginInteraction(action: "settings.save", surface: "settings")
      XCTAssertNotNil(action.id)
    }
    let rows = try interactionRows(log)
    XCTAssertEqual(rows.count, 2)
    XCTAssertEqual(rows.last?["outcome"] as? String, "unverified")
    XCTAssertEqual(rows.last?["reason"] as? String, "lifetimeEnded")
    XCTAssertEqual(rows.last?["cleanup"] as? String, "unverified")
  }

  func testConcurrentActionsRemainDistinctAndEachStartPrecedesItsResult() throws {
    let log = DiagnosticLog(url: DiagnosticLog.url(dataDirectory: directory.path))
    DispatchQueue.concurrentPerform(iterations: 50) { _ in
      let action = log.beginInteraction(action: "fixture.callback", surface: "fixture")
      action.finish(.succeeded, effect: .callbackCompleted, cleanup: .complete)
    }
    let rows = try interactionRows(log)
    XCTAssertEqual(rows.count, 100)
    let grouped = Dictionary(grouping: rows) { $0["actionID"] as? String }
    XCTAssertEqual(grouped.count, 50)
    for events in grouped.values {
      XCTAssertEqual(events.map { $0["phase"] as? String }, ["started", "completed"])
    }
  }

  func testRetriesHaveDistinctIdentityAndOnlyContentFreeFields() throws {
    let log = DiagnosticLog(url: DiagnosticLog.url(dataDirectory: directory.path))
    let first = log.beginInteraction(action: "agent.send", surface: "composer")
    first.finish(.refused, effect: .noChange, cleanup: .notRequired, reason: .busy)
    let retry = log.beginInteraction(action: "agent.send", surface: "composer")
    retry.finish(.failed, effect: .unverified, cleanup: .complete, reason: .operationFailed)
    XCTAssertNotEqual(first.id, retry.id)
    let allowed: Set<String> = [
      "schema", "runID", "actionID", "action", "surface", "phase", "durationMilliseconds",
      "outcome", "effect", "cleanup", "reason",
    ]
    for row in try interactionRows(log) {
      XCTAssertTrue(Set(row.keys).isSubset(of: allowed))
      XCTAssertEqual(row["schema"] as? String, "codescribe.interaction.v1")
      XCTAssertNotNil(UUID(uuidString: try XCTUnwrap(row["actionID"] as? String)))
    }
    let attrs = try FileManager.default.attributesOfItem(atPath: log.url.path)
    XCTAssertEqual((attrs[.posixPermissions] as? NSNumber)?.intValue, 0o600)
  }

  private func interactionRows(_ log: DiagnosticLog) throws -> [[String: Any]] {
    try log.lines().map { line in
      let marker = try XCTUnwrap(line.range(of: " interaction "))
      let data = Data(line[marker.upperBound...].utf8)
      return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }
  }
}
