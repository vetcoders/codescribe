import XCTest

@testable import Codescribe

/// Bounded source contracts. Native geometry and menu tracking need separate
/// acceptance: these assertions never claim that a ZStack proves hit testing.
final class OverlayLostRosterMenuTests: XCTestCase {
  // The audio pair stands beside the preview, which wraps in the width that is
  // left; identity owns the first line above them (Founder, 2026-11-05).
  func testPreviewAndStateWrapBesideTheAudioPair() throws {
    let row = try OverlayLostBehaviorSource.section(
      OverlayLostBehaviorSource.overlay("OverlayChannelStatusView.swift"),
      from: "func channelRow(", to: "private func rosterRowLabel(")
    try OverlayLostBehaviorSource.ordered(
      ["HStack(alignment: .top, spacing: 6) {", "VStack(alignment: .leading, spacing: 2) {",
        "contentOnly: true", "rosterRowState(channel, receiptText: receiptText)",
        "} .frame(maxWidth: .infinity, alignment: .leading)",
        "compactAgentActions(for: channel)"], in: row)
    // The audio pair has its own column, so nothing covers the preview and
    // no fade hides the text it used to sit on.
    XCTAssertFalse(row.contains("ZStack(alignment: .topTrailing)"))
    XCTAssertFalse(row.contains("LinearGradient("))
    // The inline preview is two lines; the hover card carries the whole
    // message (Maciej, 2026-11-06: "hover cards są spoko").
    XCTAssertEqual(row.components(separatedBy: "OverlayRosterMessageHint(").count - 1, 2)
  }

  func testStatePanelAndSyncRemainOutsideTheAudioAndMoreControls() throws {
    let source = try OverlayLostBehaviorSource.overlay("OverlayChannelStatusView.swift")
    let controls = try OverlayLostBehaviorSource.section(
      source, from: "func compactAgentActions(", to: "func panelControl(")
    try OverlayLostBehaviorSource.ordered(
      ["HStack(spacing: 6)", "OverlayAgentAudioControls(",
        "EmptyView().modifier(", "cardAgentMenu(for: channel.channel, showsButton: true)"],
      in: controls)
    let marks = try OverlayLostBehaviorSource.section(
      source, from: "func rowMarks(", to: "func compactAgentActions(")
    try OverlayLostBehaviorSource.ordered(
      ["if showsChain, let conversation", "onToggleDraftSync?(conversation.id)",
        "overlay-agent-sync-", "if showsFocus", "focusAgentPanel(channel)",
        "overlay-agent-focus-"], in: marks)
    let row = try OverlayLostBehaviorSource.section(
      source, from: "func channelRow(", to: "private func rosterRowLabel(")
    XCTAssertTrue(row.contains("rosterRowState(channel, receiptText: receiptText)"))
  }

  func testHeaderMenuIsProjectedOnlyThroughTheExactOwnerResolver() throws {
    let source = try OverlayLostBehaviorSource.overlay("DictationOverlayView.swift")
    let resolver = try OverlayLostBehaviorSource.section(
      source, from: "private func conversationHeaderChannel(",
      to: "private func conversationHeaderMenu(")
    for guardExpression in [
      "guard let owner = conversation.owner else { return nil }",
      "channel.channel == conversation.channel", "let identity = channel.broadcastIdentity",
      "identity.provider == owner.provider", "identity.session == owner.providerSessionID",
      "identity.leaseID == owner.leaseID",
    ] {
      XCTAssertTrue(OverlayLostBehaviorSource.compact(resolver).contains(guardExpression),
        "A digit reused by a different provider, session or lease must refuse the old title")
    }
    let menu = try OverlayLostBehaviorSource.section(
      source, from: "private func conversationHeaderMenu(",
      to: "private func conversationHeaderActions(")
    try OverlayLostBehaviorSource.ordered(
      ["conversationHeaderChannel(conversation).map", "channelStatusView.agentPillMenu("],
      in: menu)
    XCTAssertTrue(source.contains("navigationMenu: conversationHeaderMenu(conversation)"))
  }

  func testBothTitlePresentationsRefuseMenusWhenLiveActionsAreDisabled() throws {
    let source = try OverlayLostBehaviorSource.overlay("OverlayConversationView.swift")
    let titles = try OverlayLostBehaviorSource.section(
      source, from: "private var navigation:", to: "private var navigationLabelMenu:")
    XCTAssertEqual(titles.components(separatedBy: ".modifier(navigationLabelMenu)").count - 1, 2,
      "Both the standard title and Intercom title must mount the guarded menu")
    let gate = try OverlayLostBehaviorSource.section(
      source, from: "private var navigationLabelMenu:",
      to: "private var conversationNamePill:")
    try OverlayLostBehaviorSource.ordered(
      ["guard allowsLiveActions, let navigationMenu else",
        "OverlayBroadcastPolicyMenu(members: [], palette: palette, enabled: false)",
        "return navigationMenu"], in: gate)
  }

  func testFullRowKeepsFullContextMenuWhileMoreKeepsItsSecondarySubset() throws {
    let source = try OverlayLostBehaviorSource.overlay("OverlayChannelStatusView.swift")
    let row = try OverlayLostBehaviorSource.section(
      source, from: "func channelRow(", to: "private func rosterRowLabel(")
    XCTAssertTrue(OverlayLostBehaviorSource.compact(row).contains(
      ".modifier(agentPillMenu(for: channel.channel, showsInlineError: false))"))
    let more = try OverlayLostBehaviorSource.section(
      source, from: "func cardAgentMenu(", to: "func agentMicrophoneEnabled(")
    XCTAssertTrue(OverlayLostBehaviorSource.compact(more).contains("secondaryActionsOnly: true"))
  }
}
