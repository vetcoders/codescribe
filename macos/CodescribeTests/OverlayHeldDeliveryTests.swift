import AppKit
import ApplicationServices
import Foundation
import SwiftUI
import XCTest

@testable import Codescribe

final class OverlayHeldDeliveryTests: XCTestCase {
  private let deadline = "2030-03-17T17:46:50.123456+00:00"
  private var now: Date {
    MessageTimestamp.date(fromISO8601: deadline)!.addingTimeInterval(-7)
  }
  private let messageID = String(repeating: "a", count: 24)
  private let leaseID = String(repeating: "b", count: 32)

  func testTypedFutureHoldParsesMicrosecondsAndExpiresAtTheDeadline() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(typed())
    let message = try firstMessage(bus)
    XCTAssertEqual(message.controlIdentity, messageID)
    XCTAssertEqual(message.holdingSeconds(at: now), 7)
    XCTAssertEqual(message.holdSecondsRemaining, 7)
    XCTAssertEqual(message.deliveryStage(at: now), .held)
    let until = try XCTUnwrap(message.holdUntil)
    XCTAssertEqual(until.timeIntervalSince(now), 7, accuracy: 0.001)
    XCTAssertNil(message.holdingSeconds(at: until))
    XCTAssertEqual(message.deliveryStage(at: until), .sent)
    XCTAssertEqual(message.deliveryStage(at: until.addingTimeInterval(1)), .sent)
    let elapsed = try firstMessage(bus, at: until)
    XCTAssertNil(elapsed.holdSecondsRemaining)
  }

  func testMissingOrMalformedHoldSendsImmediately() throws {
    for value in [nil, "not-a-date"] as [String?] {
      var row = typed()
      row["hold_until"] = value
      var bus = OverlayChannelDelivery.Bus()
      bus.consume(row)
      let message = try firstMessage(bus)
      XCTAssertNil(message.holdUntil)
      XCTAssertEqual(message.deliveryStage(at: now), .sent)
    }
  }

  @MainActor
  func testMemoizedSnapshotReprojectsTheCountdownOnTheExistingPoll() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(typed())
    let snapshot = OverlayChannelDeliverySnapshot(
      deliveries: [], conversations: bus.conversations(busPath: "", now: now))
    let state = OverlayState()
    state.applyConversationSnapshot(snapshot, now: now)
    XCTAssertEqual(state.conversations.flatMap(\.messages).first?.holdSecondsRemaining, 7)
    state.applyConversationSnapshot(snapshot, now: now.addingTimeInterval(1))
    XCTAssertEqual(state.conversations.flatMap(\.messages).first?.holdSecondsRemaining, 6)
    state.applyConversationSnapshot(snapshot, now: now.addingTimeInterval(7))
    XCTAssertNil(state.conversations.flatMap(\.messages).first?.holdSecondsRemaining)
  }

  func testVoiceDraftWithFutureHoldIsNotHeldOrSent() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(voice(status: "utterance_draft"))
    let message = try firstMessage(bus)
    XCTAssertEqual(message.captureState, .receiving)
    XCTAssertNil(message.deliveryStage(at: now))
    XCTAssertNil(message.holdSecondsRemaining)
  }

  func testSealedVoiceUsesTheOriginalUtteranceIDForControls() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(voice(status: "transcript_sealed"))
    let held = try firstMessage(bus)
    XCTAssertEqual(held.controlIdentity, "42")
    XCTAssertEqual(held.deliveryStage(at: now), .held)
    bus.consume([
      "schema": "codescribe.delivery-control.v1", "kind": "revise",
      "identity": "42", "utterance_id": 42, "session_id": "voice-take",
      "text": "Poprawiony głos", "emitted_at": ISO8601DateFormatter().string(from: now),
      "provider": "junie", "provider_session_id": "held-fixture", "lease_id": leaseID,
    ])
    XCTAssertEqual(try firstMessage(bus).text, "Poprawiony głos")
    XCTAssertEqual(try firstMessage(bus).id, held.id)
  }

  func testEvidenceTerminalIsNotHeldUntilTheChannelTakeCloses() throws {
    var bus = OverlayChannelDelivery.Bus()
    let session = "agent-channel-1-held-fixture"
    bus.consume([
      "schema": "codescribe.transcript-evidence.v1", "session_id": session,
      "audience": "fixture", "recipients": [owner()], "reducer_revision": 2,
      "reducer_action": "record_ledger_terminal_seal", "sequence": 2,
      "occurrence_session_id": "capture", "capture_epoch": 1,
      "sample_start": 0, "sample_end": 16000, "document_index": 0,
      "label": "preview label", "rendered_text": "Zamknięty dokument",
      "hold_until": deadline,
    ])
    let preview = try firstMessage(bus)
    XCTAssertEqual(preview.captureState, .receiving)
    XCTAssertNil(preview.holdSecondsRemaining)
    bus.consume([
      "schema": "codescribe.channel-session.v1", "kind": "channel_session",
      "session_id": session, "channel": "1", "provider": "junie",
      "provider_session_id": "held-fixture", "state": "sealed",
      "opened_at": "2030-03-17T17:46:00.000000+00:00", "hold_until": deadline,
    ])
    let sealed = try firstMessage(bus)
    XCTAssertEqual(sealed.id, preview.id)
    XCTAssertEqual(sealed.text, "Zamknięty dokument")
    XCTAssertEqual(sealed.captureState, .finished)
    XCTAssertEqual(sealed.controlIdentity, session)
    XCTAssertEqual(sealed.deliveryStage(at: now), .held)
    XCTAssertEqual(sealed.holdSecondsRemaining, 7)
  }

  func testControlsProjectRevisionReleaseAndWithdrawalWithoutChangingIdentity() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(typed())
    let original = try firstMessage(bus)
    bus.consume(control(.revise, text: "Poprawiony tekst"))
    let revised = try firstMessage(bus)
    XCTAssertEqual(revised.text, "Poprawiony tekst")
    XCTAssertEqual(revised.id, original.id)
    XCTAssertEqual(revised.recipients, original.recipients)
    XCTAssertEqual(revised.holdSecondsRemaining, 7)
    bus.consume(control(.release))
    XCTAssertNil(try firstMessage(bus).holdSecondsRemaining)
    XCTAssertEqual(try firstMessage(bus).deliveryStage(at: now), .sent)
    bus.consume(control(.withdraw))
    XCTAssertEqual(try firstMessage(bus).text, "Poprawiony tekst")
    var withdrawn = OverlayChannelDelivery.Bus()
    withdrawn.consume(typed())
    withdrawn.consume(control(.withdraw))
    bus = withdrawn
    XCTAssertTrue(bus.conversations(busPath: "", now: now).flatMap(\.messages).isEmpty)
    // Replaying a source row cannot undo an explicit control.
    bus.consume(typed())
    XCTAssertTrue(bus.conversations(busPath: "", now: now).flatMap(\.messages).isEmpty)
    let restored = try JSONDecoder().decode(
      OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
    XCTAssertTrue(restored.conversations(busPath: "", now: now).flatMap(\.messages).isEmpty)
  }

  func testClosingEvidenceAndCleanTwinsProjectOneHeldVoiceMessage() throws {
    let session = "agent-channel-1-closed-twin"
    var evidence: [String: Any] = [
      "schema": "codescribe.transcript-evidence.v1", "session_id": session,
      "audience": "fixture", "recipients": [owner()], "reducer_revision": 2,
      "reducer_action": "record_ledger_terminal_seal", "sequence": 4,
      "occurrence_session_id": "capture", "capture_epoch": 1,
      "sample_start": 0, "sample_end": 16000, "document_index": 0,
      "label": "not the document", "rendered_text": "Dokument z evidence",
      "utterance_id": 42,
    ]
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(evidence)
    XCTAssertNil(try firstMessage(bus).holdSecondsRemaining)
    // The closing snapshot is the one row the Rust writer marks terminal.
    evidence["status"] = "transcript_sealed"
    evidence["terminal"] = true
    evidence["hold_until"] = deadline
    evidence["sequence"] = 5
    bus.consume(evidence)
    var clean = evidence
    clean["schema"] = "codescribe.transcript.v1"
    clean["text"] = "not the document either"
    clean["sequence"] = 6
    bus.consume(clean)
    let conversation = try XCTUnwrap(bus.conversations(busPath: "", now: now).first {
      $0.owner != nil
    })
    XCTAssertEqual(conversation.messages.count, 1)
    XCTAssertEqual(conversation.messages.first?.text, "Dokument z evidence")
    XCTAssertEqual(conversation.messages.first?.holdSecondsRemaining, 7)
    XCTAssertEqual(conversation.messages.first?.controlIdentity, "42")
    XCTAssertEqual(conversation.messages.first?.occurrenceIDs?.count, 1)
  }

  func testBroadcastRevisionStaysSharedWhenAnotherRecipientCopyArrives() throws {
    var first = typed()
    first["origin_channel"] = "0"
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(first)
    bus.consume(control(.revise, text: "Poprawiony broadcast"))
    var second = first
    var peer = owner()
    peer["channel"] = "2"
    peer["provider_session_id"] = "second-fixture"
    peer["lease_id"] = String(repeating: "c", count: 32)
    second["recipients"] = [peer]
    bus.consume(second)
    let shared = try XCTUnwrap(bus.conversations(busPath: "", now: now).first {
      $0.channel == "0"
    })
    XCTAssertEqual(shared.messages.count, 1)
    XCTAssertEqual(shared.messages.first?.text, "Poprawiony broadcast")
    XCTAssertEqual(shared.messages.first?.recipients.count, 2)
    XCTAssertEqual(shared.messages.first?.holdSecondsRemaining, 7)
  }

  func testFiveEqualTextsRemainFiveIdentitiesAndOnlyOneIsRevised() throws {
    var bus = OverlayChannelDelivery.Bus()
    for digit in 1...5 {
      var row = typed()
      row["message_id"] = String(repeating: String(digit), count: 24)
      row["text"] = "Iwo"
      bus.consume(row)
    }
    var revision = control(.revise, text: "Poprawiony Iwo")
    revision["message_id"] = String(repeating: "3", count: 24)
    revision["identity"] = String(repeating: "3", count: 24)
    bus.consume(revision)
    let messages = try XCTUnwrap(bus.conversations(busPath: "", now: now).first {
      $0.owner != nil
    }).messages
    XCTAssertEqual(messages.count, 5)
    XCTAssertEqual(Set(messages.map(\.id)).count, 5)
    XCTAssertEqual(messages.filter { $0.text == "Iwo" }.count, 4)
    XCTAssertEqual(messages.filter { $0.text == "Poprawiony Iwo" }.count, 1)
  }

  func testForeignOwnerCannotReviseAndQueueEvidenceEndsTheHeldProjection() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(typed())
    var foreign = control(.revise, text: "wrong")
    foreign["provider"] = "junie"
    foreign["provider_session_id"] = "other-session"
    bus.consume(foreign)
    var message = try firstMessage(bus)
    XCTAssertEqual(message.text, "Pierwsza wiadomość")
    message.recipients[0].queued = true
    XCTAssertNil(message.holdingSeconds(at: now))
    XCTAssertEqual(message.deliveryStage(at: now), .queued)
  }

  func testExpiredOrHoldDisabledRowsIgnoreRevisionControls() throws {
    for hold in [nil, "2000-01-01T00:00:00.000000Z"] as [String?] {
      var bus = OverlayChannelDelivery.Bus()
      var message = typed()
      message["hold_until"] = hold
      bus.consume(message)
      bus.consume(control(.revise, text: "must not replace delivered text"))
      XCTAssertEqual(try firstMessage(bus).text, "Pierwsza wiadomość")
      XCTAssertEqual(try firstMessage(bus).deliveryStage(at: now), .sent)
    }
  }

  func testReusedVoiceUtteranceLabelRequiresAnUnambiguousTake() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(voice(status: "transcript_sealed"))
    var other = voice(status: "transcript_sealed")
    other["session_id"] = "another-voice-take"
    bus.consume(other)
    var revision: [String: Any] = [
      "schema": "codescribe.delivery-control.v1", "kind": "revise",
      "identity": "42", "utterance_id": 42, "text": "Zmieniony drugi take",
      "emitted_at": ISO8601DateFormatter().string(from: now), "provider": "junie",
      "provider_session_id": "held-fixture", "lease_id": leaseID,
    ]
    bus.consume(revision)
    XCTAssertEqual(bus.conversations(busPath: "", now: now).flatMap(\.messages).filter {
      $0.text == "Zmieniony drugi take"
    }.count, 0)
    revision["session_id"] = "another-voice-take"
    bus.consume(revision)
    XCTAssertEqual(bus.conversations(busPath: "", now: now).flatMap(\.messages).filter {
      $0.text == "Zmieniony drugi take"
    }.count, 1)
  }

  @MainActor
  func testEditOnAHeldMessageWithdrawsOnceAndReturnsTheTextToAnEmptyComposer() throws {
    let host = try mountHeldComposer(draft: "")
    defer { host.close() }
    XCTAssertEqual(try host.editor().string, "")
    XCTAssertEqual(
      try host.press("overlay-message-cancel-edit-\(host.messageID)"), AXError.success.rawValue)
    host.waitUntil { host.log.events.count == 1 && host.editorString() == host.messageText }
    XCTAssertEqual(
      host.log.events.map(\.control), [.withdraw],
      "Edit must withdraw the held message once and never revise it, otherwise the original would still go out at expiry"
    )
    XCTAssertEqual(host.log.events.map(\.messageID), [host.messageID])
    XCTAssertEqual(host.log.events.map(\.text), [nil], "the withdraw carries no replacement body")
    XCTAssertEqual(
      host.log.events.map(\.ownerIsNil), [false],
      "a channel message withdraws as the conversation owner, the same rule as Withdraw")
    XCTAssertEqual(
      try host.editor().string, host.messageText,
      "after the withdraw returns, the empty composer holds the message text; otherwise the original would still go out at expiry"
    )
  }

  @MainActor
  func testEditAppendsHeldTextAfterABlankLineWhenTheComposerAlreadyHasTyping() throws {
    let host = try mountHeldComposer(draft: "Już piszę")
    defer { host.close() }
    XCTAssertEqual(try host.editor().string, "Już piszę")
    XCTAssertEqual(
      try host.press("overlay-message-cancel-edit-\(host.messageID)"), AXError.success.rawValue)
    let expected = "Już piszę\n\n\(host.messageText)"
    host.waitUntil { host.editorString() == expected }
    XCTAssertEqual(
      host.log.events.map(\.control), [.withdraw],
      "appending into a draft still has to withdraw first, otherwise the original would still go out at expiry"
    )
    XCTAssertFalse(host.log.events.contains { $0.control == .revise })
    XCTAssertEqual(
      try host.editor().string, expected,
      "typing already in the composer stays, then a blank line, then the held text; overwriting would drop it"
    )
  }

  @MainActor
  func testEditWhoseWithdrawThrowsLeavesTheComposerAndShowsTheError() throws {
    let host = try mountHeldComposer(draft: "zostaje", thrown: HeldWithdrawRefusal())
    defer { host.close() }
    XCTAssertEqual(
      try host.press("overlay-message-cancel-edit-\(host.messageID)"), AXError.success.rawValue)
    let shown = try NativeAccessibilityProbe.waitForElements(
      "overlay-message-delivery-error-\(host.messageID)")
    let element = try XCTUnwrap(
      shown.elements.first,
      "a failed withdraw must show the error on the bubble; otherwise the text would look taken back while the original still goes out at expiry"
    )
    XCTAssertTrue(
      [element.label, element.value].contains("withdraw refused"),
      "the bubble must show the withdraw error, not a silent failure, because the original would still go out at expiry"
    )
    XCTAssertEqual(host.log.events.map(\.control), [.withdraw])
    XCTAssertFalse(host.log.events.contains { $0.control == .revise })
    XCTAssertEqual(
      try host.editor().string, "zostaje",
      "a failed withdraw leaves the composer unchanged, otherwise the text would look taken back while the original still goes out at expiry"
    )
  }

  @MainActor
  func testEditOnAnExpiredHoldPublishesNothingAndLeavesTheComposer() throws {
    let host = try mountHeldComposer(at: now.addingTimeInterval(7), draft: "zostaje")
    defer { host.close() }
    XCTAssertNil(host.holdSecondsRemaining)
    let edit = try NativeAccessibilityProbe.query("overlay-message-cancel-edit-\(host.messageID)")
    XCTAssertTrue(
      edit.elements.isEmpty,
      "an expired hold must not offer Edit; a leftover control would withdraw a message that already went out, or copy it into the composer as if it were still on the clock"
    )
    XCTAssertTrue(
      host.log.events.isEmpty,
      "an expired hold publishes no delivery control and leaves the composer alone")
    XCTAssertEqual(try host.editor().string, "zostaje")
  }

  @MainActor
  func testAHeldBubbleOffersOnlyCancelEditAndSendNow() throws {
    let host = try mountHeldComposer(draft: "")
    defer { host.close() }
    for identifier in ["cancel-edit", "release"] {
      XCTAssertEqual(
        try NativeAccessibilityProbe.waitForElements(
          "overlay-message-\(identifier)-\(host.messageID)"
        ).elements.count, 1, "the held bubble must offer \(identifier)")
    }
    for retired in ["edit", "withdraw"] {
      XCTAssertTrue(
        try NativeAccessibilityProbe.query("overlay-message-\(retired)-\(host.messageID)")
          .elements.isEmpty,
        "a separate \(retired) control would take the message off the clock without returning its text, or return it while the original still goes out"
      )
    }
    XCTAssertTrue(host.log.events.isEmpty, "showing the controls publishes nothing")
  }

  func testCancelEditLabelInEnglishAndPolish() throws {
    for (language, expected) in [("en", "Cancel/Edit"), ("pl", "Cofnij/Edytuj")] {
      let path = try XCTUnwrap(Bundle.main.path(forResource: language, ofType: "lproj"))
      let bundle = try XCTUnwrap(Bundle(path: path))
      XCTAssertEqual(
        bundle.localizedString(forKey: "Cancel/Edit", value: nil, table: nil), expected)
    }
  }

  func testCountdownCopyForSevenSecondsInEnglishAndPolish() throws {
    for (language, expected) in [("en", "Sending in 7 s"), ("pl", "Wysyłka za 7 s")] {
      let path = try XCTUnwrap(Bundle.main.path(forResource: language, ofType: "lproj"))
      let bundle = try XCTUnwrap(Bundle(path: path))
      XCTAssertEqual(HeldDeliveryCopy.countdown(seconds: 7, bundle: bundle), expected)
    }
  }

  private func owner() -> [String: Any] {
    [
      "provider": "junie", "provider_session_id": "held-fixture",
      "lease_id": leaseID, "channel": "1", "name": "fixture",
    ]
  }

  private func typed() -> [String: Any] {
    [
      "schema": "codescribe.agent-user-message.v1", "kind": "agent_user_message",
      "message_id": messageID, "text": "Pierwsza wiadomość", "audience": "fixture",
      "channel": "1", "recipients": [owner()], "hold_until": deadline,
    ]
  }

  private func voice(status: String) -> [String: Any] {
    [
      "schema": "codescribe.transcript.v1", "status": status, "session_id": "voice-take",
      "utterance_id": 42, "sequence": 1, "text": "Pierwsza wiadomość",
      "channel": "1", "audience": "fixture", "recipients": [owner()], "hold_until": deadline,
    ]
  }

  private func control(_ kind: OverlayDeliveryControl, text: String? = nil) -> [String: Any] {
    var row: [String: Any] = [
      "schema": "codescribe.delivery-control.v1", "kind": kind.rawValue,
      "identity": messageID, "message_id": messageID,
      "emitted_at": ISO8601DateFormatter().string(from: now), "provider": "junie",
      "provider_session_id": "held-fixture", "lease_id": leaseID,
    ]
    row["text"] = text
    return row
  }

  private func firstMessage(_ bus: OverlayChannelDelivery.Bus, at date: Date? = nil) throws
    -> OverlayConversationMessage
  {
    try XCTUnwrap(bus.conversations(busPath: "", now: date ?? now).first {
      $0.owner != nil
    }?.messages.first)
  }

  @MainActor
  private func mountHeldComposer(
    at date: Date? = nil, draft: String, thrown: Error? = nil
  ) throws -> HeldComposerHost {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(typed())
    let conversation = try XCTUnwrap(
      bus.conversations(busPath: "", now: date ?? now).first {
        $0.owner != nil
      })
    let message = try XCTUnwrap(conversation.messages.first)
    let log = DeliveryControlLog(thrown: thrown)
    let host = NSHostingView(
      rootView: HeldEditHarness(
        conversation: conversation, draft: draft,
        onDeliveryControl: { control, message, owner, text in
          try log.record(
            control: control, messageID: message.id, text: text, ownerIsNil: owner == nil)
        }))
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 640, height: 520)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    window.orderFrontRegardless()
    let mounted = HeldComposerHost(
      window: window, host: host, log: log, messageID: message.id, messageText: message.text,
      holdSecondsRemaining: message.holdSecondsRemaining)
    mounted.settle()
    return mounted
  }
}

private struct HeldWithdrawRefusal: LocalizedError {
  var errorDescription: String? { "withdraw refused" }
}

private struct HeldEditHarness: View {
  let conversation: OverlayConversation
  @State var draft: String
  let onDeliveryControl:
    (
      OverlayDeliveryControl, OverlayConversationMessage, OverlayConversationOwner?, String?
    ) async throws -> Void

  var body: some View {
    OverlayConversationView(
      conversation: conversation, palette: .light, topInset: 20, bottomInset: 12,
      pendingControls: [], controlErrors: [:], onControl: { _, _ in },
      draft: $draft, sending: false, sendError: nil, onSend: {},
      onDeliveryControl: onDeliveryControl)
  }
}

private final class DeliveryControlLog: @unchecked Sendable {
  struct Event: Equatable {
    var control: OverlayDeliveryControl
    var messageID: String
    var text: String?
    var ownerIsNil: Bool
  }

  private let lock = NSLock()
  private var storage: [Event] = []
  private let thrown: Error?

  init(thrown: Error? = nil) { self.thrown = thrown }

  func record(
    control: OverlayDeliveryControl, messageID: String, text: String?, ownerIsNil: Bool
  ) throws {
    lock.lock()
    storage.append(
      Event(control: control, messageID: messageID, text: text, ownerIsNil: ownerIsNil))
    lock.unlock()
    if let thrown { throw thrown }
  }

  var events: [Event] {
    lock.lock()
    defer { lock.unlock() }
    return storage
  }
}

@MainActor
private final class HeldComposerHost {
  let window: NSWindow
  let host: NSView
  let log: DeliveryControlLog
  let messageID: String
  let messageText: String
  let holdSecondsRemaining: Int?

  init(
    window: NSWindow, host: NSView, log: DeliveryControlLog, messageID: String,
    messageText: String, holdSecondsRemaining: Int?
  ) {
    self.window = window
    self.host = host
    self.log = log
    self.messageID = messageID
    self.messageText = messageText
    self.holdSecondsRemaining = holdSecondsRemaining
  }

  func close() { window.close() }

  func settle() {
    host.layoutSubtreeIfNeeded()
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.2))
    host.layoutSubtreeIfNeeded()
  }

  func waitUntil(_ predicate: () -> Bool) {
    let deadline = Date().addingTimeInterval(1)
    while !predicate() && Date() < deadline {
      settle()
    }
  }

  func editorString() -> String? { editableEditor()?.string }

  func editor() throws -> NSTextView {
    try XCTUnwrap(editableEditor(), "the conversation composer must be mounted")
  }

  private func editableEditor() -> NSTextView? {
    func find(_ root: NSView) -> NSTextView? {
      if let text = root as? NSTextView, text.isEditable { return text }
      return root.subviews.lazy.compactMap { find($0) }.first
    }
    return find(host)
  }

  func press(_ identifier: String) throws -> Int32 {
    let reception = try NativeAccessibilityProbe.waitForElements(identifier)
    XCTAssertEqual(
      reception.elements.count, 1, "one control must answer \(identifier)")
    return try XCTUnwrap(NativeAccessibilityProbe.query(identifier, press: true).pressStatus)
  }
}
