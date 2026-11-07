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
  /// `speaking` names an agent whose reply is playing aloud; `dead` the
  /// digits whose follower the roster reports gone.
  private func barState(
    _ agents: [(String, String)], mode: OverlayPresentationMode, speaking: String? = nil,
    dead: Set<String> = []
  ) throws -> (OverlayState, [OverlayConversation]) {
    let broadcast = OverlayConversation(
      id: "review-all", channel: "0", name: "All", owner: nil, messages: [])
    let owned = try agents.enumerated().map { index, agent in
      let conversation = try ownerConversation(
        agent.1.lowercased(), channel: agent.0, lease: String(index, radix: 16))
      guard agent.0 == speaking else { return conversation }
      return playingReply(in: conversation)
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
    apply(conversations, open: [], dead: dead, to: state)
    state.setPresentationMode(mode)
    XCTAssertTrue(state.channelMapInHeader, "the horizontal map is on")
    XCTAssertEqual(state.channelMapLayoutModel.count, agents.count)
    XCTAssertEqual(
      state.channelMapLayoutModel.chips.map(\.digit), ["0"] + agents.map(\.0).sorted(),
      "(0) leads the agents' chips, in digit order")
    XCTAssertEqual(state.presentationMode, mode)
    return (state, conversations)
  }

  /// The conversation with one reply from its agent, playing aloud.
  private func playingReply(in conversation: OverlayConversation) -> OverlayConversation {
    var reply = OverlayConversationMessage(
      id: "reply:review-\(conversation.channel)", kind: .reply, text: "Gotowe.", order: 1,
      emittedAt: "2026-11-06T03:00:00Z", owner: conversation.owner, recipients: [],
      deliveryID: nil, replyTo: nil, unsolicited: true,
      playback: OverlayReplyPlayback(
        replyID: "review-\(conversation.channel)", ticket: "review-ticket", state: "playing",
        reason: nil, spoken: false, emittedAt: "2026-11-06T03:00:01Z"),
      busPath: "/dev/null")
    reply.sourceChannel = conversation.channel
    return OverlayConversation(
      id: conversation.id, channel: conversation.channel, name: conversation.name,
      owner: conversation.owner, messages: [reply])
  }

  /// The controller's roster: `open` names the channels the microphone feeds,
  /// `dead` the channels whose follower is gone.
  private func apply(
    _ conversations: [OverlayConversation], open: Set<String>, dead: Set<String> = [],
    to state: OverlayState
  ) {
    state.applyChannelRoster(
      conversations.map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: open.contains($0.channel),
          loud: false, autosealDeadlineUnixMs: nil, followerAlive: !dead.contains($0.channel),
          unreadSinceUnixMs: nil)
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

  private func image(of view: NSView, scale: CGFloat = 1) throws -> NSImage {
    let bitmap: NSBitmapImageRep
    if scale == 1 {
      bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
    } else {
      // A Retina render off screen: the bitmap holds `scale` pixels per
      // point and keeps the view's size in points.
      bitmap = try XCTUnwrap(
        NSBitmapImageRep(
          bitmapDataPlanes: nil, pixelsWide: Int(view.bounds.width * scale),
          pixelsHigh: Int(view.bounds.height * scale), bitsPerSample: 8, samplesPerPixel: 4,
          hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0,
          bitsPerPixel: 0))
      bitmap.size = view.bounds.size
    }
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
    _ content: some View, size: CGSize, look: Look, name: String, scale: CGFloat = 1
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
    try save(try image(of: host, scale: scale), name: "\(name)-\(look.rawValue)")
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

  /// Ten rows from the roster's own projection (your inbox above the nine
  /// agents; channel 0 lives on the bar): channel 1 open (listening dot), an
  /// unread reply on 4, agent 6 off channel 0 (`member = false`), agent 7 under the
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
      XCTAssertEqual(rows.map(\.digit), ["10"] + nine.map(\.0))
      XCTAssertEqual(rows.first?.kind, .inbox)
      XCTAssertEqual(rows.dropFirst().map(\.name), nine.map(\.1))
      XCTAssertNil(rows.first { $0.digit == "0" }, "channel 0 lives on the bar")
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

  /// The inbox under its Polish name above three agents with short names
  /// (Monika, 2026-11-07): the list is as wide as its widest row's own need,
  /// so the inbox's long name no longer leaves empty room right of the short
  /// agent rows. The unread dots of the inbox and of agent 3 stand in one
  /// column at the right edge. Drawn at 2× for review.
  func testRenderAgentsHoverListWithInboxAndShortNames() throws {
    func agent(_ digit: String, _ name: String, unread: Int) -> OverlayAgentsHoverRow {
      OverlayAgentsHoverRow(
        digit: digit, name: name, conversationID: "review-\(digit)", dot: .present,
        status: "", microphoneOpen: false, disconnected: false, identity: nil, policy: .on,
        unread: unread)
    }
    let rows = [
      OverlayAgentsHoverRow(
        kind: .inbox, digit: "10", name: "Twoja skrzynka", conversationID: "review-inbox",
        dot: .idle, status: "", microphoneOpen: false, disconnected: false, identity: nil,
        policy: .on, unread: 1),
      agent("1", "As", unread: 0), agent("3", "Bo", unread: 2), agent("6", "Ola", unread: 0),
    ]
    for look in Look.allCases {
      let model = OverlayAgentsHoverListModel()
      model.rows = rows
      try renderAlone(
        OverlayAgentsHoverList(model: model, palette: look.palette),
        size: OverlayAgentsHoverList.size(rows: rows), look: look,
        name: "bar-hover-list-inbox-short", scale: 2)
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

  // MARK: - 12. The unread dot on the Agents button

  /// The Agents button (✦) without and with a message waiting on channel 0,
  /// at the header's size: the dot's one place (the control's upper-right
  /// corner, clear of the star) and its neutral text-tone colour.
  func testRenderAgentsButtonUnreadDot() throws {
    let all = OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
    for look in Look.allCases {
      func button(_ unread: Int) -> OverlayChannelStatusView {
        OverlayChannelStatusView(
          channels: [], unavailable: false, palette: look.palette, animates: false,
          conversations: [all], unreadCounts: unread > 0 ? [all.id: unread] : [:],
          onShowMonitor: {})
      }
      XCTAssertFalse(button(0).attentionUnread)
      XCTAssertTrue(button(1).attentionUnread)
      try renderAlone(
        HStack(spacing: 12) {
          button(0)
          button(1)
        }
        .padding(10),
        size: CGSize(width: 76, height: 42), look: look, name: "bar-agents-unread-dot")
    }
  }

  // MARK: - 13. An agent speaking

  /// Agent 4 plays its reply aloud (L1): the bar's chip and the agents list
  /// under ✦ show it while it speaks. The dot blinks like the listening
  /// caret; a render is one still frame.
  func testRenderBarWhileAnAgentSpeaks() throws {
    let (state, _) = try barState(four, mode: .midi, speaking: "4")
    try renderBar(state, scene: "bar-4-speaking")
    let rows = DictationOverlayView(state: state).channelStatusView.hoverListRows
    XCTAssertEqual(rows.first { $0.digit == "4" }?.dot, .speaking)
    XCTAssertEqual(rows.first { $0.digit == "3" }?.dot, .present)
    for look in Look.allCases {
      let model = OverlayAgentsHoverListModel()
      model.rows = rows
      try renderAlone(
        OverlayAgentsHoverList(model: model, palette: look.palette),
        size: OverlayAgentsHoverList.size(rows: rows), look: look, name: "bar-hover-list-speaking")
    }
  }

  /// Mini while agent 4 plays its reply aloud, beside mini at rest with the
  /// same roster (L15): mini has no chips, so its Agents button carries the
  /// sign. A render is one still frame of the blink.
  func testRenderMiniWhileAnAgentSpeaks() throws {
    let (state, _) = try barState(four, mode: .mini, speaking: "4")
    XCTAssertEqual(
      DictationOverlayView(state: state).channelStatusView.speakingDigits, ["4"])
    try renderMini(state, scene: "mini-speaking")
  }

  /// A take being finalized (L13): the bar with four agents, its folded
  /// form and mini, where a processing caption used to stand under the
  /// header or at the folded bar's bottom.
  func testRenderBarWhileTheTakeIsFinalized() throws {
    for (mode, scene) in [
      (OverlayPresentationMode.midi, "bar-4-finalizing"),
      (.mini, "mini-finalizing"),
    ] {
      let (state, _) = try barState(four, mode: mode)
      state.handleRecordingStarted()
      state.handleRecordingFinalising()
      XCTAssertTrue(state.transcribing, "\(scene): the take is being finalized")
      if mode == .mini {
        try renderMini(state, scene: scene)
      } else {
        try renderBar(state, scene: scene)
      }
      state.finishControllerRecording()
    }
  }

  /// The roster's status messages (round 5 decision (a)): channel status
  /// unavailable (a warning), a refused toggle and a playback failure
  /// (errors), as the agents roster draws them.
  func testRenderRosterStatusMessages() throws {
    let all = OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
    for look in Look.allCases {
      var roster = OverlayChannelStatusView(
        channels: [
          OverlayChannelDelivery(
            channel: "3", agent: "Lena", deliveryID: nil, stage: nil, isOpen: false)
        ], unavailable: true, palette: look.palette, animates: false,
        toggleError: "the bus refused the change", conversations: [all],
        onSelectConversation: { _ in })
      roster.playbackError = "Speech failed: no connection to the voice service"
      try renderAlone(
        roster.monitorBody(maximumHeight: 320), size: CGSize(width: 360, height: 320),
        look: look, name: "bar-roster-status-messages")
    }
  }

  // MARK: - 14. Channel 0's name

  /// Channel 0 named by default, with a short name of the user's and with a
  /// long one, in the single row (number over the name) and the two-row bar
  /// (number beside the name). The long name is cut to the room its row
  /// leaves; the hint and the card carry it whole. Run with
  /// `-testLanguage pl` for the Polish default ("Wspólny", short "Wsp.").
  func testRenderChannelZeroNames() throws {
    for (label, stored) in [
      ("default", ""), ("short", "Ekipa"), ("long", "Zespół Codescribe i przyjaciele"),
    ] {
      for (rows, agents) in [("1row", four), ("2rows", six)] {
        let (state, _) = try barState(agents, mode: .midi)
        state.sharedChannelNameSetting = { stored }
        state.refreshSharedChannelName()
        XCTAssertEqual(
          state.channelMapLayoutModel.chips.first?.name,
          OverlaySharedChannelName.display(stored))
        try renderBar(state, scene: "bar-ch0-\(label)-\(rows)")
      }
    }
  }

  // MARK: - 15. The hover card from the roster

  /// Each agent chip's hover card as the roster builds it, so the card
  /// carries every fact the chip knows: 1 online on channel 0 with its
  /// microphone open and your message read, 3 speaking, 4 gone and off
  /// channel 0, last heard in a played reply, 5 online under the moon with
  /// a played reply and 2 unread, 6 without a liveness claim, off channel 0,
  /// under the moon and muted, 8 stalled. Run with
  /// `-testLanguage pl` for channel 0's Polish name.
  func testRenderChipHoverCardsFromTheRoster() throws {
    let agents = [
      ("1", "As"), ("3", "Lena"), ("4", "Lena2"), ("5", "Bo"), ("6", "Nina"), ("8", "Igor"),
    ]
    let policies: [String: AgentBroadcastPolicy] = ["4": .off, "5": .quiet, "6": .offQuiet]
    let all = OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let owned = try agents.enumerated().map { index, agent in
      let conversation = try ownerConversation(
        agent.1, channel: agent.0, lease: String(index, radix: 16))
      func with(_ messages: [OverlayConversationMessage]) -> OverlayConversation {
        OverlayConversation(
          id: conversation.id, channel: conversation.channel, name: conversation.name,
          owner: conversation.owner, messages: messages)
      }
      switch agent.0 {
      case "3": return playingReply(in: conversation)
      case "4", "5":
        // 4 and 5 were last heard in a reply already played aloud.
        let played = playingReply(in: conversation).messages[0]
        return with([
          OverlayConversationMessage(
            id: played.id, kind: .reply, text: played.text, order: 1,
            emittedAt: "2026-11-06T19:36:00Z", owner: played.owner, recipients: [],
            deliveryID: nil, replyTo: nil, unsolicited: true,
            playback: OverlayReplyPlayback(
              replyID: "spoken-\(agent.0)", ticket: "t-\(agent.0)", state: "spoken",
              reason: nil, spoken: true, emittedAt: "2026-11-06T19:36:05Z"),
            busPath: "/dev/null")
        ])
      case "1":
        // 1 read your newest message.
        let owner = try XCTUnwrap(conversation.owner)
        return with([
          OverlayConversationMessage(
            id: "user:review-1", kind: .user, text: "Sprawdź belkę.", order: 1,
            emittedAt: "2026-11-06T20:12:00Z", owner: nil,
            recipients: [
              OverlayConversationRecipient(
                owner: owner, deliveryID: "review-1", queued: true, accepted: true,
                acknowledged: true)
            ], deliveryID: "review-1", replyTo: nil, unsolicited: false, playback: nil,
            busPath: "")
        ])
      default: return conversation
      }
    }
    let stale = Date().addingTimeInterval(-OverlayState.followerStallHorizon - 60)
    var hudStates: [String: OverlayChannelHudProjection] = [:]
    var channels: [OverlayChannelDelivery] = []
    for (digit, name) in agents {
      hudStates[digit] = OverlayChannelHudProjection(
        open: digit == "1", loud: false, autosealDeadline: nil,
        followerAlive: digit == "4" ? false : (digit == "6" ? nil : true),
        provider: "codex", providerSessionID: "review-\(digit)",
        unreadSince: digit == "8" ? stale : nil)
      var delivery = OverlayChannelDelivery(
        channel: digit, agent: name, deliveryID: nil, stage: nil, isOpen: false)
      delivery.broadcastPolicy = policies[digit] ?? .on
      channels.append(delivery)
    }
    for look in Look.allCases {
      var roster = OverlayChannelStatusView(
        channels: channels, unavailable: false, palette: look.palette, animates: false,
        hudStates: hudStates, conversations: [all] + owned,
        unreadCounts: [owned[3].id: 2],
        onSelectConversation: { _ in })
      roster.sharedChannelStoredName = ""
      roster.mutedChannels = ["6": true]
      roster.agentProjects = ["1": "Codescribe", "4": "Vibecrafted", "5": "Codescribe"]
      let chips = roster.channelMapModel.agents
      XCTAssertEqual(chips.map(\.digit), agents.map(\.0))
      XCTAssertEqual(chips.first { $0.digit == "3" }?.speaking, true, "3 is speaking")
      for chip in chips {
        let status = try XCTUnwrap(chip.status, "\(chip.digit): an agent chip has a card")
        try renderAlone(
          OverlayChannelChipHoverCard(
            chip: chip, status: status, modifier: .control, palette: look.palette),
          size: OverlayChannelChipHoverCard.size(chip: chip, status: status, modifier: .control),
          look: look, name: "bar-card-\(chip.digit)")
      }
    }
  }

  // MARK: - 17. The unread mark on a pill (L14)

  /// A bar chip and a stacked pill, each without and with unread replies
  /// (the chip also selected), at 1× and 2×: the small dot set on the
  /// outline with its knockout in the bar's colour.
  func testRenderPillUnreadMark() throws {
    for look in Look.allCases {
      func pill(
        _ form: OverlayChannelPill.Form, unread: Bool, selected: Bool = false
      ) -> OverlayChannelPill {
        OverlayChannelPill(
          form: form,
          display: .init(figure: "5", name: "Bruno", unread: unread, selected: selected),
          hint: "", accessibilityText: "5", identifier: "review-pill", palette: look.palette,
          textScale: 1, showsHint: false, action: {})
      }
      let content = VStack(alignment: .leading, spacing: 12) {
        HStack(spacing: 14) {
          pill(.chip, unread: false)
          pill(.chip, unread: true)
          pill(.chip, unread: true, selected: true)
        }
        HStack(spacing: 14) {
          pill(.stacked, unread: false)
          pill(.stacked, unread: true)
          pill(.stacked, unread: true, selected: true)
        }
      }
      .padding(12)
      for scale in [CGFloat(1), 2] {
        try renderAlone(
          content, size: CGSize(width: 240, height: 100), look: look,
          name: "bar-pill-unread-\(Int(scale))x", scale: scale)
      }
    }
  }

  // MARK: - 16. Dead agents on the bar

  /// Gone followers among live ones: 1 Lena and 5 Bruno live, 3 Roman and
  /// 4 Tola gone, in the single row; nine agents with 2, 4 and 7 gone in the
  /// two-row bar; mini; and the agents list under ✦ for the same roster.
  /// Run with `-testLanguage pl` for channel 0's Polish name.
  func testRenderBarWithDeadAgents() throws {
    let mixed = [("1", "Lena"), ("3", "Roman"), ("4", "Tola"), ("5", "Bruno")]
    for (scene, agents, dead) in [
      ("dead-4", mixed, Set(["3", "4"])), ("dead-9", nine, Set(["2", "4", "7"])),
    ] {
      let (state, _) = try barState(agents, mode: .midi, dead: dead)
      XCTAssertEqual(
        Set(state.channelMapLayoutModel.chips.filter(\.disconnected).map(\.digit)), dead,
        "\(scene): the gone followers stay on the map")
      try renderBar(state, scene: scene)
      let rows = DictationOverlayView(state: state).channelStatusView.hoverListRows
      for look in Look.allCases {
        let model = OverlayAgentsHoverListModel()
        model.rows = rows
        try renderAlone(
          OverlayAgentsHoverList(model: model, palette: look.palette),
          size: OverlayAgentsHoverList.size(rows: rows), look: look,
          name: "bar-hover-list-\(scene)")
      }
    }
    let (mini, _) = try barState(mixed, mode: .mini, dead: ["3", "4"])
    try renderMini(mini, scene: "mini-dead-4")
  }

  // MARK: - 17. The header's secondary-click menu

  /// The header's secondary-click menu on the expanded overlay, once with an
  /// anchor chosen (Bottom Center) and once with Free motion on: which item
  /// carries the native checkmark, on the first level and in the Place
  /// submenu (L17). AppKit draws the menu, so the scene pops it
  /// up on the panel and draws the menu's own window while it tracks; the
  /// window's class names go into an attachment when none is found.
  func testRenderHeaderMenu() throws {
    let oldAnchor = OverlayPlacement.anchor
    let oldFreeMotion = OverlayPlacement.freeMotion
    defer {
      OverlayPlacement.anchor = oldAnchor
      OverlayPlacement.freeMotion = oldFreeMotion
    }
    for freeMotion in [false, true] {
      for look in Look.allCases {
        let state = OverlayState.previewListening()
        state.setPresentationMode(.expanded)
        state.selectPlacementAnchor(.bottomCenter)
        if freeMotion { state.selectFreeMotion() }
        XCTAssertEqual(state.freeMotion, freeMotion)
        let scene = freeMotion ? "free-motion" : "anchor"
        try renderHeaderMenu(state, look: look, name: "bar-header-menu-\(scene)")
        // The Place submenu alone (L17): the remembered place's checkmark.
        try renderHeaderMenu(
          state, look: look, name: "bar-header-menu-place-\(scene)", submenu: "Place")
      }
    }
  }

  /// What the tracking menu's capture saw, owned by the main actor so the
  /// timer that fires inside the menu's tracking loop can write it.
  @MainActor
  private final class MenuCapture {
    let menu: NSMenu
    var image: NSImage?
    var windows: [String] = []
    /// The timer fired inside the menu's tracking loop. False after
    /// `popUp` returns means AppKit did not track the menu at all.
    var fired = false

    init(menu: NSMenu) { self.menu = menu }

    func capture() {
      fired = true
      let candidates = NSApp.windows.filter { $0.isVisible && $0.level.rawValue > 0 }
      windows = candidates.map {
        "\(type(of: $0)) level=\($0.level.rawValue) frame=\(NSStringFromRect($0.frame))"
      }
      let menuWindow = candidates.first { "\(type(of: $0))".contains("Menu") }
      if let view = menuWindow?.contentView?.superview ?? menuWindow?.contentView,
        let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)
      {
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let image = NSImage(size: view.bounds.size)
        image.addRepresentation(bitmap)
        self.image = image
      }
      menu.cancelTracking()
    }
  }

  /// Opens the header menu on the real overlay panel in one look and saves
  /// the menu's window as `<name>-<look>`. With `submenu`, the item of that
  /// title's submenu is popped up alone (a copy, since a submenu belongs to
  /// its item), so its own checkmarks can be judged.
  ///
  /// The scene owns its state: it activates the app and ends any menu
  /// tracking an earlier test left before it pops up, because `popUp`
  /// returns at once without tracking (and the capture timer never fires)
  /// when another menu still tracks or the app is not active. A failure
  /// names whether the timer fired inside a tracking loop.
  private func renderHeaderMenu(
    _ state: OverlayState, look: Look, name: String, submenu: String? = nil
  ) throws {
    let suite = "OverlayBarReviewRenderTests.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayBarReviewRenderTests.scale"),
        sizeDefaults: defaults
      ) as? FloatingOverlayPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.appearance = look.appearance
    panel.setContentSize(NSSize(width: 470, height: DictationOverlayWindow.minSize.height))
    BackgroundTestWindow.present(panel)
    if let visible = NSScreen.main?.visibleFrame {
      // At the top of the screen the menu opens downward whole, without the
      // scroll arrow a short space below the click gives it.
      panel.setFrameOrigin(
        NSPoint(x: visible.minX + 40, y: visible.maxY - panel.frame.height - 20))
    }
    let root = try XCTUnwrap(panel.contentView)
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.15))
    let point = NSPoint(x: 150, y: root.bounds.maxY - 22)
    let event = try XCTUnwrap(
      NSEvent.mouseEvent(
        with: .rightMouseDown, location: root.convert(point, to: nil),
        modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
        windowNumber: panel.windowNumber, context: nil, eventNumber: 9_300,
        clickCount: 1, pressure: 1))
    var view = root.hitTest(point)
    var found: NSMenu?
    while let current = view, found == nil {
      found = current.menu(for: event)
      view = current.superview
    }
    let header = try XCTUnwrap(found, "\(name): no header menu at the passive header point")
    header.delegate?.menuNeedsUpdate?(header)
    var menu = header
    if let submenu {
      // The title as the app shows it, also under `-testLanguage pl`.
      let shown = Bundle.main.localizedString(forKey: submenu, value: nil, table: nil)
      let item = try XCTUnwrap(
        header.items.first { [submenu, shown].contains($0.title) }?.submenu,
        "\(name): no \(submenu) submenu among \(header.items.map(\.title))")
      item.delegate?.menuNeedsUpdate?(item)
      menu = try XCTUnwrap(item.copy() as? NSMenu)
    }
    menu.appearance = look.appearance
    // An earlier test's menu or an inactive app makes `popUp` return at
    // once, untracked; the scene starts from its own clean state.
    NSApp.activate(ignoringOtherApps: true)
    NSApp.mainMenu?.cancelTrackingWithoutAnimation()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    let capture = MenuCapture(menu: menu)
    let timer = Timer(timeInterval: 0.6, repeats: false) { _ in
      MainActor.assumeIsolated { capture.capture() }
    }
    RunLoop.main.add(timer, forMode: .common)
    defer { timer.invalidate() }
    menu.popUp(positioning: nil, at: point, in: root)
    let windows = XCTAttachment(string: capture.windows.joined(separator: "\n"))
    windows.name = "\(name)-\(look.rawValue)-windows"
    windows.lifetime = .keepAlways
    add(windows)
    let image = try XCTUnwrap(
      capture.image,
      "\(name): no menu window among \(capture.windows) (tracked: \(capture.fired))")
    try save(image, name: "\(name)-\(look.rawValue)")
  }
}
