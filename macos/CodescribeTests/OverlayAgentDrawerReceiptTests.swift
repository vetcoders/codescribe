import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayAgentDrawerReceiptTests: XCTestCase {
  private let lease = String(repeating: "a", count: 32)

  private func ownerRow(_ lease: String? = nil, session: String = "drawer-agent") -> [String: Any] {
    [
      "provider": "codex", "provider_session_id": session,
      "lease_id": lease ?? self.lease, "channel": "2", "name": "Lena",
    ]
  }

  private func fixture(
    messages: [OverlayConversationMessage]? = nil, read: Bool = false
  ) throws -> OverlayConversation {
    let owner = try XCTUnwrap(OverlayConversationOwner(row: ownerRow()))
    let question = OverlayConversationMessage(
      id: "user:question", kind: .user, text: "Review this change", order: 1,
      emittedAt: "2026-10-17T11:00:00Z", owner: nil,
      recipients: [
        OverlayConversationRecipient(
          owner: owner, deliveryID: "delivery", queued: false,
          accepted: false, acknowledged: read)
      ],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "",
      captureState: .finished)
    return OverlayConversation(
      id: owner.id, channel: "2", name: "Lena", owner: owner,
      messages: messages ?? [question])
  }

  private func view(
    _ conversation: OverlayConversation, unavailable: Bool = false,
    alive: Bool = true, unread: Int = 0
  ) -> OverlayChannelStatusView {
    OverlayChannelStatusView(
      channels: [channel], unavailable: unavailable, palette: .dark, animates: false,
      hudStates: [
        "2": OverlayChannelHudProjection(
          open: false, loud: false, autosealDeadline: nil, followerAlive: alive,
          provider: "codex", providerSessionID: conversation.owner?.providerSessionID)
      ],
      conversations: [conversation], unreadCounts: [conversation.id: unread],
      onSelectConversation: { _ in })
  }

  private var channel: OverlayChannelDelivery {
    OverlayChannelDelivery(
      channel: "2", agent: "Lena", deliveryID: "delivery", stage: .sent, isOpen: false)
  }

  private func receipt(_ conversation: OverlayConversation, unavailable: Bool = false) -> String? {
    let drawer = view(conversation, unavailable: unavailable)
    return drawer.deliveryReceiptText(for: channel, conversation: drawer.conversation(for: channel))
  }

  func testCanonicalQueueAcceptanceAndForeignAckDoNotPaintRead() throws {
    var bus = OverlayChannelDelivery.Bus()
    let session = "agent-channel-2-drawer"
    bus.consume([
      "schema": "codescribe.transcript-evidence.v1", "session_id": session,
      "occurrence_session_id": session, "capture_epoch": 1,
      "sample_start": 0, "sample_end": 1600, "document_index": 0,
      "reducer_revision": 1, "sequence": 1,
      "reducer_action": "record_ledger_terminal_seal", "audience": "Lena",
      "rendered_text": "Review this change", "recipients": [ownerRow()],
    ])
    bus.consume([
      "schema": "codescribe.transcript.v1", "session_id": session, "status": "session_ended",
    ])
    func conversation() throws -> OverlayConversation {
      try XCTUnwrap(bus.conversations(busPath: "/fixture/bus").first { $0.owner?.leaseID == lease })
    }
    XCTAssertEqual(try receipt(conversation()), "Addressed")
    let delivery = try XCTUnwrap(conversation().messages.first?.recipients.first?.deliveryID)
    var accepted = ownerRow()
    accepted.merge([
      "schema": "codescribe.native-queue.receipt.v1", "disposition": "provider_accepted",
      "delivery_id": delivery,
    ]) { _, new in new }
    bus.observeAcceptance(accepted)
    XCTAssertEqual(try receipt(conversation()), "Queue accepted")
    var ack = ownerRow(String(repeating: "b", count: 32), session: "foreign")
    ack.merge([
      "schema": "codescribe.agent-ack.v1", "delivery_id": delivery,
    ]) { _, new in new }
    bus.consume(ack)
    XCTAssertEqual(try receipt(conversation()), "Queue accepted")
    ack = ownerRow()
    ack.merge([
      "schema": "codescribe.agent-ack.v1", "delivery_id": delivery,
    ]) { _, new in new }
    bus.consume(ack)
    XCTAssertEqual(try receipt(conversation()), "Read")
  }

  func testQueuedHeldAndUnavailableRemainDifferentStates() throws {
    var question = try XCTUnwrap(fixture().messages.first)
    question.recipients[0].queued = true
    XCTAssertEqual(try receipt(fixture(messages: [question])), "Queued")
    question.recipients[0].queued = false
    question.holdSecondsRemaining = 4
    XCTAssertEqual(try receipt(fixture(messages: [question])), "Held")
    XCTAssertNil(try receipt(fixture(read: true), unavailable: true))
  }

  func testReplyAndReceivingTakeDoNotReplaceTheLastCompletedReceipt() throws {
    let previous = try fixture(read: true)
    var pending = try XCTUnwrap(fixture().messages.first)
    pending.captureState = .receiving
    let reply = OverlayConversationMessage(
      id: "reply:later", kind: .reply, text: "Working", order: 3,
      emittedAt: "2026-10-17T11:01:00Z", owner: previous.owner, recipients: [],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
    XCTAssertEqual(try receipt(fixture(messages: previous.messages + [pending, reply])), "Read")
    pending.captureState = .finished
    pending.recipients[0].queued = true
    XCTAssertEqual(try receipt(fixture(messages: previous.messages + [pending, reply])), "Queued")
  }

  func testSkippedComesOnlyFromTheFollowersReceiptForThatDeliveryAndOwner() throws {
    // Olek (2026-10-17): the current policy must not rewrite an old message's
    // fate. Skipped needs the follower's broadcast_skipped receipt
    // (bus-demux `record_broadcast_skipped`) for exactly this delivery and owner.
    var bus = OverlayChannelDelivery.Bus()
    let session = "agent-channel-0-drawer"
    bus.consume([
      "schema": "codescribe.transcript-evidence.v1", "session_id": session,
      "occurrence_session_id": session, "capture_epoch": 1,
      "sample_start": 0, "sample_end": 1600, "document_index": 0,
      "reducer_revision": 1, "sequence": 1,
      "reducer_action": "record_ledger_terminal_seal", "audience": "Lena",
      "rendered_text": "Everyone, stop", "recipients": [ownerRow()],
    ])
    bus.consume([
      "schema": "codescribe.transcript.v1", "session_id": session, "status": "session_ended",
    ])
    func conversation() throws -> OverlayConversation {
      try XCTUnwrap(bus.conversations(busPath: "/fixture/bus").first { $0.owner?.leaseID == lease })
    }
    let owner = try XCTUnwrap(conversation().owner)
    let coordinate = try XCTUnwrap(bus.receiptCoordinates().first)
    XCTAssertTrue(coordinate.broadcast, "a channel-0 seal is the kind a follower may skip")
    func skip(_ row: [String: Any]) -> [String: Any] {
      row.merging([
        "schema": "codescribe.broadcast-policy.v1", "kind": "broadcast_skipped",
        "delivery_id": coordinate.delivery, "origin_channel": "0", "policy": "off",
      ]) { _, new in new }
    }
    bus.observeBroadcastSkip(
      skip(ownerRow(String(repeating: "b", count: 32), session: "foreign")), owner: owner,
      delivery: coordinate.delivery)
    XCTAssertEqual(try receipt(conversation()), "Addressed", "a foreign lease's skip marks nothing")
    bus.observeBroadcastSkip(skip(ownerRow()), owner: owner, delivery: coordinate.delivery)
    XCTAssertEqual(try receipt(conversation()), "Skipped: off channel 0")
    XCTAssertTrue(bus.receiptCoordinates().isEmpty, "a recorded skip is never polled again")
  }

  func testTodaysPolicyNeverChangesAFrozenDeliverysReceipt() throws {
    // Lena and Olek (2026-10-17): today's channel-0 membership is the row's
    // mark, not evidence about an earlier delivery.
    var broadcast = try XCTUnwrap(fixture().messages.first)
    broadcast.sourceChannel = "0"
    let conversation = try fixture(messages: [broadcast])
    let drawer = view(conversation)
    func policyReceipt(_ policy: AgentBroadcastPolicy) -> String? {
      var row = channel
      row.broadcastPolicy = policy
      return drawer.deliveryReceiptText(for: row, conversation: conversation)
    }
    for policy in AgentBroadcastPolicy.allCases {
      XCTAssertEqual(policyReceipt(policy), "Addressed", "\(policy) leaves the receipt alone")
    }

    broadcast.recipients[0].broadcastSkipped = true
    let skipped = try fixture(messages: [broadcast])
    XCTAssertEqual(
      view(skipped).deliveryReceiptText(for: channel, conversation: skipped),
      "Skipped: off channel 0", "the receipt stands after the agent rejoins channel 0")

    broadcast.recipients[0].broadcastSkipped = nil
    broadcast.recipients[0].queued = true
    let queued = try fixture(messages: [broadcast])
    var offRow = channel
    offRow.broadcastPolicy = .off
    XCTAssertEqual(
      view(queued).deliveryReceiptText(for: offRow, conversation: queued), "Queued",
      "a copy queued before the agent left channel 0 keeps its receipt")

    // The seal-derived fallback (no message for this owner) follows the same rule.
    var sealed = OverlayChannelDelivery(
      channel: "2", agent: "Lena", deliveryID: "delivery", stage: .sent, isOpen: false)
    sealed.broadcastPolicy = .off
    XCTAssertEqual(drawer.deliveryReceiptText(for: sealed, conversation: nil), "Addressed")
    sealed.broadcastSkipped = true
    sealed.broadcastPolicy = .on
    XCTAssertEqual(
      drawer.deliveryReceiptText(for: sealed, conversation: nil), "Skipped: off channel 0")
  }

  func testWithdrawnMessageCannotAdvertiseAReceipt() throws {
    var question = try XCTUnwrap(fixture(read: true).messages.first)
    question.withdrawn = true
    XCTAssertNil(try receipt(fixture(messages: [question])))
  }

  func testChangingCurrentPolicyCannotInventAHistoricalSkipReceipt() throws {
    var broadcast = try XCTUnwrap(fixture().messages.first)
    broadcast.sourceChannel = "0"
    let conversation = try fixture(messages: [broadcast])
    let drawer = view(conversation)
    var row = channel
    row.broadcastPolicy = .on
    let publishedReceipt = drawer.deliveryReceiptText(for: row, conversation: conversation)
    row.broadcastPolicy = .off
    XCTAssertEqual(
      drawer.deliveryReceiptText(for: row, conversation: conversation), publishedReceipt,
      "Changing today's membership is not evidence that this frozen delivery was skipped")
  }

  func testAReplacementOwnerDoesNotInheritTheSavedOwnersRead() throws {
    let saved = try fixture(read: true)
    let replacement = try XCTUnwrap(
      OverlayConversationOwner(row: ownerRow(String(repeating: "b", count: 32), session: "new")))
    let current = OverlayConversation(
      id: replacement.id, channel: "2", name: "Lena", owner: replacement, messages: [])
    var drawer = view(current)
    drawer = OverlayChannelStatusView(
      channels: [channel], unavailable: false, palette: .dark, animates: false,
      hudStates: drawer.hudStates, conversations: [saved, current], onSelectConversation: { _ in })
    XCTAssertEqual(drawer.conversation(for: channel)?.id, current.id)
    XCTAssertEqual(
      drawer.deliveryReceiptText(for: channel, conversation: drawer.conversation(for: channel)),
      "Addressed")
  }

  func testNativeDrawerDistinguishesReadDisconnectedAndUnreadRepliesAtBothTextScales() throws {
    let conversation = try fixture(read: true)
    for scale in [CGFloat(1), CGFloat(1.6)] {
      let drawer = view(conversation, alive: false, unread: 8)
      let width = 320 * scale
      let host = NSHostingView(
        rootView: drawer.monitorBody(maximumHeight: 420)
          .environment(\.csTextScale, scale).frame(width: width))
      let window = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: width, height: 420),
        styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      window.orderFrontRegardless()
      defer { window.close() }
      host.layoutSubtreeIfNeeded()
      let reception = try NativeAccessibilityProbe.waitForElements(
        "overlay-view-conversation-\(conversation.id)")
      let row = try XCTUnwrap(reception.elements.first)
      XCTAssertTrue(row.value.contains("Read"), "ACK remains historical read evidence")
      XCTAssertTrue(row.value.contains("Disconnected"), "liveness is separate from read evidence")
      XCTAssertTrue(
        row.value.contains("8 unread replies"), "the count names Founder-unread replies")
      XCTAssertGreaterThan(row.height, 0)
      XCTAssertLessThanOrEqual(row.width, width)
    }
  }

  func testLongNativeRowFitsFiveTextLinesAtBothTextScales() throws {
    var message = try XCTUnwrap(fixture(read: true).messages.first)
    message.text = String(repeating: "A long routed message with enough text to wrap. ", count: 12)
    message.sourceChannel = "0"
    let conversation = try fixture(messages: [message])
    for scale in [CGFloat(1), CGFloat(1.6)] {
      let drawer = OverlayChannelStatusView(
        channels: [channel], unavailable: false, palette: .dark, animates: false,
        hudStates: view(conversation, alive: false).hudStates,
        agentDescriptors: ["2": String(repeating: "provider model repository ", count: 8)],
        conversations: [conversation], unreadCounts: [conversation.id: 123],
        onSelectConversation: { _ in })
      let width = 320 * scale
      let host = NSHostingView(
        rootView: drawer.monitorBody(maximumHeight: 420)
          .environment(\.csTextScale, scale).frame(width: width))
      let window = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: width, height: 420),
        styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      window.orderFrontRegardless()
      defer { window.close() }
      host.layoutSubtreeIfNeeded()
      let reception = try NativeAccessibilityProbe.waitForElements(
        "overlay-view-conversation-\(conversation.id)")
      let row = try XCTUnwrap(reception.elements.first)
      let layout = NSLayoutManager()
      func lineHeight(_ size: CGFloat) -> CGFloat {
        ceil(layout.defaultLineHeight(for: .systemFont(ofSize: size * scale)))
      }
      let fiveLines = lineHeight(13.5) + lineHeight(11) * 2 + lineHeight(12.5) * 2
      let spacingAndBadge: CGFloat = 8 + 4 + 4
      XCTAssertGreaterThan(row.height, 0)
      XCTAssertLessThanOrEqual(
        row.height, fiveLines + spacingAndBadge,
        "Long descriptor, preview, unread and disconnected receipt must share the five-line budget")
      XCTAssertLessThanOrEqual(row.width, width)
    }
  }
}
