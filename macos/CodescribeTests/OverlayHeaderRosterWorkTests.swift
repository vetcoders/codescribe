import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// A non-presenting fixture for the production overlay body. Keep the same
/// input and iteration count when comparing source generations on the test host.
@MainActor
final class OverlayHeaderRosterWorkTests: XCTestCase {
  private func fixture(repliesPerAgent: Int) throws -> (OverlayState, [OverlayConversation]) {
    let conversations = try (1...8).map { digit -> OverlayConversation in
      let session = "header-work-\(digit)"
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": session,
          "lease_id": AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: session)!,
          "channel": String(digit), "name": "Agent \(digit)",
        ]))
      let messages = (0..<repliesPerAgent).map { index in
        OverlayConversationMessage(
          id: "reply:header-work-\(digit)-\(index)", kind: .reply,
          text: "A retained reply used by the header work fixture.", order: UInt64(index),
          emittedAt: "2026-11-05T12:00:00Z", owner: owner, recipients: [],
          deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
      }
      return OverlayConversation(
        id: owner.id, channel: String(digit), name: owner.name, owner: owner, messages: messages)
    }
    let state = OverlayState.previewFormatted()
    optIntoChannelMap(state)
    state.channelMapPreference.selectLayout(.horizontal)
    state.setPresentationMode(.midi)
    state.agentChannelModifierSetting = { "ctrl" }
    state.applyConversationSnapshot(
      .init(
        deliveries: conversations.map {
          OverlayChannelDelivery(
            channel: $0.channel, agent: $0.name, deliveryID: nil, stage: nil, isOpen: false)
        }, conversations: conversations))
    state.applyChannelRoster(
      conversations.map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
      })
    return (state, conversations)
  }

  func testLargeHistoryHeaderWork() throws {
    let (state, conversations) = try fixture(repliesPerAgent: 2_000)
    let view = DictationOverlayView(state: state)
    let generation = state.captureGeneration
    let selection = state.selectedConversationID
    for _ in 0..<2 { withExtendedLifetime(view.body) {} }

    let iterations = 40
    let started = ProcessInfo.processInfo.systemUptime
    for _ in 0..<iterations {
      autoreleasepool { withExtendedLifetime(view.body) {} }
    }
    let elapsed = ProcessInfo.processInfo.systemUptime - started
    print(
      "HEADER_ROSTER_WORK iterations=\(iterations) agents=8 replies_per_agent=2000 elapsed_seconds=\(elapsed)"
    )

    XCTAssertEqual(state.captureGeneration, generation, "rendering cannot start capture")
    XCTAssertEqual(state.selectedConversationID, selection, "rendering cannot select a conversation")
    for conversation in conversations {
      XCTAssertEqual(
        state.conversations.first(where: { $0.id == conversation.id })?.messages,
        conversation.messages, "header evaluation must preserve the retained document")
    }
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
  }

  func testRosterReflectsUnreadChangesBetweenBodyPasses() throws {
    let (state, conversations) = try fixture(repliesPerAgent: 3)
    let first = try XCTUnwrap(conversations.first)
    let view = DictationOverlayView(state: state)
    withExtendedLifetime(view.body) {}
    XCTAssertEqual(view.channelStatusView.unreadCounts[first.id], 3)

    state.chooseConversation(first.id)
    state.setConversationVisible(true)
    withExtendedLifetime(view.body) {}
    XCTAssertEqual(view.channelStatusView.unreadCounts[first.id], 0)

    let nextReply = OverlayConversationMessage(
      id: "reply:header-work-new", kind: .reply, text: "A newly delivered reply", order: 4,
      emittedAt: "2026-11-05T12:01:00Z", owner: first.owner, recipients: [],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
    let changed = OverlayConversation(
      id: first.id, channel: first.channel, name: first.name, owner: first.owner,
      messages: first.messages + [nextReply])
    state.setConversationVisible(false)
    state.selectConversation(nil)
    state.applyConversationSnapshot(
      .init(deliveries: state.visibleChannelRows, conversations: [changed] + conversations.dropFirst()))
    withExtendedLifetime(view.body) {}
    XCTAssertEqual(view.channelStatusView.unreadCounts[first.id], 1)
  }
}
