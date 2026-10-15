import Foundation
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
    evidence["status"] = "transcript_sealed"
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
}
