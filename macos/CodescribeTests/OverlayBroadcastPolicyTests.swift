import AppKit
import Foundation
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayBroadcastPolicyTests: XCTestCase {
  func testHeaderCountsOnlyOnAmongBoundAgents() {
    let view = drawer([channel("1", policy: .on), channel("2", policy: .quiet)])
    XCTAssertEqual(view.broadcastHeader, "0 · All (1 of 2)")
    XCTAssertEqual(view.broadcastMembers.map(\.channel), ["1", "2"])
    XCTAssertEqual(
      drawer([channel("1", policy: .off), channel("2", policy: .quiet)]).broadcastHeader,
      "0 · All (0 of 2)")
    XCTAssertEqual(drawer([]).broadcastHeader, "0 · All (0 of 0)")
    XCTAssertEqual(
      drawer([channel("0", policy: .on), channel("1", policy: .on)]).broadcastHeader,
      "0 · All (1 of 1)")
  }

  func testPolishHeaderUsesBothCounts() throws {
    let polish = try XCTUnwrap(Bundle.main.path(forResource: "pl", ofType: "lproj"))
    let bundle = try XCTUnwrap(Bundle(path: polish))
    let format = bundle.localizedString(
      forKey: "0 · All (%lld of %lld)", value: nil, table: nil)
    XCTAssertEqual(String(format: format, Int64(1), Int64(2)), "0 · Wszyscy (1 z 2)")
  }

  func testOnQuietAndOffEachWearTheirOwnMark() {
    // Maciej (2026-10-17): channel 0 and channel 0 plus steer could not be
    // told apart — quiet and off shared the slashed antenna, on wore nothing.
    let on = channel("1", policy: .on)
    let quiet = channel("2", policy: .quiet)
    let off = channel("3", policy: .off)
    XCTAssertEqual(on.broadcastPolicy.statusSymbol, "antenna.radiowaves.left.and.right")
    XCTAssertEqual(quiet.broadcastPolicy.statusSymbol, "tray.and.arrow.down")
    XCTAssertEqual(off.broadcastPolicy.statusSymbol, "antenna.radiowaves.left.and.right.slash")
    XCTAssertEqual(
      Set(AgentBroadcastPolicy.allCases.map(\.statusSymbol)).count, 3, "three states, three marks")
    XCTAssertEqual(
      Set(AgentBroadcastPolicy.allCases.map(\.statusHelp)).count, 3, "three states, three tooltips")
    XCTAssertTrue(on.broadcastPolicy.receivesWake)
    XCTAssertFalse(quiet.broadcastPolicy.receivesWake)
    XCTAssertFalse(off.broadcastPolicy.receivesWake)
    // The menu stopped naming three receptions: it is two state checkboxes
    // (Maciej, 2026-10-17), and the wire enum keeps only non-menu duties.
    let source =
      (try? String(
        contentsOf: URL(fileURLWithPath: #filePath).deletingLastPathComponent()
          .deletingLastPathComponent()
          .appendingPathComponent("Codescribe/Screens/Overlay/OverlayChannelStatusView.swift"),
        encoding: .utf8)) ?? ""
    XCTAssertTrue(source.contains("\"Channel 0\""), "membership checkbox")
    XCTAssertTrue(source.contains("\"Steer (otherwise queued)\""), "steer checkbox")
  }

  /// The member row's menu (click and right-click alike) issues the selected
  /// policy through the managed helper — quiet, on and the full off that the
  /// checkboxes never exposed (Maciej, 2026-10-16).
  func testMenuActionIssuesEachBroadcastPolicyThroughTheRealHelper() async throws {
    let fixture = try Fixture()
    defer { try? FileManager.default.removeItem(at: fixture.home) }
    let identity = fixture.identity
    XCTAssertEqual(
      RealAgentBridgeInstaller.broadcastPolicyArguments(
        .quiet, for: identity, root: fixture.installer.bridgeRoot),
      [
        "--broadcast", "quiet", "--provider", identity.provider,
        "--session", identity.session, "--bus", identity.bus,
        "--bridge-home", fixture.installer.bridgeRoot.path,
      ])
    try await OverlayBroadcastPolicyMenu.setPolicy(
      .quiet, for: identity, installer: fixture.installer)
    let quiet = try fixture.receipt()
    XCTAssertEqual(quiet["policy"] as? String, "quiet")
    XCTAssertEqual(quiet["lease_id"] as? String, identity.leaseID)
    XCTAssertEqual(quiet["bus"] as? String, identity.bus)
    try await OverlayBroadcastPolicyMenu.setPolicy(
      .off, for: identity, installer: fixture.installer)
    XCTAssertEqual(try fixture.receipt()["policy"] as? String, "off")
    try await OverlayBroadcastPolicyMenu.setPolicy(
      .on, for: identity, installer: fixture.installer)
    XCTAssertEqual(try fixture.receipt()["policy"] as? String, "on")
  }

  func testPolicySnapshotDefaultsOnAndRereadsReceiptWithoutASecondStore() async throws {
    let fixture = try Fixture()
    defer { try? FileManager.default.removeItem(at: fixture.home) }
    let identity = fixture.identity
    func snapshot() async -> AgentBroadcastPolicy? {
      await RealAgentBridgeInstaller.broadcastPolicySnapshot(
        for: [identity], installer: fixture.installer)[identity]
    }
    let missing = await snapshot()
    XCTAssertEqual(missing, .on)
    try await RealAgentBridgeInstaller.setBroadcastPolicy(
      .off, for: identity, installer: fixture.installer)
    let off = await snapshot()
    XCTAssertEqual(off, .off)
    try await RealAgentBridgeInstaller.setBroadcastPolicy(
      .quiet, for: identity, installer: fixture.installer)
    let quiet = await snapshot()
    XCTAssertEqual(quiet, .quiet)
    var foreign = try fixture.receipt()
    foreign["provider_session_id"] = "foreign-session"
    try JSONSerialization.data(withJSONObject: foreign).write(to: fixture.policy, options: .atomic)
    let invalidOwner = await snapshot()
    XCTAssertEqual(invalidOwner, .on)
    try Data("{broken".utf8).write(to: fixture.policy, options: .atomic)
    let invalidJSON = await snapshot()
    XCTAssertEqual(invalidJSON, .on)
  }

  private func channel(_ digit: String, policy: AgentBroadcastPolicy) -> OverlayChannelDelivery {
    var row = OverlayChannelDelivery(
      channel: digit, agent: "agent-\(digit)", deliveryID: nil, stage: nil, isOpen: false)
    row.broadcastPolicy = policy
    return row
  }

  private func drawer(_ channels: [OverlayChannelDelivery]) -> OverlayChannelStatusView {
    OverlayChannelStatusView(
      channels: channels, unavailable: false, palette: .dark, animates: false,
      hudStates: Dictionary(
        uniqueKeysWithValues: channels.map { channel in
          (
            channel.channel,
            OverlayChannelHudProjection(
              open: false, loud: false, autosealDeadline: nil, followerAlive: true,
              provider: "junie", providerSessionID: "session-\(channel.channel)")
          )
        }))
  }

  private struct Fixture {
    let home: URL
    let installer: RealAgentBridgeInstaller
    let identity: AgentPlaybackIdentity
    var policy: URL {
      installer.bridgeRoot.appendingPathComponent(
        "runtime/broadcast-policy/\(identity.storageKey).json")
    }

    init() throws {
      // The helper compares the lease's bus against POSIX realpath, while
      // Foundation's resolvingSymlinksInPath strips `/private` from a
      // temporaryDirectory path. A scratch home under the repo's ignored
      // build/ directory crosses no symlink, so both sides see one path.
      let repo = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
      home = repo.appendingPathComponent("build/test-scratch/\(UUID().uuidString)")
      try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
      installer = RealAgentBridgeInstaller(resourceRoot: nil, homeDirectory: home, environment: [:])
      identity = AgentPlaybackIdentity(
        provider: "junie", session: "broadcast-fixture",
        bus: home.appendingPathComponent("fixture.jsonl").path)
      let lease = installer.bridgeRoot.appendingPathComponent("leases/\(identity.leaseID).json")
      try FileManager.default.createDirectory(
        at: lease.deletingLastPathComponent(), withIntermediateDirectories: true)
      try JSONSerialization.data(withJSONObject: [
        "schema": "codescribe.agent-bridge.lease.v1", "provider": identity.provider,
        "provider_session_id": identity.session, "lease_id": identity.leaseID, "bus": identity.bus,
      ]).write(to: lease)
      // Execute a scratch managed copy of the real helper, never the live bus.
      let source = try String(
        contentsOf: repo.appendingPathComponent("scripts/bus-demux.py"), encoding: .utf8)
      let firstNewline = try XCTUnwrap(source.firstIndex(of: "\n"))
      let command = installer.commandURL("cs-bus")
      try FileManager.default.createDirectory(
        at: command.deletingLastPathComponent(), withIntermediateDirectories: true)
      let managed =
        String(source[...firstNewline])
        + "# codescribe-managed-command: {\"managed_id\":\"broadcast-fixture\"}\n"
        + String(source[source.index(after: firstNewline)...])
      try managed.write(to: command, atomically: true, encoding: .utf8)
      try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: command.path)
    }

    func receipt() throws -> [String: Any] {
      try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: policy)) as? [String: Any])
    }
  }
}
