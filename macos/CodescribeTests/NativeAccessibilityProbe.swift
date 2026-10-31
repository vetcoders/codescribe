import AppKit
import Darwin
import Foundation
import XCTest

/// AX requests must cross a process boundary while the test host services its
/// main run loop. Querying our own SwiftUI AX graph can yield an empty tree or
/// deadlock; moving that self-query to another thread is not AppKit-safe.
@MainActor
enum NativeAccessibilityProbe {
  struct Element: Decodable {
    let role: String?
    let label: String
    let value: String
    let x: Double
    let y: Double
    let width: Double
    let height: Double
    var frame: NSRect { NSRect(x: x, y: y, width: width, height: height) }
  }
  struct Reception: Decodable {
    let requestID: String
    let trusted: Bool
    let elements: [Element]
    let pressStatus: Int32?
    let trace: [String]
    let errorCode: String?
  }

  /// Fixed synthetic fixture identifiers only. Never put message text, user
  /// identifiers, paths, labels, AX values or traces in the durable receipt.
  enum VerifiedAction: String {
    case voicelessReplyPlay = "overlay-reply-play-dddddddddddddddddddddddd"
  }

  private enum ProbeError: Error {
    case invalidContext, refused, timedOut, verificationFailed, receiptWriteFailed
  }

  /// Explicit Boolean contracts matter: XCTAssert functions do not throw and a
  /// Void-returning assertion closure cannot tell us whether the effect passed.
  /// Cleanup runs on every transport/postcondition error before a terminal row.
  static func verifyPress(
    _ action: VerifiedAction, testMethod: String,
    postcondition: () throws -> Bool, cleanup: () throws -> Bool
  ) throws {
    let requestID = UUID().uuidString
    var status: Int32?
    var observed = false
    var cleaned = false
    var failure: Error?
    do {
      try appendReceipt(
        action, testMethod: testMethod, requestID: requestID, phase: "started",
        outcome: "UNVERIFIED", axStatus: nil, postcondition: false, cleanup: false)
      try Task.checkCancellation()
      let reception = try query(action.rawValue, press: true, requestID: requestID)
      status = reception.pressStatus
      guard status == 0 else { throw ProbeError.refused }
      observed = try postcondition()
      try Task.checkCancellation()
    } catch {
      failure = error
    }
    do {
      cleaned = try cleanup()
    } catch {
      if failure == nil { failure = error }
    }
    if Task.isCancelled, failure == nil { failure = CancellationError() }
    let passed = failure == nil && status == 0 && observed && cleaned
    try appendReceipt(
      action, testMethod: testMethod, requestID: requestID, phase: "completed",
      outcome: passed ? "PASS" : (failure is CancellationError ? "CANCELLED" : "FAIL"),
      axStatus: status, postcondition: observed, cleanup: cleaned)
    guard passed else {
      XCTFail(
        "Synthetic test-host AX action requires transport, explicit postcondition and cleanup")
      throw failure ?? ProbeError.verificationFailed
    }
  }

  private static func appendReceipt(
    _ action: VerifiedAction, testMethod: String, requestID: String, phase: String,
    outcome: String, axStatus: Int32?, postcondition: Bool, cleanup: Bool
  ) throws {
    let environment = ProcessInfo.processInfo.environment
    guard let path = environment["CODESCRIBE_INTERACTION_RECEIPTS"],
      let runID = environment["CODESCRIBE_INTERACTION_RUN_ID"], UUID(uuidString: runID) != nil,
      let sourceSHA = environment["CODESCRIBE_INTERACTION_SOURCE_SHA"],
      sourceSHA.range(of: "^[0-9a-f]{40}$", options: .regularExpression) != nil,
      testMethod
        == "OverlayConversationAcceptanceTests.testVoicelessReplyAndAgentTextRenderEnabledPlay()"
    else { throw ProbeError.invalidContext }
    var row: [String: Any] = [
      "schema": "codescribe.test-host-interaction.v1", "runID": runID, "sourceSHA": sourceSHA,
      "requestID": requestID, "testMethod": testMethod, "identifier": action.rawValue,
      "verb": "AXPress", "hostKind": "synthetic-deriveddata-xctest", "phase": phase,
      "outcome": outcome, "axStatus": NSNull(), "postcondition": postcondition, "cleanup": cleanup,
    ]
    if let axStatus { row["axStatus"] = axStatus }
    var data = try JSONSerialization.data(withJSONObject: row, options: [.sortedKeys])
    data.append(0x0A)
    // Runner creates the private retained file. O_NOFOLLOW refuses a replaced
    // symlink; a file lock serializes writes from parallel XCTest host processes.
    let descriptor = open(path, O_WRONLY | O_APPEND | O_NOFOLLOW)
    guard descriptor >= 0 else { throw ProbeError.receiptWriteFailed }
    defer { _ = close(descriptor) }
    guard flock(descriptor, LOCK_EX) == 0 else { throw ProbeError.receiptWriteFailed }
    defer { _ = flock(descriptor, LOCK_UN) }
    try data.withUnsafeBytes { bytes in
      var offset = 0
      while offset < bytes.count {
        let count = Darwin.write(
          descriptor, bytes.baseAddress!.advanced(by: offset), bytes.count - offset)
        guard count > 0 else { throw ProbeError.receiptWriteFailed }
        offset += count
      }
    }
    guard fsync(descriptor) == 0 else { throw ProbeError.receiptWriteFailed }
  }
  static func waitForElements(_ identifier: String) throws -> Reception {
    let deadline = Date().addingTimeInterval(2)
    var reception = try query(identifier)
    while reception.elements.isEmpty && Date() < deadline {
      RunLoop.main.run(until: Date().addingTimeInterval(0.02))
      reception = try query(identifier)
    }
    return reception
  }

  static func query(
    _ identifier: String, press: Bool = false, requestID: String = UUID().uuidString
  ) throws -> Reception {
    try BackgroundTestWindow.requireIsolatedGUI()
    let path = try XCTUnwrap(
      ProcessInfo.processInfo.environment["CODESCRIBE_AX_PROBE"],
      "Run through make test-swift to build the independent AX client")
    struct Request: Encodable {
      let requestID: String
      let pid: Int32
      let identifier: String
      let press: Bool
    }
    let root = URL(fileURLWithPath: path, isDirectory: true)
    guard UUID(uuidString: requestID) != nil else { throw ProbeError.invalidContext }
    let input = root.appendingPathComponent(requestID + ".request.json")
    let output = root.appendingPathComponent(requestID + ".response.json")
    let request = Request(
      requestID: requestID, pid: ProcessInfo.processInfo.processIdentifier,
      identifier: identifier, press: press)
    try JSONEncoder().encode(request).write(to: input, options: .atomic)
    defer {
      try? FileManager.default.removeItem(at: input)
      try? FileManager.default.removeItem(at: output)
    }
    let deadline = Date().addingTimeInterval(10)
    while !FileManager.default.fileExists(atPath: output.path) && Date() < deadline {
      try Task.checkCancellation()
      RunLoop.main.run(until: Date().addingTimeInterval(0.005))
    }
    guard FileManager.default.fileExists(atPath: output.path) else {
      XCTFail("Independent AX probe exceeded its bounded deadline")
      throw ProbeError.timedOut
    }
    let value = try JSONDecoder().decode(Reception.self, from: Data(contentsOf: output))
    guard value.requestID == requestID, value.trusted, value.errorCode == nil else {
      XCTFail(
        "Independent AX client refused the request or lacks existing accessibility permission")
      throw ProbeError.refused
    }
    return value
  }
}
