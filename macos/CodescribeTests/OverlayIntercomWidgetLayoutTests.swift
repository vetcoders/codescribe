import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

final class OverlayIntercomWidgetLayoutTests: XCTestCase {
  private typealias Layout = OverlayIntercomWidgetLayout

  @MainActor
  func testFourDotHeaderAtCompactAndWideWidthsWithoutPresentingWindows() throws {
    XCTAssertNotNil(
      NSImage(systemSymbolName: "circle.grid.2x2.fill", accessibilityDescription: nil))
    let keyBefore = NSApp.keyWindow
    let suite = "CodescribeTests.header.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let state = OverlayState()
    let generation = state.captureGeneration
    let controller = OverlayIntercomWidgetController(
      state: state, textScale: TextScaleController(key: suite),
      desktopStore: OverlayIntercomDesktopStore(defaults: defaults))
    defer { controller.invalidate() }
    for palette in [OverlayAppearancePalette.light, .dark] {
      for width in [
        IntercomHeaderMetrics(agentCount: 0, scale: 1).minimumWidth(paging: false), 400,
      ] {
        let host = NSHostingView(
          rootView: controller.offscreenHeader(width: width)
            .environment(\.colorScheme, palette.appearance == .dark ? .dark : .light)
            .environment(\.csTextScale, 1)
            .frame(width: width, height: Layout.controlHeight(scale: 1))
            .background(palette.desktopBackground.color))
        host.frame.size = NSSize(width: width, height: Layout.controlHeight(scale: 1))
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        let attachment = XCTAttachment(
          image: NSImage(cgImage: try XCTUnwrap(bitmap.cgImage), size: host.bounds.size))
        attachment.name = "four-dot-header-\(palette.appearance)-\(Int(width))"
        attachment.lifetime = .keepAlways
        add(attachment)
      }
    }
    XCTAssertEqual(controller.interactionWindowCounts.owned, 0)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(NSApp.keyWindow === keyBefore)
  }

  func testExpandedConversationIsAlwaysAboveItsSiblingRows() throws {
    let cards = try ["2", "5", "8"].map { channel in
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": "session-\(channel)",
          "lease_id": String(repeating: channel, count: 32), "channel": channel,
          "name": "Agent \(channel)",
        ]))
      return OverlayIntercomCardIdentity(
        digit: channel,
        conversation: .init(
          id: owner.id, channel: channel, name: owner.name, owner: owner, messages: []))
    }
    var presentation = OverlayIntercomPresentation()
    presentation.expand(cards[1])
    XCTAssertEqual(presentation.frontOrder(cards), [cards[0], cards[2], cards[1]])
    XCTAssertEqual(presentation.expanded, [cards[1]])
  }

  func testTwoAndThreeRowCardsReserveAgentInformationBeforePreview() throws {
    for scale in [CGFloat(0.8), 1, 1.6, 2] {
      let metrics = IntercomContentMetrics(names: ["Agent"], digits: ["2"], scale: scale)
      let width = max(metrics.actionsWidth, 400 * scale)
      let row = Layout.controlHeight(scale: scale)
      XCTAssertEqual(metrics.cardContent(width: width, cardHeight: row), .header)
      for height in [2 * row, 3 * row] {
        XCTAssertEqual(metrics.cardContent(width: width, cardHeight: height), .context)
        XCTAssertNil(metrics.compactPreviewLayout(width: width, cardHeight: height))
      }
      let later = try XCTUnwrap(metrics.compactPreviewLayout(width: width, cardHeight: 8 * row))
      XCTAssertEqual(metrics.cardContent(width: width, cardHeight: 8 * row), .preview)
      XCTAssertGreaterThan(later.height, 3 * row)
      XCTAssertLessThan(later.height, 8 * row)
      XCTAssertTrue(later.showsComposer)
      XCTAssertFalse(
        try XCTUnwrap(metrics.compactPreviewLayout(width: width, cardHeight: 5 * row)).showsComposer
      )
    }
  }

  func testUnreadablyNarrowOrHeaderOnlyCardsDoNotMountClippedPreview() {
    for scale in [CGFloat(0.8), 1, 1.6] {
      let metrics = IntercomContentMetrics(names: ["Agent"], digits: ["2"], scale: scale)
      XCTAssertNil(metrics.compactPreviewLayout(width: metrics.iconSide, cardHeight: 2000))
      XCTAssertNil(metrics.compactPreviewLayout(width: 500, cardHeight: metrics.iconSide))
      XCTAssertEqual(metrics.cardContent(width: 500, cardHeight: metrics.iconSide), .header)
    }
  }

  @MainActor
  func testAgentMetadataUsesTheExactOwnerAndDeclaredExecutionLocation() throws {
    let identity = AgentPlaybackIdentity(provider: "codex", session: "fixture", bus: "/fixture/bus")
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": identity.provider, "provider_session_id": identity.session,
        "lease_id": identity.leaseID, "channel": "3", "name": "Agent",
      ]))
    var binding = AgentPlaybackBinding(
      identity: identity, model: "gpt-6.1-sol", repositoryName: "codescribe",
      workspace: "/fixture/codescribe",
      frame: .init(session: "codescribe", paneID: 3),
      terminal: .init(host: "iTerm2", hostBundleID: "com.googlecode.iterm2", tty: "/dev/ttys011"))
    let state = OverlayState()
    let conversation = OverlayConversation(
      id: owner.id, channel: "3", name: owner.name, owner: owner, messages: [])
    state.applyBoundAgents(["3": binding])
    XCTAssertEqual(
      state.conversationAgentDescriptor(conversation), "Codex · gpt-6.1-sol · codescribe")
    XCTAssertEqual(state.conversationAgentLocation(conversation), "Frame · codescribe · 3")
    binding.frame = nil
    state.applyBoundAgents(["3": binding])
    XCTAssertEqual(state.conversationAgentLocation(conversation), "iTerm2 · /dev/ttys011")
    binding.terminal = nil
    XCTAssertEqual(binding.location, "/fixture/codescribe")
    binding.workspace = nil
    XCTAssertNil(binding.location, "No declared location must not invent a host")
    let replacement = AgentPlaybackIdentity(
      provider: "codex", session: "replacement", bus: "/fixture/new")
    state.applyBoundAgents(["3": .init(identity: replacement, model: "other-model")])
    XCTAssertNil(state.conversationAgentBinding(conversation), "A reused channel is another owner")
    let wrongLease = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": identity.provider, "provider_session_id": identity.session,
        "lease_id": String(repeating: "f", count: 32), "channel": "3", "name": "Agent",
      ]))
    state.applyBoundAgents(["3": binding])
    XCTAssertNil(
      state.conversationAgentBinding(
        .init(
          id: wrongLease.id, channel: "3", name: "Agent", owner: wrongLease, messages: [])))
  }

  @MainActor
  func testProductionCardPhaseReviewSheetInLightAppearance() throws {
    try reviewProductionCardPhases(dark: false)
  }

  @MainActor
  func testProductionCardPhaseReviewSheetInDarkAppearance() throws {
    try reviewProductionCardPhases(dark: true)
  }

  @MainActor
  private func reviewProductionCardPhases(dark: Bool) throws {
    func render(
      channel: String, text: String?, height: CGFloat, dark: Bool,
      expanded: Bool = false, hover: Bool = false, longDraft: Bool = false
    ) throws
      -> Data
    {
      let session = "fixture-\(channel)"
      let identity = AgentPlaybackIdentity(provider: "codex", session: session, bus: "/fixture/bus")
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": "fixture-\(channel)",
          "lease_id": identity.leaseID, "channel": channel, "name": "Agent",
        ]))
      let messages: [OverlayConversationMessage] =
        text.map { body in
          [
            OverlayConversationMessage(
              id: "fixture-reply", kind: .reply, text: body, order: 1, emittedAt: "",
              owner: owner, recipients: [], deliveryID: nil, replyTo: nil,
              unsolicited: false, playback: nil, busPath: "")
          ]
        } ?? []
      let conversation = OverlayConversation(
        id: owner.id, channel: channel, name: owner.name, owner: owner, messages: messages)
      let bridgeRoot = RealAgentBridgeInstaller().bridgeRoot
      guard bridgeRoot.path.contains("codescribe-test-data.") else {
        XCTFail("Native fixture requires make test-swift's isolated bridge root")
        throw CocoaError(.fileWriteNoPermission)
      }
      let leases = bridgeRoot.appendingPathComponent("leases", isDirectory: true)
      try FileManager.default.createDirectory(at: leases, withIntermediateDirectories: true)
      let leaseURL = leases.appendingPathComponent(identity.leaseID + ".json")
      let previousLease = try? Data(contentsOf: leaseURL)
      let lease: [String: Any] = [
        "schema": "codescribe.agent-bridge.lease.v1", "lease_id": identity.leaseID,
        "provider": identity.provider, "provider_session_id": identity.session,
        "bus": identity.bus, "model": "gpt-6.1-sol", "repository_name": "codescribe",
        "workspace": "/fixture/codescribe", "frame": ["session": "codescribe", "pane_id": 11],
      ]
      try JSONSerialization.data(withJSONObject: lease).write(to: leaseURL, options: .atomic)
      defer {
        if let previousLease {
          try? previousLease.write(to: leaseURL, options: .atomic)
        } else {
          try? FileManager.default.removeItem(at: leaseURL)
        }
      }
      let state = OverlayState()
      state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
      state.applyChannelRoster([
        .init(
          channel: channel, audience: owner.name, provider: owner.provider,
          providerSessionId: owner.providerSessionID, open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
      ])
      state.applyBoundAgents([
        channel: .init(
          identity: identity, model: "gpt-6.1-sol", repositoryName: "codescribe",
          workspace: "/fixture/codescribe",
          frame: .init(session: "codescribe", paneID: 11))
      ])
      let retainedDraft =
        longDraft
        ? "Preserved draft " + String(repeating: "Shared text stays intact. ", count: 20) : ""
      state.conversationDrafts[conversation.id] = retainedDraft
      state.publishConversationText = { _, _, _ in XCTFail("Mounting a preview must not send") }
      let keyWindow = NSApp.keyWindow
      let generation = state.captureGeneration
      let suite = "CodescribeTests.compact.\(UUID().uuidString)"
      let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
      defer { defaults.removePersistentDomain(forName: suite) }
      let controller = OverlayIntercomWidgetController(
        state: state, textScale: TextScaleController(key: suite),
        desktopStore: OverlayIntercomDesktopStore(defaults: defaults))
      defer { controller.invalidate() }
      let palette: OverlayAppearancePalette = dark ? .dark : .light
      let card = OverlayIntercomCardIdentity(digit: channel, conversation: conversation)
      let content: AnyView =
        hover
        ? AnyView(controller.offscreenHover(card, palette: palette))
        : AnyView(
          controller.offscreenCard(
            card, width: 360, height: height, scale: 1, expanded: expanded))
      let host = NSHostingView(
        rootView:
          content
          .environment(\.colorScheme, dark ? .dark : .light)
          .environment(\.csTextScale, 1)
          .symbolEffectsRemoved()
          .transaction { $0.disablesAnimations = true }
          .background(palette.desktopBackground.color))
      host.sizingOptions = []
      host.frame = NSRect(x: 0, y: 0, width: 360, height: height)
      let window = NSWindow(
        contentRect: NSRect(x: -10000, y: -10000, width: 360, height: height),
        styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      defer { window.close() }
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
      host.layoutSubtreeIfNeeded()
      let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
      host.cacheDisplay(in: host.bounds, to: bitmap)
      let image = NSImage(size: host.bounds.size)
      image.addRepresentation(bitmap)
      let attachment = XCTAttachment(image: image)
      attachment.name =
        "Compact-\(channel)-\(Int(height))-\(dark ? "dark" : "light")-\(text == nil ? "empty" : "message")-\(expanded ? "open" : hover ? "hover" : "card")-\(longDraft ? "long-draft" : "idle")"
      attachment.lifetime = .keepAlways
      add(attachment)
      var header = Data()
      let headerPixels = Int(30 * CGFloat(bitmap.pixelsHigh) / height)
      for y in 0..<headerPixels {
        var previousBrightness: CGFloat?
        for x in 0..<bitmap.pixelsWide {
          let color = try XCTUnwrap(bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB))
          let brightness =
            color.redComponent * 0.2126 + color.greenComponent * 0.7152 + color.blueComponent
            * 0.0722
          let ink = previousBrightness.map { abs(brightness - $0) > 0.06 } ?? false
          previousBrightness = brightness
          header.append(ink ? 1 : 0)
        }
      }
      var pendingViews: [NSView] = [host]
      var editors: [NSTextView] = []
      while let view = pendingViews.popLast() {
        if let editor = view as? NSTextView, editor.isEditable { editors.append(editor) }
        pendingViews.append(contentsOf: view.subviews)
      }
      if !expanded {
        let shouldHaveInput = !hover && height >= 220
        XCTAssertEqual(editors.count, shouldHaveInput ? 1 : 0)
        for editor in editors {
          XCTAssertEqual(editor.string, retainedDraft)
          XCTAssertLessThanOrEqual(try XCTUnwrap(editor.enclosingScrollView).frame.height, 28.5)
        }
      }
      XCTAssertTrue(NSApp.keyWindow === keyWindow, "Resizing or hovering must not take focus")
      XCTAssertEqual(controller.interactionWindowCounts.owned, 0)
      XCTAssertEqual(controller.interactionWindowCounts.visible, 0)
      XCTAssertEqual(state.captureGeneration, generation)
      XCTAssertEqual(state.conversationDrafts[conversation.id], retainedDraft)
      return header
    }
    let richMessage = """
      ## Wynik pracy
      **Gotowe:** wspólne formatowanie na tafli.

      - Tożsamość i status najpierw
      - Potem czytelna treść wiadomości

      ```swift
      let agent = "Agent"
      print(agent)
      ```
      """
    do {
      // Native control hover and backdrop compositing can change between hosts.
      // Preserve both review images rather than treating a pixel count as proof
      // of absent content. The phase policy above pins the metadata-only range.
      for height in [CGFloat(30), 60, 90] {
        _ = try render(channel: "3", text: nil, height: height, dark: dark)
        _ = try render(channel: "3", text: richMessage, height: height, dark: dark)
      }
      for height in [CGFloat(150), 220] {
        _ = try render(channel: "3", text: nil, height: height, dark: dark)
        _ = try render(channel: "3", text: richMessage, height: height, dark: dark)
      }
      _ = try render(channel: "3", text: richMessage, height: 220, dark: dark, longDraft: true)
      _ = try render(channel: "3", text: richMessage, height: 240, dark: dark, hover: true)
      _ = try render(channel: "3", text: richMessage, height: 360, dark: dark, expanded: true)
      let three = try render(channel: "3", text: nil, height: 30, dark: dark)
      let four = try render(channel: "4", text: nil, height: 30, dark: dark)
      XCTAssertNotEqual(
        three, four, "The compact header must paint its digit")
    }
  }

  func testHeaderPreservesBothAudioHitRegionsBeforeOptionalLabelsAndCount() {
    for scale in [CGFloat(0.8), 1, 1.6, 2] {
      for paging in [false, true] {
        let metrics = IntercomHeaderMetrics(
          agentCount: 12, scale: scale)
        let minimum = metrics.minimumWidth(paging: paging)
        let compact = metrics.content(width: minimum, paging: paging)
        XCTAssertFalse(compact.showsLabels)
        XCTAssertFalse(compact.showsCount)
        XCTAssertEqual(metrics.audioWidth, 2 * Layout.controlHeight(scale: scale) + 6)
        XCTAssertGreaterThan(
          minimum, metrics.groupWidth + metrics.audioWidth + 2 * metrics.controlHeight)
        let withCount = metrics.width(showsLabels: false, showsCount: true, paging: paging)
        XCTAssertTrue(metrics.content(width: withCount, paging: paging).showsCount)
        XCTAssertFalse(metrics.content(width: withCount - 0.1, paging: paging).showsCount)
        let full = metrics.width(showsLabels: true, showsCount: true, paging: paging)
        XCTAssertTrue(metrics.content(width: full, paging: paging).showsLabels)
        XCTAssertFalse(metrics.content(width: full - 0.1, paging: paging).showsLabels)
        XCTAssertGreaterThan(full, withCount)
      }
    }
  }

  func testHeaderBudgetRespondsToCountAndActualPagerPresence() {
    for scale in [CGFloat(0.8), 1, 1.6] {
      let empty = IntercomHeaderMetrics(
        agentCount: 1, scale: scale)
      let busy = IntercomHeaderMetrics(
        agentCount: 25, scale: scale)
      XCTAssertGreaterThan(busy.countWidth, empty.countWidth)
      XCTAssertEqual(busy.mailboxWidth, empty.mailboxWidth)
      XCTAssertEqual(
        empty.minimumWidth(paging: false), busy.minimumWidth(paging: false),
        "metadata cannot take room from the mandatory commands")
      XCTAssertGreaterThan(busy.minimumWidth(paging: true), busy.minimumWidth(paging: false))
      let fullWithoutBadges = empty.width(showsLabels: true, showsCount: true, paging: false)
      XCTAssertFalse(busy.content(width: fullWithoutBadges, paging: false).showsLabels)
    }
  }

  func testPagingCountsActualTallCardsSoLowerAgentsRemainReachable() {
    XCTAssertEqual(
      Layout.visibleRange(
        count: 8, offset: 0, visibleHeight: 900, scale: 1, cardHeight: 260), 0..<3)
    XCTAssertEqual(
      Layout.visibleRange(
        count: 8, offset: 99, visibleHeight: 900, scale: 1, cardHeight: 260), 5..<8)
  }

  func testResizeHonorsTheSharedMailboxMenuAndHeaderControls() {
    for scale in [CGFloat(0.8), 1, 1.6] {
      let screen = CGRect(x: -1440, y: 0, width: 1440, height: 900)
      let metrics = IntercomContentMetrics(names: ["Lena"], digits: ["2"], scale: scale)
      let floor = IntercomHeaderMetrics(agentCount: 2, scale: scale).minimumWidth(paging: false)
      let base = CGRect(x: -1000, y: 400, width: 400, height: 200)
      let left = metrics.resizedGroupFrame(
        base, delta: CGSize(width: 1000, height: 0), edge: .left,
        count: 2, visibleFrame: screen, minimumWidth: floor)
      XCTAssertEqual(left.width, floor)
      XCTAssertEqual(left.maxX, base.maxX, "the opposite edge remains anchored")
      let right = metrics.resizedGroupFrame(
        base, delta: CGSize(width: -1000, height: 0), edge: .right,
        count: 2, visibleFrame: screen, minimumWidth: floor)
      XCTAssertEqual(right.width, floor)
      XCTAssertEqual(right.minX, base.minX)
      XCTAssertTrue(screen.contains(left))
      XCTAssertTrue(screen.contains(right))
    }
  }

  func testHumanAndBroadcastMailboxesActuallyExpandWithoutBecomingAgents() {
    let inbox = OverlayConversation.humanInbox()
    let all = OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
    var presentation = OverlayIntercomPresentation()
    for conversation in [inbox, all] {
      let identity = OverlayIntercomCardIdentity(
        digit: conversation.channel, conversation: conversation)
      presentation.expand(identity)
      XCTAssertEqual(
        presentation.expanded, [identity], "a mailbox click must reach the content state")
      XCTAssertNil(conversation.owner, "opening a mailbox never creates an agent")
      presentation.close(identity)
      XCTAssertTrue(presentation.expanded.isEmpty)
    }
    for channel in ["0", "2", "10"] {
      let unknown = OverlayConversation(
        id: "unknown", channel: channel, name: "Unbound", owner: nil, messages: [])
      presentation.expand(.init(digit: channel, conversation: unknown))
      XCTAssertTrue(presentation.expanded.isEmpty, "arbitrary ownerless routes remain forbidden")
    }
  }

  func testDraggingTheGroupBelowTheScreenRecoversTheWholeVisibleStack() {
    for screen in [
      CGRect(x: 0, y: 0, width: 1440, height: 900),
      CGRect(x: -1680, y: -90, width: 1680, height: 1050),
    ] {
      let proposed = CGRect(
        x: screen.minX + 100, y: screen.minY + 120, width: 400, height: 30)
      let metrics = IntercomContentMetrics(names: ["Lena"], digits: ["2"], scale: 1)
      let height = metrics.fittedCardHeight(260, count: 5, available: screen.height)
      let result = Layout.clampedHeader(
        proposed, count: 5, visibleFrame: screen, cardHeight: height)
      XCTAssertGreaterThan(result.minY, proposed.minY)
      let lostHeader = proposed.offsetBy(dx: 0, dy: -1000)
      let recovered = Layout.clampedHeader(
        lostHeader, count: 5, visibleFrame: screen, cardHeight: height)
      XCTAssertEqual(recovered, result)
      XCTAssertTrue(screen.contains(recovered))
      for index in 0..<5 {
        XCTAssertTrue(
          screen.contains(
            Layout.rowFrame(
              index: index, header: recovered, count: 5, visibleFrame: screen, cardHeight: height)))
      }
    }
  }

  func testMeasuredWidthSelectsTheWholeGroupsContentAtThreeTextScales() {
    for scale in [CGFloat(0.8), 1, 1.6] {
      let metrics = IntercomContentMetrics(
        names: ["A", "Klaudiusz"], digits: ["1", "9"], scale: scale)
      let short = IntercomContentMetrics(names: ["A"], digits: ["1"], scale: scale)
      XCTAssertGreaterThan(metrics.nameWidth, short.nameWidth)
      XCTAssertEqual(metrics.tier(width: metrics.iconSide, cardHeight: metrics.iconSide), .icon)
      XCTAssertEqual(
        metrics.tier(width: metrics.minimumNameWidth - 0.1, cardHeight: metrics.iconSide), .icon)
      XCTAssertEqual(
        metrics.tier(width: metrics.minimumNameWidth, cardHeight: metrics.iconSide), .name)
      XCTAssertEqual(metrics.tier(width: metrics.nameWidth, cardHeight: metrics.iconSide), .name)
      XCTAssertEqual(
        metrics.tier(width: metrics.actionsWidth - 0.1, cardHeight: metrics.iconSide), .name)
      XCTAssertEqual(
        metrics.tier(width: metrics.actionsWidth, cardHeight: metrics.iconSide), .actions)
      XCTAssertEqual(
        metrics.fittedWidth(metrics.minimumNameWidth - 1, available: 2000), metrics.iconSide,
        "compact cards snap to a square instead of stretching an empty digit row")
    }
  }

  func testNamesTruncateBetweenTheShortestRowAndTheWholeName() {
    for scale in [CGFloat(0.8), 1, 1.6] {
      let metrics = IntercomContentMetrics(
        names: ["A", "Klaudiusz-div0"], digits: ["1", "9"], scale: scale)
      XCTAssertLessThan(metrics.minimumNameWidth, metrics.nameWidth)
      XCTAssertGreaterThan(metrics.minimumNameWidth, metrics.iconSide)
      let between = (metrics.minimumNameWidth + metrics.nameWidth) / 2
      XCTAssertEqual(
        metrics.fittedWidth(between, available: 2000), between,
        "a width between the shortest name row and the whole name is kept, not snapped")
      XCTAssertEqual(metrics.tier(width: between, cardHeight: metrics.iconSide), .name)
      let short = IntercomContentMetrics(names: ["A"], digits: ["1"], scale: scale)
      XCTAssertEqual(short.minimumNameWidth, short.nameWidth, "a short name has nothing to cut")
    }
  }

  /// Maciej, 2026-10-23 seal: digit, antenna, moon, dot and a sliver of the
  /// name is a bad narrow row; the microphone comes first, then the speaker,
  /// and the antenna and moon only once the row is much wider.
  func testNarrowRowsKeepTheMicrophoneBeforeTheAntennaAndMoon() {
    for scale in [CGFloat(0.8), 1, 1.6, 2] {
      let metrics = IntercomContentMetrics(
        names: ["Lena", "Klaudiusz-div0"], digits: ["2", "9"], scale: scale)
      let narrowest = metrics.rowControls(width: metrics.minimumNameWidth)
      XCTAssertGreaterThanOrEqual(narrowest.slots, 1, "the narrowest name row has a microphone")
      XCTAssertFalse(narrowest.marks)
      XCTAssertFalse(metrics.rowControls(width: metrics.nameWidth).marks)
      XCTAssertFalse(metrics.rowControls(width: metrics.actionsWidth - 0.1).marks)
      XCTAssertEqual(metrics.rowControls(width: metrics.actionsWidth), .all)
      var previous = 0
      for width in stride(from: metrics.minimumNameWidth, to: metrics.actionsWidth, by: 1) {
        let row = metrics.rowControls(width: width)
        XCTAssertGreaterThanOrEqual(row.slots, previous, "widening never drops a control")
        previous = row.slots
        let used =
          metrics.minimumNameWidth - metrics.marksWidth
          + CGFloat(row.slots) * Layout.controlHeight(scale: scale)
        XCTAssertLessThanOrEqual(used, width, "every shown control fits beside the shortest name")
      }
      XCTAssertEqual(previous, 3, "the speaker and focus return before the marks")
    }
  }

  func testOneSharedPlaceIsTheMicrophoneUntilTheAgentTalks() {
    typealias Audio = OverlayAgentAudioControls
    XCTAssertFalse(Audio.sharedShowsSpeaker(speaking: false, muted: false, open: false))
    XCTAssertTrue(Audio.sharedShowsSpeaker(speaking: true, muted: false, open: false))
    XCTAssertFalse(
      Audio.sharedShowsSpeaker(speaking: true, muted: false, open: true),
      "a running take keeps its microphone")
    XCTAssertFalse(Audio.sharedShowsSpeaker(speaking: true, muted: true, open: false))
    XCTAssertFalse(Audio.sharedShowsSpeaker(speaking: true, muted: nil, open: false))
  }

  /// Maciej, 2026-10-24: stretching must keep the roster inside the screen.
  /// Intermediate heights remain continuous until the shared budget is full.
  func testCardsStretchContinuouslyWithinTheWholeRosterBudget() {
    for scale in [CGFloat(0.8), 1, 1.6] {
      let metrics = IntercomContentMetrics(names: ["Lena"], digits: ["2"], scale: scale)
      let icon = metrics.iconSide
      for requested in [0, icon / 2, icon] {
        XCTAssertEqual(
          metrics.fittedCardHeight(requested, count: 3, available: 2000), icon,
          "never shorter than one row")
      }
      for requested in [1.3 * icon, 1.5 * icon, 2.7 * icon] {
        XCTAssertEqual(
          metrics.fittedCardHeight(requested, count: 3, available: 2000), requested,
          "any height in between, not a snap")
      }
      let maximum = (2000 - Layout.controlHeight(scale: scale)) / 3 - Layout.gap
      XCTAssertEqual(metrics.fittedCardHeight(5000, count: 3, available: 2000), maximum)
      XCTAssertEqual(
        metrics.tier(width: metrics.stackedActionsWidth, cardHeight: 2 * icon), .actions,
        "two rows' height moves the controls under the name")
      XCTAssertEqual(
        metrics.tier(width: metrics.stackedActionsWidth, cardHeight: 1.6 * icon), .name)
    }
  }

  func testTallNarrowCardsMoveTheSameControlsBelowTheName() {
    for scale in [CGFloat(0.8), 1, 1.6] {
      let metrics = IntercomContentMetrics(names: ["A"], digits: ["1"], scale: scale)
      let width = metrics.stackedActionsWidth
      XCTAssertLessThan(width, metrics.actionsWidth)
      XCTAssertGreaterThanOrEqual(width, 3 * Layout.controlHeight(scale: scale) + 9)
      XCTAssertEqual(metrics.tier(width: width, cardHeight: 2 * metrics.iconSide - 0.1), .name)
      XCTAssertEqual(metrics.tier(width: width, cardHeight: 2 * metrics.iconSide), .actions)
      XCTAssertEqual(
        metrics.tier(width: metrics.iconSide, cardHeight: 500), .icon,
        "height alone cannot reveal a name or controls that do not fit")
      if metrics.nameWidth < width {
        XCTAssertEqual(metrics.tier(width: metrics.nameWidth, cardHeight: 500), .name)
      }
    }
  }

  func testGroupMinimumUsesTheWidestAndTallestIconContent() {
    for scale in [CGFloat(0.8), 1, 1.6] {
      let group = IntercomContentMetrics(names: ["A", "B"], digits: ["1", "123456"], scale: scale)
      let single = IntercomContentMetrics(names: ["B"], digits: ["123456"], scale: scale)
      let small = IntercomContentMetrics(names: ["A"], digits: ["1"], scale: scale)
      XCTAssertEqual(group.iconSide, max(single.iconSide, small.iconSide))
      XCTAssertEqual(group.minimumGroupSize(count: 2).width, group.iconSide)
      XCTAssertEqual(
        group.minimumGroupSize(count: 2).height,
        Layout.controlHeight(scale: scale) + 2 * (group.iconSide + Layout.gap))
    }
  }

  func testTextAndControlMetricsAreIndependentOfTheSelectedTier() {
    for scale in [CGFloat(0.8), 1, 1.6] {
      let metrics = IntercomContentMetrics(names: ["Lena"], digits: ["2"], scale: scale)
      let before = metrics
      for width in [metrics.iconSide, metrics.nameWidth, metrics.actionsWidth, 1200] {
        _ = metrics.tier(width: width, cardHeight: metrics.iconSide)
        XCTAssertEqual(metrics, before)
        XCTAssertEqual(metrics.nameFontSize, 11 * scale)
        XCTAssertEqual(metrics.digitFontSize, 10 * scale)
        XCTAssertEqual(
          metrics.actionsWidth - metrics.nameWidth, 3 * Layout.controlHeight(scale: scale))
      }
    }
  }

  /// One corner for the header, every row and a mini fiszka; never half the
  /// row, which drew a two-line card as a stadium (Maciej, 2026-10-23).
  func testRowsAreRoundedCardsNotStadiumsAtEveryScale() {
    for scale in [CGFloat(1), 1.5, 2] {
      let radius = Layout.rowCornerRadius(scale: scale)
      XCTAssertLessThan(radius, Layout.controlHeight(scale: scale) / 2, "a one-line row")
      XCTAssertLessThan(radius, Layout.controlHeight(scale: scale), "a two-line card")
    }
    XCTAssertEqual(Layout.rowCornerRadius(scale: 0.8), Layout.rowCornerRadius(scale: 1))
    XCTAssertLessThanOrEqual(Layout.gap, 4, "the stack reads as one list")
  }

  func testCardsUseCompactRowsBeforePagingAwayFittingSiblings() {
    let metrics = IntercomContentMetrics(names: ["Lena"], digits: ["2"], scale: 1)
    let minimum = metrics.minimumGroupSize(count: 4)
    let row = metrics.fittedCardHeight(200, count: 4, available: minimum.height)
    XCTAssertEqual(row, metrics.iconSide)
    XCTAssertEqual(metrics.tier(width: metrics.nameWidth, cardHeight: row), .name)
    XCTAssertEqual(metrics.nameFontSize, 11)
    XCTAssertEqual(metrics.digitFontSize, 10)
    let tall = metrics.fittedCardHeight(200, count: 4, available: 700)
    XCTAssertGreaterThanOrEqual(tall, 2 * metrics.iconSide)
    let screen = CGRect(x: 0, y: 0, width: 1000, height: 700)
    let header = Layout.clampedHeader(
      CGRect(x: 900, y: screen.maxY - 30, width: 200, height: 30),
      count: 4, visibleFrame: screen, cardHeight: tall)
    let shown = Layout.visibleRange(
      count: 4, offset: 0, visibleHeight: screen.height, scale: 1, cardHeight: tall)
    XCTAssertEqual(shown, 0..<4)
    XCTAssertEqual(
      Layout.visibleRange(
        count: 4, offset: 3, visibleHeight: screen.height, scale: 1, cardHeight: tall), 0..<4)
    for index in 0..<shown.count {
      let frame = Layout.rowFrame(
        index: index, header: header, count: shown.count,
        visibleFrame: screen, cardHeight: tall)
      XCTAssertTrue(screen.contains(frame))
      XCTAssertEqual(frame.height, tall)
    }
  }

  func testGroupResizeSnappingKeepsTheDraggedEdgesAnchor() {
    let metrics = IntercomContentMetrics(names: ["Klaudiusz"], digits: ["1"], scale: 1)
    let frame = CGRect(x: 100, y: 300, width: 300, height: 200)
    let screen = CGRect(x: 0, y: 0, width: 1000, height: 800)
    let right = metrics.resizedGroupFrame(
      frame, delta: CGSize(width: -250, height: -40),
      edge: .bottomRight, count: 2, visibleFrame: screen)
    XCTAssertEqual(right.minX, frame.minX)
    XCTAssertEqual(right.width, metrics.iconSide)
    XCTAssertEqual(right.maxY, frame.maxY)
    let left = metrics.resizedGroupFrame(
      frame, delta: CGSize(width: 250, height: 0),
      edge: .left, count: 2, visibleFrame: screen)
    XCTAssertEqual(left.maxX, frame.maxX)
    XCTAssertEqual(left.width, metrics.iconSide)
    XCTAssertEqual(left.height, frame.height)
    let again = metrics.resizedGroupFrame(
      frame, delta: CGSize(width: 250, height: 0),
      edge: .left, count: 2, visibleFrame: screen)
    XCTAssertEqual(left, again, "absolute deltas never compound across events")
    let wider = metrics.resizedGroupFrame(
      frame, delta: CGSize(width: 120, height: 0), edge: .right, count: 2, visibleFrame: screen)
    XCTAssertEqual(wider.minX, frame.minX, "a row's right edge keeps the left one")
    XCTAssertEqual(wider.width, 420)
    XCTAssertEqual(wider.height, frame.height)
  }

  /// Every row resizes the group by its own edges (Maciej, 2026-10-23: "na
  /// każdy rząd, w lewo, w prawo, w dół i w górę"), and the edge under the
  /// pointer follows it: the rows down to a dragged bottom edge share the
  /// drag, the rows from a dragged top edge down share it.
  func testARowsEdgeFollowsThePointer() {
    XCTAssertEqual(Layout.rowEdgeFactor(edge: .bottom, index: 0, count: 4), 4)
    XCTAssertEqual(Layout.rowEdgeFactor(edge: .bottom, index: 3, count: 4), 1)
    XCTAssertEqual(Layout.rowEdgeFactor(edge: .bottomRight, index: 1, count: 4), 2)
    XCTAssertEqual(Layout.rowEdgeFactor(edge: .top, index: 0, count: 4), 1)
    XCTAssertEqual(Layout.rowEdgeFactor(edge: .top, index: 3, count: 4), 4)
    XCTAssertEqual(Layout.rowEdgeFactor(edge: .left, index: 2, count: 4), 1)
    XCTAssertEqual(Layout.rowEdgeFactor(edge: .bottom, index: nil, count: 4), 1, "the header")
    XCTAssertEqual(Layout.rowEdgeFactor(edge: .bottom, index: 7, count: 4), 1, "a paged-out row")

    // Row 1 of 4 dragged down by 20: rows 0 and 1 grow 10 each, so its bottom
    // edge lands 20 lower, under the pointer.
    let metrics = IntercomContentMetrics(names: ["Klaudiusz"], digits: ["1"], scale: 1)
    let row = metrics.iconSide
    let header = Layout.controlHeight(scale: 1)
    let group = CGRect(
      x: 100, y: 200, width: 300, height: header + 4 * (row + Layout.gap))
    let screen = CGRect(x: 0, y: 0, width: 1000, height: 900)
    let factor = Layout.rowEdgeFactor(edge: .bottom, index: 1, count: 4)
    let down = metrics.resizedGroupFrame(
      group, delta: CGSize(width: 0, height: -20 * factor), edge: .bottom, count: 4,
      visibleFrame: screen)
    let rowHeight = (down.height - header) / 4 - Layout.gap
    XCTAssertEqual(rowHeight, row + 10, accuracy: 0.001)
    XCTAssertEqual(down.maxY, group.maxY, "the header stays")
    let rowOneBottom = down.maxY - header - 2 * (rowHeight + Layout.gap)
    let before = group.maxY - header - 2 * (row + Layout.gap)
    XCTAssertEqual(rowOneBottom, before - 20, accuracy: 0.001)

    // Its top dragged up by 30: rows 1–3 share it, the group's bottom stays.
    let upFactor = Layout.rowEdgeFactor(edge: .top, index: 1, count: 4)
    let up = metrics.resizedGroupFrame(
      group, delta: CGSize(width: 0, height: 30 * upFactor), edge: .top, count: 4,
      visibleFrame: screen)
    XCTAssertEqual(up.minY, group.minY, "the bottom stays")
    XCTAssertEqual(up.height - group.height, 40, accuracy: 0.001)
  }

  private func conversation(
    _ name: String, channel: String, lease: String
  ) throws -> OverlayConversation {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "\(name)-session",
        "lease_id": String(repeating: lease, count: 32), "channel": channel, "name": name,
      ]))
    return OverlayConversation(
      id: owner.id, channel: channel, name: name, owner: owner, messages: [])
  }

  private func identity(
    _ name: String, channel: String, lease: String
  ) throws -> OverlayIntercomCardIdentity {
    OverlayIntercomCardIdentity(
      digit: channel, conversation: try conversation(name, channel: channel, lease: lease))
  }

  /// The preview is the click (Maciej, 2026-10-22 14:27Z, relayed): a click
  /// opens that same card as the conversation, without a pinned preview.
  func testClickExpandsThatSameCard() throws {
    let lena = try identity("lena", channel: "2", lease: "a")
    let astra = try identity("astra", channel: "3", lease: "b")
    var presentation = OverlayIntercomPresentation()

    presentation.expand(astra)
    XCTAssertEqual(presentation.expanded, [astra])

    presentation.expand(lena)
    XCTAssertEqual(presentation.expanded, [lena], "one attached conversation at a time")
    XCTAssertFalse(presentation.folded)
  }

  /// Detaching left the presentation layer (2026-10-22): a detached fiszka is
  /// a durable OverlayIntercomInstance. Inside the group one conversation
  /// opens at a time; another agent's click replaces it, close returns it.
  func testGroupOpensOneConversationAndAnotherClickReplacesIt() throws {
    let lena = try identity("lena", channel: "2", lease: "a")
    let astra = try identity("astra", channel: "3", lease: "b")
    var presentation = OverlayIntercomPresentation()

    presentation.expand(lena)
    XCTAssertEqual(presentation.expanded, [lena])
    presentation.expand(astra)
    XCTAssertEqual(presentation.expanded, [astra], "one attached conversation at a time")
    presentation.close(astra)
    XCTAssertTrue(presentation.expanded.isEmpty)
  }

  /// A digit with no bound conversation can paint a capsule, never a composer.
  func testPlaceholderWithoutOwnerCannotOpenAConversation() {
    let placeholder = OverlayIntercomCardIdentity(digit: "4", conversation: nil)
    XCTAssertEqual(placeholder.name, "4")
    var presentation = OverlayIntercomPresentation()
    presentation.expand(placeholder)
    XCTAssertTrue(presentation.expanded.isEmpty)
  }

  /// A reused channel digit is another conversation: its draft and commands
  /// must not reach the previous owner's card.
  func testCardIdentityFollowsTheOwnerNotTheDigit() throws {
    let lena = try conversation("lena", channel: "3", lease: "a")
    let astra = try conversation("astra", channel: "3", lease: "b")
    let card = OverlayIntercomCardIdentity(digit: "3", conversation: lena)

    XCTAssertTrue(card.matches(lena))
    XCTAssertFalse(card.matches(astra))
    XCTAssertNotEqual(card, OverlayIntercomCardIdentity(digit: "3", conversation: astra))
    XCTAssertEqual(
      card, OverlayIntercomCardIdentity(digit: "3", conversation: lena, name: "Lena (renamed)"),
      "a display name is not identity")
    XCTAssertEqual(
      Set([card, OverlayIntercomCardIdentity(digit: "3", conversation: lena, name: "other")]).count,
      1)
  }

  /// Compact rest: the pill is as tall as the Intercom header at scale 1 and
  /// grows with text size; the desktop keeps a real gap between pills.
  func testColumnStaysOnEveryNormalDesktopWithRealGapsAtEveryScale() {
    XCTAssertEqual(Layout.rowHeight, Layout.headerHeight, "pills match the Intercom header")
    for scale in [CGFloat(1), 1.5, 2] {
      XCTAssertEqual(Layout.controlHeight(scale: scale), 30 * scale)
      for visible in [
        CGRect(x: 0, y: 0, width: 1440, height: 900),
        CGRect(x: -1920, y: 40, width: 1920, height: 1040),
        CGRect(x: 0, y: 0, width: 800, height: 450),
      ] {
        for count in 1...9 {
          let shown = Layout.visibleRange(
            count: count, offset: 0, visibleHeight: visible.height, scale: scale
          ).count
          XCTAssertGreaterThan(shown, 0)
          let header = Layout.clampedHeader(
            CGRect(x: visible.maxX + 600, y: visible.maxY + 200, width: 300, height: 30),
            count: shown, visibleFrame: visible, scale: scale)
          XCTAssertTrue(visible.contains(header))
          XCTAssertEqual(header.height, Layout.controlHeight(scale: scale))
          var previous = header
          for index in 0..<shown {
            let row = Layout.rowFrame(
              index: index, header: header, count: shown, visibleFrame: visible, scale: scale)
            XCTAssertEqual(row.height, Layout.controlHeight(scale: scale))
            XCTAssertTrue(visible.contains(row), "scale \(scale), \(count) agents, row \(index)")
            XCTAssertEqual(
              previous.minY - row.maxY, Layout.gap, accuracy: 0.001,
              "the desktop retains a real pointer gap")
            previous = row
          }
        }
      }
    }
  }

  /// Nine full-size pills cannot fit a short screen at a large text size;
  /// the column pages them instead of shrinking or leaving the display.
  func testVisibleRangePagesFullSizePillsOnShortDisplays() {
    XCTAssertEqual(
      Layout.visibleRange(count: 9, offset: 0, visibleHeight: 900, scale: 1), 0..<9)
    // 450 pt minus the 60 pt header holds six 60 pt rows with 4 pt gaps.
    let short = Layout.visibleRange(count: 9, offset: 0, visibleHeight: 450, scale: 2)
    XCTAssertEqual(short, 0..<6)
    XCTAssertEqual(
      Layout.visibleRange(count: 9, offset: 7, visibleHeight: 450, scale: 2), 3..<9,
      "the last agent stays reachable")
    XCTAssertEqual(
      Layout.visibleRange(count: 9, offset: -3, visibleHeight: 450, scale: 2), 0..<6)
    XCTAssertEqual(
      Layout.visibleRange(count: 0, offset: 0, visibleHeight: 450, scale: 1), 0..<0)
  }

  /// A click opens the conversation downward from the same card's top edge;
  /// the right edge is the anchor.
  func testTheConversationOpensDownFromTheSameCard() {
    let screen = CGRect(x: 0, y: 0, width: 1440, height: 900)
    for scale in [CGFloat(1), 1.5, 2] {
      let height = Layout.controlHeight(scale: scale)
      // High on the display, so even the scale-2 card fits below it.
      let row = CGRect(x: 1220, y: 860 - height, width: 212, height: height)

      let card = Layout.conversationFrame(
        below: row, size: Layout.conversationSize(scale: scale), visibleFrame: screen,
        scale: scale)
      XCTAssertEqual(card.maxX, row.maxX, accuracy: 0.001)
      XCTAssertEqual(card.maxY, row.maxY, accuracy: 0.001, "the card unrolls downward")
      XCTAssertGreaterThanOrEqual(card.width, Layout.minimumConversationSize(scale: scale).width)
      XCTAssertTrue(screen.contains(card))
    }

    let nearBottom = CGRect(x: 1220, y: 40, width: 212, height: 30)
    let clamped = Layout.conversationFrame(
      below: nearBottom, size: Layout.conversationSize(scale: 1), visibleFrame: screen, scale: 1)
    XCTAssertTrue(screen.contains(clamped), "a low card moves up rather than leave the display")
  }

  /// Every edge keeps the opposite one; the minimum keeps the composer usable.
  func testResizeHandlesKeepTheirAnchorsAndTheMinimum() {
    let screen = CGRect(x: 0, y: 0, width: 1440, height: 900)
    let frame = CGRect(x: 1000, y: 300, width: 380, height: 360)
    let minimum = Layout.minimumConversationSize(scale: 1)

    let wider = Layout.resizedFrame(
      frame, delta: CGSize(width: -100, height: 0), edge: .left, minimum: minimum,
      visibleFrame: screen)
    XCTAssertEqual(wider.minX, 900)
    XCTAssertEqual(wider.maxX, frame.maxX, "the right edge stays anchored")
    XCTAssertEqual(wider.height, frame.height)

    let narrow = Layout.resizedFrame(
      frame, delta: CGSize(width: 300, height: 0), edge: .left, minimum: minimum,
      visibleFrame: screen)
    XCTAssertEqual(narrow.width, minimum.width)
    XCTAssertEqual(narrow.maxX, frame.maxX)

    let taller = Layout.resizedFrame(
      frame, delta: CGSize(width: -50, height: -40), edge: .bottomLeft, minimum: minimum,
      visibleFrame: screen)
    XCTAssertEqual(taller.maxY, frame.maxY, "the top edge stays where the card hangs")
    XCTAssertEqual(taller.minY, 260)
    XCTAssertEqual(taller.minX, 950)

    let right = Layout.resizedFrame(
      frame, delta: CGSize(width: 500, height: 0), edge: .bottomRight, minimum: minimum,
      visibleFrame: screen)
    XCTAssertEqual(right.minX, frame.minX)
    XCTAssertEqual(right.maxX, screen.maxX, "a corner drag stops at the display edge")

    let side = Layout.resizedFrame(
      frame, delta: CGSize(width: 40, height: -30), edge: .right, minimum: minimum,
      visibleFrame: screen)
    XCTAssertEqual(side, CGRect(x: 1000, y: 300, width: 420, height: 360), "width only")
    let bottom = Layout.resizedFrame(
      frame, delta: CGSize(width: 40, height: -30), edge: .bottom, minimum: minimum,
      visibleFrame: screen)
    XCTAssertEqual(bottom, CGRect(x: 1000, y: 270, width: 380, height: 390), "height only")
    let top = Layout.resizedFrame(
      frame, delta: CGSize(width: 40, height: 30), edge: .top, minimum: minimum,
      visibleFrame: screen)
    XCTAssertEqual(top, CGRect(x: 1000, y: 300, width: 380, height: 390), "the bottom stays")
    let ceiling = Layout.resizedFrame(
      frame, delta: CGSize(width: 0, height: 900), edge: .top, minimum: minimum,
      visibleFrame: screen)
    XCTAssertEqual(ceiling.maxY, screen.maxY, "a top drag stops at the display edge")
    let floor = Layout.resizedFrame(
      frame, delta: CGSize(width: 0, height: -900), edge: .top, minimum: minimum,
      visibleFrame: screen)
    XCTAssertEqual(floor.height, minimum.height)
  }

  /// The resting width follows the measured text, not the old wide minimum.
  func testRestingWidthFitsTheContentWithoutAWideMinimum() {
    let width = Layout.restingWidth(headerText: 60, agentText: 40, scale: 1)
    XCTAssertEqual(width, 211, "the agent row reserves reception, status, focus, audio and gaps")
    XCTAssertLessThan(width, 250, "no artificial 250 pt floor")
    XCTAssertGreaterThan(
      Layout.restingWidth(headerText: 60, agentText: 200, scale: 1), width,
      "a longer name widens the group")
  }

  /// Maciej, 2026-10-22: channel 0 is pinned on top and channels 1–9 scroll
  /// under it. The pinned row's measured height shrinks the agents' viewport.
  func testPinnedAllShrinksTheAgentsViewportInsteadOfScrollingAway() {
    XCTAssertEqual(
      OverlayAgentMonitorLayout.viewportHeight(
        maximumHeight: 600, headerHeight: 22, pinnedHeight: 90, contentHeight: nil), 456)
    XCTAssertEqual(
      OverlayAgentMonitorLayout.viewportHeight(
        maximumHeight: 600, headerHeight: 22, pinnedHeight: 90, contentHeight: 200), 200)
    XCTAssertEqual(
      OverlayAgentMonitorLayout.viewportHeight(
        maximumHeight: 600, headerHeight: 22, pinnedHeight: 180, contentHeight: nil), 366,
      "larger text in All takes viewport height, never All's place")
    XCTAssertEqual(
      OverlayAgentMonitorLayout.viewportHeight(
        maximumHeight: 300, headerHeight: 22, pinnedHeight: 400, contentHeight: nil), 0)
  }

  /// Olga's P1, 2026-10-22: dictating into a visible card's draft keeps the
  /// main overlay closed. Every other draft capture keeps the existing show.
  func testDraftCaptureInAVisibleCardKeepsTheMainOverlayClosed() {
    typealias Presentation = OverlayIntercomPresentation
    XCTAssertFalse(
      Presentation.shouldShowMainOverlayForDraftCapture(
        verticalIntercomActive: true, mainPanelVisible: false,
        conversationPresentedInIntercom: true),
      "the card already presents this draft; no second window")
    XCTAssertTrue(
      Presentation.shouldShowMainOverlayForDraftCapture(
        verticalIntercomActive: false, mainPanelVisible: false,
        conversationPresentedInIntercom: false),
      "the horizontal strip keeps today's behaviour")
    XCTAssertTrue(
      Presentation.shouldShowMainOverlayForDraftCapture(
        verticalIntercomActive: true, mainPanelVisible: true,
        conversationPresentedInIntercom: true),
      "an already visible main overlay keeps showing its own composer")
    XCTAssertTrue(
      Presentation.shouldShowMainOverlayForDraftCapture(
        verticalIntercomActive: true, mainPanelVisible: false,
        conversationPresentedInIntercom: false),
      "a draft with no visible card still gets a window to speak into")
  }

  /// The All row sits in the pinned builder, outside the agents' ScrollView,
  /// and appears exactly once.
  func testAllRowIsBuiltOnceOutsideTheAgentsScroll() throws {
    let url = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens/Overlay/OverlayChannelStatusView.swift")
    let source = try String(contentsOf: url, encoding: .utf8)
    let start = try XCTUnwrap(source.range(of: "private func scaledMonitorBody("))
    let end = try XCTUnwrap(
      source.range(of: "var currentConversations", range: start.upperBound..<source.endIndex))
    let body = String(source[start.lowerBound..<end.lowerBound])
    let pinned = try XCTUnwrap(body.range(of: "} pinned: { rowFocus in"))
    let content = try XCTUnwrap(
      body.range(of: "} content: { rowFocus in", range: pinned.upperBound..<body.endIndex))
    let pinnedSection = body[pinned.upperBound..<content.lowerBound]
    let contentSection = body[content.upperBound...]
    XCTAssertTrue(pinnedSection.contains("broadcastRow(broadcast, rowFocus: rowFocus)"))
    XCTAssertFalse(contentSection.contains("broadcastRow("), "no second All inside the scroll")
    XCTAssertEqual(body.components(separatedBy: "broadcastRow(").count - 1, 1)
  }
}
