import AppKit
import Foundation
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class AgentSendHoldTests: XCTestCase {
  func testStepperRoundTripsThroughTheSettingsViewModelAndReloads() {
    var stored = CsSettings.sample
    var writes: [(String, String)] = []
    let engine = MockSettingsEngine(
      settingsLoader: { stored },
      updateConfigObserver: { key, value in
        writes.append((key, value))
        if key == "CODESCRIBE_AGENT_SEND_HOLD_SEC", let seconds = UInt32(value) {
          stored.agentSendHoldSec = seconds
        }
      })
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
    XCTAssertEqual(model.agentSendHoldSeconds.wrappedValue, 10)
    for seconds in [0, 7, 30] {
      model.agentSendHoldSeconds.wrappedValue = seconds
      XCTAssertEqual(model.settings.agentSendHoldSec, UInt32(seconds))
      XCTAssertEqual(writes.last?.0, "CODESCRIBE_AGENT_SEND_HOLD_SEC")
      XCTAssertEqual(writes.last?.1, String(seconds))
    }
    let reopened = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
    XCTAssertEqual(reopened.agentSendHoldSeconds.wrappedValue, 30)
    model.agentSendHoldSeconds.wrappedValue = 31
    XCTAssertEqual(stored.agentSendHoldSec, 30)
    model.agentSendHoldSeconds.wrappedValue = -1
    XCTAssertEqual(stored.agentSendHoldSec, 0)
  }

  func testRejectedSettingWriteDoesNotInventANewSnapshot() {
    let model = SettingsViewModel(
      engine: MockSettingsEngine(updateConfigObserver: { _, _ in
        throw CocoaError(.fileWriteNoPermission)
      }), permissionProbe: MockPermissionProbe())
    model.agentSendHoldSeconds.wrappedValue = 0
    XCTAssertEqual(model.settings.agentSendHoldSec, 10)
    XCTAssertNotNil(model.lastError)
  }

  func testThreeControlCommandsCarryTheVerifiedOwnerAndOriginalBus() throws {
    let fixture = try Fixture()
    defer { try? FileManager.default.removeItem(at: fixture.home) }
    for control in OverlayDeliveryControl.allCases {
      let arguments = try RealAgentBridgeInstaller.deliveryControlArguments(
        control, identity: fixture.messageID, owner: fixture.owner,
        busPath: fixture.identity.bus, root: fixture.installer.bridgeRoot)
      XCTAssertEqual(arguments, [
        "--" + control.rawValue, fixture.messageID,
        "--channel", "1", "--provider", "junie", "--session", "held-fixture",
        "--lease", fixture.identity.leaseID, "--bus", fixture.identity.bus,
        "--bridge-home", fixture.installer.bridgeRoot.path,
      ])
      XCTAssertEqual(
        try RealAgentBridgeInstaller.deliveryControlArguments(
          control, identity: fixture.messageID, owner: nil,
          busPath: fixture.identity.bus, root: fixture.installer.bridgeRoot),
        [
          "--" + control.rawValue, fixture.messageID, "--channel", "0",
          "--bridge-home", fixture.installer.bridgeRoot.path,
        ])
    }
    for seconds in [UInt32(0), 7, 30] {
      let arguments = try RealAgentBridgeInstaller.busTextArguments(
        holdSeconds: seconds, owner: fixture.owner, root: fixture.installer.bridgeRoot)
      XCTAssertEqual(Array(arguments.prefix(3)), ["--send-text", "--hold-seconds", String(seconds)])
    }
    XCTAssertThrowsError(try RealAgentBridgeInstaller.deliveryControlArguments(
      .release, identity: fixture.messageID, owner: fixture.owner,
      busPath: fixture.home.appendingPathComponent("other.jsonl").path,
      root: fixture.installer.bridgeRoot))
    try fixture.writeLease(session: "foreign-session")
    XCTAssertThrowsError(try RealAgentBridgeInstaller.deliveryControlArguments(
      .withdraw, identity: fixture.messageID, owner: fixture.owner,
      busPath: fixture.identity.bus, root: fixture.installer.bridgeRoot))
  }

  func testComposerEscapeExitsRevisionModeWithoutSubmitting() throws {
    var cancelled = false
    var submitted = false
    let composer = OverlayConversationComposer(
      palette: .dark, draft: .constant("Poprawiona wiadomość"), sending: false,
      onSubmit: { submitted = true }, onCancel: { cancelled = true })
    let host = NSHostingView(rootView: composer)
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 480, height: 120)
    let window = NSWindow(
      contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    host.layoutSubtreeIfNeeded()
    func editor(in root: NSView) -> NSTextView? {
      if let text = root as? NSTextView, text.isEditable { return text }
      return root.subviews.lazy.compactMap { editor(in: $0) }.first
    }
    let text = try XCTUnwrap(editor(in: host))
    window.makeFirstResponder(text)
    let escape = try XCTUnwrap(NSEvent.keyEvent(
      with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
      windowNumber: window.windowNumber, context: nil,
      characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53))
    text.keyDown(with: escape)
    XCTAssertTrue(cancelled)
    XCTAssertFalse(submitted)
  }

  private struct Fixture {
    let home: URL
    let installer: RealAgentBridgeInstaller
    let identity: AgentPlaybackIdentity
    let owner: OverlayConversationOwner
    let messageID = String(repeating: "c", count: 24)

    init() throws {
      let repo = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
      home = repo.appendingPathComponent("build/test-scratch/\(UUID().uuidString)")
      try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
      installer = RealAgentBridgeInstaller(resourceRoot: nil, homeDirectory: home, environment: [:])
      identity = AgentPlaybackIdentity(
        provider: "junie", session: "held-fixture",
        bus: home.appendingPathComponent("fixture.jsonl").path)
      owner = try XCTUnwrap(OverlayConversationOwner(row: [
        "provider": identity.provider, "provider_session_id": identity.session,
        "lease_id": identity.leaseID, "channel": "1", "name": "fixture",
      ]))
      try writeLease(session: identity.session)
    }

    func writeLease(session: String) throws {
      let lease = installer.bridgeRoot.appendingPathComponent("leases/\(identity.leaseID).json")
      try FileManager.default.createDirectory(
        at: lease.deletingLastPathComponent(), withIntermediateDirectories: true)
      try JSONSerialization.data(withJSONObject: [
        "schema": "codescribe.agent-bridge.lease.v1", "provider": identity.provider,
        "provider_session_id": session, "lease_id": identity.leaseID, "bus": identity.bus,
      ]).write(to: lease)
    }
  }
}
