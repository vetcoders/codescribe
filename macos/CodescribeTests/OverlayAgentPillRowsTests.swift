import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Falsifiers for the pill-rows cut (Maciej, channel 1, 2026-10-16 23:30Z):
/// a click on the name pill unfolds, under it, one pill per agent that
/// mirrors the agent's card — microphone + speaker + name + agent/model/repo —
/// and the open list steals neither the colour nor the scrolling of the
/// conversation underneath. The host removed the dim and the click-catcher
/// (fba885b6a; pinned in OverlayAgentDrawerDockingTests and
/// OverlayHeaderChromeTests). These pin the seams that cut left open: each
/// row's wiring, the row's shape, and the conversation staying untouched.
@MainActor
final class OverlayAgentPillRowsTests: XCTestCase {

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

  private func tail(of source: String, from: String) throws -> String {
    let start = try XCTUnwrap(source.range(of: from), "missing section start: \(from)")
    return String(source[start.lowerBound...])
  }

  private func agentConversation(
    _ name: String, channel: String, session: String, lease: String, replyPlaying: Bool = false
  ) throws -> OverlayConversation {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session,
        "lease_id": String(repeating: lease, count: 32), "channel": channel, "name": name,
      ]))
    let playback =
      replyPlaying
      ? OverlayReplyPlayback(
        replyID: "reply:\(name)-1", ticket: "ticket-\(name)", state: "playing", reason: nil,
        spoken: true, emittedAt: "2026-10-17T01:40:01Z")
      : nil
    let reply = OverlayConversationMessage(
      id: "reply:\(name)-1", kind: .reply, text: "on it", order: 1,
      emittedAt: "2026-10-17T01:40:00Z", owner: owner, recipients: [], deliveryID: nil,
      replyTo: nil, unsolicited: false, playback: playback, busPath: "")
    return OverlayConversation(
      id: owner.id, channel: channel, name: name, owner: owner, messages: [reply])
  }

  private func hud(session: String, open: Bool, alive: Bool = true) -> OverlayChannelHudProjection {
    OverlayChannelHudProjection(
      open: open, loud: false, autosealDeadline: nil, followerAlive: alive,
      provider: "codex", providerSessionID: session)
  }

  private struct Roster {
    let broadcast: OverlayConversation
    let lena: OverlayConversation
    let astra: OverlayConversation
    var channels: [OverlayChannelDelivery] {
      [
        OverlayChannelDelivery(
          channel: "2", agent: "lena", deliveryID: "receipt-2", stage: .received, isOpen: false),
        OverlayChannelDelivery(
          channel: "3", agent: "astra", deliveryID: "receipt-3", stage: .received, isOpen: true),
      ]
    }
  }

  private func roster() throws -> Roster {
    Roster(
      broadcast: OverlayConversation(
        id: "rows-all", channel: "0", name: "All", owner: nil, messages: []),
      lena: try agentConversation(
        "lena", channel: "2", session: "lena-session", lease: "a", replyPlaying: true),
      astra: try agentConversation("astra", channel: "3", session: "astra-session", lease: "b"))
  }

  // MARK: - Wiring: each row carries the agent card's controls

  func testPillRowControlsAreWiredLikeTheDockedColumnRows() throws {
    let roster = try roster()
    var microphoneToggles: [UInt8] = []
    var playbackToggles: [String] = []
    var view = OverlayChannelStatusView(
      channels: roster.channels, unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "0": hud(session: "all", open: false),
        "2": hud(session: "lena-session", open: true),
        "3": hud(session: "astra-session", open: false),
      ],
      onToggleChannel: { microphoneToggles.append($0) },
      pendingToggleChannels: ["3"],
      conversations: [roster.broadcast, roster.lena, roster.astra],
      selectedConversationID: nil, unreadCounts: [:], onSelectConversation: { _ in })
    view.mutedChannels = ["0": false, "2": true]
    view.pendingMuteChannels = ["0"]
    view.onTogglePlayback = { playbackToggles.append($0) }

    let entries = view.pillEntries
    XCTAssertEqual(
      entries.map(\.id), [roster.broadcast.id, roster.lena.id, roster.astra.id],
      "0 · All leads the agents; transcription is the header's, not a row")
    XCTAssertEqual(entries.first?.dot, .broadcast, "the shared channel wears the antenna")

    let lena = try XCTUnwrap(entries.first { $0.id == roster.lena.id }?.controls)
    XCTAssertEqual(lena.channel, "2")
    XCTAssertTrue(lena.open, "the HUD's open state wins over the roster row's isOpen")
    XCTAssertEqual(lena.muted, true)
    XCTAssertTrue(lena.microphoneEnabled)
    XCTAssertTrue(lena.playbackEnabled)
    XCTAssertFalse(lena.broadcast)
    XCTAssertTrue(lena.speaking, "a reply coming out of the speaker lights the row's speaker")
    lena.onMicrophone()
    lena.onPlayback()
    XCTAssertEqual(microphoneToggles, [2], "the microphone toggles the row's own channel")
    XCTAssertEqual(playbackToggles, ["2"], "the speaker mutes the row's own channel")

    let astra = try XCTUnwrap(entries.first { $0.id == roster.astra.id }?.controls)
    XCTAssertEqual(astra.channel, "3")
    XCTAssertFalse(astra.open, "a closed HUD wins over the roster row's isOpen")
    XCTAssertNil(astra.muted, "no playback status yet: the speaker reports unavailable")
    XCTAssertFalse(astra.microphoneEnabled, "a pending channel toggle disables the microphone")
    XCTAssertTrue(astra.playbackEnabled)
    XCTAssertFalse(astra.speaking)

    let all = try XCTUnwrap(entries.first { $0.id == roster.broadcast.id }?.controls)
    XCTAssertEqual(all.channel, "0")
    XCTAssertTrue(all.broadcast, "0 · All speaks to everyone")
    XCTAssertTrue(all.microphoneEnabled, "channel 0 with an armed HUD can open the microphone")
    XCTAssertFalse(all.playbackEnabled, "a pending global mute disables the speaker")
    XCTAssertEqual(all.muted, false)
    all.onMicrophone()
    all.onPlayback()
    XCTAssertEqual(microphoneToggles, [2, 0])
    XCTAssertEqual(playbackToggles, ["2", "0"])
  }

  // MARK: - Disconnected readers: greyed from the first frame, microphone asleep

  func testDisconnectedReaderDimsItsPillAndSleepsItsMicrophone() throws {
    let roster = try roster()
    var view = OverlayChannelStatusView(
      channels: roster.channels, unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "0": hud(session: "all", open: false),
        "2": hud(session: "lena-session", open: false, alive: false),
        "3": hud(session: "astra-session", open: false),
      ],
      onToggleChannel: { _ in },
      pendingToggleChannels: [],
      conversations: [roster.broadcast, roster.lena, roster.astra],
      selectedConversationID: nil, unreadCounts: [:], onSelectConversation: { _ in })
    view.onTogglePlayback = { _ in }

    let entries = view.pillEntries
    let lena = try XCTUnwrap(entries.first { $0.id == roster.lena.id })
    XCTAssertTrue(lena.disconnected, "a binding whose reader died is greyed from the first frame")
    XCTAssertEqual(lena.dot, .sleeping)
    let lenaControls = lena.controls
    XCTAssertFalse(lenaControls.microphoneEnabled, "nobody reads a dead reader's channel")
    XCTAssertTrue(lenaControls.playbackEnabled, "its recorded replies still play")

    let astra = try XCTUnwrap(entries.first { $0.id == roster.astra.id })
    XCTAssertFalse(astra.disconnected, "a live reader keeps its colour")
    XCTAssertTrue(astra.controls.microphoneEnabled)
    let all = try XCTUnwrap(entries.first { $0.id == roster.broadcast.id })
    XCTAssertFalse(all.disconnected, "0 · All has no reader of its own")
  }

  func testBothRosterShapesDimTheDisconnectedRowAndSleepItsMicrophone() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let modifier = try section(
      of: source, from: "struct OverlayDisconnectedRowDim: ViewModifier {", to: "\n}\n")
    XCTAssertTrue(modifier.contains(".opacity(disconnected ? Self.opacity : 1)"))
    XCTAssertTrue(modifier.contains(".grayscale(disconnected ? 1 : 0)"))

    let docked = try section(
      of: source, from: "private func channelRow(", to: "private func rosterRowLabel(")
    XCTAssertTrue(
      docked.contains("&& !hasDeadFollower(channel),"),
      "the docked row's microphone sleeps for a dead reader")
    XCTAssertTrue(
      docked.contains("OverlayDisconnectedRowDim(disconnected: hasDeadFollower(channel))"),
      "the docked row dims as one unit")

    let pills = try section(
      of: source, from: "private func pillEntryView(", to: "private func policyMembers(")
    XCTAssertTrue(
      pills.contains("OverlayDisconnectedRowDim(disconnected: entry.disconnected)"),
      "the pill row dims its controls and pill together")
  }

  func testPillRowControlsFollowTheMonitorRowsAvailabilityRules() throws {
    let roster = try roster()
    // Channel 0 without a HUD: the monitor's 0 · All row keeps its microphone
    // disabled, and so does the pill row. An agent without a HUD is not a
    // current conversation at all: it has no row, so it has no controls.
    var unarmed = OverlayChannelStatusView(
      channels: roster.channels, unavailable: false, palette: .dark, animates: false,
      hudStates: ["2": hud(session: "lena-session", open: false)],
      onToggleChannel: { _ in },
      conversations: [roster.broadcast, roster.lena, roster.astra],
      selectedConversationID: nil, unreadCounts: [:], onSelectConversation: { _ in })
    unarmed.onTogglePlayback = { _ in }
    let unarmedEntries = unarmed.pillEntries
    XCTAssertEqual(
      unarmedEntries.map(\.id), [roster.broadcast.id, roster.lena.id],
      "an agent whose channel has no HUD is a saved conversation, not a row")
    let unarmedAll = try XCTUnwrap(
      unarmedEntries.first { $0.id == roster.broadcast.id }?.controls)
    XCTAssertFalse(unarmedAll.microphoneEnabled, "channel 0 needs its HUD before it speaks")
    XCTAssertTrue(unarmedAll.playbackEnabled)
    let unarmedLena = try XCTUnwrap(unarmedEntries.first { $0.id == roster.lena.id }?.controls)
    XCTAssertFalse(unarmedLena.open, "the HUD's closed state wins over the roster row")
    XCTAssertTrue(unarmedLena.microphoneEnabled)

    // No playback handler: every speaker is disabled, the microphones stay.
    let silent = OverlayChannelStatusView(
      channels: roster.channels, unavailable: false, palette: .dark, animates: false,
      hudStates: ["0": hud(session: "all", open: false)],
      onToggleChannel: { _ in },
      conversations: [roster.broadcast, roster.lena, roster.astra],
      selectedConversationID: nil, unreadCounts: [:], onSelectConversation: { _ in })
    for entry in silent.pillEntries {
      let controls = try XCTUnwrap(entry.controls, entry.id)
      XCTAssertFalse(controls.playbackEnabled, "\(entry.id): no playback handler, no speaker")
      XCTAssertTrue(
        controls.microphoneEnabled, "\(entry.id): the microphone does not depend on playback")
    }

    // The bridge is unavailable: no microphone anywhere, like the docked rows.
    var unavailable = OverlayChannelStatusView(
      channels: roster.channels, unavailable: true, palette: .dark, animates: false,
      hudStates: ["0": hud(session: "all", open: false)],
      onToggleChannel: { _ in },
      conversations: [roster.broadcast, roster.lena, roster.astra],
      selectedConversationID: nil, unreadCounts: [:], onSelectConversation: { _ in })
    unavailable.onTogglePlayback = { _ in }
    for entry in unavailable.pillEntries {
      let controls = try XCTUnwrap(entry.controls, entry.id)
      XCTAssertFalse(controls.microphoneEnabled, "\(entry.id): an unavailable bridge opens nothing")
    }

    // The selected conversation has no pill, so it has no row controls either.
    let selected = OverlayChannelStatusView(
      channels: roster.channels, unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "0": hud(session: "all", open: false),
        "2": hud(session: "lena-session", open: false),
        "3": hud(session: "astra-session", open: false),
      ],
      conversations: [roster.broadcast, roster.lena, roster.astra],
      selectedConversationID: roster.lena.id, unreadCounts: [:], onSelectConversation: { _ in })
    XCTAssertEqual(
      selected.pillEntries.map(\.id), [roster.broadcast.id, roster.astra.id],
      "the open conversation keeps its controls in the header, not in the stack")
  }

  // MARK: - Shape: the row is the agent card in miniature

  func testPillRowMountsTheControlsBesideThePillUnderOneGlassGroup() throws {
    let roster = try overlaySource("OverlayChannelStatusView.swift")
    let stack = try tail(of: roster, from: "struct OverlayAgentPillStack: View")
    XCTAssertTrue(stack.contains("ScrollView(.vertical)"), "a long roster scrolls inside the stack")
    XCTAssertTrue(
      stack.contains("overlayGlassGroup("),
      "the pills and their controls morph as one glass group")
    XCTAssertFalse(
      stack.contains("rowGlassContainer("), "no per-row container: one group for the stack")
    XCTAssertTrue(stack.contains(".accessibilityIdentifier(\"overlay-agent-pill-stack\")"))
    XCTAssertTrue(stack.contains(".accessibilityIdentifier(\"overlay-agent-pill-\\(entry.id)\")"))
    XCTAssertTrue(
      stack.contains(".accessibilityIdentifier(\"overlay-agent-pill-row-\\(entry.id)\")"),
      "each row is one accessibility element")
    XCTAssertTrue(
      stack.contains("let controls = entry.controls"),
      "every row has controls: the stack holds only conversations")
    XCTAssertTrue(
      stack.contains("OverlayAgentAudioControls("), "the row reuses the card's controls")
    for wiring in [
      "open: controls.open", "muted: controls.muted",
      "microphoneEnabled: controls.microphoneEnabled",
      "playbackEnabled: controls.playbackEnabled", "onMicrophone: controls.onMicrophone",
      "onPlayback: controls.onPlayback", "broadcast: controls.broadcast",
      "speaking: controls.speaking",
    ] {
      XCTAssertTrue(stack.contains(wiring), "the row passes \(wiring) through")
    }
    XCTAssertTrue(stack.contains(".fixedSize()"), "the controls keep their intrinsic width")
    XCTAssertTrue(
      stack.contains(
        ".accessibilityIdentifier(\"overlay-agent-pill-controls-\\(controls.channel)\")"),
      "the controls group is addressed by channel digit")
    XCTAssertFalse(
      stack.contains("overlay-channel-toggle-"),
      "the stack never borrows the docked column's control identifiers")

    let controls = try section(
      of: roster, from: "struct OverlayAgentAudioControls: View",
      to: "struct OverlayBroadcastPolicyMenu")
    XCTAssertTrue(controls.contains(".accessibilityIdentifier(\"overlay-agent-microphone\")"))
    XCTAssertTrue(controls.contains(".accessibilityIdentifier(\"overlay-agent-speaker\")"))

    let entry = try section(
      of: roster, from: "struct OverlayAgentPillEntry: Identifiable", to: "struct Controls {")
    XCTAssertTrue(
      entry.contains("let controls: Controls"),
      "controls are a required field: transcription left the list (Maciej, 2026-10-17)")
    XCTAssertFalse(
      roster.contains("\"transcription\""), "no transcription row survives in the roster view")
  }

  func testExpandedRowMirrorsTheHeaderWithControlsBeforeThePill() throws {
    // 2026-10-17, after Maciej's placement
    // question: the header's cluster [microphone + speaker][name pill] and
    // the stack's mount both sit trailing inside the same horizontal padding,
    // so no measuring is needed. The row is the header's mirror image —
    // controls first, the pill as the trailing element, spacing 6 — and the
    // stack aligns its rows trailing, so the pills line up under the name
    // pill and the controls under the controls. No pixel assertions.
    let roster = try overlaySource("OverlayChannelStatusView.swift")
    let stack = try tail(of: roster, from: "struct OverlayAgentPillStack: View")
    let body = try section(
      of: stack, from: "var body: some View {", to: "private func pillEntryView")
    XCTAssertTrue(
      body.contains("VStack(alignment: .trailing, spacing: 6)"),
      "the stack aligns its rows to the trailing edge, under the name pill")
    // The row is returned directly since the PPM cut (9058ffbaf) folded the
    // disclosure branch; its first HStack in the stack's tail is the row.
    let row = try section(of: stack, from: "HStack(spacing: 6) {", to: "overlay-agent-pill-row-")
    XCTAssertTrue(row.hasPrefix("HStack(spacing: 6)"), "spacing 6 as in the header")
    let controls = try XCTUnwrap(row.range(of: "OverlayAgentAudioControls("))
    let pill = try XCTUnwrap(
      row.range(of: #"(?m)^\s*pill\s*$"#, options: .regularExpression),
      "the row mounts the pill built above it")
    XCTAssertLessThan(
      controls.lowerBound, pill.lowerBound,
      "the controls come first: [microphone + speaker][pill], as in the header")
  }

  // MARK: - The conversation underneath stays untouched

  func testOpenStackLeavesTheConversationUntouched() throws {
    let conversation = try overlaySource("OverlayConversationView.swift")
    let mentions = conversation.components(separatedBy: "agentPillStack").count - 1
    XCTAssertEqual(
      mentions, 2,
      "the stack is declared once and mounted once; a modifier keyed on it (dim, disable, catcher) is a regression"
    )
    XCTAssertNil(
      conversation.range(
        of: #"(opacity|scrollDisabled|allowsHitTesting|disabled)\([^)]*agentPillStack"#,
        options: .regularExpression),
      "neither colour nor hit-testing nor scrolling depends on the open stack")
    XCTAssertNil(
      conversation.range(
        of: #"Color\.clear\s*\.frame\(maxWidth: \.infinity, maxHeight: \.infinity\)"#,
        options: .regularExpression),
      "no full-area clear button over the conversation, under any identifier")
    XCTAssertFalse(conversation.contains("Close the agents list"))

    let top = try section(
      of: conversation, from: ".overlay(alignment: .top)",
      to: ".onAppear { scrollToLatest(proxy) }")
    XCTAssertNotNil(
      top.range(of: #"Spacer\(minLength: 0\)\s*pillStack"#, options: .regularExpression),
      "the stack hangs trailing under the name pill and takes only its own width")
    XCTAssertTrue(top.contains(".onExitCommand { pillStack.onDismiss() }"), "Esc closes it")
  }
}
