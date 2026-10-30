import Foundation
import XCTest

@testable import Codescribe

/// Only the argument builder and an isolated JSON reader run here. No bus helper,
/// agent session, application window, or input target participates in this class.
@MainActor
final class AgentBridgeArgvTransportTests: XCTestCase {
  private var scratch: URL!

  override func setUpWithError() throws {
    scratch = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-argv-tests-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
  }

  override func tearDownWithError() throws {
    if let scratch {
      try FileManager.default.removeItem(at: scratch)
    }
  }

  func testReplyAt512PolishScalarsSurvivesFoundationArgvWithoutExpansion() throws {
    let text = String(repeating: "\u{0105}", count: 512)
    let messageID = "reply-boundary-512"
    let author = "\u{0141}ucja \u{017B}\u{00F3}\u{0142}\u{0107}"
    let reference = try XCTUnwrap(
      OverlayUserReplyReference(row: ["message_id": messageID, "author": author, "text": text]))
    XCTAssertEqual(text.unicodeScalars.count, 512)

    let arguments = try RealAgentBridgeInstaller.busTextArguments(
      holdSeconds: 0, owner: nil, root: scratch, userReplyTo: reference)
    let witness = try observeInPython(arguments)
    let reply = try XCTUnwrap(witness.reply)
    XCTAssertEqual(reply.count, 3)
    let receivedText = try XCTUnwrap(reply["text"])
    XCTAssertEqual(receivedText.scalars.count, 512)
    XCTAssertLessThanOrEqual(receivedText.scalars.count, 512, "The helper's cap must stay 512")
    try assertExact(reply["text"], text)
    try assertExact(reply["author"], author)
    try assertExact(reply["message_id"], messageID)
    XCTAssertNil(witness.peers)
  }

  func testReplyPreservesMixedNormalizationEmojiEscapesAndMessageIdentity() throws {
    // Escape spelling fixes the intended scalars independently of editor normalization.
    let composed = "\u{0105}\u{00E9}\u{00C5}"
    let decomposed = "a\u{0328}e\u{0301}A\u{030A}"
    let emoji = "\u{1F469}\u{1F3FD}\u{200D}\u{1F4BB}\u{1D11E}"
    let text = composed + " | " + decomposed + " | " + emoji + " | \"quote\" \\path\nline \\u0105"
    let author = "Zo\u{00EB} / Zoe\u{0308} \u{1F9D1}\u{200D}\u{1F52C}"
    let messageID = "reply-\u{00E9}-e\u{0301}-\u{1F680}-\"id\"-\\u00e9"
    let reference = try XCTUnwrap(
      OverlayUserReplyReference(row: ["message_id": messageID, "author": author, "text": text]))

    let arguments = try RealAgentBridgeInstaller.busTextArguments(
      holdSeconds: 0, owner: nil, root: scratch, userReplyTo: reference)
    let witness = try observeInPython(arguments)
    let reply = try XCTUnwrap(witness.reply)
    XCTAssertEqual(reply.count, 3)
    try assertExact(reply["text"], text)
    try assertExact(reply["author"], author)
    try assertExact(reply["message_id"], messageID)
    XCTAssertNil(witness.peers)
  }

  func testAlsoSentToPreservesExactPeerIdentitiesThroughFoundationArgv() throws {
    let primary = try makeOwner(
      provider: "codex", session: "argv-primary", lease: String(repeating: "1", count: 32),
      channel: "1")
    let leases = scratch.appendingPathComponent("leases")
    try FileManager.default.createDirectory(at: leases, withIntermediateDirectories: true)
    let receipt: [String: String] = [
      "schema": "codescribe.agent-bridge.lease.v1", "provider": primary.provider,
      "provider_session_id": primary.providerSessionID, "lease_id": primary.leaseID,
      "bus": scratch.appendingPathComponent("unused.jsonl").path,
    ]
    try JSONSerialization.data(withJSONObject: receipt)
      .write(to: leases.appendingPathComponent("\(primary.leaseID).json"))

    // These sessions have canonically equivalent prefixes but distinct bytes.
    // Neither the transport nor the test oracle may normalize or conflate them.
    let suffix = "-\u{1F469}\u{200D}\u{1F4BB}-\"peer\"-\\u0105"
    let peers = [
      try makeOwner(
        provider: "claude", session: "sync-\u{0105}" + suffix,
        lease: String(repeating: "a", count: 32), channel: "8"),
      try makeOwner(
        provider: "codex", session: "sync-a\u{0328}" + suffix,
        lease: String(repeating: "b", count: 32), channel: "9"),
    ]
    let arguments = try RealAgentBridgeInstaller.busTextArguments(
      holdSeconds: 0, owner: primary, root: scratch, alsoSentTo: peers)
    let witness = try observeInPython(arguments)
    XCTAssertNil(witness.reply, "Exercise --also-sent-to independently of --user-reply-to")
    let receivedPeers = try XCTUnwrap(witness.peers)
    XCTAssertEqual(receivedPeers.count, peers.count)
    for (received, expected) in zip(receivedPeers, peers) {
      XCTAssertEqual(received.count, 4)
      try assertExact(received["provider_session_id"], expected.providerSessionID)
      try assertExact(received["provider"], expected.provider)
      try assertExact(received["lease_id"], expected.leaseID)
      try assertExact(received["channel"], expected.channel)
    }
    let first = try XCTUnwrap(receivedPeers.first?["provider_session_id"])
    let last = try XCTUnwrap(receivedPeers.last?["provider_session_id"])
    XCTAssertNotEqual(first.utf8, last.utf8)
    XCTAssertNotEqual(first.scalars, last.scalars)
  }

  private func makeOwner(
    provider: String, session: String, lease: String, channel: String
  ) throws -> OverlayConversationOwner {
    try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": provider, "provider_session_id": session, "lease_id": lease,
        "channel": channel, "name": "Argv fixture",
      ]))
  }

  private struct ScalarWitness: Decodable {
    let utf8: [UInt8]
    let scalars: [UInt32]
  }

  private struct ChildWitness: Decodable {
    let reply: [String: ScalarWitness]?
    let peers: [[String: ScalarWitness]]?
  }

  private func assertExact(
    _ witness: ScalarWitness?, _ expected: String,
    file: StaticString = #filePath, line: UInt = #line
  ) throws {
    let received = try XCTUnwrap(witness, file: file, line: line)
    // Swift String equality accepts canonical equivalence; integer arrays do not.
    XCTAssertEqual(received.utf8, Array(expected.utf8), file: file, line: line)
    XCTAssertEqual(
      received.scalars, expected.unicodeScalars.map(\.value), file: file, line: line)
  }

  private enum ReaderError: Error {
    case timedOut
    case unsuccessfulExit
  }

  private func observeInPython(_ arguments: [String]) throws -> ChildWitness {
    let outputURL = scratch.appendingPathComponent("reader-output.json")
    let errorURL = scratch.appendingPathComponent("reader-error.txt")
    XCTAssertTrue(FileManager.default.createFile(atPath: outputURL.path, contents: nil))
    XCTAssertTrue(FileManager.default.createFile(atPath: errorURL.path, contents: nil))
    let output = try FileHandle(forWritingTo: outputURL)
    let diagnostics = try FileHandle(forWritingTo: errorURL)
    defer {
      try? output.close()
      try? diagnostics.close()
    }

    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/python3")
    // Pass the production arguments unchanged: test-side escaping or normalization
    // would bypass the Foundation boundary this regression must exercise.
    process.arguments = ["-I", "-c", Self.metadataReader] + arguments
    process.environment = [
      "HOME": scratch.path, "TMPDIR": scratch.path, "PATH": "/usr/bin:/bin",
      "LC_CTYPE": "UTF-8",
    ]
    process.currentDirectoryURL = scratch
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = output
    process.standardError = diagnostics
    try process.run()
    let deadline = Date().addingTimeInterval(15)
    while process.isRunning, Date() < deadline { Thread.sleep(forTimeInterval: 0.01) }
    guard !process.isRunning else {
      process.terminate()
      process.waitUntilExit()
      XCTFail("The isolated JSON reader timed out")
      throw ReaderError.timedOut
    }
    process.waitUntilExit()
    guard process.terminationReason == .exit, process.terminationStatus == 0 else {
      let error = String(decoding: try Data(contentsOf: errorURL), as: UTF8.self)
      XCTFail("JSON reader exited with \(process.terminationStatus): \(error)")
      throw ReaderError.unsuccessfulExit
    }
    return try JSONDecoder().decode(ChildWitness.self, from: Data(contentsOf: outputURL))
  }

  /// The helper decodes each metadata flag with json.loads. Return only integer
  /// observations, so the result transport cannot conceal normalization either.
  private static let metadataReader = #"""
    import json
    import sys

    arguments = sys.argv[1:]

    def payload(flag):
        if flag not in arguments:
            return None
        if arguments.count(flag) != 1:
            raise ValueError("duplicate metadata flag")
        return json.loads(arguments[arguments.index(flag) + 1])

    def observe(fields):
        return {
            key: {"utf8": list(value.encode("utf-8")), "scalars": [ord(c) for c in value]}
            for key, value in fields.items()
        }

    reply = payload("--user-reply-to")
    peers = payload("--also-sent-to")
    json.dump({
        "reply": None if reply is None else observe(reply),
        "peers": None if peers is None else [observe(peer) for peer in peers],
    }, sys.stdout, ensure_ascii=True)
    """#
}
