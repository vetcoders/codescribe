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
    let log = DiagnosticLog(url: DiagnosticLog.url(churnDirectory: directory.path))
    log.append(
      category: "overlay-presentation", "frame.completed midi gen=7",
      at: Date(timeIntervalSince1970: 1_000_000_000.5))
    let text = try String(contentsOf: log.url, encoding: .utf8)
    XCTAssertEqual(
      text, "2001-09-09T01:46:40.500Z overlay-presentation frame.completed midi gen=7\n")
  }

  func testURLIsLogsDiagnosticLogAndFirstAppendCreatesTheDirectory() {
    let url = DiagnosticLog.url(churnDirectory: directory.path)
    XCTAssertEqual(url.path, directory.appendingPathComponent("logs/diagnostic.log").path)
    let logs = directory.appendingPathComponent("logs").path
    XCTAssertFalse(FileManager.default.fileExists(atPath: logs))
    DiagnosticLog(url: url).append(category: "launch", "first")
    XCTAssertTrue(FileManager.default.fileExists(atPath: url.path))
  }

  func testTwoAppendsLandInOrderOnOneFile() throws {
    let log = DiagnosticLog(url: DiagnosticLog.url(churnDirectory: directory.path))
    log.append(category: "a", "one")
    log.append(category: "b", "two")
    let lines = try log.lines()
    XCTAssertEqual(lines.count, 2)
    XCTAssertTrue(lines[0].hasSuffix(" a one"))
    XCTAssertTrue(lines[1].hasSuffix(" b two"))
  }

  func testOversizedFileRollsToGenerationOneAtOpen() throws {
    let url = DiagnosticLog.url(churnDirectory: directory.path)
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
}
