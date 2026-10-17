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
    let entries = view.pillEntries
    XCTAssertEqual(
      entries.map(\.id), [broadcast.id, astra.id],
      "0 · All first, then the roster order; never the selected one, never a transcription row")

    let zero = try XCTUnwrap(entries.first { $0.id == broadcast.id })
    XCTAssertEqual(zero.title, String(localized: "0 · All"))
    XCTAssertTrue(zero.isBroadcast)
    let agent = try XCTUnwrap(entries.first { $0.id == astra.id })
    XCTAssertEqual(agent.title, "3 · astra")
    XCTAssertEqual(agent.descriptor, "codex", "the pill shows the provider descriptor")
    XCTAssertEqual(agent.unread, 4)
    XCTAssertNotNil(agent.quietSymbol, "a quiet broadcast policy marks the pill")
    XCTAssertEqual(zero.dot, .broadcast, "the shared channel wears the antenna, not a delivery dot")
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

  func testPillStackClosesOnEscapeAndStealsNeitherColorNorScroll() throws {
    // Maciej, 2026-10-17: the open stack steals neither color nor scrolling.
    // Esc and the name pill's toggle route to hideAgentSidebar.
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
    XCTAssertFalse(
      conversation.contains("overlay-agent-pill-dismiss"),
      "no full-area catcher: the conversation keeps its clicks and its scroll")
    XCTAssertFalse(
      conversation.contains("agentPillStack == nil ? 1 : "),
      "the open stack never dims the conversation or the composer")
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

  func testWideTranscriptionKeepsItsWidthUntilTheRosterIsAskedFor() throws {
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    XCTAssertNil(state.selectedConversationID, "the transcription surface has no conversation")
    let recorder = OverlayDrawerFrameRecorder()
    let host = NSHostingView(
      rootView:
        DictationOverlayView(state: state)
        .onPreferenceChange(OverlayDrawerFramePreferenceKey.self) { recorder.frame = $0 }
        .frame(width: 1200, height: 500))
    host.sizingOptions = []
    let panel = NSPanel(
      contentRect: CGRect(x: 0, y: 0, width: 1200, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.contentView = host
    panel.orderFrontRegardless()
    defer { panel.orderOut(nil) }
    func settle() {
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      host.layoutSubtreeIfNeeded()
    }
    settle()
    XCTAssertTrue(state.agentDrawerDocked, "1200 wide at 1.0 is docked")
    XCTAssertEqual(recorder.frame, .zero, "no column eats the transcription's width unasked")

    state.showAgentMonitor()
    settle()
    XCTAssertTrue(state.showsAgentMonitor, "docked transcription: the button opens the column")
    XCTAssertEqual(recorder.frame.width, 300, accuracy: 1, "the asked-for roster docks")

    state.hideAgentSidebar()
    settle()
    XCTAssertEqual(recorder.frame, .zero, "Esc gives the transcription its width back")

    state.selectConversation(lena.id)
    settle()
    XCTAssertFalse(state.showsAgentMonitor)
    XCTAssertEqual(recorder.frame.width, 300, accuracy: 1, "beside a conversation it is permanent")
  }

  func testDockedTranscriptionOpensAndClosesTheColumnThroughTheFlag() throws {
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.setAgentDrawerDocked(true)
    XCTAssertFalse(state.dockedRosterColumnShown)

    state.showAgentMonitor()
    XCTAssertTrue(
      state.showsAgentMonitor, "no conversation: the click opens, it has nothing to focus")
    XCTAssertTrue(state.dockedRosterColumnShown)

    state.selectConversation(lena.id)
    XCTAssertFalse(state.showsAgentMonitor, "selection closes the flag")
    XCTAssertTrue(state.dockedRosterColumnShown, "beside a conversation the column stays")

    state.showTranscription()
    XCTAssertNil(state.selectedConversationID)
    XCTAssertFalse(state.dockedRosterColumnShown, "back on the transcription the column is gone")

    // Docking engages while the stack is open on the transcription: the stack
    // becomes the column instead of vanishing.
    state.setAgentDrawerDocked(false)
    state.showAgentMonitor()
    state.setAgentDrawerDocked(true)
    XCTAssertTrue(state.showsAgentMonitor)
    XCTAssertTrue(state.dockedRosterColumnShown)
  }

  // MARK: - Row model and broadcast regression (deliverable 1, 878667f9f)

  func testDockedRowsKeepProviderModelVisibleAlongsideTimePreviewAndBadge() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let row = try section(
      of: source, from: "private func channelRow(", to: "private func conversationRow(")
    XCTAssertTrue(row.contains("lineLimit(2)"), "rows carry a two-line message preview")
    XCTAssertTrue(row.contains("drawerPreview("), "rows render the last message preview")
    XCTAssertTrue(
      row.contains("overlay-agent-descriptor-"),
      "wide rows must show the same provider/model identity as narrow pills")
    XCTAssertTrue(row.contains(".help("), "descriptor and status live in the row tooltip")
    XCTAssertTrue(
      row.contains("overlay-channel-delivery-"), "the status dot keeps the delivery identifier")
    XCTAssertTrue(row.contains("overlay-agent-row-"), "row identifier kept")
  }

  func testRememberedDrawerWidthLeavesConversationSpaceAndScalesWithText() throws {
    let defaults = try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))
    defaults.set(480.0, forKey: "Overlay.agentDrawerWidth.v1")
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(lena.id)
    state.conversationDrafts[lena.id] = "unfinished message"
    for (width, scale, expected) in [
      (1200.0, 1.0, 480.0), (900.0, 1.0, 380.0),
      (1400.0, 1.4, 672.0),
    ] as [(CGFloat, CGFloat, CGFloat)] {
      let recorder = OverlayDrawerFrameRecorder()
      let host = NSHostingView(
        rootView: DictationOverlayView(state: state)
          .defaultAppStorage(defaults)
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
      XCTAssertEqual(recorder.frame.width, expected, accuracy: 1)
      XCTAssertEqual(recorder.frame.minX, width - expected, accuracy: 1)
      XCTAssertGreaterThanOrEqual(recorder.frame.minX, 520 * scale - 1)
      XCTAssertEqual(state.selectedConversationID, lena.id)
      XCTAssertEqual(state.conversationDrafts[lena.id], "unfinished message")
      panel.orderOut(nil)
    }
    XCTAssertEqual(
      defaults.double(forKey: "Overlay.agentDrawerWidth.v1"), 480,
      "window clamping must not overwrite the user's remembered width")
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

  func testBroadcastPolicyRidesTheRowsContextMenuWithNoDisclosureList() throws {
    // Maciej (2026-10-17, "Słowo"): wake policy moves to the agent row's
    // right-click menu; the disclosure list under 0 · All is gone.
    let source = try overlaySource("OverlayChannelStatusView.swift")
    XCTAssertFalse(
      source.contains("overlay-broadcast-disclosure"),
      "the chevron list under 0 · All is gone; Saved conversations keeps its own disclosure")
    XCTAssertFalse(source.contains("OverlayBroadcastDisclosure"), "the old wrapper type is gone")
    let monitor = try section(
      of: source, from: "func monitorBody(maximumHeight:", to: "var currentConversations")
    XCTAssertTrue(
      monitor.contains("OverlayBroadcastPolicyMenu("),
      "the docked 0 · All row carries every member's wake policy")
    let pills = try XCTUnwrap(
      source.range(of: "struct OverlayAgentPillStack"), "the pill stack exists")
    XCTAssertTrue(
      String(source[pills.lowerBound...]).contains("OverlayBroadcastPolicyMenu("),
      "pill rows carry the same wake policy menu")
    XCTAssertTrue(
      source.contains("members: broadcastMembers.filter { $0.channel == channel.channel }"),
      "an agent's own docked row carries only its own policy")
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
    XCTAssertTrue(canvas.contains("maximumHeight: geometry.size.height, width: drawerWidth"))
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

  // MARK: - Wide-window evacuation (Maciej, 2026-10-17)

  /// The docked column already names the agent and carries its microphone and
  /// speaker, so the identity pill must leave with it: the host hands the
  /// column's visibility to the conversation view, and the view drops the
  /// whole navigation row (controls and pill) with a collapsed measurement.
  func testDockedColumnEvacuatesTheIdentityPillAndItsControls() throws {
    let overlay = try overlaySource("DictationOverlayView.swift")
    let canvas = try section(
      of: overlay, from: "private func canvasStack", to: "private var header: some View")
    XCTAssertTrue(
      canvas.contains("hidesNavigation: columnShown"),
      "the visible docked column is exactly when the pill evacuates")
    let conversation = try overlaySource("OverlayConversationView.swift")
    let anchor = try section(
      of: conversation, from: ".overlay(alignment: .top)", to: ".onChange(of: conversation.id)")
    XCTAssertTrue(
      anchor.contains("if hidesNavigation"),
      "the navigation row is conditional, not just transparent")
    XCTAssertTrue(
      anchor.contains("navigationHeight = 0"),
      "the content margin follows the evacuated row")
  }

  /// Broadcast-policy choices in the pill's context menu are checkboxes, so
  /// they must use the menu Toggle's native state column like the header
  /// menu's checkboxes, not a checkmark squatting in the icon slot.
  func testPolicyMenuUsesNativeCheckboxToggles() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let actions = try section(
      of: source, from: "private func policyActions", to: "private func apply")
    XCTAssertTrue(actions.contains("Toggle("), "policy rows are menu checkboxes")
    XCTAssertFalse(
      actions.contains("systemImage: \"checkmark\""),
      "no icon-slot checkmark impersonating a checkbox")
  }
}

@MainActor
private final class OverlayDrawerFrameRecorder {
  var frame = CGRect.zero
}
