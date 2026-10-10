import XCTest

@testable import Codescribe

final class DictionaryWordPinsTests: XCTestCase {
  private func fixture(_ lines: String, session: String = "take-a") throws -> URL {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    let directory = root.appendingPathComponent("sessions")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    try Data(lines.utf8).write(to: directory.appendingPathComponent("\(session).slots.jsonl"))
    addTeardownBlock { try FileManager.default.removeItem(at: root) }
    return root
  }

  private func row(text: String = "osiem", start: Int = 100, end: Int = 200) -> String {
    """
    {"schema":"codescribe.occurrence_slots.v1","session":"take-a","capture_epoch":1,"sample_start":0,"sample_end":1000,"sample_rate_hz":1000,"slots":[{"producer":"whisper","sample_start":\(start),"sample_end":\(end),"text":"\(text)","observation":{"producer":"whisper","request":4,"generation":5}}]}
    """
  }

  func testLatestOccurrenceReplacesLabelWithoutChangingPinIdentity() throws {
    let root = try fixture(row() + "\n" + row(text: "Osiem") + "\n")
    let snapshot = try DictionaryPinSnapshot.read(root: root.path, requested: "take-a")
    XCTAssertEqual(snapshot.pins.count, 1)
    XCTAssertEqual(snapshot.pins.first?.text, "Osiem")
    XCTAssertEqual(snapshot.pins.first?.timeRange, "0.100–0.200 s")
    XCTAssertEqual(snapshot.pins.first?.id, "take-a:1:0:1000:whisper:4:5:100:200")
  }

  func testIncompleteAppendDoesNotEraseLastCompleteReceipt() throws {
    let root = try fixture(row() + "\n{\"schema\":")
    let snapshot = try DictionaryPinSnapshot.read(root: root.path, requested: "take-a")
    XCTAssertEqual(snapshot.pins.count, 1)
  }

  func testForeignSessionCannotBeBoundByFilenameOrText() throws {
    let root = try fixture(row(), session: "take-b")
    XCTAssertThrowsError(try DictionaryPinSnapshot.read(root: root.path, requested: "take-b"))
    XCTAssertTrue(try DictionaryPinSnapshot.read(root: root.path, requested: "../take-a").pins.isEmpty)
  }

  func testOutOfOccurrencePinIsRefusedRatherThanClamped() throws {
    let root = try fixture(row(start: 900, end: 1100) + "\n")
    XCTAssertThrowsError(try DictionaryPinSnapshot.read(root: root.path, requested: "take-a"))
  }
}
