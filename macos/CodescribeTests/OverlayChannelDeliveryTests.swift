import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayChannelDeliveryTests: XCTestCase {
  func testFixtureTriadRequiresNewestSealPendingEnvelopeAndMatchingBusAck() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    var statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .sent)
    XCTAssertEqual(statuses.first?.deliveryID, Fixture.firstID)
    XCTAssertEqual(statuses.first?.isOpen, true)

    try fixture.lease(pending: [fixture.envelope(Fixture.firstID)])
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .queued)

    // The durable marker can precede the watcher row. It is not that row.
    try fixture.marker(Fixture.firstID)
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .queued)
    try fixture.append(fixture.ack(Fixture.firstID, channel: "2"))
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .queued, "another channel cannot acknowledge this one")
    try fixture.append(fixture.ack(Fixture.firstID))
    try fixture.lease(pending: [])
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .received)
    XCTAssertEqual(statuses.first?.isOpen, true, "receipt does not close the microphone")

    try fixture.append(fixture.seal(2))
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.deliveryID, "d8281fc3cceedc07762ee89e")
    XCTAssertEqual(statuses.first?.stage, .sent, "new words must not inherit the old receipt")
    try fixture.append(fixture.ack(Fixture.firstID))
    try fixture.append(fixture.seal(1))
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .sent)
  }

  func testFiveIdenticalLabelsRemainFiveDistinctOutboundDeliveries() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let expected = [
      Fixture.firstID, "d8281fc3cceedc07762ee89e", "73aaf65a701a5d96129ef750",
      "8796735e3c8ab23e9dbf319c", "769a770d54e99f754fd91423",
    ]
    for sequence in 1...5 {
      try fixture.append(fixture.seal(sequence))
      let statuses = try await reader.read()
      XCTAssertEqual(statuses.first?.deliveryID, expected[sequence - 1])
      XCTAssertEqual(statuses.first?.stage, .sent)
    }
    XCTAssertEqual(Set(expected).count, 5)
  }

  func testTerminalEvidenceUsesPythonPhaseIdentityAcrossDocumentRowsAndRestart() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let id = "82da0959f6f76c5ca5eff8d5"
    for index in 0...4 {
      try fixture.append([
        "schema": "codescribe.transcript-evidence.v1", "session_id": "take-a",
        "audience": "james", "sequence": index + 10, "document_index": index,
        "reducer_revision": 7, "reducer_action": "record_ledger_terminal_seal",
        "rendered_text": "Iwo Iwo Iwo Iwo Iwo",
      ])
    }
    try fixture.lease(pending: [fixture.envelope(id)])
    var statuses = try await OverlayChannelDeliveryReader(root: fixture.root).read()
    XCTAssertEqual(statuses.first?.deliveryID, id)
    XCTAssertEqual(statuses.first?.stage, .queued)
    try fixture.append(fixture.ack(id))
    try fixture.lease(pending: [])
    statuses = try await OverlayChannelDeliveryReader(root: fixture.root).read()
    XCTAssertEqual(statuses.first?.stage, .received)
  }

  func testOpenStateSurvivesSealAndOnlyMatchingLifecycleClosesIt() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    try fixture.append([
      "schema": "codescribe.transcript.v1", "session_id": "another-take", "status": "session_ended",
    ])
    var statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.isOpen, true)
    var oldClose = fixture.open()
    oldClose["state"] = "sealed"
    oldClose["opened_at"] = "earlier"
    try fixture.append(oldClose)
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.isOpen, true)
    try fixture.append([
      "schema": "codescribe.transcript.v1", "session_id": "take-a", "status": "session_ended",
    ])
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.isOpen, false)
    XCTAssertEqual(statuses.first?.stage, .sent)
  }

  func testLeaseRebindingAndBusRotationCannotRetainReceivedOrOpenState() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    try fixture.append(fixture.open())
    try fixture.append(fixture.seal(1))
    try fixture.append(fixture.ack(Fixture.firstID))
    var statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .received)
    try fixture.bind(session: "new-agent-session")
    statuses = try await reader.read()
    XCTAssertTrue(statuses.isEmpty, "the previous owner's lease is not attached")
    try fixture.bind()
    try Data().write(to: fixture.bus, options: .atomic)
    statuses = try await reader.read()
    XCTAssertNil(statuses.first?.stage)
    XCTAssertEqual(statuses.first?.isOpen, false)
  }

  func testPartialBusLineWaitsForNewlineAndMalformedReceiptFailsRead() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    let data = try JSONSerialization.data(withJSONObject: fixture.seal(1))
    try data.write(to: fixture.bus)
    var statuses = try await reader.read()
    XCTAssertNil(statuses.first?.stage)
    try fixture.appendBytes(Data([10]))
    statuses = try await reader.read()
    XCTAssertEqual(statuses.first?.stage, .sent)
    try fixture.appendBytes(Data("not-json\n".utf8))
    do {
      _ = try await reader.read()
      XCTFail("corrupt status must not be presented as current evidence")
    } catch {
      // OverlayState surfaces a read error instead of declaring an empty mailbox.
    }
  }

  func testLoudOpenStateShowsPanelAndRefusesHideWithoutChangingTranscript() throws {
    let state = OverlayState.previewFormatted()
    let transcript = state.activeText
    var shown = 0
    var hidden = 0
    let panel = NSPanel(
      contentRect: NSRect(x: 0, y: 0, width: 500, height: 300),
      styleMask: [.borderless], backing: .buffered, defer: false)
    let controller = OverlayController(
      state: state, overlayEnabledProvider: { false }, assistiveStatusProvider: { false },
      panelFactory: { _, _ in panel }, orderPanelFront: { _ in shown += 1 },
      orderPanelOut: { _ in hidden += 1 })
    let open = OverlayChannelDelivery(
      channel: "1", agent: "james", deliveryID: nil, stage: nil, isOpen: true)
    state.applyChannelDelivery([open])
    XCTAssertEqual(shown, 1, "open channel overrides the ordinary dictation visibility preference")
    controller.hide()
    controller.hideForAgentHandoff()
    XCTAssertEqual(hidden, 0)
    XCTAssertEqual(state.activeText, transcript)
    state.applyChannelDelivery([])
    controller.hide()
    XCTAssertEqual(hidden, 1)
    // Automatic hides yield to the open channel; the human Close intent does not.
    state.applyChannelDelivery([open])
    state.relayIntent(.close)
    XCTAssertEqual(hidden, 2, "an open channel must never veto the brand-dot close")
    XCTAssertTrue(state.hasOpenChannel, "closing the panel does not rewrite channel evidence")
  }

  func testChannelDetailsDoNotIncreaseCollapsedPanelHeight() {
    let state = OverlayState.previewFormatted()
    let panel = DictationOverlayWindow.make(
      state: state, textScale: TextScaleController(key: "ChannelCompactChromeTests"))
    defer { (panel as? FloatingOverlayPanel)?.invalidatePresence() }
    state.toggleCollapsed()
    let before = panel.frame.height
    state.applyChannelDelivery(
      (1...9).map {
        OverlayChannelDelivery(
          channel: String($0), agent: "Agent \($0)", deliveryID: nil,
          stage: nil, isOpen: true)
      })
    panel.contentView?.layoutSubtreeIfNeeded()
    XCTAssertEqual(panel.frame.height, before, accuracy: 0.5)
    XCTAssertEqual(panel.frame.height, DictationOverlayWindow.collapsedHeight, accuracy: 0.5)
  }

  func testChannelChromeIsOutsideCollapseGateAndDragStillPrecedesGlass() throws {
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent()
    let view = try String(
      contentsOf: root.appendingPathComponent(
        "Codescribe/Screens/Overlay/DictationOverlayView.swift"), encoding: .utf8)
    let start = try XCTUnwrap(view.range(of: "private var header: some View"))
    let end = try XCTUnwrap(
      view.range(of: "private var fullHeader", range: start.upperBound..<view.endIndex))
    let header = String(view[start.lowerBound..<end.lowerBound])
    XCTAssertFalse(header.contains("OverlayChannelStatusView("))
    let controls = try XCTUnwrap(view.range(of: "private func justifiedHeader(compact: Bool)"))
    XCTAssertTrue(view[controls.lowerBound...].contains("OverlayChannelStatusView("))
    XCTAssertFalse(header.contains("!state.isCollapsed"))
    let drag = try XCTUnwrap(header.range(of: "OverlayWindowDragRegion"))
    let glass = try XCTUnwrap(header.range(of: ".modifier(OverlayHeaderChrome())"))
    XCTAssertLessThan(drag.lowerBound, glass.lowerBound)
    let status = try String(
      contentsOf: root.appendingPathComponent(
        "Codescribe/Screens/Overlay/OverlayChannelStatusView.swift"), encoding: .utf8)
    XCTAssertTrue(status.contains("overlay-channel-open-"))
    XCTAssertTrue(status.contains("Microphone active"))
    XCTAssertTrue(status.contains(".popover(isPresented: $showsDetails"))
    XCTAssertTrue(status.contains("overlay-channel-delivery-"))
    XCTAssertFalse(status.contains("Divider("))
    XCTAssertFalse(status.contains("glassEffect("))
    // The header shows one glyph, never the microphone: mic = recording only.
    XCTAssertFalse(status.contains("antenna.radiowaves"))
    XCTAssertFalse(status.contains("hasOpenChannel ? \"mic.fill\""))
    // A click opens details and nothing else: it cannot light ␆.
    XCTAssertTrue(status.contains("showsDetails.toggle()"))
    XCTAssertEqual(status.components(separatedBy: "showsDetails.toggle()").count - 1, 1)
  }

  // MARK: Agent glyph (Annex A3/A4 — the state table is the Codex root's proposal)

  func testAgentGlyphIsOneCharacterWithOneLabelPerState() {
    let table: [(OverlayAgentGlyph, String, String)] = [
      (.attached, "\u{2756}", "Agent attached"),
      (.open, "\u{2756}", "Agent channel open"),
      (.awaitingReceipt, "\u{28F8}", "Waiting for the agent to confirm receipt"),
      (.acknowledged, "\u{2406}", "Agent confirmed receipt"),
      (.unavailable, "\u{26A0}\u{FE0E}", "Agent channel status unavailable"),
    ]
    XCTAssertEqual(table.map(\.0), OverlayAgentGlyph.allCases, "every state is named once")
    for (glyph, character, label) in table {
      XCTAssertEqual(glyph.character, character)
      XCTAssertEqual(glyph.character.count, 1, "\(glyph) spends exactly one character")
      XCTAssertEqual(glyph.label, label)
    }
    XCTAssertEqual(Set(table.map(\.2)).count, table.count, "labels tell every state apart")
    XCTAssertEqual(
      OverlayAgentGlyph.unavailable.character.unicodeScalars.last, "\u{FE0E}",
      "the warning sign is the monochrome text form, never the colour emoji")
    XCTAssertEqual(OverlayAgentGlyph.allCases.filter(\.pulses), [.awaitingReceipt])
  }

  func testAgentGlyphResolvesWorstNewsFirstFromProjectionOnly() {
    func channel(_ id: String, _ stage: OverlayChannelDelivery.Stage?, open: Bool = false)
      -> OverlayChannelDelivery
    {
      OverlayChannelDelivery(
        channel: id, agent: "Agent \(id)", deliveryID: stage == nil ? nil : "d\(id)",
        stage: stage, isOpen: open)
    }
    XCTAssertNil(OverlayAgentGlyph.resolve(channels: [], unavailable: false), "no agent, no slot")
    XCTAssertEqual(OverlayAgentGlyph.resolve(channels: [], unavailable: true), .unavailable)
    XCTAssertEqual(OverlayAgentGlyph.resolve(channels: [channel("1", nil)], unavailable: false), .attached)
    XCTAssertEqual(
      OverlayAgentGlyph.resolve(channels: [channel("1", nil, open: true)], unavailable: false),
      .open)
    for stage in [OverlayChannelDelivery.Stage.sent, .queued] {
      XCTAssertEqual(
        OverlayAgentGlyph.resolve(channels: [channel("1", stage, open: true)], unavailable: false),
        .awaitingReceipt)
    }
    XCTAssertEqual(
      OverlayAgentGlyph.resolve(channels: [channel("1", .received, open: true)], unavailable: false),
      .acknowledged)
    XCTAssertEqual(
      OverlayAgentGlyph.resolve(
        channels: [channel("1", .received), channel("2", .sent)], unavailable: false),
      .awaitingReceipt, "one unconfirmed delivery keeps the slot waiting")
    XCTAssertEqual(
      OverlayAgentGlyph.resolve(channels: [channel("1", .received)], unavailable: true),
      .unavailable, "an unreadable status never shows a stale receipt")
  }

  /// ␆ comes from the agent's own `codescribe.agent-ack.v1` row for this
  /// delivery, channel and agent — not from the durable marker, another
  /// channel, re-reading over time, or an earlier take's receipt.
  func testAcknowledgedGlyphLightsOnlyFromTheMatchingAgentAckRow() async throws {
    let fixture = try Fixture()
    defer { fixture.remove() }
    let reader = OverlayChannelDeliveryReader(root: fixture.root)
    func glyph() async throws -> OverlayAgentGlyph? {
      OverlayAgentGlyph.resolve(channels: try await reader.read(), unavailable: false)
    }
    var current = try await glyph()
    XCTAssertEqual(current, .attached)
    try fixture.append(fixture.open())
    current = try await glyph()
    XCTAssertEqual(current, .open)
    try fixture.append(fixture.seal(1))
    current = try await glyph()
    XCTAssertEqual(current, .awaitingReceipt)
    try fixture.lease(pending: [fixture.envelope(Fixture.firstID)])
    try fixture.marker(Fixture.firstID)
    for _ in 0..<3 {
      current = try await glyph()
      XCTAssertEqual(current, .awaitingReceipt, "marker and elapsed reads are not the agent's row")
    }
    try fixture.append(fixture.ack(Fixture.firstID, channel: "2"))
    current = try await glyph()
    XCTAssertEqual(current, .awaitingReceipt, "another channel cannot acknowledge this one")
    try fixture.append(fixture.ack(Fixture.firstID))
    current = try await glyph()
    XCTAssertEqual(current, .acknowledged)
    try fixture.append(fixture.seal(2))
    current = try await glyph()
    XCTAssertEqual(current, .awaitingReceipt, "new words must not inherit the old receipt")
  }

  func testAgentGlyphKeepsOneFixedSlotSoNeighboursNeverMove() {
    let inputs: [(OverlayAgentGlyph, [OverlayChannelDelivery], Bool)] = [
      (.attached, [.init(channel: "1", agent: "a", deliveryID: nil, stage: nil, isOpen: false)], false),
      (.open, [.init(channel: "1", agent: "a", deliveryID: nil, stage: nil, isOpen: true)], false),
      (.awaitingReceipt, [.init(channel: "1", agent: "a", deliveryID: "d", stage: .queued, isOpen: false)], false),
      (.acknowledged, [.init(channel: "1", agent: "a", deliveryID: "d", stage: .received, isOpen: false)], false),
      (.unavailable, [.init(channel: "1", agent: "a", deliveryID: nil, stage: nil, isOpen: false)], true),
    ]
    var sizes: [CGSize] = []
    for (expected, channels, unavailable) in inputs {
      for palette in [OverlayAppearancePalette.light, .dark] {
        let view = OverlayChannelStatusView(
          channels: channels, unavailable: unavailable, palette: palette, animates: false)
        XCTAssertEqual(view.glyph, expected)
        let host = NSHostingView(rootView: view)
        host.layoutSubtreeIfNeeded()
        sizes.append(host.fittingSize)
      }
    }
    for size in sizes {
      XCTAssertEqual(size.width, OverlayAgentGlyph.slotSize.width, accuracy: 0.5)
      XCTAssertEqual(size.height, OverlayAgentGlyph.slotSize.height, accuracy: 0.5)
    }
  }

  private struct Fixture {
    static let leaseID = "0123456789abcdef0123456789abcdef"
    static let firstID = "b67e9660afb01eb53ab2de50"
    let root: URL
    var bus: URL { root.appendingPathComponent("fixture.jsonl") }
    init() throws {
      root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
      try FileManager.default.createDirectory(
        at: root.appendingPathComponent("leases"), withIntermediateDirectories: true)
      try Data().write(to: bus)
      try bind()
      try lease(pending: [])
    }
    func remove() { try? FileManager.default.removeItem(at: root) }
    func write(_ value: [String: Any], to url: URL) throws {
      try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]).write(
        to: url, options: .atomic)
    }
    func bind(session: String = "agent-session") throws {
      try write(
        [
          "schema": "vc.agent-audience-binding.v1",
          "bindings": [
            "1": [
              "audience": "james", "provider": "codex", "provider_session_id": session,
            ]
          ],
        ], to: root.appendingPathComponent("vc.agent-audience-binding.v1.json"))
    }
    func lease(pending: [[String: Any]]) throws {
      try write(
        [
          "schema": "codescribe.agent-bridge.lease.v1", "lease_id": Self.leaseID,
          "provider": "codex", "provider_session_id": "agent-session", "bus": bus.path,
          "pending": pending,
        ], to: root.appendingPathComponent("leases/\(Self.leaseID).json"))
    }
    func marker(_ id: String) throws {
      let directory = root.appendingPathComponent("acknowledgments/\(Self.leaseID)")
      try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      try write(
        ["lease_id": Self.leaseID, "delivery_id": id],
        to: directory.appendingPathComponent("\(id).json"))
    }
    func append(_ row: [String: Any]) throws {
      var bytes = try JSONSerialization.data(withJSONObject: row)
      bytes.append(10)
      try appendBytes(bytes)
    }
    func appendBytes(_ bytes: Data) throws {
      let file = try FileHandle(forWritingTo: bus)
      defer { try? file.close() }
      try file.seekToEnd()
      try file.write(contentsOf: bytes)
    }
    func envelope(_ id: String) -> [String: Any] { ["kind": "seal", "delivery_id": id] }
    func seal(_ sequence: Int) -> [String: Any] {
      [
        "schema": "codescribe.transcript.v1", "session_id": "take-a", "sequence": sequence,
        "utterance_id": "u\(sequence)", "audience": "james", "status": "transcript_sealed",
        "text": "Iwo",
      ]
    }
    func ack(_ id: String, channel: String = "1") -> [String: Any] {
      [
        "schema": "codescribe.agent-ack.v1", "kind": "agent_ack", "channel": channel,
        "agent": "james", "delivery_id": id,
      ]
    }
    func open() -> [String: Any] {
      [
        "schema": "codescribe.channel-session.v1", "kind": "channel_session", "channel": "1",
        "agent": "james", "state": "open", "loud": true, "session_id": "take-a",
        "provider": "codex", "provider_session_id": "agent-session",
        "opened_at": "2026-09-29T08:00:00Z",
      ]
    }
  }
}
