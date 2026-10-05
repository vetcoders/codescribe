import AppKit
import Foundation
import SwiftUI
import XCTest

@testable import Codescribe

final class OverlayConversationAcceptanceTests: XCTestCase {
  func testFollowerDeliveryIdentitySurvivesPrunedMailboxAndQuietRestart() throws {
    // Golden IDs produced by the canonical Python follower, including Unicode case folding.
    let cases: [(String, Bool, String)] = [
      ("Lena", false, "9655faaf3646b0a670acb8d8"),
      ("Lena", true, "cf7eb3fe3ae22e59c9624a0b"),
      ("Straße", false, "4c052a44a94110dc857c9df9"),
      ("Straße", true, "c24d69d2aadaf3054ebc3b6e"),
      ("*", false, "5065093fac94bf219c2b211e"),
      ("*", true, "b4433a2426097d2bd4c556a7"),
    ]
    for (audience, terminal, delivery) in cases {
      var bus = OverlayChannelDelivery.Bus()
      var row = occurrence(0, revision: 1)
      row["audience"] = audience
      if !terminal { row["reducer_action"] = "apply_manual_edit" }
      bus.consume(row)
      XCTAssertEqual(try all(bus).messages.first?.recipients.first?.deliveryID, delivery)
      bus = try JSONDecoder().decode(
        OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
      var ack = owner(leaseA)
      ack.merge(["schema": "codescribe.agent-ack.v1", "delivery_id": delivery]) { _, new in new }
      bus.consume(ack)
      bus.consume(reply(String(repeating: "d", count: 24), delivery: delivery))
      let messages = try all(bus).messages
      XCTAssertEqual(messages.first?.recipients.first?.acknowledged, true)
      XCTAssertEqual(messages.last?.replyTo, messages.first?.recipients.first?.deliveryID)
    }
  }

  func testTypedMessagesRetainDistinctIdentityAndCausalReceipt() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<5 {
      let identity = String(format: "%024x", index + 1)
      bus.consume([
        "schema": "codescribe.agent-user-message.v1", "kind": "agent_user_message",
        "message_id": identity, "text": "Iwo", "audience": "lena", "channel": "2",
        "recipients": [owner(leaseA)], "emitted_at": "2026-10-05T10:00:00Z",
      ])
    }
    let rows = try all(bus).messages
    XCTAssertEqual(rows.count, 5)
    XCTAssertEqual(Set(rows.map(\.id)).count, 5)
    let delivery = try XCTUnwrap(rows.first?.recipients.first?.deliveryID)
    bus.consume(reply(String(repeating: "d", count: 24), delivery: delivery))
    XCTAssertEqual(try all(bus).messages.last?.replyTo, delivery)
    bus = try JSONDecoder().decode(OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
    XCTAssertEqual(try all(bus).messages.count, 6)
  }

  @MainActor
  func testComposerReturnPublishesEditorBytesWithoutEndingEditing() throws {
    var draft = "Szkic"
    var sent: [String] = []
    let onSend = { sent.append(draft) }
    try withComposer(
      draft: Binding(get: { draft }, set: { draft = $0 }), onSend: onSend
    ) {
      scroll, editor in
      editor.string = "Ostatnie słowa z edytora"
      editor.keyDown(with: try returnEvent())
      XCTAssertEqual(sent, ["Ostatnie słowa z edytora"])
      XCTAssertTrue(editor.window?.firstResponder === editor)
      XCTAssertLessThan(scroll.bounds.height, 40, "a single-line draft must keep the input compact")
      XCTAssertEqual(scroll.borderType, .noBorder, "the shared composer supplies chrome")
    }
  }

  @MainActor
  func testComposerShiftReturnInsertsLineAndReturnSendsBothLines() throws {
    var draft = "Pierwsza"
    var sent: [String] = []
    let onSend = { sent.append(draft) }
    try withComposer(
      draft: Binding(get: { draft }, set: { draft = $0 }), onSend: onSend
    ) {
      _, editor in
      editor.setSelectedRange(NSRange(location: editor.string.utf16.count, length: 0))
      editor.keyDown(with: try returnEvent(modifiers: .shift))
      XCTAssertTrue(sent.isEmpty)
      XCTAssertEqual(editor.string, "Pierwsza\n")
      editor.insertText("Druga", replacementRange: editor.selectedRange())
      editor.keyDown(with: try returnEvent())
      XCTAssertEqual(sent, ["Pierwsza\nDruga"])
    }
  }

  @MainActor
  func testComposerEmptyOrPendingReturnDoesNotSend() throws {
    for pending in [false, true] {
      var draft = pending ? "Już wysyłana" : "  "
      var sends = 0
      let onSend = { sends += 1 }
      try withComposer(
        draft: Binding(get: { draft }, set: { draft = $0 }), sending: pending,
        onSend: onSend
      ) { _, editor in
        editor.keyDown(with: try returnEvent())
        XCTAssertEqual(sends, 0)
      }
    }
  }

  @MainActor
  func testComposerMarkedTextAndOtherCommandsRemainNative() throws {
    var draft = "Tekst"
    var sends = 0
    let onSend = { sends += 1 }
    try withComposer(draft: Binding(get: { draft }, set: { draft = $0 }), onSend: onSend) {
      _, editor in
      editor.setMarkedText(
        "拼", selectedRange: NSRange(location: 1, length: 0),
        replacementRange: editor.selectedRange())
      XCTAssertTrue(editor.hasMarkedText())
      editor.keyDown(with: try returnEvent())
      XCTAssertEqual(sends, 0, "Return commits composition without submitting")
      editor.unmarkText()
      editor.setSelectedRange(NSRange(location: editor.string.utf16.count, length: 0))
      let end = editor.selectedRange().location
      editor.moveLeft(nil)
      XCTAssertLessThan(editor.selectedRange().location, end)
      XCTAssertEqual(sends, 0)
    }
  }

  @MainActor
  func testConversationOpensAtLatestAndBubblesHaveOppositeEdges() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<12 {
      bus.consume(
        occurrence(
          index * 3200, revision: index + 1,
          text: "Wiadomość człowieka, która ma pozostać po prawej stronie rozmowy."))
      let delivery = try XCTUnwrap(try all(bus).messages.last?.recipients.first?.deliveryID)
      bus.consume(reply(String(format: "%024x", index + 1), delivery: delivery))
    }
    let conversation = try XCTUnwrap(
      bus.conversations(busPath: busPath).first { $0.channel == "2" })
    for scheme in [ColorScheme.dark, .light] {
      let view = OverlayConversationView(
        conversation: conversation, palette: .resolve(scheme), topInset: 50, bottomInset: 20,
        pendingControls: [], controlErrors: [:], onControl: { _, _ in }, onShowMonitor: {},
        draft: .constant(""), sending: false, sendError: nil, onSend: {})
      let host = NSHostingView(
        rootView: view.background(OverlayAppearancePalette.resolve(scheme).desktopBackground.color)
          .preferredColorScheme(scheme))
      host.frame = NSRect(x: 0, y: 0, width: 600, height: 500)
      let window = NSWindow(
        contentRect: host.frame, styleMask: .borderless,
        backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      defer { window.close() }
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.08))
      func scrollViews(_ root: NSView) -> [NSScrollView] {
        (root as? NSScrollView).map { [$0] } ?? root.subviews.flatMap(scrollViews)
      }
      let scroll = try XCTUnwrap(scrollViews(host).first { $0.bounds.height > 150 })
      let viewport = scroll.convert(scroll.bounds, to: host)
      XCTAssertEqual(
        viewport.height, host.bounds.height, accuracy: 1,
        "messages must scroll beneath the fixed header and composer, without a clipped middle strip"
      )
      XCTAssertEqual(viewport.minY, host.bounds.minY, accuracy: 1)
      let document = try XCTUnwrap(scroll.documentView)
      XCTAssertGreaterThan(
        scroll.documentVisibleRect.maxY, document.bounds.height - 50,
        "opening a channel must reveal the latest row")
      let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
      host.cacheDisplay(in: host.bounds, to: bitmap)
      let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
      try png.write(
        to: FileManager.default.temporaryDirectory
          .appendingPathComponent("codescribe-conversation-\(scheme).png"))
      var warmCount = 0
      var warmX: Double = 0
      var neutralCount = 0
      var neutralX: Double = 0
      // Read actual rendered fills, including wrapping, at both appearance settings.
      for y in stride(from: 90, to: bitmap.pixelsHigh - 30, by: 4) {
        for x in stride(from: 0, to: bitmap.pixelsWide, by: 4) {
          guard let color = bitmap.colorAt(x: x, y: y) else {
            continue
          }
          let r = color.redComponent
          let g = color.greenComponent
          let b = color.blueComponent
          if r - g > 0.035 && g - b > 0.025 {
            warmCount += 1
            warmX += Double(x)
          }
          let agentFill = scheme == .dark ? 0.149 : 0.914
          if abs(r - agentFill) < 0.018 && abs(r - g) < 0.03 {
            neutralCount += 1
            neutralX += Double(x)
          }
        }
      }
      XCTAssertGreaterThan(warmCount, 100)
      XCTAssertGreaterThan(neutralCount, 100)
      XCTAssertGreaterThan(warmX / Double(max(1, warmCount)), Double(bitmap.pixelsWide) / 2)
      XCTAssertLessThan(neutralX / Double(max(1, neutralCount)), Double(bitmap.pixelsWide) / 2)

    }
  }

  @MainActor
  private func returnEvent(modifiers: NSEvent.ModifierFlags = []) throws -> NSEvent {
    try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: modifiers,
        timestamp: 0, windowNumber: 0, context: nil, characters: "\r",
        charactersIgnoringModifiers: "\r",
        isARepeat: false, keyCode: 36))
  }

  @MainActor
  private func withComposer(
    draft: Binding<String>, sending: Bool = false, onSend: @escaping () -> Void,
    inspect: (NSScrollView, NSTextView) throws -> Void
  ) throws {
    let owner = try XCTUnwrap(OverlayConversationOwner(row: owner(leaseA)))
    let view = OverlayConversationView(
      conversation: .init(id: owner.id, channel: "2", name: "Lena", owner: owner, messages: []),
      palette: .dark, topInset: 50, bottomInset: 20, pendingControls: [], controlErrors: [:],
      onControl: { _, _ in }, onShowMonitor: {}, draft: draft, sending: sending, sendError: nil,
      onSend: onSend)
    let host = NSHostingView(rootView: view)
    host.frame = NSRect(x: 0, y: 0, width: 600, height: 500)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless,
      backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    host.layoutSubtreeIfNeeded()
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.03))
    func editableTextView(_ root: NSView) -> NSTextView? {
      if let editor = root as? NSTextView, editor.isEditable { return editor }
      return root.subviews.lazy.compactMap(editableTextView).first
    }
    let editor = try XCTUnwrap(editableTextView(host))
    let scroll = try XCTUnwrap(editor.enclosingScrollView)
    XCTAssertTrue(window.makeFirstResponder(editor))
    try inspect(scroll, editor)
  }

  private let busPath = "/fixture/transcript-events.jsonl"
  private let leaseA = String(repeating: "a", count: 32)
  private let leaseB = String(repeating: "b", count: 32)

  private func owner(
    _ lease: String, session: String = "agent-a", name: String = "Lena", channel: String = "2"
  ) -> [String: Any] {
    [
      "provider": "codex", "provider_session_id": session, "lease_id": lease,
      "name": name, "channel": channel,
    ]
  }

  private func occurrence(
    _ start: Int, revision: Int, text: String = "Iwo", recipients: [[String: Any]]? = nil
  ) -> [String: Any] {
    [
      "schema": "codescribe.transcript-evidence.v1", "session_id": "agent-channel-2-take-a",
      "occurrence_session_id": "agent-channel-2-take-a", "capture_epoch": 1,
      "sample_start": start, "sample_end": start + 1600, "document_index": 0,
      "reducer_revision": revision, "sequence": revision,
      "reducer_action": "record_ledger_terminal_seal", "audience": "Lena",
      "rendered_text": text, "recipients": recipients ?? [owner(leaseA)],
    ]
  }

  private func reply(_ replyID: String, delivery: String? = nil) -> [String: Any] {
    var row = owner(leaseA)
    row.merge([
      "schema": "codescribe.agent-reply.v1", "reply_id": replyID,
      "text": "Odpowiedź", "association": delivery == nil ? "unsolicited" : "addressed",
      "emitted_at": "2026-10-05T04:00:00Z",
    ]) { _, new in new }
    if let delivery { row["delivery_id"] = delivery }
    return row
  }

  private func playback(_ replyID: String, ticket: String, state: String, time: String) -> [String:
    Any]
  {
    var row = owner(leaseA)
    row.merge([
      "schema": "codescribe.agent-reply-playback.v1", "reply_id": replyID,
      "playback_ticket": ticket, "state": state, "emitted_at": time,
      "spoken": state == "spoken",
    ]) { _, new in new }
    return row
  }

  private func all(_ bus: OverlayChannelDelivery.Bus) throws -> OverlayConversation {
    try XCTUnwrap(bus.conversations(busPath: busPath).first { $0.channel == "0" })
  }

  func testFiveEqualLabelsInOneCaptureAndDocumentStayFiveRanges() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<5 { bus.consume(occurrence(index * 3200, revision: index + 1)) }
    let rows = try all(bus).messages
    XCTAssertEqual(rows.count, 5)
    XCTAssertEqual(Set(rows.map(\.id)).count, 5)
    XCTAssertEqual(rows.map(\.text), Array(repeating: "Iwo", count: 5))
    bus.consume(occurrence(0, revision: 10, text: "Iwo poprawione"))
    bus.consume(occurrence(0, revision: 1, text: "stara odpowiedź"))
    let revised = try all(bus).messages
    XCTAssertEqual(revised.count, 5)
    XCTAssertEqual(revised.first?.text, "Iwo poprawione")
    XCTAssertEqual(revised.map(\.id), rows.map(\.id))
  }

  func testBroadcastIsOneQuestionWithFrozenRecipientsAndNoLateInheritance() throws {
    var bus = OverlayChannelDelivery.Bus()
    let recipients = [owner(leaseA), owner(leaseB, session: "agent-b", name: "Adam", channel: "4")]
    var question = occurrence(0, revision: 1, recipients: recipients)
    question["audience"] = "*"
    bus.consume(question)
    XCTAssertEqual(try all(bus).messages.count, 1)
    XCTAssertEqual(try all(bus).messages.first?.recipients.count, 2)
    let leaseC = String(repeating: "c", count: 32)
    bus.observeLease(
      [
        "schema": "codescribe.agent-bridge.lease.v1", "provider": "codex",
        "provider_session_id": "agent-c", "lease_id": leaseC, "bus": busPath,
        "pending": [],
      ], channel: "3",
      binding: ["provider": "codex", "provider_session_id": "agent-c", "audience": "Astra"])
    let conversations = bus.conversations(busPath: busPath)
    XCTAssertEqual(conversations.filter { $0.owner != nil && !$0.messages.isEmpty }.count, 2)
    XCTAssertTrue(
      conversations.filter { $0.owner?.leaseID == leaseC }.allSatisfy { $0.messages.isEmpty })
    XCTAssertEqual(try all(bus).messages.first?.recipients.count, 2)
  }

  func testLateAcknowledgmentRetainsReplyAndForeignOwnerCannotAcknowledge() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(occurrence(0, revision: 1))
    let delivery = try XCTUnwrap(try all(bus).messages.first?.recipients.first?.deliveryID)
    bus.consume(reply(String(repeating: "d", count: 24), delivery: delivery))
    var foreign = owner(leaseB, session: "agent-b")
    foreign.merge(["schema": "codescribe.agent-ack.v1", "delivery_id": delivery]) { _, new in new }
    bus.consume(foreign)
    XCTAssertEqual(try all(bus).messages.first?.recipients.first?.acknowledged, false)
    var ack = owner(leaseA)
    ack.merge(["schema": "codescribe.agent-ack.v1", "delivery_id": delivery]) { _, new in new }
    bus.consume(ack)
    let rows = try all(bus).messages
    XCTAssertEqual(rows.count, 2)
    XCTAssertEqual(rows.first?.recipients.first?.acknowledged, true)
    XCTAssertEqual(rows.last?.text, "Odpowiedź")
    XCTAssertEqual(rows.last?.replyTo, delivery)
  }

  func testRebindingAndRenamingDoNotRelabelHistoricalReply() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(reply(String(repeating: "d", count: 24)))
    bus.observeLease(
      [
        "schema": "codescribe.agent-bridge.lease.v1", "provider": "codex",
        "provider_session_id": "agent-b", "lease_id": leaseB, "bus": busPath,
        "pending": [],
      ], channel: "2",
      binding: ["provider": "codex", "provider_session_id": "agent-b", "audience": "Nowy"])
    let old = try XCTUnwrap(
      bus.conversations(busPath: busPath).first { $0.owner?.leaseID == leaseA })
    XCTAssertEqual(old.name, "Lena")
    XCTAssertEqual(old.messages.first?.owner?.providerSessionID, "agent-a")
    XCTAssertTrue(
      bus.conversations(busPath: busPath).filter { $0.owner?.leaseID == leaseB }.allSatisfy {
        $0.messages.isEmpty
      })
  }

  func testPriorPlaybackTicketCannotCompleteOrRearmNewPlayback() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(reply(String(repeating: "d", count: 24)))
    bus.consume(
      playback(
        String(repeating: "d", count: 24), ticket: "old", state: "waiting",
        time: "2026-10-05T04:00:00Z"))
    bus.consume(
      playback(
        String(repeating: "d", count: 24), ticket: "new", state: "playing",
        time: "2026-10-05T04:00:02Z"))
    bus.consume(
      playback(
        String(repeating: "d", count: 24), ticket: "old", state: "spoken",
        time: "2026-10-05T04:00:03Z"))
    XCTAssertEqual(try all(bus).messages.last?.playback?.ticket, "new")
    bus.consume(
      playback(
        String(repeating: "d", count: 24), ticket: "old", state: "waiting",
        time: "2026-10-05T04:00:04Z"))
    XCTAssertEqual(try all(bus).messages.last?.playback?.ticket, "new")
    XCTAssertEqual(try all(bus).messages.last?.playback?.state, "playing")
  }

  func testProjectionRoundTripPreservesDistinctRowsAndCausalOwnership() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<5 { bus.consume(occurrence(index * 3200, revision: index + 1)) }
    let delivery = try XCTUnwrap(try all(bus).messages.last?.recipients.first?.deliveryID)
    bus.consume(reply(String(repeating: "d", count: 24), delivery: delivery))
    let restored = try JSONDecoder().decode(
      OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
    XCTAssertEqual(restored.conversations(busPath: busPath), bus.conversations(busPath: busPath))
  }

  func testRawObservationsCannotCreateConversationText() throws {
    var bus = OverlayChannelDelivery.Bus()
    var observation = occurrence(0, revision: 1)
    observation["schema"] = "codescribe.raw.v1"
    observation["kind"] = "correction"
    bus.consume(observation)
    XCTAssertTrue(try all(bus).messages.isEmpty)
  }

  func testHistoryRetainsOnlyTheLatestBoundedRows() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<300 { bus.consume(occurrence(index * 3200, revision: index + 1)) }
    let rows = try all(bus).messages
    XCTAssertEqual(rows.count, 256)
    XCTAssertEqual(rows.first?.order, 45)
    XCTAssertEqual(rows.last?.order, 300)
  }

  func testReplyIdentifierCannotMasqueradeAsLeaseIdentifier() throws {
    XCTAssertNil(OverlayConversationOwner(row: owner(String(repeating: "a", count: 24))))
    XCTAssertNotNil(OverlayConversationOwner(row: owner(leaseA)))
  }
}
