import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Falsifiers for the agents-drawer cut (Founder, 2026-10-16; mockup
/// `szuflada-agentow.html`): the roster docks as a right-hand column on wide
/// windows, becomes a pill stack under the name pill on narrow ones, and the
/// bubble cap follows ⌘+ text scale. RED before, GREEN after.
@MainActor
final class OverlayAgentDrawerDockingTests: XCTestCase {

  // MARK: - Fixtures

  private func overlaySource(_ name: String) throws -> String {
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent()
    return try String(
      contentsOf: root.appendingPathComponent("Codescribe/Screens/Overlay/\(name)"),
      encoding: .utf8)
  }

  private func section(of source: String, from: String, to: String) throws -> String {
    let start = try XCTUnwrap(source.range(of: from), "missing section start: \(from)")
    let end = try XCTUnwrap(
      source.range(of: to, range: start.upperBound..<source.endIndex),
      "missing section end: \(to)")
    return String(source[start.lowerBound..<end.lowerBound])
  }

  private func ownerConversation(
    _ name: String, channel: String, session: String, lease: String, lastReply: String? = nil
  ) throws -> OverlayConversation {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session,
        "lease_id": String(repeating: lease, count: 32), "channel": channel, "name": name,
      ]))
    var messages: [OverlayConversationMessage] = []
    if let lastReply {
      messages.append(
        OverlayConversationMessage(
          id: "reply:\(name)-1", kind: .reply, text: lastReply, order: 1,
          emittedAt: "2026-10-16T14:53:00Z", owner: owner, recipients: [],
          deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: ""))
    }
    return OverlayConversation(
      id: owner.id, channel: channel, name: name, owner: owner, messages: messages)
  }

  private func hud(session: String) -> OverlayChannelHudProjection {
    OverlayChannelHudProjection(
      open: false, loud: false, autosealDeadline: nil, followerAlive: true,
      provider: "codex", providerSessionID: session)
  }

  private func rosterFixture() throws -> (
    broadcast: OverlayConversation, lena: OverlayConversation, astra: OverlayConversation
  ) {
    let broadcast = OverlayConversation(
      id: "dock-all", channel: "0", name: "All", owner: nil, messages: [])
    let lena = try ownerConversation(
      "lena", channel: "2", session: "lena-session", lease: "a", lastReply: "PR is green")
    let astra = try ownerConversation(
      "astra", channel: "3", session: "astra-session", lease: "b", lastReply: "on it")
    return (broadcast, lena, astra)
  }

  // MARK: - Threshold (deliverables 1 and 3)

  func testDockingThresholdFollowsTextScale() {
    XCTAssertTrue(
      OverlayAgentDrawerLayout.isDocked(windowWidth: 1200, textScale: 1.0),
      "1200 at 1.0 docks")
    XCTAssertFalse(
      OverlayAgentDrawerLayout.isDocked(windowWidth: 800, textScale: 1.0),
      "800 at 1.0 falls back to pills")
    XCTAssertFalse(
      OverlayAgentDrawerLayout.isDocked(windowWidth: 1200, textScale: 1.6),
      "1200 at 1.6 needs 1312 — pills, so large text never squeezes bubbles")
    XCTAssertTrue(
      OverlayAgentDrawerLayout.isDocked(windowWidth: 820, textScale: 1.0),
      "the exact 820 × scale boundary docks")
    XCTAssertTrue(OverlayAgentDrawerLayout.isDocked(windowWidth: 1312, textScale: 1.6))
    XCTAssertFalse(OverlayAgentDrawerLayout.isDocked(windowWidth: 819.9, textScale: 1.0))
    XCTAssertEqual(OverlayAgentDrawerLayout.drawerWidth(textScale: 1.4), 420, accuracy: 0.001)
    XCTAssertEqual(OverlayAgentDrawerLayout.pillWidth(textScale: 1.0), 224, accuracy: 0.001)
  }

  func testBubbleCapScalesWithTextAndYieldsToTheWindow() {
    XCTAssertEqual(
      OverlayConversationView.bubbleCap(contentWidth: 2000, textScale: 1.0), 660,
      accuracy: 0.001)
    XCTAssertEqual(
      OverlayConversationView.bubbleCap(contentWidth: 2000, textScale: 1.4), 924,
      accuracy: 0.001, "⌘+ grows the cap with the letters")
    XCTAssertEqual(
      OverlayConversationView.bubbleCap(contentWidth: 700, textScale: 1.0), (700 - 40) * 0.82,
      accuracy: 0.001)
    XCTAssertEqual(
      OverlayConversationView.bubbleCap(contentWidth: 700, textScale: 1.4), (700 - 40) * 0.82,
      accuracy: 0.001, "the window bound wins regardless of scale")
  }

  // MARK: - Pill stack content (deliverable 2)

  func testPillStackListsEveryConversationExceptTheSelectedOneInRosterOrder() throws {
    let (broadcast, lena, astra) = try rosterFixture()
    var quietDelivery = OverlayChannelDelivery(
      channel: "3", agent: "astra", deliveryID: "receipt-3", stage: .received, isOpen: false)
    quietDelivery.broadcastPolicy = .quiet
    let view = OverlayChannelStatusView(
      channels: [
        OverlayChannelDelivery(
          channel: "2", agent: "lena", deliveryID: "receipt-2", stage: .received, isOpen: false),
        quietDelivery,
      ],
      unavailable: false, palette: .dark, animates: false,
      hudStates: ["2": hud(session: "lena-session"), "3": hud(session: "astra-session")],
      conversations: [broadcast, lena, astra], selectedConversationID: lena.id,
      unreadCounts: [astra.id: 4], onSelectConversation: { _ in })
    // Production wires the transcription target; the pill exists only with it.
    var stackView = view
    stackView.onShowTranscription = {}

    let entries = stackView.pillEntries
    XCTAssertEqual(
      entries.map(\.id), ["transcription", broadcast.id, astra.id],
      "transcription first, then the roster order, never the selected conversation")

    let zero = try XCTUnwrap(entries.first { $0.id == broadcast.id })
    XCTAssertEqual(zero.title, String(localized: "0 · All"))
    XCTAssertTrue(zero.isBroadcast)
    let agent = try XCTUnwrap(entries.first { $0.id == astra.id })
    XCTAssertEqual(agent.title, "3 · astra")
    XCTAssertEqual(agent.descriptor, "codex", "the pill shows the provider descriptor")
    XCTAssertEqual(agent.unread, 4)
    XCTAssertNotNil(agent.quietSymbol, "a quiet broadcast policy marks the pill")
    let transcription = try XCTUnwrap(entries.first { $0.id == "transcription" })
    XCTAssertNil(transcription.conversationID, "the transcription pill selects nil")
  }

  func testPillSelectionChangesTheConversationAndClosesTheStack() throws {
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(broadcast.id)
    state.showAgentMonitor()
    XCTAssertTrue(state.showsAgentMonitor)

    state.selectConversation(lena.id)

    XCTAssertEqual(state.selectedConversationID, lena.id)
    XCTAssertFalse(state.showsAgentMonitor, "selecting a pill closes the stack")
  }

  func testPillStackClosesOnEscapeAndDimsTheConversation() throws {
    // Esc and the click-away both route to hideAgentSidebar.
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(lena.id)
    state.showAgentMonitor()
    XCTAssertTrue(state.showsAgentMonitor)
    state.hideAgentSidebar()
    XCTAssertFalse(state.showsAgentMonitor)
    XCTAssertEqual(state.selectedConversationID, lena.id, "closing keeps the conversation")

    let conversation = try overlaySource("OverlayConversationView.swift")
    // The stack mounts in the top overlay, directly under the name pill row.
    let topOverlay = try section(
      of: conversation, from: ".overlay(alignment: .top)", to: "private var composer")
    XCTAssertTrue(
      topOverlay.contains("if let pillStack = agentPillStack"),
      "the stack mounts under the name pill")
    XCTAssertTrue(
      conversation.contains("overlay-agent-pill-dismiss"),
      "a click on the conversation closes the stack")
    XCTAssertTrue(conversation.contains("0.42"), "the conversation dims to 42% while open")
    XCTAssertTrue(topOverlay.contains(".onExitCommand"), "Esc closes the stack")
    let roster = try overlaySource("OverlayChannelStatusView.swift")
    XCTAssertTrue(
      roster.contains("overlay-agent-pill-stack"), "the pill stack carries its identifier")
  }

  // MARK: - Docked column (deliverables 1 and 4)

  func testDockingEngagementClosesThePillStackAndKeepsSelectionAndDraft() throws {
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(lena.id)
    state.conversationDrafts[lena.id] = "half-typed draft"
    state.showAgentMonitor()
    XCTAssertTrue(state.showsAgentMonitor)
    let focusRevision = state.conversationFocusRevision

    state.setAgentDrawerDocked(true)

    XCTAssertFalse(state.showsAgentMonitor, "the pill stack closes when docking engages")
    XCTAssertEqual(state.selectedConversationID, lena.id, "the selection survives the switch")
    XCTAssertEqual(state.conversationDrafts[lena.id], "half-typed draft", "drafts survive")
    XCTAssertEqual(
      state.conversationFocusRevision, focusRevision,
      "no scroll-reset signal crosses a pure mode switch")

    state.setAgentDrawerDocked(false)
    XCTAssertFalse(state.showsAgentMonitor, "undocking does not reopen the stack")
  }

  func testDockedNamePillFocusesTheSelectedRowInsteadOfOpening() throws {
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(lena.id)
    state.setAgentDrawerDocked(true)
    let focus = state.agentDrawerFocusRevision

    state.showAgentMonitor()

    XCTAssertFalse(state.showsAgentMonitor, "docked: nothing opens, the column is already there")
    XCTAssertEqual(
      state.agentDrawerFocusRevision, focus + 1, "docked: the click focuses the current row")
  }

  func testDockedDrawerReservesTheScaledColumnBesideTheConversation() throws {
    for (width, scale, drawerWidth) in [(1200.0, 1.0, 300.0), (1400.0, 1.4, 420.0)]
      as [(CGFloat, CGFloat, CGFloat)]
    {
      let (broadcast, lena, _) = try rosterFixture()
      let state = OverlayState.previewFormatted()
      state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
      state.selectConversation(lena.id)
      let recorder = OverlayDrawerFrameRecorder()
      let host = NSHostingView(
        rootView:
          DictationOverlayView(state: state)
          .environment(\.csTextScale, scale)
          .onPreferenceChange(OverlayDrawerFramePreferenceKey.self) { recorder.frame = $0 }
          .frame(width: width, height: 500))
      host.sizingOptions = []
      let panel = NSPanel(
        contentRect: CGRect(x: 0, y: 0, width: width, height: 500),
        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
      panel.contentView = host
      panel.orderFrontRegardless()
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      host.layoutSubtreeIfNeeded()

      let frame = recorder.frame
      XCTAssertEqual(
        frame.width, drawerWidth, accuracy: 1,
        "width \(width) scale \(scale): the column is 300 × scale")
      XCTAssertEqual(
        frame.minX, width - drawerWidth, accuracy: 1,
        "the column owns the right edge; the conversation keeps width − 300 × scale")
      XCTAssertEqual(
        frame.maxY, 500, accuracy: 1, "the column runs the full height under the header")
      XCTAssertEqual(
        state.selectedConversationID, lena.id, "layout never disturbs the selection")
      panel.orderOut(nil)
    }

    // Narrow: no docked column at all, and the pill stack publishes its own frame.
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(lena.id)
    let recorder = OverlayDrawerFrameRecorder()
    let host = NSHostingView(
      rootView:
        DictationOverlayView(state: state)
        .onPreferenceChange(OverlayDrawerFramePreferenceKey.self) { recorder.frame = $0 }
        .frame(width: 800, height: 500))
    host.sizingOptions = []
    let panel = NSPanel(
      contentRect: CGRect(x: 0, y: 0, width: 800, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.contentView = host
    panel.orderFrontRegardless()
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    host.layoutSubtreeIfNeeded()
    XCTAssertEqual(recorder.frame, .zero, "800 wide at 1.0: no docked column")
    panel.orderOut(nil)
  }

  // MARK: - Row model and broadcast regression (deliverable 1, 878667f9f)

  func testDockedRowsCarryTimePreviewAndBadgeAndMoveDescriptorToTooltip() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let row = try section(
      of: source, from: "private func channelRow(", to: "private func conversationRow(")
    XCTAssertTrue(row.contains("lineLimit(2)"), "rows carry a two-line message preview")
    XCTAssertTrue(row.contains("drawerPreview("), "rows render the last message preview")
    XCTAssertFalse(
      row.contains("overlay-agent-descriptor-"), "the descriptor leaves the row body")
    XCTAssertTrue(row.contains(".help("), "descriptor and status live in the row tooltip")
    XCTAssertTrue(
      row.contains("overlay-channel-delivery-"), "the status dot keeps the delivery identifier")
    XCTAssertTrue(row.contains("overlay-agent-row-"), "row identifier kept")
  }

  func testDrawerPreviewPrefixesOwnMessagesAndChannelZeroSenders() throws {
    let (broadcast, lena, _) = try rosterFixture()
    let own = OverlayConversationMessage(
      id: "user:1", kind: .user, text: "first line\nsecond line", order: 1,
      emittedAt: "2026-10-16T14:54:00Z", owner: nil, recipients: [],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
    XCTAssertEqual(
      OverlayChannelStatusView.drawerPreview(message: own, channel: lena.channel),
      "You: first line second line", "own messages carry the localized prefix, one line")
    let reply = lena.messages[0]
    XCTAssertEqual(
      OverlayChannelStatusView.drawerPreview(message: reply, channel: broadcast.channel),
      "lena: PR is green", "channel 0 names the sender")
    XCTAssertEqual(
      OverlayChannelStatusView.drawerPreview(message: reply, channel: lena.channel),
      "PR is green", "an agent's own chat needs no prefix")
  }

  func testBroadcastPolicyMenuSitsUnderTheAllRowInDockedAndInThePillStack() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let monitor = try section(
      of: source, from: "func monitorBody(maximumHeight:", to: "var currentConversations")
    XCTAssertTrue(
      monitor.contains("OverlayBroadcastDisclosure("),
      "the docked column keeps the 878667f9f menu under 0 · All")
    let pills = try XCTUnwrap(
      source.range(of: "struct OverlayAgentPillStack"), "the pill stack exists")
    XCTAssertTrue(
      String(source[pills.lowerBound...]).contains("OverlayBroadcastDisclosure("),
      "the 0 · All pill entry carries the same broadcast policy menu")
  }

  // MARK: - Hosting structure (deliverables 1 and 4)

  func testDockedColumnIsASiblingNotAnOverlayAndTheFloatingCardIsGone() throws {
    let overlay = try overlaySource("DictationOverlayView.swift")
    let canvas = try section(
      of: overlay, from: "private func canvasStack", to: "private var header: some View")
    XCTAssertFalse(
      canvas.contains("overlay-agent-drawer-dismiss"), "the floating card's backdrop is gone")
    // The roster enters the canvas only as the docked HStack sibling; the
    // actions handle's badge is the one remaining topTrailing overlay and it
    // stays empty of roster content.
    XCTAssertTrue(canvas.contains("agentDrawerColumn(maximumHeight: geometry.size.height)"))
    var rest = canvas
    while let range = rest.range(of: ".overlay(alignment: .topTrailing)") {
      let window = rest[range.lowerBound...].prefix(600)
      XCTAssertFalse(window.contains("monitorBody"), "the roster is not a floating overlay")
      XCTAssertFalse(window.contains("overlay-agent-sidebar"))
      rest = String(rest[range.upperBound...])
    }
    let column = try section(
      of: overlay, from: "private func agentDrawerColumn", to: "// MARK: Header")
    XCTAssertTrue(
      column.contains("channelStatusView.monitorBody"), "the column hosts the same roster body")
    XCTAssertTrue(column.contains("overlay-agent-sidebar"))
    XCTAssertTrue(
      canvas.contains("setAgentDrawerDocked"), "the canvas reports docked mode to the state")
    let conversation = try overlaySource("OverlayConversationView.swift")
    XCTAssertTrue(
      conversation.contains("OverlayAgentDrawerLayout.pillWidth(textScale: textScale)"),
      "the name pill and the stack share 224 × scale")
  }
}

@MainActor
private final class OverlayDrawerFrameRecorder {
  var frame = CGRect.zero
}
