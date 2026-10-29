import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayBusWindowTests: XCTestCase {
  private func conversation(
    _ digit: String, session: String = "initial", lease: String = "a"
  ) throws -> OverlayConversation {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "window-\(digit)-\(session)",
        "lease_id": String(repeating: lease, count: 32), "channel": digit,
        "name": digit == "2" ? "Tola" : "Lena",
      ]))
    return OverlayConversation(
      id: owner.id, channel: digit, name: owner.name, owner: owner, messages: [])
  }

  private func apply(_ conversations: [OverlayConversation], to state: OverlayState) {
    state.applyChannelRoster(
      conversations.map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: $0.owner == nil ? nil : true,
          unreadSinceUnixMs: nil)
      })
    state.applyConversationSnapshot(.init(deliveries: [], conversations: conversations))
  }

  func testViewingAndFoldingKeepTheRecipientAndEachOwnersDraft() throws {
    let first = try conversation("2")
    let second = try conversation("3", lease: "b")
    let state = OverlayState()
    apply([first, second], to: state)
    state.initializeComposerTarget(first)
    state.setConversationDraft("Private first draft", for: first.id)
    state.setConversationDraft("Private second draft", for: second.id)

    state.selectConversation(second.id, expand: false)
    state.initializeComposerTarget(second)
    state.foldAgentList()
    state.unfoldAgentList()

    XCTAssertEqual(state.selectedConversationID, second.id)
    XCTAssertEqual(state.resolvedComposerTarget(viewing: second)?.id, first.id)
    XCTAssertEqual(state.conversationDraft(for: first.id), "Private first draft")
    XCTAssertEqual(state.conversationDraft(for: second.id), "Private second draft")
    XCTAssertFalse(state.conversationMicrophoneOpen(first))
    XCTAssertFalse(state.conversationMicrophoneOpen(second))

    state.chooseComposerTarget(second)
    XCTAssertEqual(state.resolvedComposerTarget(viewing: first)?.id, second.id)
    XCTAssertEqual(state.conversationDraft(for: first.id), "Private first draft")
  }

  func testReusedDigitCannotAdoptTheOldRecipientsDraftOrSelection() throws {
    let old = try conversation("2")
    let replacement = try conversation("2", session: "replacement", lease: "b")
    let state = OverlayState()
    apply([old], to: state)
    state.chooseComposerTarget(old)
    state.setConversationDraft("Keep for the original owner", for: old.id)
    apply([replacement], to: state)
    state.selectConversation(replacement.id, expand: false)

    XCTAssertEqual(state.composerTarget?.id, old.id)
    XCTAssertNil(state.resolvedComposerTarget(viewing: replacement))
    XCTAssertFalse(state.composerTargetIsAvailable(old))
    XCTAssertTrue(state.composerTargetIsAvailable(replacement))
    XCTAssertEqual(state.conversationDraft(for: old.id), "Keep for the original owner")
    XCTAssertEqual(state.conversationDraft(for: replacement.id), "")

    state.chooseComposerTarget(replacement)
    XCTAssertEqual(state.resolvedComposerTarget(viewing: replacement)?.id, replacement.id)
    XCTAssertEqual(state.conversationDraft(for: replacement.id), "")
  }

  func testSameConversationIDWithDifferentOwnerDoesNotResolve() throws {
    let old = try conversation("2")
    var wrongOwner = try conversation("2", session: "replacement", lease: "b")
    wrongOwner = OverlayConversation(
      id: old.id, channel: wrongOwner.channel, name: wrongOwner.name,
      owner: wrongOwner.owner, messages: [])
    let state = OverlayState()
    apply([old], to: state)
    state.chooseComposerTarget(old)
    apply([wrongOwner], to: state)
    XCTAssertNil(state.resolvedComposerTarget(viewing: wrongOwner))
    XCTAssertFalse(state.composerTargetIsAvailable(old))
  }

  func testInboxDoesNotReplaceAnExplicitBroadcastRecipient() throws {
    let privateConversation = try conversation("2")
    let broadcast = OverlayConversation(
      id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let inbox = OverlayConversation(
      id: "10", channel: "10", name: "Your Inbox", owner: nil, messages: [])
    let state = OverlayState()
    apply([broadcast, inbox, privateConversation], to: state)
    state.chooseComposerTarget(broadcast)
    state.selectConversation(inbox.id, expand: false)
    state.chooseComposerTarget(inbox)
    XCTAssertEqual(state.composerTarget?.id, broadcast.id)
    XCTAssertEqual(state.resolvedComposerTarget(viewing: inbox)?.id, broadcast.id)
    state.selectConversation(privateConversation.id, expand: false)
    XCTAssertEqual(state.resolvedComposerTarget(viewing: privateConversation)?.id, broadcast.id)
  }

  func testReplyNavigationRetainsMessageAndOwnerAcrossRepeatedRequests() throws {
    let first = try conversation("2")
    let second = try conversation("3", lease: "b")
    let state = OverlayState()
    apply([first, second], to: state)
    state.chooseComposerTarget(first)
    state.selectConversation(second.id, expand: false)
    state.navigateToConversationMessage(first, messageID: "reply:retained")
    let initial = try XCTUnwrap(state.conversationNavigationRequest)
    XCTAssertEqual(initial.conversationID, first.id)
    XCTAssertEqual(initial.messageID, "reply:retained")
    XCTAssertEqual(state.selectedConversationID, first.id)
    state.navigateToConversationMessage(first, messageID: "reply:retained")
    XCTAssertNotEqual(state.conversationNavigationRequest?.revision, initial.revision)

    let replacement = try conversation("2", session: "replacement", lease: "b")
    apply([replacement, second], to: state)
    let prior = state.conversationNavigationRequest
    state.navigateToConversationMessage(first, messageID: "foreign-owner")
    XCTAssertEqual(state.conversationNavigationRequest, prior)
  }

  private var row: OverlayChannelDelivery {
    OverlayChannelDelivery(
      channel: "2", agent: "Tola", deliveryID: "delivery", stage: .sent, isOpen: false)
  }

  private func roster(
    _ conversation: OverlayConversation, alive: Bool = true, open: Bool = false,
    palette: OverlayAppearancePalette = .dark
  ) -> OverlayChannelStatusView {
    OverlayChannelStatusView(
      channels: [row], unavailable: false, palette: palette, animates: false,
      hudStates: [
        "2": .init(
          open: open, loud: false, autosealDeadline: nil, followerAlive: alive,
          provider: conversation.owner?.provider,
          providerSessionID: conversation.owner?.providerSessionID)
      ],
      onToggleChannel: { _ in }, conversations: [conversation], onSelectConversation: { _ in })
  }

  private func outgoing(
    in conversation: OverlayConversation, acknowledged: Bool = false
  ) throws -> OverlayConversationMessage {
    OverlayConversationMessage(
      id: "user:window", kind: .user, text: "Please review the window", order: 1,
      emittedAt: "2026-10-29T10:00:00Z", owner: nil,
      recipients: [
        .init(
          owner: try XCTUnwrap(conversation.owner), deliveryID: "delivery", queued: false,
          accepted: false, acknowledged: acknowledged)
      ],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "",
      captureState: .finished)
  }

  func testQueueAcceptanceDoesNotBecomeADeliveredOrReadCheck() throws {
    var conversation = try conversation("2")
    var message = try outgoing(in: conversation)
    for accepted in [false, true] {
      message.recipients[0].accepted = accepted
      message.recipients[0].queued = true
      conversation.messages = [message]
      let display = try XCTUnwrap(
        roster(conversation).deliveryReceiptDisplay(for: row, conversation: conversation))
      XCTAssertEqual(display.symbol, "clock")
    }
  }

  func testCanonicalReceiptGlyphBelongsOnlyToItsFrozenDelivery() throws {
    var conversation = try conversation("2")
    var message = try outgoing(in: conversation)
    for (state, symbol) in [
      (OverlayReceiptState.State.delivered, "checkmark"), (.read, "double-check"),
      (.skipped, "forward.end"), (.noReceipt, "circle.dotted"),
    ] {
      message.receipt = OverlayMessageReceipt(
        messageID: message.id,
        deliveries: [
          .init(
            deliveryID: "delivery", name: "Tola", channel: "2", state: state,
            wake: nil, deliveredAt: nil, at: nil)
        ])
      conversation.messages = [message]
      XCTAssertEqual(
        roster(conversation).deliveryReceiptDisplay(for: row, conversation: conversation)?.symbol,
        symbol)
    }
    message.receipt = OverlayMessageReceipt(
      messageID: message.id,
      deliveries: [
        .init(
          deliveryID: "foreign-delivery", name: "Tola", channel: "2", state: .read,
          wake: nil, deliveredAt: nil, at: nil)
      ])
    conversation.messages = [message]
    XCTAssertEqual(
      roster(conversation).deliveryReceiptDisplay(for: row, conversation: conversation)?.symbol,
      "clock")
  }

  func testLaterReplyCannotLendItsTimeToAnOutgoingReceipt() throws {
    var conversation = try conversation("2")
    let sent = try outgoing(in: conversation, acknowledged: true)
    let reply = OverlayConversationMessage(
      id: "reply:later", kind: .reply, text: "The review is ready", order: 2,
      emittedAt: "2026-10-29T10:05:00Z", owner: conversation.owner, recipients: [],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
    conversation.messages = [sent, reply]
    let display = try XCTUnwrap(
      roster(conversation).deliveryReceiptDisplay(for: row, conversation: conversation))
    let sentTime = try XCTUnwrap(MessageTimestamp.date(fromISO8601: sent.emittedAt))
      .formatted(date: .abbreviated, time: .shortened)
    XCTAssertEqual(display.symbol, "double-check")
    XCTAssertTrue(display.helpText.contains(sentTime))
    XCTAssertEqual(conversation.messages.last?.id, reply.id)
  }

  func testHeldAndWithdrawnMessagesCannotAdvertiseRead() throws {
    var conversation = try conversation("2")
    var message = try outgoing(in: conversation, acknowledged: true)
    message.holdSecondsRemaining = 5
    conversation.messages = [message]
    XCTAssertEqual(
      roster(conversation).deliveryReceiptDisplay(for: row, conversation: conversation)?.symbol,
      "pause.circle")
    message.withdrawn = true
    conversation.messages = [message]
    XCTAssertNil(roster(conversation).deliveryReceiptDisplay(for: row, conversation: conversation))
  }

  func testDisconnectedOpenMicrophoneCanStillBeStoppedButNotStarted() throws {
    let conversation = try conversation("2")
    var view = roster(conversation, alive: false, open: true)
    XCTAssertTrue(view.agentMicrophoneEnabled(for: row))
    XCTAssertTrue(try XCTUnwrap(view.agentPillMenuActions(for: "2")?.microphone).enabled)
    view.pendingToggleChannels = ["2"]
    XCTAssertFalse(view.agentMicrophoneEnabled(for: row))
    XCTAssertFalse(roster(conversation, alive: false).agentMicrophoneEnabled(for: row))
  }

  func testMuteRemainsAvailableAfterReaderDisconnects() throws {
    let conversation = try conversation("2")
    var view = roster(conversation, alive: false)
    view.onTogglePlayback = { _ in }
    view.mutedChannels = ["2": false]
    XCTAssertTrue(view.agentPlaybackEnabled(for: row))
    XCTAssertTrue(try XCTUnwrap(view.agentPillMenuActions(for: "2")?.playback).enabled)
    view.pendingMuteChannels = ["2"]
    XCTAssertFalse(view.agentPlaybackEnabled(for: row))
  }

  func testCompactCardsRenderWithinTheirHeightBudgetInBothAppearances() throws {
    var conversation = try conversation("2")
    conversation.messages = [try outgoing(in: conversation, acknowledged: true)]
    for dark in [false, true] {
      for scale in [CGFloat(1), 1.6] {
        for width in [CGFloat(280), 340] {
          let palette: OverlayAppearancePalette = dark ? .dark : .light
          var view = roster(conversation, palette: palette)
          view.textScale = scale
          let host = NSHostingView(
            rootView: view.channelRow(row)
              .frame(width: width).padding(10)
              .background(palette.desktopBackground.color)
              .environment(\.colorScheme, dark ? .dark : .light))
          let size = host.fittingSize
          let window = NSPanel(
            contentRect: NSRect(origin: NSPoint(x: -10000, y: -10000), size: size),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
          window.isReleasedWhenClosed = false
          window.contentView = host
          defer {
            window.contentView = nil
            window.close()
          }
          host.layoutSubtreeIfNeeded()
          let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
          host.cacheDisplay(in: host.bounds, to: bitmap)
          let image = NSImage(size: size)
          image.addRepresentation(bitmap)
          let attachment = XCTAttachment(image: image)
          attachment.name = "Bus-card-\(dark ? "dark" : "light")-\(Int(width))-scale-\(scale)"
          attachment.lifetime = .keepAlways
          add(attachment)
          XCTAssertGreaterThan(size.height, 40 * scale)
          XCTAssertLessThanOrEqual(size.height - 20, 110 * scale)
        }
      }
    }
  }

  func testReceptionRestrictionSymbolExistsInTheSystemLibrary() {
    XCTAssertNotNil(
      NSImage(
        systemSymbolName: "antenna.radiowaves.left.and.right.slash", accessibilityDescription: nil))
  }
}
