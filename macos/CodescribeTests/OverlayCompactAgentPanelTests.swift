import AppKit
import SwiftUI
import Vision
import XCTest

@testable import Codescribe

@MainActor
final class OverlayCompactAgentPanelTests: XCTestCase {
  private func fixture() throws -> (OverlayState, [OverlayConversation]) {
    let all = OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let inbox = OverlayConversation.humanInbox()
    let digits = ["1", "5", "6", "8"]
    let counts = [1, 42, 99, 156]
    let agents = try zip(digits, counts).map { digit, count in
      let session = "compact-fixture-\(digit)"
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": session, "channel": digit,
          "lease_id": AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: session),
          "name": "Agent \(digit)",
        ]))
      let messages = (0..<count).map { index in
        OverlayConversationMessage(
          id: "reply:compact-\(digit)-\(index)", kind: .reply,
          text: "Reply \(index + 1) from Agent \(digit)", order: UInt64(index + 1),
          emittedAt: "2026-10-30T06:00:00Z", owner: owner, recipients: [],
          deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
      }
      return OverlayConversation(
        id: owner.id, channel: digit, name: owner.name, owner: owner, messages: messages)
    }
    let inventory = [all, inbox] + agents
    let state = OverlayState()
    state.applyChannelRoster(
      inventory.filter { !$0.isHumanInbox }.map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: $0.channel == "6", loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: $0.owner == nil ? nil : true,
          unreadSinceUnixMs: nil)
      })
    state.applyConversationSnapshot(
      .init(
        deliveries: agents.map {
          .init(
            channel: $0.channel, agent: $0.name, deliveryID: "delivery-\($0.channel)",
            stage: $0.channel == "8" ? .queued : .received, isOpen: $0.channel == "6")
        }, conversations: inventory))
    state.selectConversation(inbox.id)
    state.chooseComposerTarget(agents[0])
    state.setAgentPanelPresentation(.compact)
    return (state, agents)
  }

  func testCompactSelectionAndUnreadNeverReplaceTheChannelStatus() throws {
    let (state, agents) = try fixture()
    let view = DictationOverlayView(state: state).channelStatusView
    let row = try XCTUnwrap(state.visibleChannelRows.first { $0.channel == "5" })
    let display = view.compactPillDisplay(agents[1], channel: row)
    XCTAssertEqual(display.dot, .received)
    XCTAssertEqual(display.unreadCount, 42)
    XCTAssertTrue(display.unread)
    XCTAssertFalse(display.selected)
    let inbox = try XCTUnwrap(state.conversations.first(where: \.isHumanInbox))
    XCTAssertTrue(view.compactPillDisplay(inbox, channel: nil).selected)
    XCTAssertNil(view.compactPillDisplay(inbox, channel: nil).dot)
    let generation = state.captureGeneration
    state.chooseConversation(agents[1].id)
    XCTAssertEqual(state.agentPanelPresentation, .compact)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertEqual(state.resolvedComposerTarget(viewing: agents[1])?.id, agents[0].id)
    XCTAssertFalse(state.conversationMicrophoneOpen(agents[1]))
    XCTAssertTrue(state.conversationMicrophoneOpen(agents[2]), "selection keeps the open channel")
    let selected = DictationOverlayView(state: state).channelStatusView
      .compactPillDisplay(agents[1], channel: row)
    XCTAssertTrue(selected.selected)
    XCTAssertEqual(selected.dot, .received)
    XCTAssertEqual(selected.unreadCount, 42)
  }

  func testPanelPresentationRemembersCompactAndFullyHiddenSeparately() throws {
    let suite = UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    defaults.set(false, forKey: "Overlay.agentSidebarVisible.v1")
    let state = OverlayState()
    state.restoreAgentSidebar(defaults: defaults)
    XCTAssertEqual(state.agentPanelPresentation, .compact)
    XCTAssertNil(defaults.object(forKey: "Overlay.agentSidebarVisible.v1"))
    state.setAgentPanelPresentation(.hidden)
    let restored = OverlayState()
    restored.restoreAgentSidebar(defaults: defaults)
    XCTAssertEqual(restored.agentPanelPresentation, .hidden)
    restored.setAgentPanelPresentation(.expanded)
    XCTAssertTrue(restored.showsAgentMonitor)
    restored.hideAgentSidebar()
    XCTAssertEqual(restored.agentPanelPresentation, .compact)
  }

  func testCompactGeometryKeepsCornerBadgesInsideSymmetricGutters() {
    for scale: CGFloat in [0.8, 1, 1.5, 2] {
      let button = OverlayChannelPill.compactButtonWidth(textScale: scale)
      let inset = OverlayAgentDrawerLayout.compactHorizontalInset(textScale: scale)
      let width = OverlayAgentDrawerLayout.compactWidth(textScale: scale)
      XCTAssertEqual(width, button + 2 * inset, accuracy: 0.01)
      for count in [1, 42, 99, 156] {
        let radius = OverlayAgentUnreadBadge.railDiameter(count: count, textScale: scale) / 2
        XCTAssertGreaterThanOrEqual(inset - radius, 4 * scale - 0.01)
      }
    }
  }

  func testCompactInboxSelectionIsCenteredAndCountsStayReadable() throws {
    for scale: CGFloat in [1, 1.5, 2] {
      let (state, _) = try fixture()
      let width = OverlayAgentDrawerLayout.compactWidth(textScale: scale)
      let view = OverlayChannelStatusView(
        channels: state.visibleChannelRows.filter { $0.channel != "0" },
        unavailable: false, palette: .dark, animates: false,
        hudStates: state.channelHudStates, conversations: state.conversations,
        selectedConversationID: state.selectedConversationID,
        unreadCounts: Dictionary(
          uniqueKeysWithValues: state.conversations.map {
            ($0.id, state.unreadReplies(in: $0))
          }),
        onSelectConversation: { id in
          if let id { state.chooseConversation(id) }
        })
      let host = NSHostingView(
        rootView: view.compactBody(
          maximumHeight: 680,
          paneHeaderHeight: OverlayAgentDrawerLayout.paneHeaderHeight(textScale: scale)
        )
        .frame(width: width, height: 680, alignment: .top)
        .environment(\.csTextScale, scale)
        .environment(\.colorScheme, .dark)
        .environment(\.layoutDirection, scale == 2 ? .rightToLeft : .leftToRight)
        .background(OverlayAppearancePalette.dark.desktopBackground.color))
      host.sizingOptions = []
      host.frame = CGRect(x: 0, y: 0, width: width, height: 680)
      let panel = NSPanel(
        contentRect: CGRect(x: -10000, y: -10000, width: width, height: 680),
        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
      panel.isReleasedWhenClosed = false
      panel.contentView = host
      BackgroundTestWindow.present(panel)
      defer {
        panel.contentView = nil
        panel.close()
      }
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.2))
      host.layoutSubtreeIfNeeded()
      let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
      host.cacheDisplay(in: host.bounds, to: bitmap)
      let image = NSImage(size: host.bounds.size)
      image.addRepresentation(bitmap)
      let attachment = XCTAttachment(image: image)
      attachment.name = "Compact-agents-selected-inbox-scale-\(scale)"
      attachment.lifetime = .keepAlways
      add(attachment)
      let ratio = CGFloat(bitmap.pixelsWide) / width
      var bands: [(Int, Int, Int, Int, Int)] = []
      var active: (Int, Int, Int, Int, Int)?
      for y in stride(from: 0, to: bitmap.pixelsHigh, by: 2) {
        var xs: [Int] = []
        for x in stride(from: 0, to: bitmap.pixelsWide, by: 2) {
          guard let c = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else { continue }
          if c.blueComponent > c.redComponent + 0.01,
            c.redComponent > c.greenComponent + 0.005
          {
            xs.append(x)
          }
        }
        if xs.count > Int(OverlayChannelPill.compactButtonWidth(textScale: scale) * ratio * 0.18) {
          let lo = xs.min()!
          let hi = xs.max()!
          if let a = active {
            active = (a.0, y, min(a.2, lo), max(a.3, hi), a.4 + xs.count)
          } else {
            active = (y, y, lo, hi, xs.count)
          }
        } else if let a = active {
          bands.append(a)
          active = nil
        }
      }
      if let a = active { bands.append(a) }
      let selected = try XCTUnwrap(bands.max { $0.4 < $1.4 })
      let center = CGFloat(selected.2 + selected.3) / 2 / ratio
      XCTAssertEqual(center, width / 2, accuracy: 1.5, "the selected Inbox pill is centered")
      if scale >= 1.5 {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.recognitionLanguages = ["en-US"]
        request.usesLanguageCorrection = false
        try VNImageRequestHandler(cgImage: try XCTUnwrap(bitmap.cgImage), options: [:]).perform([
          request
        ])
        let observations = request.results ?? []
        let text = observations.compactMap { $0.topCandidates(1).first?.string }.joined(
          separator: " ")
        XCTAssertTrue(text.contains("42"), text)
        XCTAssertTrue(text.contains("99"), text)
        for result in observations
        where result.topCandidates(1).first?.string.contains("42") == true {
          XCTAssertGreaterThan(result.boundingBox.minX, 0.01, "two-digit text clears the RTL edge")
          XCTAssertLessThan(result.boundingBox.maxX, 0.99, "two-digit text clears the edge")
        }
      }
    }
  }

  func testCompactPanelWindowReview() throws {
    try renderWindowReview(width: 900, scale: 1, selectsInbox: true)
  }

  func testCompactPanelNarrowConversationReview() throws {
    try renderWindowReview(width: 470, scale: 1.5, selectsInbox: false)
  }

  func testCompactPanelLightAppearanceReview() throws {
    try renderWindowReview(width: 900, scale: 1.5, selectsInbox: true, dark: false)
  }

  private func renderWindowReview(width: CGFloat, scale: CGFloat, selectsInbox: Bool) throws {
    let (state, agents) = try fixture()
    if !selectsInbox { state.chooseConversation(agents[1].id) }
    let host = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .frame(width: width, height: 680).environment(\.colorScheme, dark ? .dark : .light)
        .environment(\.csTextScale, scale)
        .transaction { $0.disablesAnimations = true })
    host.frame = CGRect(x: 0, y: 0, width: width, height: 680)
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.2))
    host.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let image = NSImage(size: host.bounds.size)
    image.addRepresentation(bitmap)
    let attachment = XCTAttachment(image: image)
    attachment.name =
      "Compact-panel-window-\(Int(width))-scale-\(scale)-inbox-\(selectsInbox)-dark-\(dark)"
    attachment.lifetime = .keepAlways
    add(attachment)
  }
}
