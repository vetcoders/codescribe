import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayAgentRosterLayoutTests: XCTestCase {
  func testClockStaysAtTrailingEdgeAcrossWidthsAndTextScales() throws {
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
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "roster-layout",
        "lease_id": String(repeating: "a", count: 32), "channel": "3", "name": "Astra",
      ]))
    let conversation = OverlayConversation(
      id: owner.id, channel: "3", name: "Astra", owner: owner, messages: [])
    let channel = OverlayChannelDelivery(
      channel: "3", agent: "Astra", deliveryID: nil, stage: nil, isOpen: false)
    let row = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .dark, animates: false,
      onToggleChannel: { _ in }, conversations: [conversation], onSelectConversation: { _ in })
    for width in [CGFloat(240), 300, 400] {
      let host = NSHostingView(rootView: row.channelRow(channel).frame(width: width))
      let window = show(host, width: width)
      defer { window.close() }
      let label = try XCTUnwrap(
        NativeAccessibilityProbe.waitForElements("overlay-view-conversation-\(owner.id)")
          .elements.first)
      let microphone = try XCTUnwrap(
        NativeAccessibilityProbe.waitForElements("overlay-agent-microphone").elements.first)
      XCTAssertGreaterThan(label.width, width * 0.8, "controls do not reserve a full-height column")
      // Native AX coordinates grow down from the top of the screen.
      XCTAssertGreaterThanOrEqual(microphone.y, label.frame.maxY - 1)
      XCTAssertLessThanOrEqual(microphone.frame.maxX, window.frame.maxX + 1)
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
      let window = show(host, width: width)
      defer { window.close() }
      XCTAssertEqual(recorder.frame.width, 44, accuracy: 1)
      XCTAssertEqual(recorder.frame.maxX, width, accuracy: 1)
      state.unfoldAgentList()
      settle(host)
      XCTAssertTrue(state.showsAgentMonitor)
      state.foldAgentList()
      settle(host)
      XCTAssertEqual(recorder.frame.width, 44, accuracy: 1)
      XCTAssertNil(state.selectedConversationID)
    }
  }

  private func show<Content: View>(_ host: NSHostingView<Content>, width: CGFloat) -> NSWindow {
    let window = NSWindow(
      contentRect: NSRect(x: 150, y: 150, width: width, height: 500),
      styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    window.orderFrontRegardless()
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
