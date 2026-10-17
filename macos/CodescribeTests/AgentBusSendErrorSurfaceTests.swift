import Foundation
import XCTest

@testable import Codescribe

/// Falsifiers for the send-error surface cut (059bfa51, 2026-10-17): the
/// publisher used to route the helper's stderr to /dev/null, so every refusal
/// collapsed into a bare "could not be sent" and, during the dead-session
/// broadcast P0, the Founder never saw "channel was rebound or its agent is
/// not listening". These drive the real `sendBusText` through a fake managed
/// `cs-bus` and pin the seams: the helper's last stderr line reaches the
/// thrown error without its "cs-bus: " prefix, the exit-code headline mapping
/// stays, a silent failure keeps the bare headline, and a chatty helper cannot
/// deadlock the sender behind a full pipe.
final class AgentBusSendErrorSurfaceTests: XCTestCase {

  // MARK: - Fixture

  /// Installs a managed `cs-bus` under a throwaway home that swallows stdin,
  /// writes `stderr` verbatim and exits with `status`. Returns that home.
  private func installFakeHelper(stderr: String, status: Int32) throws -> URL {
    let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    let bin = home.appendingPathComponent(".local/bin")
    try FileManager.default.createDirectory(at: bin, withIntermediateDirectories: true)
    let executable = bin.appendingPathComponent("cs-bus")
    let payload = Data(stderr.utf8).base64EncodedString()
    try """
    #!/usr/bin/python3
    # codescribe-managed-command: {"managed_id":"send-error-surface-test"}
    import base64, sys
    sys.stdin.read()
    sys.stderr.write(base64.b64decode("\(payload)").decode("utf-8"))
    sys.stderr.flush()
    sys.exit(\(status))
    """.write(to: executable, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes(
      [.posixPermissions: 0o700], ofItemAtPath: executable.path)
    return home
  }

  /// Publishes one broadcast through the helper installed under `home` and
  /// returns the thrown error, `nil` when the send succeeded. Static and
  /// self-free so a child task can run it without crossing an isolation region.
  private static func send(home: URL) async -> NSError? {
    do {
      try await RealAgentBridgeInstaller.sendBusText(
        owner: nil, text: "dead-session broadcast", holdSeconds: 0,
        installer: RealAgentBridgeInstaller(
          resourceRoot: nil, homeDirectory: home, environment: [:]))
      return nil
    } catch {
      return error as NSError
    }
  }

  /// The error `sendBusText` throws for a helper that fails this way.
  private func sendError(stderr: String, status: Int32) async throws -> NSError {
    let home = try installFakeHelper(stderr: stderr, status: status)
    defer { try? FileManager.default.removeItem(at: home) }
    let error = await Self.send(home: home)
    return try XCTUnwrap(error, "exit \(status) must surface as a thrown send error")
  }

  // MARK: - Falsifiers

  func testHelperRefusalReachesTheSendErrorWithoutItsPrefix() async throws {
    let refusal = "channel was rebound or its agent is not listening; draft retained"
    let error = try await sendError(
      stderr: "cs-bus: binding 2 resolved\ncs-bus: \(refusal)\n", status: 3)
    XCTAssertEqual(error.domain, "Codescribe.BusText")
    XCTAssertEqual(error.code, 3)
    let lines = error.localizedDescription.components(separatedBy: "\n")
    XCTAssertEqual(
      lines.count, 2, "headline plus the helper's last line: \(error.localizedDescription)")
    XCTAssertFalse(lines[0].isEmpty, "the localized headline stays on top")
    XCTAssertEqual(lines[1], refusal)
    XCTAssertFalse(error.localizedDescription.contains("cs-bus: "), "prefix is noise to a reader")
    XCTAssertFalse(
      error.localizedDescription.contains("binding 2 resolved"),
      "only the last line names the culprit")
  }

  func testPartialDeliveryKeepsItsOwnHeadlineAndStillNamesTheHelperLine() async throws {
    let partial = try await sendError(
      stderr: "cs-bus: skipped beta (channel 2): not listening\n", status: 4)
    let retained = try await sendError(
      stderr: "cs-bus: no agents are listening; draft retained\n", status: 3)
    XCTAssertEqual(partial.code, 4)
    XCTAssertEqual(retained.code, 3)
    let partialLines = partial.localizedDescription.components(separatedBy: "\n")
    let retainedLines = retained.localizedDescription.components(separatedBy: "\n")
    XCTAssertEqual(partialLines.count, 2)
    XCTAssertEqual(retainedLines.count, 2)
    XCTAssertNotEqual(
      partialLines[0], retainedLines[0], "exit 4 keeps the partial-delivery headline")
    XCTAssertEqual(partialLines[1], "skipped beta (channel 2): not listening")
    XCTAssertEqual(retainedLines[1], "no agents are listening; draft retained")
  }

  func testSilentFailureKeepsTheBareHeadline() async throws {
    let silent = try await sendError(stderr: "", status: 2)
    XCTAssertEqual(silent.code, 2)
    XCTAssertFalse(silent.localizedDescription.isEmpty)
    XCTAssertFalse(
      silent.localizedDescription.contains("\n"), "no refusal line means no trailing newline")
    let whitespace = try await sendError(stderr: "  \n\n", status: 2)
    XCTAssertEqual(
      whitespace.localizedDescription, silent.localizedDescription,
      "blank stderr is the same as no stderr")
  }

  func testChattyHelperCannotDeadlockTheSender() async throws {
    // Far beyond the kernel pipe buffer: a publisher that waits for exit
    // before draining stderr never returns from here.
    let noise = String(repeating: "cs-bus: progress line that nobody reads\n", count: 4096)
    let refusal = "channel was rebound or its agent is not listening; draft retained"
    let home = try installFakeHelper(stderr: noise + "cs-bus: \(refusal)\n", status: 3)
    defer { try? FileManager.default.removeItem(at: home) }
    struct Outcome: Sendable {
      let code: Int
      let description: String
    }
    let outcome = try await withThrowingTaskGroup(of: Outcome?.self) { group in
      group.addTask {
        guard let error = await Self.send(home: home) else {
          return Outcome(code: 0, description: "")
        }
        return Outcome(code: error.code, description: error.localizedDescription)
      }
      group.addTask {
        try await Task.sleep(nanoseconds: 20_000_000_000)
        return nil
      }
      let first = try await group.next() ?? nil
      group.cancelAll()
      return first
    }
    let result = try XCTUnwrap(outcome, "sender did not return within 20 s: stderr pipe deadlock")
    XCTAssertEqual(result.code, 3)
    XCTAssertEqual(result.description.components(separatedBy: "\n").last, refusal)
  }
}
