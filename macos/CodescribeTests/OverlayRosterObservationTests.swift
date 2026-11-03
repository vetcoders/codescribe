import Foundation
import Observation
import XCTest
import os

@testable import Codescribe

@MainActor
final class OverlayRosterObservationTests: XCTestCase {
  func testMessageUpdatesDoNotInvalidateMicrophoneEligibility() throws {
    let state = OverlayState()
    state.applyChannelRoster([roster()])
    let owner = try owner(lease: "a")
    let conversation = conversation(owner)
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
    XCTAssertTrue(state.canToggleConversationMicrophone(conversation))
    let changed = OSAllocatedUnfairLock(initialState: false)
    withObservationTracking {
      _ = state.canToggleConversationMicrophone(conversation)
    } onChange: {
      changed.withLock { $0 = true }
    }
    // Exercise the same production boundary as live transcript/history polls.
    // An unrelated message must still paint, without invalidating the mic control.
    let updated = OverlayConversation(
      id: conversation.id, channel: conversation.channel, name: conversation.name, owner: owner,
      messages: [
        .init(
          id: "live-user", kind: .user, text: "Nowy fragment", order: 1, emittedAt: "",
          owner: owner, recipients: [], deliveryID: nil, replyTo: nil, unsolicited: false,
          playback: nil, busPath: "")
      ])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [updated]))
    XCTAssertEqual(state.conversations.first?.messages.first?.text, "Nowy fragment")
    XCTAssertFalse(changed.withLock { $0 })
    state.applyChannelRoster([roster(session: "replacement")])
    XCTAssertTrue(changed.withLock { $0 }, "Ownership replacement must invalidate the control")
    XCTAssertFalse(state.canToggleConversationMicrophone(conversation))
  }

  func testAmbiguousLeaseStaysIneligibleWhenItsSiblingIsArchived() throws {
    let first = try owner(lease: "a")
    let second = try owner(lease: "b")
    let state = OverlayState()
    state.applyChannelRoster([roster()])
    let conversations = [conversation(first), conversation(second)]
    state.applyConversationSnapshot(
      .init(deliveries: [], conversations: conversations, archivedOwners: [first]))
    XCTAssertFalse(state.canToggleConversationMicrophone(conversations[0]))
    XCTAssertFalse(state.canToggleConversationMicrophone(conversations[1]))
    XCTAssertTrue(
      OverlayChannelStatusView.currentConversations(
        conversations: conversations, archivedOwners: [first], hudStates: state.channelHudStates
      ).isEmpty)
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversations[1]]))
    XCTAssertTrue(state.canToggleConversationMicrophone(conversations[1]))
  }

  func testRosterKeepsInputOrderAndSpecialMailboxes() throws {
    let live = try owner(lease: "a")
    let stale = try owner(lease: "b", session: "old")
    let inbox = OverlayConversation.humanInbox()
    let broadcast = OverlayConversation(
      id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let state = OverlayState()
    state.applyChannelRoster([roster()])
    let input = [conversation(stale), inbox, conversation(live), broadcast]
    XCTAssertEqual(
      OverlayChannelStatusView.currentConversations(
        conversations: input, archivedOwners: [], hudStates: state.channelHudStates
      ).map(\.id), [inbox.id, live.id, broadcast.id])
  }

  private func owner(lease: Character, session: String = "roster-session") throws
    -> OverlayConversationOwner
  {
    try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session, "channel": "4", "name": "agent",
        "lease_id": String(repeating: String(lease), count: 32),
      ]))
  }

  private func conversation(_ owner: OverlayConversationOwner) -> OverlayConversation {
    .init(id: owner.id, channel: owner.channel, name: owner.name, owner: owner, messages: [])
  }

  private func roster(session: String = "roster-session") -> CsChannelRosterState {
    .init(
      channel: "4", audience: "agent", provider: "codex", providerSessionId: session,
      open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
  }
}
