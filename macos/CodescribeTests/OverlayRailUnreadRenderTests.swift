import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Review render of the folded agents rail's unread counts (1, 9, 10, 99),
/// drawn the way the overlay mounts it: `OverlayChannelStatusView.railBody`
/// inside the rail's own width (`OverlayAgentDrawerLayout.railWidth`) and
/// the resize band's insets on both sides, with channel 0 under its antenna,
/// your inbox's envelope and the agents on 3, 4 and 5. Channel 0 carries
/// 1 unread reply, 3 carries 9, 4 carries 10 and 5 carries 99; the inbox has
/// none. Every scene renders in dark and in light.
///
/// The assertions only make sure the rail projects the intended counts; the
/// image is judged by a person. Each PNG is attached
/// (`rail-unread-<scene>-<dark|light>`, kept always) and, when
/// `CODESCRIBE_RENDER_DIR` is set, written there as `<name>.png` (the
/// directory is created, files are overwritten). Xcode forwards only
/// `TEST_RUNNER_` variables into the test host, so under `make test-swift`
/// export `TEST_RUNNER_CODESCRIBE_RENDER_DIR=<dir>`. Same rendering path as
/// `OverlayBarReviewRenderTests`: `cacheDisplay` of an off-screen window, no
/// behind-window glass.
@MainActor
final class OverlayRailUnreadRenderTests: XCTestCase {

  private enum Look: String, CaseIterable {
    case dark, light

    var scheme: ColorScheme { self == .dark ? .dark : .light }
    var palette: OverlayAppearancePalette { self == .dark ? .dark : .light }
    var appearance: NSAppearance? { NSAppearance(named: self == .dark ? .darkAqua : .aqua) }
  }

  // MARK: - Fixtures

  private func ownerConversation(
    _ name: String, channel: String, lease: String
  ) throws -> OverlayConversation {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "review-\(channel)",
        "lease_id": String(repeating: lease, count: 32), "channel": channel, "name": name,
      ]))
    return OverlayConversation(
      id: owner.id, channel: channel, name: name, owner: owner, messages: [])
  }

  // MARK: - Rendering

  private func image(of view: NSView) throws -> NSImage {
    let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
    view.cacheDisplay(in: view.bounds, to: bitmap)
    let image = NSImage(size: view.bounds.size)
    image.addRepresentation(bitmap)
    return image
  }

  /// Attaches the image and, with `CODESCRIBE_RENDER_DIR` set, writes it there
  /// as `<name>.png`, replacing an earlier render.
  private func save(_ image: NSImage, name: String) throws {
    let attachment = XCTAttachment(image: image)
    attachment.name = name
    attachment.lifetime = .keepAlways
    add(attachment)
    guard
      let path = ProcessInfo.processInfo.environment["CODESCRIBE_RENDER_DIR"]?
        .trimmingCharacters(in: .whitespacesAndNewlines), !path.isEmpty
    else { return }
    let directory = URL(fileURLWithPath: path, isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let bitmap = try XCTUnwrap(
      image.representations.compactMap { $0 as? NSBitmapImageRep }.first, "\(name): no bitmap")
    let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]), name)
    try png.write(to: directory.appendingPathComponent("\(name).png"), options: .atomic)
  }

  /// A SwiftUI view alone on the palette's sheet colour.
  private func renderAlone(
    _ content: some View, size: CGSize, look: Look, name: String
  ) throws {
    let host = NSHostingView(
      rootView:
        content
        .frame(width: size.width, height: size.height, alignment: .topLeading)
        .background(look.palette.desktopBackground.color)
        .environment(\.colorScheme, look.scheme)
        .transaction { $0.disablesAnimations = true })
    host.frame = NSRect(origin: .zero, size: size)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.appearance = look.appearance
    window.contentView = host
    BackgroundTestWindow.present(window)
    defer { window.close() }
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.12))
    host.layoutSubtreeIfNeeded()
    try save(try image(of: host), name: "\(name)-\(look.rawValue)")
  }

  // MARK: - Folded rail

  /// The rail at its own width, 200 pt tall: 0 · envelope · 3 · 4 · 5.
  func testRenderFoldedRailUnreadCounts() throws {
    let all = OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let inbox = OverlayConversation.humanInbox(messages: [])
    let agents = [("3", "lena"), ("4", "kaja"), ("5", "olek")]
    let owned = try agents.enumerated().map { index, agent in
      try ownerConversation(agent.1, channel: agent.0, lease: String(index, radix: 16))
    }
    let conversations = [all, inbox] + owned
    let counts = ["0": 1, "3": 9, "4": 10, "5": 99]
    var unreadCounts: [String: Int] = [:]
    for conversation in [all] + owned {
      unreadCounts[conversation.id] = counts[conversation.channel]
    }
    var hudStates: [String: OverlayChannelHudProjection] = [:]
    var channels: [OverlayChannelDelivery] = []
    for (digit, name) in agents {
      hudStates[digit] = OverlayChannelHudProjection(
        open: false, loud: false, autosealDeadline: nil, followerAlive: true,
        provider: "codex", providerSessionID: "review-\(digit)")
      var delivery = OverlayChannelDelivery(
        channel: digit, agent: name, deliveryID: nil, stage: nil, isOpen: false)
      delivery.broadcastIdentity = AgentPlaybackIdentity(
        provider: "codex", session: "review-\(digit)", bus: "/dev/null")
      delivery.broadcastPolicy = .on
      channels.append(delivery)
    }

    for look in Look.allCases {
      let roster = OverlayChannelStatusView(
        channels: channels, unavailable: false, palette: look.palette, animates: false,
        hudStates: hudStates, conversations: conversations,
        unreadCounts: unreadCounts, onSelectConversation: { _ in })
      XCTAssertEqual(roster.railPillDisplay(all, channel: nil).unreadCount, 1)
      XCTAssertEqual(roster.railPillDisplay(inbox, channel: nil).unreadCount, 0)
      XCTAssertEqual(roster.railPillDisplay(inbox, channel: nil).symbol, "envelope")
      for (index, channel) in channels.enumerated() {
        let display = roster.railPillDisplay(owned[index], channel: channel)
        XCTAssertEqual(display.figure, channel.channel)
        XCTAssertEqual(display.unreadCount, counts[channel.channel])
      }
      // As `DictationOverlayView.agentRail` mounts it: the rail's width with
      // the resize band's inset on both sides.
      let width = OverlayAgentDrawerLayout.railWidth(textScale: 1)
      try renderAlone(
        roster.railBody(maximumHeight: 200)
          .padding(.horizontal, OverlayResizeHit.band)
          .frame(width: width),
        size: CGSize(width: ceil(width), height: 200), look: look, name: "rail-unread-1-9-10-99")
    }
  }
}
