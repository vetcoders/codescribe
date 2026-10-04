import Foundation
import XCTest

@testable import Codescribe

@MainActor
final class RealThreadsRestorationTests: XCTestCase {
  func testPersistedCallIdentityNameAndOutcomeReachTheExistingInspector() throws {
    let use = try stored([
      ["type": "tool_use", "id": "call-a", "name": "read_file", "input": [:]],
      ["type": "tool_use", "id": "call-b", "name": "list_files", "input": [:]],
    ])
    let result = try stored(
      [
        [
          "type": "tool_result", "tool_use_id": "call-a", "is_error": false,
          "content": [["type": "text", "text": "raw private output"]],
        ],
        [
          "type": "tool_result", "tool_use_id": "call-b", "is_error": true,
          "content": [["type": "text", "text": "raw failure payload"]],
        ],
      ], role: "user")
    let messages = RealThreadsEngine.restoredMessages(from: [use, result])
    XCTAssertEqual(messages.count, 1)
    XCTAssertEqual(messages[0].role, .tool)
    let lines = messages[0].toolLines
    XCTAssertEqual(lines.map(\.callID), ["call-a", "call-b"])
    XCTAssertEqual(lines.map(\.detail), ["read_file", "list_files"])
    XCTAssertEqual(lines.map(\.verb), ["ran", "failed"])
    XCTAssertEqual(lines.map(\.state), [.succeeded, .failed])
    for line in lines {
      XCTAssertTrue(line.hasInspectPayload)
      XCTAssertNil(line.reason, "Raw payload is not the persisted live summary")
      XCTAssertNil(line.startedAt)
      XCTAssertNil(line.durationMs, "Message timestamps do not establish duration")
      let callID = try XCTUnwrap(line.callID)
      XCTAssertTrue(line.technicalCopyText.contains("call_id: " + callID))
      XCTAssertFalse(line.technicalCopyText.contains("raw "))
      XCTAssertFalse(line.technicalCopyText.contains("summary:"))
      XCTAssertFalse(line.technicalCopyText.contains("duration:"))
    }
    XCTAssertTrue(lines[0].technicalCopyText.contains("status: succeeded"))
    XCTAssertTrue(lines[1].technicalCopyText.contains("status: failed"))
  }

  func testResultOccurrencesAreNotDeduplicatedAndFutureUsesDoNotRenameEarlierRows() throws {
    let first = try stored(
      [
        ["type": "tool_result", "tool_use_id": "future", "is_error": false]
      ], role: "user")
    let sameMessage = try stored([
      ["type": "tool_use", "id": "future", "name": "read_file"],
      ["type": "tool_use", "id": "second", "name": "read_file"],
      ["type": "tool_result", "tool_use_id": "future", "is_error": true],
      ["type": "tool_result", "tool_use_id": "second", "is_error": false],
      ["type": "tool_result", "tool_use_id": "future", "is_error": false],
    ])
    let unrelated = try stored(
      [
        ["type": "tool_result", "tool_use_id": "uncorrelated", "is_error": true]
      ], role: "user")
    let lines = RealThreadsEngine.restoredMessages(from: [first, sameMessage, unrelated])
      .flatMap(\.toolLines)
    XCTAssertEqual(lines.count, 5)
    XCTAssertEqual(Set(lines.map(\.id)).count, 5)
    XCTAssertEqual(lines.map(\.callID), ["future", "future", "second", "future", "uncorrelated"])
    XCTAssertEqual(
      lines.map(\.detail), ["tool result", "read_file", "read_file", "read_file", "tool result"])
    XCTAssertEqual(lines.map(\.state), [.succeeded, .failed, .succeeded, .succeeded, .failed])
    XCTAssertTrue(lines.allSatisfy(\.hasInspectPayload))
  }

  func testMissingOrMalformedOutcomeStaysUnknownWithoutDiscardingSiblingResults() throws {
    let message = try stored(
      [
        ["type": "unrelated", "id": 17, "name": ["bad"]],
        42,
        ["type": "tool_result", "tool_use_id": "missing"],
        ["type": "tool_result", "tool_use_id": "null", "is_error": NSNull()],
        ["type": "tool_result", "tool_use_id": "string", "is_error": "false"],
        ["type": "tool_result", "tool_use_id": "number", "is_error": 1],
        ["type": "tool_result", "tool_use_id": "valid", "is_error": true],
      ], role: "user")
    let lines = try XCTUnwrap(RealThreadsEngine.restoredMessages(from: [message]).first).toolLines
    XCTAssertEqual(lines.count, 5)
    XCTAssertEqual(lines.map(\.callID), ["missing", "null", "string", "number", "valid"])
    XCTAssertEqual(lines.map(\.verb), ["ended", "ended", "ended", "ended", "failed"])
    XCTAssertEqual(lines.map(\.state), [.unknown, .unknown, .unknown, .unknown, .failed])
    for line in lines.prefix(4) {
      XCTAssertTrue(line.technicalCopyText.contains("status: ended"))
      XCTAssertFalse(line.technicalCopyText.contains("status: succeeded"))
    }
  }

  func testMalformedIdentityKeepsResultRowsWithoutInventingInspectableFields() throws {
    let message = try stored([
      ["type": "tool_use", "id": "  exact-id  ", "name": "file_read"],
      ["type": "tool_use", "id": "malformed-name", "name": 12],
      ["type": "tool_result", "is_error": true],
      ["type": "tool_result", "tool_use_id": "", "is_error": false],
      ["type": "tool_result", "tool_use_id": " \n ", "is_error": true],
      ["type": "tool_result", "tool_use_id": 12, "is_error": false],
      ["type": "tool_result", "tool_use_id": "  exact-id  ", "is_error": false],
      ["type": "tool_result", "tool_use_id": "malformed-name", "is_error": true],
    ])
    let lines = try XCTUnwrap(RealThreadsEngine.restoredMessages(from: [message]).first).toolLines
    XCTAssertEqual(lines.count, 6)
    for line in lines.prefix(4) {
      XCTAssertNil(line.callID)
      XCTAssertFalse(line.hasInspectPayload)
      XCTAssertEqual(line.detail, "tool result")
      XCTAssertNil(line.reason)
      XCTAssertNil(line.durationMs)
    }
    XCTAssertEqual(lines[4].callID, "  exact-id  ", "A nonblank ID is retained exactly")
    XCTAssertEqual(lines[4].detail, "file_read")
    XCTAssertEqual(lines[5].detail, "tool result", "A malformed name cannot be fabricated")
    XCTAssertTrue(lines[5].hasInspectPayload)
  }

  func testInvalidWholeJsonRetainsExistingFlattenedTextWithoutFakeToolMetadata() {
    let messages = RealThreadsEngine.restoredMessages(from: [
      CsThreadMessage(role: "user", text: "question", rawJson: "broken JSON", timestampMs: 1234),
      CsThreadMessage(role: "assistant", text: "answer", rawJson: "{}", timestampMs: 1235),
    ])
    XCTAssertEqual(messages.map(\.text), ["question", "answer"])
    XCTAssertEqual(messages.map(\.role), [.you, .assistant])
    XCTAssertTrue(messages.allSatisfy { $0.toolLines.isEmpty })
  }

  private func stored(
    _ blocks: [Any], role: String = "assistant", text: String = ""
  ) throws -> CsThreadMessage {
    let data = try JSONSerialization.data(withJSONObject: blocks, options: [.sortedKeys])
    return CsThreadMessage(
      role: role, text: text, rawJson: try XCTUnwrap(String(data: data, encoding: .utf8)),
      timestampMs: 1234)
  }
}
