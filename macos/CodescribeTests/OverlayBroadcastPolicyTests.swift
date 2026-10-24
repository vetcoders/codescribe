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

  func testMembershipAndTheMoonEachWearTheirOwnMark() {
    // Maciej (2026-10-17): channel 0 and channel 0 plus steer could not be
    // told apart. Maciej (2026-10-23): the moon is its own axis with its own
    // mark, and "off channel 0 with the moon lit" exists.
    let on = channel("1", policy: .on)
    let quiet = channel("2", policy: .quiet)
    let off = channel("3", policy: .off)
    let offQuiet = channel("4", policy: .offQuiet)
    XCTAssertEqual(on.broadcastPolicy.statusSymbol, "antenna.radiowaves.left.and.right")
    XCTAssertEqual(quiet.broadcastPolicy.statusSymbol, "antenna.radiowaves.left.and.right")
    XCTAssertEqual(off.broadcastPolicy.statusSymbol, "antenna.radiowaves.left.and.right.slash")
    XCTAssertEqual(offQuiet.broadcastPolicy.statusSymbol, "antenna.radiowaves.left.and.right.slash")
    XCTAssertEqual(on.broadcastPolicy.quietSymbol, "moon")
    XCTAssertEqual(quiet.broadcastPolicy.quietSymbol, "moon.fill")
    XCTAssertEqual(offQuiet.broadcastPolicy.quietSymbol, "moon.fill")
    XCTAssertEqual(
      Set(AgentBroadcastPolicy.allCases.map { $0.statusSymbol + $0.quietSymbol }).count, 4,
      "four states, four mark pairs")
    XCTAssertEqual(Set(AgentBroadcastPolicy.allCases.map(\.statusHelp)).count, 2)
    XCTAssertEqual(Set(AgentBroadcastPolicy.allCases.map(\.quietHelp)).count, 2)
    XCTAssertTrue(on.broadcastPolicy.receivesWake)
    XCTAssertFalse(quiet.broadcastPolicy.receivesWake)
    XCTAssertFalse(off.broadcastPolicy.receivesWake)
    XCTAssertFalse(offQuiet.broadcastPolicy.receivesWake)
    XCTAssertEqual(offQuiet.broadcastPolicy.membershipArgument, "off")
    XCTAssertEqual(offQuiet.broadcastPolicy.moonArgument, "on")
    // The menu stopped naming three receptions: it is two state checkboxes
    // (Maciej, 2026-10-17), and the wire enum keeps only non-menu duties.
    let source =
      (try? String(
        contentsOf: URL(fileURLWithPath: #filePath).deletingLastPathComponent()
          .deletingLastPathComponent()
          .appendingPathComponent("Codescribe/Screens/Overlay/OverlayChannelStatusView.swift"),
        encoding: .utf8)) ?? ""
    XCTAssertTrue(source.contains("\"Channel 0\""), "membership checkbox")
    // DND reverses the former wake checkbox: ticked means quiet.
    XCTAssertTrue(source.contains("\"Do not disturb\""), "quiet checkbox")
    XCTAssertFalse(source.contains("\"Steer (otherwise queued)\""), "no jargon label")
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
        "--broadcast", "on", "--quiet", "on", "--provider", identity.provider,
        "--session", identity.session, "--bus", identity.bus,
        "--bridge-home", fixture.installer.bridgeRoot.path,
      ])
    try await OverlayBroadcastPolicyMenu.setPolicy(
      .quiet, for: identity, installer: fixture.installer)
    let quiet = try fixture.receipt()
    XCTAssertEqual(quiet["policy"] as? String, "on")
    XCTAssertEqual(quiet["quiet"] as? Bool, true)
    XCTAssertEqual(quiet["lease_id"] as? String, identity.leaseID)
    XCTAssertEqual(quiet["bus"] as? String, identity.bus)
    // Off channel 0 with the moon lit: the pair the one-knob wire never had.
    try await OverlayBroadcastPolicyMenu.setPolicy(
      .offQuiet, for: identity, installer: fixture.installer)
    let offQuiet = try fixture.receipt()
    XCTAssertEqual(offQuiet["policy"] as? String, "off")
    XCTAssertEqual(offQuiet["quiet"] as? Bool, true)
    try await OverlayBroadcastPolicyMenu.setPolicy(
      .off, for: identity, installer: fixture.installer)
    XCTAssertEqual(try fixture.receipt()["policy"] as? String, "off")
    XCTAssertEqual(try fixture.receipt()["quiet"] as? Bool, false)
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
    try await RealAgentBridgeInstaller.setBroadcastPolicy(
      .offQuiet, for: identity, installer: fixture.installer)
    let offQuiet = await snapshot()
    XCTAssertEqual(offQuiet, .offQuiet)
    // A receipt from before the split spells the lit moon as policy "quiet".
    var legacy = try fixture.receipt()
    legacy["policy"] = "quiet"
    legacy.removeValue(forKey: "quiet")
    try JSONSerialization.data(withJSONObject: legacy).write(to: fixture.policy, options: .atomic)
    let legacyQuiet = await snapshot()
    XCTAssertEqual(legacyQuiet, .quiet)
    var foreign = try fixture.receipt()
    foreign["provider_session_id"] = "foreign-session"
    try JSONSerialization.data(withJSONObject: foreign).write(to: fixture.policy, options: .atomic)
    let invalidOwner = await snapshot()
    XCTAssertEqual(invalidOwner, .on)
    try Data("{broken".utf8).write(to: fixture.policy, options: .atomic)
    let invalidJSON = await snapshot()
    XCTAssertEqual(invalidJSON, .on)
  }

  /// Copilot on #183: from `.on`, the moon and then the antenna before the
  /// next 500 ms poll wrote `.off`, because the second click started from the
  /// cached receipt. Each change now starts from the previous read-back.
  func testQuickMoonThenAntennaComposeOnTheReadBack() async throws {
    let helper = PolicyHelper()
    let ledger = helper.ledger()
    let identity = PolicyHelper.identity
    // The roster still paints the old receipt: no poll has run in between.
    try await ledger.change(identity, polled: .on) { $0.moon.toggle() }
    try await ledger.change(identity, polled: .on) { $0.member.toggle() }
    XCTAssertEqual(helper.written, [.quiet, .offQuiet])
    XCTAssertEqual(ledger.policy(for: identity, polled: .on), .offQuiet)
    // The reverse order cannot rejoin channel 0 either.
    let reverse = PolicyHelper(stored: .off)
    let other = reverse.ledger()
    try await other.change(identity, polled: .off) { $0.member.toggle() }
    try await other.change(identity, polled: .off) { $0.moon.toggle() }
    XCTAssertEqual(reverse.written, [.on, .quiet])
  }

  /// One change in flight holds every surface for that agent: the marks, the
  /// pill menu and the Intercom share this ledger, not three pending flags.
  func testASecondChangeWhileOneIsInFlightWritesNothing() async throws {
    let helper = PolicyHelper()
    let ledger = helper.ledger()
    let identity = PolicyHelper.identity
    helper.duringWrite = {
      XCTAssertTrue(ledger.isPending(identity))
      try await ledger.change(identity, polled: .on) { $0.member = false }
    }
    try await ledger.change(identity, polled: .on) { $0.moon = true }
    XCTAssertEqual(helper.written, [.quiet])
    XCTAssertFalse(ledger.isPending(identity))
  }

  /// A poll that began before a write carries the old receipt; it must not
  /// paint over the read-back. The next poll, started after, is the truth.
  func testAPollStartedBeforeTheWriteCannotPaintTheOldReceipt() async throws {
    let helper = PolicyHelper()
    let ledger = helper.ledger()
    let identity = PolicyHelper.identity
    let before = ledger.revision
    try await ledger.change(identity, polled: .on) { $0.moon = true }
    XCTAssertFalse(ledger.admitsPoll(startedAt: before))
    XCTAssertEqual(ledger.policy(for: identity, polled: .on), .quiet)
    XCTAssertTrue(ledger.admitsPoll(startedAt: ledger.revision))
    XCTAssertTrue(ledger.readBacks.isEmpty)
    XCTAssertEqual(ledger.policy(for: identity, polled: .quiet), .quiet)
  }

  func testARefusedWriteLeavesTheReceiptAndReleasesTheHold() async {
    let helper = PolicyHelper()
    helper.refuses = true
    let ledger = helper.ledger()
    let identity = PolicyHelper.identity
    do {
      try await ledger.change(identity, polled: .on) { $0.moon = true }
      XCTFail("the helper's refusal reaches the surface that names it")
    } catch {}
    XCTAssertFalse(ledger.isPending(identity))
    XCTAssertEqual(ledger.policy(for: identity, polled: .on), .on)
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

  /// Stands in for the helper: each write leaves its receipt and the read-back
  /// returns it, so the ledger's composition shows without a bus.
  @MainActor
  private final class PolicyHelper {
    static let identity = AgentPlaybackIdentity(
      provider: "junie", session: "ledger-fixture", bus: "/dev/null")
    var stored: AgentBroadcastPolicy
    var written: [AgentBroadcastPolicy] = []
    var refuses = false
    var duringWrite: (@MainActor () async throws -> Void)?

    init(stored: AgentBroadcastPolicy = .on) { self.stored = stored }

    func ledger() -> OverlayBroadcastPolicyLedger {
      OverlayBroadcastPolicyLedger(
        write: { [self] policy, _ in
          if refuses { throw CocoaError(.featureUnsupported) }
          written.append(policy)
          if let hook = duringWrite {
            duringWrite = nil
            try await hook()
          }
          stored = policy
        },
        readBack: { [self] _ in stored })
    }
  }
}
