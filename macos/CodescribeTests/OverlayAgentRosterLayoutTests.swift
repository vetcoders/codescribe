import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayAgentRosterLayoutTests: XCTestCase {
  func testClockStaysAtTrailingEdgeAcrossWidthsAndTextScales() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    for width in [CGFloat(240), 300, 400] {
      for scale in [CGFloat(1), 1.5, 2] {
        let host = NSHostingView(
          rootView: OverlayAgentRosterLabel(
            title: "3 · Klaudiusz", channel: "3", conversationID: "fixture",
            descriptor: "Codex · model · repository",
            preview: "Founder → Astra: a message preview that can wrap onto two lines.",
            timestamp: Date(timeIntervalSince1970: 1_792_512_000), unreadCount: 8,
            dot: .idle, statusLabel: "Attached", statusDetail: "Fixture",
            broadcastPolicy: nil, palette: .dark, textScale: scale
          ).frame(width: width))
        let window = show(host, width: width)
        defer { window.close() }
        let clock = try XCTUnwrap(
          NativeAccessibilityProbe.waitForElements("overlay-agent-timestamp-fixture").elements.first
        )
        let title = try XCTUnwrap(
          NativeAccessibilityProbe.waitForElements("overlay-agent-title-3").elements.first)
        XCTAssertEqual(clock.frame.maxX, window.frame.maxX, accuracy: 1)
        XCTAssertGreaterThan(
          title.width, 0, "the identity retains space at \(width), scale \(scale)")
        XCTAssertLessThanOrEqual(title.frame.maxX, clock.frame.minX + 1)
        XCTAssertGreaterThan(clock.height, 0)
      }
    }
  }

  func testActionButtonsSitBelowThePrimaryLabel() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "roster-layout",
        "lease_id": String(repeating: "a", count: 32), "channel": "3", "name": "Astra",
      ]))
    let conversation = OverlayConversation(
      id: owner.id, channel: "3", name: "Astra", owner: owner, messages: [])
    let channel = OverlayChannelDelivery(
      channel: "3", agent: "Astra", deliveryID: nil, stage: nil, isOpen: false)
    var row = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "3": OverlayChannelHudProjection(
          open: false, loud: false, autosealDeadline: nil, followerAlive: true,
          provider: owner.provider, providerSessionID: owner.providerSessionID, frameDetected: true)
      ],
      onToggleChannel: { _ in }, conversations: [conversation], onSelectConversation: { _ in })
    row.archiveCandidates = ["3": owner]
    row.onArchiveAgent = { _ in }
    row.panelFocusAvailable = true
    row.onFocusAgentPanel = { _ in }
    for width in [CGFloat(240), 300, 400] {
      let host = NSHostingView(rootView: row.channelRow(channel).frame(width: width))
      let window = show(host, width: width)
      defer { window.close() }
      let label = try XCTUnwrap(
        NativeAccessibilityProbe.waitForElements("overlay-view-conversation-\(owner.id)")
          .elements.first)
      let controls = try NativeAccessibilityProbe.waitForElements("overlay-agent-row-controls-3")
      XCTAssertFalse(controls.elements.isEmpty, controls.trace.joined(separator: "\n"))
      XCTAssertGreaterThan(label.width, width * 0.8, "controls do not reserve a full-height column")
      // Native AX coordinates grow down from the top of the screen.
      for control in controls.elements {
        XCTAssertGreaterThanOrEqual(control.y, label.frame.maxY - 1)
        XCTAssertLessThanOrEqual(control.frame.maxX, window.frame.maxX + 1)
      }
    }
  }

  func testFirstRevealHasRailBeforeAnyConversationWasOpened() {
    for width in [CGFloat(700), 1200] {
      let state = OverlayState.previewFormatted()
      XCTAssertNil(state.selectedConversationID)
      let recorder = FirstRevealRailRecorder()
      let host = NSHostingView(
        rootView: DictationOverlayView(state: state)
          .onPreferenceChange(OverlayAgentRailFramePreferenceKey.self) { recorder.frame = $0 }
          .frame(width: width, height: 500))
      host.sizingOptions = []
      // This case measures first-layout preferences, not AX. Keep it away
      // from the physical pointer, whose legitimate rail hover opens the list.
      let window = show(host, width: width, origin: NSPoint(x: -10000, y: -10000))
      defer { window.close() }
      XCTAssertFalse(state.showsAgentMonitor)
      XCTAssertEqual(
        recorder.frame.width, OverlayAgentDrawerLayout.railWidth(textScale: 1), accuracy: 1)
      XCTAssertEqual(recorder.frame.maxX, width, accuracy: 1)
      state.unfoldAgentList()
      settle(host)
      XCTAssertTrue(state.showsAgentMonitor)
      state.foldAgentList()
      settle(host)
      XCTAssertEqual(
        recorder.frame.width, OverlayAgentDrawerLayout.railWidth(textScale: 1), accuracy: 1)
      XCTAssertNil(state.selectedConversationID)
    }
  }

  private func show<Content: View>(
    _ host: NSHostingView<Content>, width: CGFloat, origin: NSPoint = NSPoint(x: 150, y: 150)
  ) -> NSWindow {
    let window = NSWindow(
      contentRect: NSRect(origin: origin, size: NSSize(width: width, height: 500)),
      styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    BackgroundTestWindow.present(window)
    settle(host)
    return window
  }

  private func settle(_ host: NSView) {
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    host.layoutSubtreeIfNeeded()
  }
}

@MainActor
private final class FirstRevealRailRecorder {
  var frame = CGRect.zero
}
