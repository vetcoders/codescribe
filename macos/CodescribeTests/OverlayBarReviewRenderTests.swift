import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Review renders of the redesigned bar (Founder's bar brief, 2026-11-04,
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
  private let three = [("1", "As"), ("3", "Lena"), ("6", "Nina")]
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
      state.channelMapLayoutModel.chips.map(\.digit), ["0"] + agents.map(\.0).sorted(),
      "(0) leads the agents' chips, in digit order")
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

  /// Three agents (Founder, 2026-11-04: the bar at three beside six and nine).
  func testRenderBarWithThreeAgents() throws {
    let (state, _) = try barState(three, mode: .midi)
    let size = try renderBar(state, scene: "bar-3")
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

  /// A dictation in the bar: no addressee, the take's mode colour on the
  /// brand dot and the Stop.
  func testRenderBarWithFourAgentsDictating() throws {
    let (state, _) = try barState(four, mode: .midi)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.setPresentationMode(.midi)
    XCTAssertTrue(state.recording)
    XCTAssertTrue(state.channelMapLayoutModel.openDigits.isEmpty, "a dictation has no addressee")
    try renderBar(state, scene: "bar-4-take-dictation")
  }

  func testRenderBarWithNineAgentsTakingThree() throws {
    let (state, conversations) = try barState(nine, mode: .midi)
    openChannels(["2", "4", "7"], conversations, in: state)
    let open = state.channelMapLayoutModel.chips.filter(\.microphoneOpen).map(\.digit)
    XCTAssertEqual(Set(open), ["2", "4", "7"])
    try renderBar(state, scene: "bar-9-take-3")
  }

  // MARK: - 7. The agents list under ✦

  /// Eleven rows from the roster's own projection (channel 0 and your inbox
  /// above the nine agents): channel 1 open (listening dot), an unread reply
  /// on 0 and 4, agent 6 off channel 0 (`member = false`), agent 7 under the
  /// moon (zzz), agent 8 stalled (olive with "!"), agent 9 without a liveness
  /// claim ("?") and without a helper receipt.
  func testRenderAgentsHoverListWithNineAgents() throws {
    func identity(_ digit: String) -> AgentPlaybackIdentity {
      AgentPlaybackIdentity(provider: "codex", session: "review-\(digit)", bus: "/dev/null")
    }
    let policies: [String: AgentBroadcastPolicy] = ["6": .off, "7": .quiet]
    let all = OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let inbox = OverlayConversation.humanInbox(messages: [])
    let agentConversations = try nine.enumerated().map { index, agent in
      try ownerConversation(agent.1, channel: agent.0, lease: String(index, radix: 16))
    }
    let conversations = [all, inbox] + agentConversations
    let unreadConversation = try XCTUnwrap(agentConversations.first { $0.channel == "4" })
    var hudStates: [String: OverlayChannelHudProjection] = [:]
    var channels: [OverlayChannelDelivery] = []
    let stale = Date().addingTimeInterval(-OverlayState.followerStallHorizon - 60)
    for (digit, name) in nine {
      hudStates[digit] = OverlayChannelHudProjection(
        open: digit == "1", loud: false, autosealDeadline: nil,
        followerAlive: digit == "9" ? nil : true,
        provider: "codex", providerSessionID: "review-\(digit)",
        unreadSince: digit == "8" ? stale : nil)
      var delivery = OverlayChannelDelivery(
        channel: digit, agent: name, deliveryID: nil, stage: nil, isOpen: false)
      delivery.broadcastIdentity = identity(digit)
      delivery.broadcastPolicy = policies[digit] ?? .on
      channels.append(delivery)
    }
    let ledger = OverlayBroadcastPolicyLedger.shared
    let previousReceipts = ledger.receipted
    defer { ledger.noteReceipts(previousReceipts) }
    let testReceipts = Set(nine.map(\.0).filter { $0 != "9" }.map(identity))

    for look in Look.allCases {
      ledger.noteReceipts(testReceipts)
      let roster = OverlayChannelStatusView(
        channels: channels, unavailable: false, palette: look.palette, animates: false,
        hudStates: hudStates, conversations: conversations,
        unreadCounts: [all.id: 1, unreadConversation.id: 2])
      let rows = roster.hoverListRows
      XCTAssertEqual(rows.map(\.digit), ["0", "10"] + nine.map(\.0))
      XCTAssertEqual(rows.prefix(2).map(\.kind), [.broadcast, .inbox])
      XCTAssertEqual(rows.dropFirst(2).map(\.name), nine.map(\.1))
      XCTAssertEqual(rows.first { $0.digit == "0" }?.unread, 1)
      XCTAssertEqual(
        rows.first { $0.digit == "1" }?.dot, .listening,
        "the open channel's row reads as listening")
      XCTAssertEqual(rows.first { $0.digit == "4" }?.unread, 2)
      XCTAssertEqual(rows.first { $0.digit == "6" }?.policy.member, false)
      XCTAssertEqual(rows.first { $0.digit == "7" }?.policy.moon, true)
      XCTAssertEqual(rows.first { $0.digit == "7" }?.dot, .asleep, "the moon: zzz")
      XCTAssertEqual(rows.first { $0.digit == "8" }?.dot, .stalled, "past the horizon: !")
      XCTAssertEqual(rows.first { $0.digit == "9" }?.dot, .idle, "no liveness claim: ?")
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
        OverlayChannelChipStatus(presence: .inactive, reception: .off, provider: "claude")
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

  // MARK: - 10. Sidebar receipts, bubble receipts and narrow rail (Tor B & Tor C)

  func testRenderSidebarReceiptsAndRail() throws {
    func makeOwner(_ name: String, channel: String, lease: String) throws
      -> OverlayConversationOwner
    {
      try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": "review-\(channel)",
          "lease_id": String(repeating: lease, count: 32), "channel": channel, "name": name,
        ]))
    }
    let borys = try makeOwner("borys", channel: "2", lease: "2")
    let miksa = try makeOwner("miksa", channel: "6", lease: "6")
    let nela = try makeOwner("nela", channel: "7", lease: "7")
    let kaja = try makeOwner("kaja", channel: "8", lease: "8")
    let olek = try makeOwner("olek", channel: "9", lease: "9")

    let broadcastMsg = OverlayConversationMessage(
      id: "typed:0123456789abcdef00000100", kind: .user,
      text: "Przeczytajcie o kolorach i znacznikach busa.",
      order: 1, emittedAt: "2026-11-04T17:31:00Z", owner: nil,
      recipients: [
        OverlayConversationRecipient(
          owner: borys, deliveryID: "d-borys-0", queued: true, accepted: true, acknowledged: true),
        OverlayConversationRecipient(
          owner: miksa, deliveryID: "d-miksa-0", queued: false, accepted: false,
          acknowledged: false, broadcastSkipped: true),
      ],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "",
      sourceChannel: "0",
      sentTo: [
        OverlayConversationReach(name: "borys", channel: "2"),
        OverlayConversationReach(name: "olek", channel: "9"),
      ],
      receipt: OverlayMessageReceipt(
        messageID: "0123456789abcdef00000100",
        sentTo: [
          OverlayConversationReach(name: "borys", channel: "2"),
          OverlayConversationReach(name: "olek", channel: "9"),
        ],
        deliveries: [
          .init(
            deliveryID: "d-borys-0", name: "borys", channel: "2", state: .read,
            wake: "on", deliveredAt: "2026-11-04T17:31:01Z", at: "2026-11-04T17:31:05Z"),
          .init(
            deliveryID: "d-miksa-0", name: "miksa", channel: "6", state: .skipped,
            wake: nil, deliveredAt: nil, at: "2026-11-04T17:31:01Z"),
        ]))

    let borysMsg = OverlayConversationMessage(
      id: "typed:0123456789abcdef00000101", kind: .user,
      text: "Borys, zintegruj wszystkie trzy tory i zrób screeny.",
      order: 2, emittedAt: "2026-11-04T17:36:00Z", owner: nil,
      recipients: [
        OverlayConversationRecipient(
          owner: borys, deliveryID: "d-borys-1", queued: true, accepted: true, acknowledged: true)
      ],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "",
      sourceChannel: "2")

    let olekMsg = OverlayConversationMessage(
      id: "typed:0123456789abcdef00000102", kind: .user,
      text: "Olek, przygotuj słownik kolorów z kodu.",
      order: 3, emittedAt: "2026-11-04T17:37:00Z", owner: nil,
      recipients: [
        OverlayConversationRecipient(
          owner: olek, deliveryID: "d-olek-1", queued: true, accepted: true, acknowledged: false)
      ],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "",
      sourceChannel: "9",
      receipt: OverlayMessageReceipt(
        messageID: "0123456789abcdef00000102",
        deliveries: [
          .init(
            deliveryID: "d-olek-1", name: "olek", channel: "9", state: .delivered,
            wake: "on", deliveredAt: "2026-11-04T17:37:01Z", at: "2026-11-04T17:37:01Z")
        ]))

    let nelaMsg = OverlayConversationMessage(
      id: "typed:0123456789abcdef00000103", kind: .user,
      text: "Nela, czekam na potwierdzenie.",
      order: 4, emittedAt: "2026-11-04T17:20:00Z", owner: nil,
      recipients: [
        OverlayConversationRecipient(
          owner: nela, deliveryID: "d-nela-1", queued: true, accepted: false, acknowledged: false)
      ],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "",
      sourceChannel: "7")

    let kajaMsg = OverlayConversationMessage(
      id: "typed:0123456789abcdef00000104", kind: .user,
      text: "Kaja, sprawdź mapę popoverów i kropkę przed imieniem.",
      order: 5, emittedAt: "2026-11-04T17:38:00Z", owner: nil,
      recipients: [
        OverlayConversationRecipient(
          owner: kaja, deliveryID: "d-kaja-1", queued: true, accepted: true, acknowledged: false)
      ],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "",
      sourceChannel: "8")

    let allConv = OverlayConversation(
      id: "0", channel: "0", name: "All", owner: nil, messages: [broadcastMsg])
    let inboxConv = OverlayConversation.humanInbox(messages: [])
    let borysConv = OverlayConversation(
      id: borys.id, channel: "2", name: "borys", owner: borys, messages: [borysMsg])
    // miksa only has the skipped broadcast in raw messages; visibleMessages filters it out!
    let miksaConv = OverlayConversation(
      id: miksa.id, channel: "6", name: "miksa", owner: miksa, messages: [broadcastMsg])
    let nelaConv = OverlayConversation(
      id: nela.id, channel: "7", name: "nela", owner: nela, messages: [nelaMsg])
    let kajaConv = OverlayConversation(
      id: kaja.id, channel: "8", name: "kaja", owner: kaja, messages: [kajaMsg])
    let olekConv = OverlayConversation(
      id: olek.id, channel: "9", name: "olek", owner: olek, messages: [olekMsg])

    let stale = Date().addingTimeInterval(-OverlayState.followerStallHorizon - 120)
    let hudStates: [String: OverlayChannelHudProjection] = [
      "2": .init(
        open: false, loud: false, autosealDeadline: nil, followerAlive: true,
        provider: "codex", providerSessionID: "review-2"),
      "6": .init(
        open: false, loud: false, autosealDeadline: nil, followerAlive: true,
        provider: "codex", providerSessionID: "review-6"),
      "7": .init(
        open: false, loud: false, autosealDeadline: nil, followerAlive: true,
        provider: "codex", providerSessionID: "review-7", unreadSince: stale),
      "8": .init(
        open: false, loud: false, autosealDeadline: nil, followerAlive: true,
        provider: "codex", providerSessionID: "review-8"),
      "9": .init(
        open: false, loud: false, autosealDeadline: nil, followerAlive: true,
        provider: "codex", providerSessionID: "review-9"),
    ]
    func identity(_ ch: String) -> AgentPlaybackIdentity {
      AgentPlaybackIdentity(provider: "codex", session: "review-\(ch)", bus: "/dev/null")
    }
    var ch2 = OverlayChannelDelivery(
      channel: "2", agent: "borys", deliveryID: "d-borys-1", stage: .received, isOpen: false)
    ch2.broadcastIdentity = identity("2")
    ch2.broadcastPolicy = .on
    var ch6 = OverlayChannelDelivery(
      channel: "6", agent: "miksa", deliveryID: "d-miksa-0", stage: .sent, isOpen: false)
    ch6.broadcastIdentity = identity("6")
    ch6.broadcastPolicy = .off
    ch6.broadcastSkipped = true
    var ch7 = OverlayChannelDelivery(
      channel: "7", agent: "nela", deliveryID: "d-nela-1", stage: .queued, isOpen: false)
    ch7.broadcastIdentity = identity("7")
    ch7.broadcastPolicy = .off
    var ch8 = OverlayChannelDelivery(
      channel: "8", agent: "kaja", deliveryID: "d-kaja-1", stage: .queued, isOpen: false)
    ch8.broadcastIdentity = identity("8")
    ch8.broadcastPolicy = .quiet
    var ch9 = OverlayChannelDelivery(
      channel: "9", agent: "olek", deliveryID: "d-olek-1", stage: .queued, isOpen: false)
    ch9.broadcastIdentity = identity("9")
    ch9.broadcastPolicy = .on

    let ledger = OverlayBroadcastPolicyLedger.shared
    let previousReceipts = ledger.receipted
    defer { ledger.noteReceipts(previousReceipts) }
    ledger.noteReceipts(Set(["2", "6", "7", "8", "9"].map(identity)))

    let conversations = [allConv, inboxConv, borysConv, miksaConv, nelaConv, kajaConv, olekConv]
    let channels = [ch2, ch6, ch7, ch8, ch9]
    let unreads = [kajaConv.id: 2, olekConv.id: 4]

    for look in Look.allCases {
      let roster = OverlayChannelStatusView(
        channels: channels, unavailable: false, palette: look.palette, animates: false,
        hudStates: hudStates,
        agentDescriptors: [
          "2": "agy · Codescribe", "6": "Codex · Codescribe", "7": "Codex · Vibecrafted",
          "8": "Claude Code · Codescribe", "9": "Codex · Codescribe",
        ],
        conversations: conversations, selectedConversationID: borysConv.id,
        unreadCounts: unreads, onSelectConversation: { _ in })
      XCTAssertEqual(roster.deliveryReceiptDisplay(for: ch2, conversation: borysConv)?.symbol, "double-check")
      XCTAssertEqual(roster.deliveryReceiptDisplay(for: ch2, conversation: borysConv)?.isRead, true)
      XCTAssertEqual(roster.deliveryReceiptDisplay(for: ch8, conversation: kajaConv)?.symbol, "checkmark")
      XCTAssertEqual(roster.deliveryReceiptDisplay(for: ch8, conversation: kajaConv)?.isRead, false)
      XCTAssertEqual(roster.deliveryReceiptDisplay(for: ch9, conversation: olekConv)?.symbol, "double-check")
      XCTAssertEqual(roster.deliveryReceiptDisplay(for: ch9, conversation: olekConv)?.isRead, false)
      XCTAssertNil(roster.deliveryReceiptDisplay(for: ch6, conversation: miksaConv))
      try renderAlone(
        roster.monitorBody(maximumHeight: 540).padding(10),
        size: CGSize(width: 300, height: 540), look: look, name: "bus-sidebar-receipts")
      try renderAlone(
        roster.railBody(maximumHeight: 300).padding(6),
        size: CGSize(width: 64, height: 300), look: look, name: "bus-narrow-rail")

      let bubbleConv = OverlayConversation(
        id: borys.id, channel: "2", name: "borys", owner: borys,
        messages: [borysMsg, broadcastMsg])
      let convView = OverlayConversationView(
        conversation: bubbleConv, palette: look.palette, topInset: 8, bottomInset: 8,
        pendingControls: [], controlErrors: [:], onControl: { _, _ in },
        draft: .constant(""), sending: false, sendError: nil, onSend: {},
        hidesNavigation: true, showsComposer: false)
      try renderAlone(
        convView, size: CGSize(width: 380, height: 220), look: look, name: "bus-bubble-receipts")

      let intercomState = OverlayState()
      intercomState.applyConversationSnapshot(
        .init(deliveries: channels, conversations: conversations))
      let staleMs = Int64(stale.timeIntervalSince1970 * 1_000)
      intercomState.applyChannelRoster([
        .init(
          channel: "2", audience: "borys", provider: "codex", providerSessionId: "review-2",
          open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
          unreadSinceUnixMs: nil),
        .init(
          channel: "6", audience: "miksa", provider: "codex", providerSessionId: "review-6",
          open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
          unreadSinceUnixMs: nil),
        .init(
          channel: "7", audience: "nela", provider: "codex", providerSessionId: "review-7",
          open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
          unreadSinceUnixMs: staleMs),
        .init(
          channel: "8", audience: "kaja", provider: "codex", providerSessionId: "review-8",
          open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
          unreadSinceUnixMs: nil),
        .init(
          channel: "9", audience: "olek", provider: "codex", providerSessionId: "review-9",
          open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
          unreadSinceUnixMs: nil),
      ])
      let suite = "CodescribeTests.intercom-render.\(UUID().uuidString)"
      let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
      defer { defaults.removePersistentDomain(forName: suite) }
      let store = OverlayIntercomDesktopStore(defaults: defaults)
      let ctrlTight = OverlayIntercomWidgetController(
        state: intercomState, textScale: TextScaleController(key: suite + ".t"), desktopStore: store)
      let ctrlWide = OverlayIntercomWidgetController(
        state: intercomState, textScale: TextScaleController(key: suite + ".w"), desktopStore: store)
      let id2 = OverlayIntercomCardIdentity(digit: "2", conversation: borysConv)
      let id7 = OverlayIntercomCardIdentity(digit: "7", conversation: nelaConv)
      let id8 = OverlayIntercomCardIdentity(digit: "8", conversation: kajaConv)
      let intercomPreview = VStack(alignment: .leading, spacing: 10) {
        Text(verbatim: "Intercom fiszki — wąskie (230 pt) i szerokie (320 pt)")
          .font(.system(size: 11, weight: .semibold))
          .foregroundStyle(look.palette.mutedText.color)
        HStack(alignment: .top, spacing: 12) {
          VStack(spacing: 6) {
            ctrlTight.offscreenCard(id2, width: 230, height: 46, scale: 1)
              .frame(width: 230, height: 46)
            ctrlTight.offscreenCard(id7, width: 230, height: 46, scale: 1)
              .frame(width: 230, height: 46)
            ctrlTight.offscreenCard(id8, width: 230, height: 46, scale: 1)
              .frame(width: 230, height: 46)
          }
          VStack(spacing: 6) {
            ctrlWide.offscreenCard(id2, width: 320, height: 46, scale: 1)
              .frame(width: 320, height: 46)
            ctrlWide.offscreenCard(id7, width: 320, height: 46, scale: 1)
              .frame(width: 320, height: 46)
            ctrlWide.offscreenCard(id8, width: 320, height: 46, scale: 1)
              .frame(width: 320, height: 46)
          }
        }
      }
      .padding(12)
      try renderAlone(
        intercomPreview, size: CGSize(width: 590, height: 200), look: look,
        name: "bus-intercom-fiszki")
    }
  }

  // MARK: - 11. The header's chosen window controls

  /// The expanded overlay on its transcription, pinned: the transcription
  /// control and the pin are both chosen, side by side in one header.
  func testRenderExpandedTranscriptionAndPinChosen() throws {
    let (state, _) = try barState(four, mode: .midi)
    state.engine = OverlayChromePolicyEngine()
    state.setKeepVisibleBetweenTakes(true)
    state.showTranscription()
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(state.windowContentSurface, .transcription)
    XCTAssertTrue(state.keepVisibleBetweenTakes)
    let panel = panel(for: state)
    for look in Look.allCases {
      try renderOverlay(
        state, panel: panel, size: NSSize(width: 620, height: 140), look: look,
        name: "bar-expanded-transcription-pinned")
    }
  }
}
