import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Review renders of the redesigned bar (Monika's bar brief, 2026-11-04,
/// §10 and §11E), drawn before an app is built: mini at rest and during a
/// take, the bar with one to nine agents, its narrowest and widest widths,
/// a take in the bar, the agents list under ✦, an agent chip's hover card
/// and the mini ⇄ bar round trip.
/// Every scene renders in dark and in light.
///
/// The assertions only make sure each render shows the intended state and
/// the size production gives it (`miniSize`, `midiBarGeometry`); the image is
/// judged by a person. Each PNG is attached (`bar-<scene>-<dark|light>`,
/// kept always) and, when `CODESCRIBE_RENDER_DIR` is set, written there as
/// `bar-<scene>-<dark|light>.png` (the directory is created, files are
/// overwritten). Xcode forwards only `TEST_RUNNER_` variables into the test
/// host, so under `make test-swift` export
/// `TEST_RUNNER_CODESCRIBE_RENDER_DIR=<dir>`.
///
/// `cacheDisplay` draws the SwiftUI layers of a window that is not on screen
/// (`BackgroundTestWindow.present` keeps it transparent and behind), so no
/// scene needs an isolated GUI session. It does not draw behind-window glass
/// materials; the glass is judged live on screen, not from these images.
@MainActor
final class OverlayBarReviewRenderTests: XCTestCase {

  // MARK: - Rosters

  private let one = [("3", "Lena")]
  private let four = [("1", "As"), ("3", "Lena"), ("4", "Lena2"), ("6", "Nina")]
  private var five: [(String, String)] { four + [("8", "Igor")] }
  private var six: [(String, String)] { five + [("2", "Konstantyna")] }
  private let nine = [
    ("1", "As"), ("2", "Konstantyna"), ("3", "Lena"), ("4", "Lena2"), ("5", "Bo"),
    ("6", "Nina"), ("7", "Maksymilian"), ("8", "Igor"), ("9", "Hania"),
  ]

  private enum Look: String, CaseIterable {
    case dark, light

    var scheme: ColorScheme { self == .dark ? .dark : .light }
    var palette: OverlayAppearancePalette { self == .dark ? .dark : .light }
    var appearance: NSAppearance? { NSAppearance(named: self == .dark ? .darkAqua : .aqua) }
  }

  // MARK: - Fixtures

  private func ownerConversation(
    _ name: String, channel: String, lease: String
  ) throws -> OverlayConversation {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "review-\(channel)",
        "lease_id": String(repeating: lease, count: 32), "channel": channel, "name": name,
      ]))
    return OverlayConversation(
      id: owner.id, channel: channel, name: name, owner: owner, messages: [])
  }

  /// Agents bound on their digits, each with a current conversation, channel
  /// 0's broadcast conversation, the map on in the horizontal layout and the
  /// chord read as Ctrl. The returned conversations include channel 0.
  private func barState(
    _ agents: [(String, String)], mode: OverlayPresentationMode
  ) throws -> (OverlayState, [OverlayConversation]) {
    let broadcast = OverlayConversation(
      id: "review-all", channel: "0", name: "All", owner: nil, messages: [])
    let owned = try agents.enumerated().map { index, agent in
      try ownerConversation(
        agent.1.lowercased(), channel: agent.0, lease: String(index, radix: 16))
    }
    let state = OverlayState(micAccessProvider: { true })
    optIntoChannelMap(state)
    state.applyConversationSnapshot(
      .init(
        deliveries: owned.map {
          OverlayChannelDelivery(
            channel: $0.channel, agent: $0.name, deliveryID: "review-\($0.channel)",
            stage: .received, isOpen: false)
        }, conversations: [broadcast] + owned))
    state.agentChannelModifierSetting = { "ctrl" }
    let conversations = [broadcast] + owned
    apply(conversations, open: [], to: state)
    state.setPresentationMode(mode)
    XCTAssertTrue(state.channelMapInHeader, "the horizontal map is on")
    XCTAssertEqual(state.channelMapLayoutModel.count, agents.count)
    XCTAssertEqual(
      state.channelMapLayoutModel.chips.map(\.digit), ["0"] + agents.map(\.0),
      "(0) leads the agents' chips")
    XCTAssertEqual(state.presentationMode, mode)
    return (state, conversations)
  }

  /// The controller's roster: `open` names the channels the microphone feeds.
  private func apply(
    _ conversations: [OverlayConversation], open: Set<String>, to state: OverlayState
  ) {
    state.applyChannelRoster(
      conversations.map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: open.contains($0.channel),
          loud: false, autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
      })
  }

  /// An agent take: the chord opens `open`, and the form stays as it was.
  private func openChannels(
    _ open: Set<String>, _ conversations: [OverlayConversation], in state: OverlayState
  ) {
    let mode = state.presentationMode
    apply(conversations, open: open, to: state)
    XCTAssertEqual(state.presentationMode, mode, "the chord keeps the form")
    XCTAssertEqual(Set(state.channelMapLayoutModel.openDigits), open)
    XCTAssertTrue(state.miniTakeLive, "an open channel is a live take")
  }

  // MARK: - Geometry from production

  /// The bar's geometry as the window owner measures it
  /// (`DictationOverlayWindow.make`'s channel-map fit).
  private func barGeometry(_ state: OverlayState) -> OverlayMidiBarGeometry {
    DictationOverlayWindow.midiBarGeometry(
      map: state.channelMapLayoutModel, textScale: 1,
      visibleWidth: NSScreen.main?.visibleFrame.width)
  }

  /// A borderless overlay panel told what the window owner tells it: whether
  /// the map is on, what the live take adds to mini, and the bar's geometry.
  private func panel(for state: OverlayState) -> FloatingOverlayPanel {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 200, y: 200, width: 410, height: 46),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    let map = state.channelMapLayoutModel
    panel.setChannelMapPresentation(enabled: state.channelMapInMini)
    panel.setMiniTake(
      width: DictationOverlayWindow.miniTakeWidth(
        live: state.miniTakeLive, openDigits: map.openDigits))
    panel.setMidiBarGeometry(state.channelMapInHeader ? barGeometry(state) : .classic)
    return panel
  }

  // MARK: - Rendering

  private func image(of view: NSView) throws -> NSImage {
    let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
    view.cacheDisplay(in: view.bounds, to: bitmap)
    let image = NSImage(size: view.bounds.size)
    image.addRepresentation(bitmap)
    return image
  }

  /// Attaches the image and, with `CODESCRIBE_RENDER_DIR` set, writes it there
  /// as `<name>.png`, replacing an earlier render.
  private func save(_ image: NSImage, name: String) throws {
    let attachment = XCTAttachment(image: image)
    attachment.name = name
    attachment.lifetime = .keepAlways
    add(attachment)
    guard
      let path = ProcessInfo.processInfo.environment["CODESCRIBE_RENDER_DIR"]?
        .trimmingCharacters(in: .whitespacesAndNewlines), !path.isEmpty
    else { return }
    let directory = URL(fileURLWithPath: path, isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let bitmap = try XCTUnwrap(
      image.representations.compactMap { $0 as? NSBitmapImageRep }.first, "\(name): no bitmap")
    let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]), name)
    try png.write(to: directory.appendingPathComponent("\(name).png"), options: .atomic)
  }

  /// The real overlay view in the overlay panel at `size`, in one look.
  private func renderOverlay(
    _ state: OverlayState, panel: FloatingOverlayPanel, size: NSSize, look: Look, name: String
  ) throws {
    let host = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .environment(\.csTextScale, 1)
        .environment(\.colorScheme, look.scheme)
        .defaultAppStorage(try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString)))
        .transaction { $0.disablesAnimations = true })
    host.sizingOptions = []
    panel.appearance = look.appearance
    panel.contentView = OverlayContentContainer(hosting: host)
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    panel.setFrame(NSRect(origin: NSPoint(x: 200, y: 200), size: size), display: true)
    let root = try XCTUnwrap(panel.contentView)
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.15))
    root.layoutSubtreeIfNeeded()
    XCTAssertEqual(root.bounds.size, size, "\(name): the render is the panel's size")
    try save(try image(of: root), name: "\(name)-\(look.rawValue)")
  }

  /// A SwiftUI view alone on the palette's sheet colour.
  private func renderAlone(
    _ content: some View, size: CGSize, look: Look, name: String
  ) throws {
    let host = NSHostingView(
      rootView:
        content
        .frame(width: size.width, height: size.height, alignment: .topLeading)
        .background(look.palette.desktopBackground.color)
        .environment(\.colorScheme, look.scheme)
        .transaction { $0.disablesAnimations = true })
    host.frame = NSRect(origin: .zero, size: size)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.appearance = look.appearance
    window.contentView = host
    BackgroundTestWindow.present(window)
    defer { window.close() }
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.12))
    host.layoutSubtreeIfNeeded()
    try save(try image(of: host), name: "\(name)-\(look.rawValue)")
  }

  /// Mini at the panel's own `miniSize`, in both looks; returns that size.
  @discardableResult
  private func renderMini(_ state: OverlayState, scene: String) throws -> NSSize {
    XCTAssertEqual(state.presentationMode, .mini)
    let panel = panel(for: state)
    let size = panel.miniSize
    XCTAssertEqual(size.height, DictationOverlayWindow.miniHeight)
    XCTAssertEqual(
      size.width,
      DictationOverlayWindow.miniFloorWidth
        + DictationOverlayWindow.miniTakeWidth(
          live: state.miniTakeLive, openDigits: state.channelMapLayoutModel.openDigits),
      "mini is its controls plus what the take adds")
    for look in Look.allCases {
      try renderOverlay(state, panel: panel, size: size, look: look, name: "bar-\(scene)")
    }
    return size
  }

  /// The bar at `width` (default: the panel's own `midiSize`), in both looks;
  /// returns the size it was drawn at.
  @discardableResult
  private func renderBar(
    _ state: OverlayState, scene: String, width: CGFloat? = nil
  ) throws -> NSSize {
    XCTAssertEqual(state.presentationMode, .midi)
    let panel = panel(for: state)
    let geometry = barGeometry(state)
    let size =
      width.map { NSSize(width: $0, height: geometry.height(forWidth: $0)) } ?? panel.midiSize
    if width == nil {
      XCTAssertEqual(size.width, geometry.naturalWidth, "the bar opens at its natural width")
    }
    XCTAssertGreaterThanOrEqual(size.width, geometry.minimumWidth - 0.5)
    XCTAssertLessThanOrEqual(size.width, geometry.maximumWidth + 0.5)
    for look in Look.allCases {
      try renderOverlay(state, panel: panel, size: size, look: look, name: "bar-\(scene)")
    }
    return size
  }

  // MARK: - 1. Mini at rest

  /// 127 × 36: close dot · pencil · microphone · ✦ · chevron. The order is
  /// read from the image.
  func testRenderMiniRest() throws {
    let (state, _) = try barState(four, mode: .mini)
    XCTAssertFalse(state.miniTakeLive)
    XCTAssertTrue(state.channelMapLayoutModel.openDigits.isEmpty)
    let size = try renderMini(state, scene: "mini-rest")
    XCTAssertEqual(size, NSSize(width: 127, height: 36), "brief §3: mini at rest is 127 × 36")
  }

  // MARK: - 2. Mini during a dictation

  /// Waveform and clock, no addressees.
  func testRenderMiniTakeDictation() throws {
    let (state, _) = try barState(four, mode: .mini)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    // A take may unfold the overlay by preference; the scene is mini.
    state.setPresentationMode(.mini)
    XCTAssertTrue(state.recording)
    XCTAssertTrue(state.miniTakeLive)
    XCTAssertTrue(state.channelMapLayoutModel.openDigits.isEmpty, "a dictation has no addressee")
    let size = try renderMini(state, scene: "mini-take-dictation")
    XCTAssertEqual(
      size.width,
      DictationOverlayWindow.miniFloorWidth
        + DictationOverlayWindow.miniTakeWidth(live: true, openDigits: []))
  }

  // MARK: - 3. Mini during an agent take

  func testRenderMiniTakeToOneAgent() throws {
    let (state, conversations) = try barState(four, mode: .mini)
    openChannels(["4"], conversations, in: state)
    try renderMini(state, scene: "mini-take-1")
  }

  func testRenderMiniTakeToThreeAgents() throws {
    let (state, conversations) = try barState(nine, mode: .mini)
    openChannels(["2", "4", "7"], conversations, in: state)
    try renderMini(state, scene: "mini-take-3")
  }

  /// Channel 0: no addressee row; the Agents button wears the megaphone.
  func testRenderMiniTakeToAll() throws {
    let (state, conversations) = try barState(four, mode: .mini)
    openChannels(["0"], conversations, in: state)
    XCTAssertEqual(state.channelMapLayoutModel.count, 4, "channel 0 is not an agent")
    let size = try renderMini(state, scene: "mini-take-all")
    XCTAssertEqual(
      size.width,
      DictationOverlayWindow.miniFloorWidth
        + DictationOverlayWindow.miniTakeWidth(live: true, openDigits: ["0"]))
  }

  // MARK: - 4. The bar with one to nine agents

  /// One row, each name under its circle.
  func testRenderBarWithOneAgent() throws {
    let (state, _) = try barState(one, mode: .midi)
    let size = try renderBar(state, scene: "bar-1")
    XCTAssertEqual(size.height, barGeometry(state).singleRowHeight, "one row")
  }

  func testRenderBarWithFourAgents() throws {
    let (state, _) = try barState(four, mode: .midi)
    let size = try renderBar(state, scene: "bar-4")
    XCTAssertEqual(size.height, barGeometry(state).singleRowHeight, "one row")
  }

  func testRenderBarWithFiveAgents() throws {
    let (state, _) = try barState(five, mode: .midi)
    let size = try renderBar(state, scene: "bar-5")
    XCTAssertEqual(size.height, barGeometry(state).singleRowHeight, "one row")
  }

  /// Six agents and more: always two rows.
  func testRenderBarWithSixAgents() throws {
    let (state, _) = try barState(six, mode: .midi)
    let geometry = barGeometry(state)
    XCTAssertTrue(geometry.singleRowMinimumWidth.isInfinite)
    let size = try renderBar(state, scene: "bar-6")
    XCTAssertEqual(size.height, geometry.twoRowHeight, "two rows")
  }

  /// Long names and the Lena / Lena2 pair.
  func testRenderBarWithNineAgents() throws {
    let (state, _) = try barState(nine, mode: .midi)
    let geometry = barGeometry(state)
    XCTAssertTrue(geometry.singleRowMinimumWidth.isInfinite)
    let size = try renderBar(state, scene: "bar-9")
    XCTAssertEqual(size.height, geometry.twoRowHeight, "two rows")
  }

  // MARK: - 5. Narrowest and widest

  func testRenderBarWithNineAgentsAtItsMinimumWidth() throws {
    let (state, _) = try barState(nine, mode: .midi)
    let geometry = barGeometry(state)
    let size = try renderBar(state, scene: "bar-9-narrow", width: geometry.minimumWidth)
    XCTAssertEqual(size.width, geometry.minimumWidth)
    XCTAssertEqual(size.height, geometry.twoRowHeight)
  }

  func testRenderBarWithFiveAgentsAtItsMaximumWidth() throws {
    let (state, _) = try barState(five, mode: .midi)
    let geometry = barGeometry(state)
    let size = try renderBar(state, scene: "bar-5-wide", width: geometry.maximumWidth)
    XCTAssertEqual(size.width, geometry.maximumWidth)
    XCTAssertEqual(size.height, geometry.singleRowHeight, "every name whole, one row")
  }

  // MARK: - 6. A take in the bar

  func testRenderBarWithFourAgentsTakingOne() throws {
    let (state, conversations) = try barState(four, mode: .midi)
    openChannels(["4"], conversations, in: state)
    XCTAssertEqual(
      state.channelMapLayoutModel.chips.first { $0.digit == "4" }?.microphoneOpen, true)
    try renderBar(state, scene: "bar-4-take-1")
  }

  func testRenderBarWithNineAgentsTakingThree() throws {
    let (state, conversations) = try barState(nine, mode: .midi)
    openChannels(["2", "4", "7"], conversations, in: state)
    let open = state.channelMapLayoutModel.chips.filter(\.microphoneOpen).map(\.digit)
    XCTAssertEqual(Set(open), ["2", "4", "7"])
    try renderBar(state, scene: "bar-9-take-3")
  }

  // MARK: - 7. The agents list under ✦

  /// Nine rows from the roster's own projection: channel 1 open (listening
  /// dot), an unread reply on 4, agent 6 off channel 0 (`member = false`),
  /// agent 7 under the moon, agent 9 without a helper receipt (unknown marks).
  func testRenderAgentsHoverListWithNineAgents() throws {
    func identity(_ digit: String) -> AgentPlaybackIdentity {
      AgentPlaybackIdentity(provider: "codex", session: "review-\(digit)", bus: "/dev/null")
    }
    let policies: [String: AgentBroadcastPolicy] = ["6": .off, "7": .quiet]
    let conversations = try nine.enumerated().map { index, agent in
      try ownerConversation(agent.1, channel: agent.0, lease: String(index, radix: 16))
    }
    let unreadConversation = try XCTUnwrap(conversations.first { $0.channel == "4" })
    var hudStates: [String: OverlayChannelHudProjection] = [:]
    var channels: [OverlayChannelDelivery] = []
    for (digit, name) in nine {
      hudStates[digit] = OverlayChannelHudProjection(
        open: digit == "1", loud: false, autosealDeadline: nil, followerAlive: true,
        provider: "codex", providerSessionID: "review-\(digit)")
      var delivery = OverlayChannelDelivery(
        channel: digit, agent: name, deliveryID: nil, stage: nil, isOpen: false)
      delivery.broadcastIdentity = identity(digit)
      delivery.broadcastPolicy = policies[digit] ?? .on
      channels.append(delivery)
    }
    let ledger = OverlayBroadcastPolicyLedger.shared
    let previousReceipts = ledger.receipted
    defer { ledger.noteReceipts(previousReceipts) }
    ledger.noteReceipts(Set(nine.map(\.0).filter { $0 != "9" }.map(identity)))

    for look in Look.allCases {
      let roster = OverlayChannelStatusView(
        channels: channels, unavailable: false, palette: look.palette, animates: false,
        hudStates: hudStates, conversations: conversations,
        unreadCounts: [unreadConversation.id: 2])
      let rows = roster.agentsHoverRows
      XCTAssertEqual(rows.map(\.digit), nine.map(\.0))
      XCTAssertEqual(rows.map(\.name), nine.map(\.1))
      XCTAssertEqual(rows.first?.dot, .listening, "the open channel's row reads as listening")
      XCTAssertEqual(rows.first { $0.digit == "4" }?.unread, 2)
      XCTAssertEqual(rows.first { $0.digit == "6" }?.policy.member, false)
      XCTAssertEqual(rows.first { $0.digit == "7" }?.policy.moon, true)
      XCTAssertFalse(ledger.policyKnown(identity("9")), "no receipt: unknown, not on")
      XCTAssertTrue(ledger.policyKnown(identity("1")))
      let model = OverlayAgentsHoverListModel()
      model.rows = rows
      try renderAlone(
        OverlayAgentsHoverList(model: model, palette: look.palette),
        size: OverlayAgentsHoverList.size(rows: rows), look: look, name: "bar-hover-list-9")
    }
  }

  // MARK: - 9. An agent chip's hover card

  /// The card under a chip, alone: online with an unread reply and the
  /// chord; not active without a confirmed channel 0 policy; no data, off
  /// channel 0 under the moon, with a stalled-follower flag.
  func testRenderChipHoverCards() throws {
    let cards: [(String, String, String, OverlayChannelChipStatus)] = [
      (
        "bar-9-hover-card", "5", "Kuba",
        OverlayChannelChipStatus(
          presence: .online, reception: .member, unread: 2, provider: "codex")
      ),
      (
        "bar-9-hover-card-inactive", "3", "Lena",
        OverlayChannelChipStatus(presence: .inactive, reception: .unknown, provider: "claude")
      ),
      (
        "bar-9-hover-card-unknown", "7", "Maksymilian",
        OverlayChannelChipStatus(
          presence: .unknown, stalled: true, reception: .off, quiet: true, provider: "codex")
      ),
    ]
    for (name, digit, agent, status) in cards {
      let chip = OverlayChannelMapChip(
        digit: digit, name: agent, disconnected: status.presence == .inactive,
        microphoneOpen: false, unread: status.unread > 0, selectable: true, selected: false,
        status: status)
      let size = OverlayChannelChipHoverCard.size(chip: chip, status: status, modifier: .control)
      let lines = OverlayChannelChipHoverCard.lines(chip: chip, status: status, modifier: .control)
      XCTAssertEqual(lines.last?.text, "Ctrl+" + digit, "\(name): the chord is the last line")
      for look in Look.allCases {
        try renderAlone(
          OverlayChannelChipHoverCard(
            chip: chip, status: status, modifier: .control, palette: look.palette),
          size: size, look: look, name: name)
      }
    }
  }

  // MARK: - 8. Mini to the bar and back

  /// A still click on the free background: mini opens the bar, the bar folds
  /// back to mini. Both end frames are rendered.
  func testRenderMiniToBarAndBack() throws {
    let (state, _) = try barState(four, mode: .mini)
    state.advancePresentationFromHeader()
    XCTAssertEqual(state.presentationMode, .midi, "mini opens the bar")
    try renderBar(state, scene: "mini-to-bar-and-back-bar")
    state.advancePresentationFromHeader()
    XCTAssertEqual(state.presentationMode, .mini, "the bar folds back to mini")
    try renderMini(state, scene: "mini-to-bar-and-back-mini")
  }
}
