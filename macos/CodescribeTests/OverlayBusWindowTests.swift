import AppKit
import SwiftUI
import Vision
import XCTest

@testable import Codescribe

@MainActor
final class OverlayBusWindowTests: XCTestCase {
  private func conversation(
    _ digit: String, session: String = "initial", lease: String = "a", canonicalLease: Bool = false,
    name: String? = nil
  ) throws -> OverlayConversation {
    let sessionID = "window-\(digit)-\(session)"
    let leaseID =
      canonicalLease
      ? AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: sessionID)
      : String(repeating: lease, count: 32)
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": sessionID,
        "lease_id": leaseID, "channel": digit,
        "name": name ?? (digit == "2" ? "Tola" : "Lena"),
      ]))
    return OverlayConversation(
      id: owner.id, channel: digit, name: owner.name, owner: owner, messages: [])
  }

  private func apply(_ conversations: [OverlayConversation], to state: OverlayState) {
    state.applyChannelRoster(
      conversations.filter { $0.owner != nil || $0.channel == "0" }.map {
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

  func testExplicitWriteChoosesItsRecipientWhileOrdinaryNavigationDoesNot() throws {
    let first = try conversation("2")
    let second = try conversation("3", lease: "b")
    let state = OverlayState()
    apply([first, second], to: state)
    state.chooseComposerTarget(first)
    state.selectConversation(first.id, expand: false)
    state.setConversationDraft("First private draft", for: first.id)
    state.setConversationDraft("Second private draft", for: second.id)
    let roster = DictationOverlayView(state: state).channelStatusView
    let write = try XCTUnwrap(roster.agentPillMenuActions(for: "3")?.write)
    XCTAssertTrue(write.enabled)
    write.perform()
    XCTAssertEqual(state.selectedConversationID, second.id)
    XCTAssertEqual(state.resolvedComposerTarget(viewing: second)?.id, second.id)

    roster.selectChannelConversation("2")
    XCTAssertEqual(state.selectedConversationID, first.id)
    XCTAssertEqual(state.resolvedComposerTarget(viewing: first)?.id, second.id)
    XCTAssertEqual(state.conversationDraft(for: first.id), "First private draft")
    XCTAssertEqual(state.conversationDraft(for: second.id), "Second private draft")
    XCTAssertFalse(state.conversationMicrophoneOpen(second))
  }

  func testRetainedHistoryDoesNotKeepARetiredRecipientSendable() throws {
    let old = try conversation("2")
    let replacement = try conversation("2", session: "replacement", lease: "b")
    let state = OverlayState()
    apply([old], to: state)
    state.chooseComposerTarget(old)
    apply([replacement], to: state)
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [old, replacement]))

    XCTAssertTrue(state.conversations.contains { $0.id == old.id }, "history remains readable")
    XCTAssertFalse(state.availableComposerRecipients.contains { $0.id == old.id })
    XCTAssertTrue(state.availableComposerRecipients.contains { $0.id == replacement.id })
    XCTAssertFalse(state.composerTargetIsAvailable(old))
    XCTAssertNil(state.resolvedComposerTarget(viewing: replacement))
    XCTAssertEqual(state.composerTarget?.id, old.id)
  }

  func testLateHeldReturnKeepsANewerExplicitRecipientAndItsDraft() async throws {
    let first = try conversation("2")
    let second = try conversation("3", lease: "b")
    let state = OverlayState()
    apply([first, second], to: state)
    let revision = state.prepareHeldComposerEdit(first)
    state.setConversationDraft("Keep first", for: first.id)
    let completion = Task { @MainActor in
      state.returnHeldComposerText("Returned held words", to: first, revision: revision)
    }
    state.chooseComposerTarget(second)
    state.setConversationDraft("Newer second draft", for: second.id)
    let mayFocus = await completion.value

    XCTAssertFalse(mayFocus)
    XCTAssertEqual(state.composerTarget?.id, second.id)
    XCTAssertEqual(state.conversationDraft(for: first.id), "Keep first\n\nReturned held words")
    XCTAssertEqual(state.conversationDraft(for: second.id), "Newer second draft")
  }

  func testHeldReturnDoesNotStealFocusAfterSwitchingAwayAndBack() throws {
    let first = try conversation("2")
    let second = try conversation("3", lease: "b")
    let state = OverlayState()
    apply([first, second], to: state)
    let revision = state.prepareHeldComposerEdit(first)
    state.chooseComposerTarget(second)
    state.chooseComposerTarget(first)
    XCTAssertFalse(state.returnHeldComposerText("Returned", to: first, revision: revision))
    XCTAssertEqual(state.conversationDraft(for: first.id), "Returned")
  }

  func testHeldReturnToRetiredOwnerDoesNotMakeItsReplacementTheRecipient() throws {
    let old = try conversation("2")
    let replacement = try conversation("2", session: "replacement", lease: "b")
    let state = OverlayState()
    apply([old], to: state)
    let revision = state.prepareHeldComposerEdit(old)
    apply([replacement], to: state)
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [old, replacement]))
    XCTAssertFalse(
      state.returnHeldComposerText("Retained original text", to: old, revision: revision))
    XCTAssertEqual(state.conversationDraft(for: old.id), "Retained original text")
    XCTAssertEqual(state.conversationDraft(for: replacement.id), "")
    XCTAssertNil(state.resolvedComposerTarget(viewing: replacement))
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

  private func withMessages(
    _ conversation: OverlayConversation, _ messages: [OverlayConversationMessage]
  ) -> OverlayConversation {
    OverlayConversation(
      id: conversation.id, channel: conversation.channel, name: conversation.name,
      owner: conversation.owner, messages: messages)
  }

  func testQueueAcceptanceDoesNotBecomeADeliveredOrReadCheck() throws {
    var conversation = try conversation("2")
    var message = try outgoing(in: conversation)
    for accepted in [false, true] {
      message.recipients[0].accepted = accepted
      message.recipients[0].queued = true
      conversation = withMessages(conversation, [message])
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
      conversation = withMessages(conversation, [message])
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
    conversation = withMessages(conversation, [message])
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
    conversation = withMessages(conversation, [sent, reply])
    let display = try XCTUnwrap(
      roster(conversation).deliveryReceiptDisplay(for: row, conversation: conversation))
    let sentTime = try XCTUnwrap(MessageTimestamp.date(fromISO8601: sent.emittedAt))
      .formatted(date: .abbreviated, time: .shortened)
    XCTAssertEqual(display.symbol, "double-check")
    XCTAssertTrue(display.helpText.contains(sentTime))
    XCTAssertEqual(conversation.messages.last?.id, reply.id)
  }

  func testCanonicalAcknowledgmentOutranksAnEarlierDeliveredReceipt() throws {
    var conversation = try conversation("2")
    var message = try outgoing(in: conversation, acknowledged: true)
    message.receipt = OverlayMessageReceipt(
      messageID: message.id,
      deliveries: [
        .init(
          deliveryID: "delivery", name: "Tola", channel: "2", state: .delivered,
          wake: nil, deliveredAt: nil, at: nil)
      ])
    conversation = withMessages(conversation, [message])
    XCTAssertEqual(
      roster(conversation).deliveryReceiptDisplay(for: row, conversation: conversation)?.symbol,
      "double-check")
  }

  func testHeldAndWithdrawnMessagesCannotAdvertiseRead() throws {
    var conversation = try conversation("2")
    var message = try outgoing(in: conversation, acknowledged: true)
    message.holdSecondsRemaining = 5
    conversation = withMessages(conversation, [message])
    XCTAssertEqual(
      roster(conversation).deliveryReceiptDisplay(for: row, conversation: conversation)?.symbol,
      "pause.circle")
    message.withdrawn = true
    conversation = withMessages(conversation, [message])
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

  func testRailCountAndUnreadFlagUseTheSameConversationIdentity() throws {
    let original = try conversation("2")
    let replacement = try conversation("2", session: "replacement", lease: "b")
    let view = OverlayChannelStatusView(
      channels: [row], unavailable: false, palette: .dark, animates: false,
      conversations: [original, replacement], unreadCounts: [original.id: 12],
      onSelectConversation: { _ in })
    let unread = view.railPillDisplay(original, channel: row)
    XCTAssertEqual(unread.unreadCount, 12)
    XCTAssertTrue(unread.unread)
    let fresh = view.railPillDisplay(replacement, channel: row)
    XCTAssertEqual(fresh.unreadCount, 0)
    XCTAssertFalse(fresh.unread)
  }

  func testCompactCardsRenderWithinTheirHeightBudgetInBothAppearances() throws {
    var conversation = try conversation("2")
    conversation = withMessages(conversation, [try outgoing(in: conversation, acknowledged: true)])
    for dark in [false, true] {
      for scale in [CGFloat(1)] {
        for width in [CGFloat(280), 340] {
          let palette: OverlayAppearancePalette = dark ? .dark : .light
          let view = roster(conversation, palette: palette)
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

  func testStandardSidebarKeepsShortNamesAndCompleteChannelDigitsVisible() throws {
    let reporter = try conversation("3", name: "Reporter")
    func normalized(_ text: String) -> String {
      text.lowercased().filter { $0.isLetter || $0.isNumber }
    }
    for scale: CGFloat in [1, 1.5] {
      for width: CGFloat in [240, 300] {
        for name in ["Tola", "Lena", "lena2", "Wera", "Nina"] {
          var target = try conversation("2", canonicalLease: true, name: name)
          target = withMessages(
            target,
            [
              try outgoing(in: target, acknowledged: true),
              previewReply(in: reporter, text: "Ready to review this window."),
            ])
          let channel = OverlayChannelDelivery(
            channel: "2", agent: name, deliveryID: "delivery", stage: .received, isOpen: false)
          var view = OverlayChannelStatusView(
            channels: [channel], unavailable: false, palette: .dark, animates: false,
            hudStates: [
              "2": .init(
                open: false, loud: false, autosealDeadline: nil, followerAlive: true,
                provider: target.owner?.provider, providerSessionID: target.owner?.providerSessionID
              )
            ], onToggleChannel: { _ in }, conversations: [target], unreadCounts: [target.id: 12],
            onSelectConversation: { _ in })
          view.mutedChannels = ["2": false]
          view.onTogglePlayback = { _ in }
          let host = NSHostingView(
            rootView: view.monitorBody(maximumHeight: 400 * scale)
              .frame(width: width * scale)
              .environment(\.csTextScale, scale)
              .environment(\.colorScheme, .dark)
              .background(OverlayAppearancePalette.dark.desktopBackground.color))
          let size = host.fittingSize
          let panel = NSPanel(
            contentRect: NSRect(origin: NSPoint(x: -10_000, y: -10_000), size: size),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
          panel.isReleasedWhenClosed = false
          panel.contentView = host
          BackgroundTestWindow.present(panel)
          defer { panel.orderOut(nil) }
          host.layoutSubtreeIfNeeded()
          RunLoop.main.run(until: Date().addingTimeInterval(0.06))
          host.layoutSubtreeIfNeeded()
          let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
          host.cacheDisplay(in: host.bounds, to: bitmap)
          let request = VNRecognizeTextRequest()
          request.recognitionLevel = .accurate
          request.recognitionLanguages = ["en-US"]
          request.usesLanguageCorrection = false
          try VNImageRequestHandler(cgImage: try XCTUnwrap(bitmap.cgImage), options: [:])
            .perform([request])
          let text = (request.results ?? []).compactMap { observation -> (String, CGRect)? in
            guard let value = observation.topCandidates(1).first?.string else { return nil }
            return (normalized(value), observation.boundingBox)
          }
          let wanted = normalized(name)
          let names = text.filter { $0.0.contains(wanted) }
          XCTAssertFalse(
            names.isEmpty,
            "name=\(name) width=\(width) scale=\(scale) became an ellipsis; painted text=\(text.map(\.0))"
          )
          if !names.isEmpty {
            let completeDigit = names.contains { candidate in
              if let range = candidate.0.range(of: wanted),
                candidate.0[..<range.lowerBound].contains("2")
              {
                return true
              }
              return text.contains { number in
                number.0 == "2" && number.1.maxX <= candidate.1.minX + 0.01
                  && abs(number.1.midY - candidate.1.midY) < candidate.1.height
              }
            }
            XCTAssertTrue(completeDigit, "the complete channel digit must precede the painted name")
          }
        }
      }
    }
  }

  func testRosterPreviewFitsShortMessagesAndScrollsLongMessagesWithoutShadow() throws {
    let target = try conversation("2")
    let short = previewReply(in: target, text: "A short, complete reply.")
    let long = previewReply(
      in: target,
      text: String(repeating: "A complete paragraph that remains available.\n\n", count: 40))
    let parent = NSPanel(
      contentRect: NSRect(x: -10_000, y: -10_000, width: 400, height: 100),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    let control = NSView(frame: NSRect(x: 10, y: 10, width: 250, height: 60))
    parent.contentView?.addSubview(control)
    BackgroundTestWindow.present(parent)
    let owner = OverlayHintAnchor()
    owner.view = control
    defer {
      owner.hide()
      parent.orderOut(nil)
    }
    func show(_ message: OverlayConversationMessage) throws -> NSPanel {
      owner.show(
        message.bodyText, palette: .dark, leading: true,
        content: AnyView(
          OverlayRosterMessagePreview(name: "Tola", message: message, palette: .dark)),
        size: CGSize(width: 330, height: 240), interactive: true)
      let panel = try XCTUnwrap(OverlayHintPanel.shared.presentedPanel)
      panel.contentView?.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.08))
      panel.contentView?.layoutSubtreeIfNeeded()
      return panel
    }
    func attachPreview(_ panel: NSPanel, message: OverlayConversationMessage, name: String) throws {
      let host = NSHostingView(
        rootView: OverlayRosterMessagePreview(name: "Tola", message: message, palette: .dark)
          .frame(width: panel.frame.width, height: panel.frame.height)
          .background(OverlayAppearancePalette.dark.desktopBackground.color))
      host.frame = NSRect(origin: .zero, size: panel.frame.size)
      host.layoutSubtreeIfNeeded()
      let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
      host.cacheDisplay(in: host.bounds, to: bitmap)
      let image = NSImage(size: panel.frame.size)
      image.addRepresentation(bitmap)
      let attachment = XCTAttachment(image: image)
      attachment.name = name
      attachment.lifetime = .keepAlways
      add(attachment)
    }
    let shortPanel = try show(short)
    let shortHeight = shortPanel.frame.height
    try attachPreview(shortPanel, message: short, name: "Sidebar-hover-short")
    XCTAssertLessThan(shortHeight, 140, "a short message must not reserve the full preview height")
    XCTAssertFalse(shortPanel.hasShadow)
    XCTAssertFalse(shortPanel.canBecomeKey)
    let longPanel = try show(long)
    try attachPreview(longPanel, message: long, name: "Sidebar-hover-long")
    XCTAssertGreaterThan(longPanel.frame.height, shortHeight)
    XCTAssertLessThanOrEqual(longPanel.frame.height, 240)
    XCTAssertEqual(longPanel.frame.width, 330, accuracy: 1)
    func scrollViews(_ view: NSView) -> [NSScrollView] {
      (view as? NSScrollView).map { [$0] } ?? view.subviews.flatMap(scrollViews)
    }
    let scroll = try XCTUnwrap(scrollViews(try XCTUnwrap(longPanel.contentView)).first)
    XCTAssertGreaterThan(
      try XCTUnwrap(scroll.documentView).bounds.height, scroll.contentView.bounds.height,
      "the full long message needs a real scrollable document")
  }

  func testSidebarReviewAtStandardWidths() throws {
    let names = ["miksa-sztudio", "tola", "lena", "lena2", "wera", "nina"]
    let conversations = try names.enumerated().map { offset, name in
      let target = try conversation(String(offset + 1), canonicalLease: true, name: name)
      return withMessages(
        target,
        [
          try outgoing(in: target, acknowledged: true),
          previewReply(
            in: target,
            text:
              "The full name and channel stay readable. The last message remains available on hover."
          ),
        ])
    }
    let state = OverlayState()
    apply(conversations, to: state)
    state.selectConversation(conversations[0].id)
    for width: CGFloat in [240, 300] {
      let view = OverlayChannelStatusView.intercomRoster(
        state: state, palette: .dark, animates: false)
      let host = NSHostingView(
        rootView: view.monitorBody(maximumHeight: 900)
          .frame(width: width, height: 900)
          .environment(\.csTextScale, 1)
          .environment(\.colorScheme, .dark)
          .background(OverlayAppearancePalette.dark.desktopBackground.color))
      host.frame = NSRect(x: 0, y: 0, width: width, height: 900)
      host.layoutSubtreeIfNeeded()
      let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
      host.cacheDisplay(in: host.bounds, to: bitmap)
      let image = NSImage(size: host.bounds.size)
      image.addRepresentation(bitmap)
      let attachment = XCTAttachment(image: image)
      attachment.name = "Sidebar-standard-\(Int(width))"
      attachment.lifetime = .keepAlways
      add(attachment)
    }
  }

  private func previewReply(
    in conversation: OverlayConversation, text: String
  ) -> OverlayConversationMessage {
    OverlayConversationMessage(
      id: "reply:hover-review", kind: .reply, text: text, order: 2,
      emittedAt: "2026-10-29T10:01:00Z", owner: conversation.owner, recipients: [],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
  }

  func testRosterHoverKeepsTheFullLatestMessageAndItsParagraphs() throws {
    var target = try conversation("2")
    XCTAssertNil(OverlayRosterMessageHint.fullText(in: target))
    let body =
      "## Latest message\n\n" + String(repeating: "A complete paragraph.\n\n", count: 100)
      + "END-OF-LAST-MESSAGE"
    let reply = previewReply(in: target, text: body)
    target = withMessages(target, [try outgoing(in: target), reply])
    let preview = try XCTUnwrap(OverlayRosterMessageHint.fullText(in: target))
    XCTAssertTrue(preview.contains(body), "the preview must not apply the card's two-line cap")
    XCTAssertTrue(preview.hasSuffix("END-OF-LAST-MESSAGE"))
    XCTAssertFalse(preview.contains("Please review the window"))
    XCTAssertTrue(preview.contains("Tola → You"))

    var sent = try outgoing(in: target)
    sent.text = "A newer user message\n\nWith a second paragraph"
    target = withMessages(target, [reply, sent])
    let outgoingPreview = try XCTUnwrap(OverlayRosterMessageHint.fullText(in: target))
    XCTAssertTrue(outgoingPreview.contains(sent.text))
    XCTAssertFalse(outgoingPreview.contains("END-OF-LAST-MESSAGE"))
  }

  func testRosterHoverWaitsAndKeepsConversationRecipientAndDraftUntouched() async throws {
    var first = try conversation("2")
    first = withMessages(
      first, [previewReply(in: first, text: "First paragraph\n\nFinal paragraph")])
    let second = try conversation("3", lease: "b")
    let state = OverlayState()
    apply([first, second], to: state)
    state.selectConversation(second.id)
    state.chooseComposerTarget(second)
    state.setConversationDraft("Keep the second recipient's draft", for: second.id)
    let generation = state.captureGeneration
    let firstRow = try XCTUnwrap(state.visibleChannelRows.first { $0.channel == first.channel })
    let (panel, host) = previewPanel(
      AnyView(DictationOverlayView(state: state).channelStatusView.channelRow(firstRow)))
    defer {
      setPreviewHover(false, in: host)
      panel.contentView = nil
      panel.orderOut(nil)
    }
    XCTAssertGreaterThan(setPreviewHover(true, in: host), 0)
    XCTAssertNil(OverlayHintPanel.shared.presentedTitle, "passing over a row must not flash a card")
    try await Task.sleep(for: .milliseconds(150))
    XCTAssertNil(OverlayHintPanel.shared.presentedTitle)
    try await Task.sleep(for: .milliseconds(300))
    XCTAssertEqual(
      OverlayHintPanel.shared.presentedTitle, OverlayRosterMessageHint.fullText(in: first))
    XCTAssertFalse(try XCTUnwrap(OverlayHintPanel.shared.presentedPanel).canBecomeKey)
    XCTAssertEqual(state.selectedConversationID, second.id)
    XCTAssertEqual(state.resolvedComposerTarget(viewing: second)?.id, second.id)
    XCTAssertEqual(state.conversationDraft(for: second.id), "Keep the second recipient's draft")
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertFalse(state.conversationMicrophoneOpen(first))
    XCTAssertFalse(state.conversationMicrophoneOpen(second))
    setPreviewHover(false, in: host)
    try await Task.sleep(for: .milliseconds(300))
    XCTAssertNil(OverlayHintPanel.shared.presentedTitle)
  }

  func testBriefOrRemovedRosterHoverCannotPresentAStaleMessage() async throws {
    var target = try conversation("2")
    target = withMessages(target, [previewReply(in: target, text: "Retired owner message")])
    let (panel, host) = previewPanel(
      AnyView(
        Text("Tola").frame(maxWidth: .infinity, maxHeight: .infinity)
          .modifier(OverlayRosterMessageHint(conversation: target, palette: .dark, textScale: 1))))
    defer {
      setPreviewHover(false, in: host)
      panel.contentView = nil
      panel.orderOut(nil)
    }
    XCTAssertGreaterThan(setPreviewHover(true, in: host), 0)
    setPreviewHover(false, in: host)
    try await Task.sleep(for: .milliseconds(450))
    XCTAssertNil(OverlayHintPanel.shared.presentedTitle)

    setPreviewHover(true, in: host)
    host.rootView = AnyView(Text("The old row has left the roster"))
    host.layoutSubtreeIfNeeded()
    try await Task.sleep(for: .milliseconds(450))
    XCTAssertNil(
      OverlayHintPanel.shared.presentedTitle, "a pending old owner cannot reopen a preview")
  }

  private func previewPanel(_ view: AnyView) -> (NSPanel, NSHostingView<AnyView>) {
    let host = NSHostingView(rootView: view)
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 320, height: 140)
    let panel = NSPanel(
      contentRect: NSRect(x: 300, y: 400, width: 320, height: 140),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.contentView = host
    BackgroundTestWindow.present(panel)
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.06))
    host.layoutSubtreeIfNeeded()
    return (panel, host)
  }

  @discardableResult
  private func setPreviewHover(_ inside: Bool, in view: NSView) -> Int {
    var count = 0
    if let region = view as? OverlayMenuCursorRegionView,
      region.onHover != nil, region.bounds.width > 100
    {
      let point =
        inside
        ? region.convert(NSPoint(x: region.bounds.midX, y: region.bounds.midY), to: nil)
        : NSPoint(x: -10_000, y: -10_000)
      region.updateHover(at: point)
      count += 1
    }
    for child in view.subviews { count += setPreviewHover(inside, in: child) }
    return count
  }

  func testLastMessageHoverPreviewReview() throws {
    let target = try conversation("2")
    let message = previewReply(
      in: target,
      text: """
        ## Window review

        The sidebar now keeps the last message available without leaving the conversation you are writing in.

        - Hover over an agent's name or preview.
        - Move into this card to scroll longer messages.
        - The recipient, draft and microphone stay as you left them.

        **The complete message remains available below the fold.**

        A final paragraph confirms that the preview keeps the message's original structure.
        """)
    let host = NSHostingView(
      rootView: OverlayRosterMessagePreview(name: target.name, message: message, palette: .dark)
        .frame(width: 380, height: 280)
        .environment(\.colorScheme, .dark))
    host.frame = NSRect(x: 0, y: 0, width: 380, height: 280)
    host.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let image = NSImage(size: host.bounds.size)
    image.addRepresentation(bitmap)
    let attachment = XCTAttachment(image: image)
    attachment.name = "Bus-last-message-hover-preview"
    attachment.lifetime = .keepAlways
    add(attachment)
  }

  func testHorizontalWindowReviewInDarkSplitLayout() throws {
    try renderWindowForReview(dark: true, rail: false, scale: 1, width: 1000)
  }

  func testHorizontalWindowReviewInLightSplitLayout() throws {
    try renderWindowForReview(dark: false, rail: false, scale: 1, width: 1000)
  }

  func testHorizontalWindowReviewWithFoldedSidebar() throws {
    try renderWindowForReview(dark: true, rail: true, scale: 1, width: 760)
  }

  func testHorizontalWindowReviewAtLargerTextScale() throws {
    try renderWindowForReview(dark: true, rail: false, scale: 1.5, width: 1060)
  }

  private func renderWindowForReview(
    dark: Bool, rail: Bool, scale: CGFloat, width: CGFloat
  ) throws {
    var first = try conversation("2", canonicalLease: true)
    let second = try conversation("3", lease: "b", canonicalLease: true)
    let sent = try outgoing(in: first, acknowledged: true)
    let reply = OverlayConversationMessage(
      id: "reply:window-review", kind: .reply,
      text:
        "The compact sidebar keeps the controls visible. The text destination stays separate from the conversation you are viewing.",
      order: 2, emittedAt: "2026-10-29T10:01:00Z", owner: first.owner, recipients: [],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
    first = withMessages(first, [sent, reply])
    let all = OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let inbox = OverlayConversation(
      id: "10", channel: "10", name: "Your Inbox", owner: nil, messages: [])
    let inventory = [all, inbox, first, second]
    let state = OverlayState()
    apply(inventory, to: state)
    state.applyConversationSnapshot(
      .init(
        deliveries: [
          .init(
            channel: "2", agent: first.name, deliveryID: "delivery", stage: .received, isOpen: false
          ),
          .init(channel: "3", agent: second.name, deliveryID: nil, stage: nil, isOpen: false),
        ], conversations: inventory))
    state.applyBoundAgents(
      Dictionary(
        uniqueKeysWithValues: [first, second].map { conversation in
          let owner = conversation.owner!
          let identity = AgentPlaybackIdentity(
            provider: owner.provider, session: owner.providerSessionID,
            bus: FileManager.default.temporaryDirectory.appendingPathComponent(
              "window-review-bus.jsonl"
            ).path)
          return (
            conversation.channel,
            AgentPlaybackBinding(
              identity: identity, model: "Fixture model", repositoryName: "Codescribe")
          )
        }))
    state.selectConversation(first.id)
    state.chooseComposerTarget(second)
    state.setConversationDraft("A retained draft for Lena", for: second.id)
    if rail { state.foldAgentList() }
    let row = try XCTUnwrap(state.visibleChannelRows.first { $0.channel == first.channel })
    XCTAssertEqual(row.broadcastIdentity?.leaseID, first.owner?.leaseID)
    XCTAssertFalse(state.visibleChannelRows.contains { $0.channel == "10" })
    let height: CGFloat = 680
    let host = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .frame(width: width, height: height)
        .environment(\.csTextScale, scale)
        .environment(\.colorScheme, dark ? .dark : .light)
        .symbolEffectsRemoved()
        .transaction { $0.disablesAnimations = true })
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: width, height: height)
    let panel = NSPanel(
      contentRect: host.frame, styleMask: [.borderless, .nonactivatingPanel],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
    panel.contentView = host
    defer {
      panel.contentView = nil
      panel.close()
    }
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.12))
    host.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let image = NSImage(size: host.bounds.size)
    image.addRepresentation(bitmap)
    let attachment = XCTAttachment(image: image)
    attachment.name =
      "Bus-window-\(dark ? "dark" : "light")-\(rail ? "rail" : "split")-scale-\(scale)"
    attachment.lifetime = .keepAlways
    add(attachment)
    XCTAssertEqual(state.selectedConversationID, first.id)
    XCTAssertEqual(state.resolvedComposerTarget(viewing: first)?.id, second.id)
    XCTAssertEqual(state.conversationDraft(for: second.id), "A retained draft for Lena")
    XCTAssertFalse(state.conversationMicrophoneOpen(first))
    XCTAssertFalse(state.conversationMicrophoneOpen(second))
    XCTAssertFalse(panel.isVisible, "review fixtures do not claim the user's desktop")
  }
}
