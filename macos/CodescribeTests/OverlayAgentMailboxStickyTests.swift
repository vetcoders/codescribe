import AppKit
import SwiftUI
import Vision
import XCTest

@testable import Codescribe

/// Scroll the real roster in an unshown panel and read its rendered pixels.
/// No desktop window, global AX query, microphone or clipboard is used.
@MainActor
final class OverlayAgentMailboxStickyTests: XCTestCase {
  func testInboxStaysVisibleWhenAgentsScrollInLight() throws {
    try assertPinnedInbox(palette: .light, scale: 1)
  }

  func testInboxStaysVisibleWhenAgentsScrollInDark() throws {
    try assertPinnedInbox(palette: .dark, scale: 1)
  }

  private func assertPinnedInbox(palette: OverlayAppearancePalette, scale: CGFloat) throws {
    let priorKeyWindow = NSApp.keyWindow
    var selections: [String?] = []
    var toggles: [UInt8] = []
    var conversations = [
      OverlayConversation(id: "channel:0", channel: "0", name: "All", owner: nil, messages: []),
      OverlayConversation(
        id: "10", channel: "10", name: "INBOX FIXTURE", owner: nil, messages: []),
    ]
    var channels: [OverlayChannelDelivery] = []
    var hud: [String: OverlayChannelHudProjection] = [:]
    for digit in 1...9 {
      let channel = String(digit)
      let name = digit == 9 ? "ZEBRA" : "Agent \(digit)"
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": "sticky-\(digit)",
          "lease_id": String(repeating: String(digit), count: 32),
          "channel": channel, "name": name,
        ]))
      conversations.append(
        OverlayConversation(id: owner.id, channel: channel, name: name, owner: owner, messages: []))
      channels.append(
        OverlayChannelDelivery(
          channel: channel, agent: name, deliveryID: nil, stage: nil, isOpen: false))
      hud[channel] = .init(
        open: false, loud: false, autosealDeadline: nil, followerAlive: true,
        provider: owner.provider, providerSessionID: owner.providerSessionID)
    }
    let roster = OverlayChannelStatusView(
      channels: channels, unavailable: false, palette: palette, animates: false,
      hudStates: hud, onToggleChannel: { toggles.append($0) }, conversations: conversations,
      onSelectConversation: { selections.append($0) })
    XCTAssertEqual(roster.currentConversations.filter(\.isHumanInbox).count, 1)
    let root = roster.monitorBody(maximumHeight: 360)
      .environment(\.csTextScale, scale)
      .frame(width: 400)
      .background(palette.desktopBackground.color)
      .preferredColorScheme(palette.appearance == .dark ? .dark : .light)
    let host = NSHostingView(rootView: root)
    host.frame = NSRect(x: 0, y: 0, width: 400, height: 360)
    let panel = NSPanel(
      contentRect: host.frame, styleMask: [.borderless, .nonactivatingPanel],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.contentView = host
    defer { panel.close() }
    settle(host)
    let scroll = try XCTUnwrap(scrollViews(host).first)
    let document = try XCTUnwrap(scroll.documentView)
    let distance = document.bounds.height - scroll.contentView.bounds.height
    XCTAssertGreaterThan(distance, 100, "the production agents must actually overflow")
    let beforeOrigin = scroll.contentView.bounds.origin.y
    let before = try snapshot(host, name: "before-\(palette.appearance)")
    let inboxBefore = try XCTUnwrap(textBox("INBOX FIXTURE", in: before))
    scroll.contentView.scroll(to: NSPoint(x: 0, y: distance))
    scroll.reflectScrolledClipView(scroll.contentView)
    settle(host)
    XCTAssertGreaterThan(abs(scroll.contentView.bounds.origin.y - beforeOrigin), 40)
    let after = try snapshot(host, name: "after-\(palette.appearance)")
    let inboxAfter = try XCTUnwrap(textBox("INBOX FIXTURE", in: after))
    XCTAssertEqual(inboxBefore.midY, inboxAfter.midY, accuracy: 0.015)
    XCTAssertNotNil(
      textBox("ZEBRA", in: after),
      "the agent list really reached its end: \(after.compactMap { $0.topCandidates(1).first?.string })"
    )
    XCTAssertTrue(selections.isEmpty)
    XCTAssertTrue(toggles.isEmpty)
    XCTAssertFalse(panel.isVisible)
    XCTAssertTrue(NSApp.keyWindow === priorKeyWindow)
  }

  private func settle(_ view: NSView) {
    view.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.15))
    view.layoutSubtreeIfNeeded()
  }

  private func scrollViews(_ view: NSView) -> [NSScrollView] {
    if let scroll = view as? NSScrollView { return [scroll] }
    return view.subviews.flatMap(scrollViews)
  }

  private func snapshot(_ view: NSView, name: String) throws -> [VNRecognizedTextObservation] {
    let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
    view.cacheDisplay(in: view.bounds, to: bitmap)
    let image = try XCTUnwrap(bitmap.cgImage)
    let attachment = XCTAttachment(image: NSImage(cgImage: image, size: view.bounds.size))
    attachment.name = name
    attachment.lifetime = .keepAlways
    add(attachment)
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .fast
    request.usesLanguageCorrection = false
    request.recognitionLanguages = ["en-US"]
    request.customWords = ["INBOX FIXTURE", "ZEBRA"]
    try VNImageRequestHandler(cgImage: image).perform([request])
    return request.results ?? []
  }

  private func textBox(_ text: String, in rows: [VNRecognizedTextObservation]) -> CGRect? {
    rows.first {
      $0.topCandidates(1).first?.string.localizedCaseInsensitiveContains(text) == true
    }?.boundingBox
  }
}
