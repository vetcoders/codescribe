import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// One owner for the shared hint panel. Founder, 2026-11-07 (Z1): resting on ✦
/// flickered between the agents list and the "2 · nela" preview card. The roster
/// scrolls without a scroll clip, so a row scrolled out of its viewport kept a
/// live cursor region under ✦; and a card closed by another owner re-armed
/// itself while that region still read as hovered.
@MainActor
final class OverlayHintOwnershipTests: XCTestCase {
  private static let windowSize = NSSize(width: 900, height: 420)

  private func agent(_ digit: String, name: String) throws -> OverlayConversation {
    let sessionID = "ownership-\(digit)"
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": sessionID,
        "lease_id": String(repeating: digit, count: 32), "channel": digit, "name": name,
      ]))
    let reply = OverlayConversationMessage(
      id: "reply:ownership-\(digit)", kind: .reply,
      text: "\(name) reports a finished step.\n\nA second paragraph keeps the card tall.",
      order: 1, emittedAt: "2026-11-07T05:5\(digit):00Z", owner: owner, recipients: [],
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
    return OverlayConversation(
      id: owner.id, channel: digit, name: name, owner: owner, messages: [reply])
  }

  private func rosterState() throws -> (OverlayState, [OverlayConversation]) {
    let agents = try [
      agent("2", name: "nela"), agent("3", name: "Roman"), agent("4", name: "Tola"),
      agent("5", name: "Bruno"), agent("6", name: "teo"),
    ]
    let all = OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let inbox = OverlayConversation(
      id: "10", channel: "10", name: "Your Inbox", owner: nil, messages: [])
    let inventory = [all, inbox] + agents
    let state = OverlayState()
    state.applyChannelRoster(
      inventory.filter { $0.owner != nil || $0.channel == "0" }.map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: $0.owner == nil ? nil : true,
          unreadSinceUnixMs: nil)
      })
    state.applyConversationSnapshot(.init(deliveries: [], conversations: inventory))
    // The open conversation is Bruno's, as in the recording.
    state.selectConversation(agents[3].id)
    return (state, agents)
  }

  /// The expanded window as the user sees it: header with ✦, the conversation
  /// and the roster column. The roster's rows scroll up under the header.
  private func expandedWindow(_ state: OverlayState) -> (FloatingOverlayPanel, NSView) {
    let size = Self.windowSize
    let host = NSHostingView(
      rootView: AnyView(
        DictationOverlayView(state: state).frame(width: size.width, height: size.height)
          .symbolEffectsRemoved()
          .transaction { $0.disablesAnimations = true }))
    host.sizingOptions = []
    host.frame = NSRect(origin: .zero, size: size)
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(origin: NSPoint(x: 200, y: 200), size: size),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.contentView = host
    BackgroundTestWindow.present(panel)
    for _ in 0..<3 { settle(host, for: 0.1) }
    return (panel, host)
  }

  /// ✦ is the header control whose hover opens the agents list: a rich,
  /// interactive card that is no roster row's message. Probe each header
  /// control in turn; the bar has no other rich hint.
  private func starRegion(
    in panel: FloatingOverlayPanel, above roster: NSScrollView, cards: Set<String>
  ) async throws -> OverlayMenuCursorRegionView? {
    let viewport = roster.convert(roster.bounds, to: nil)
    let candidates = cursorRegions(of: panel).filter {
      !$0.isDescendant(of: roster) && $0.onHover != nil
        && $0.convert($0.bounds, to: nil).minY > viewport.maxY
        && $0.bounds.width < 60 && $0.bounds.height < 60
    }
    for candidate in candidates {
      let frame = candidate.convert(candidate.bounds, to: nil)
      move(panel, to: NSPoint(x: frame.midX, y: frame.midY))
      try await Task.sleep(for: .milliseconds(420))
      let rich = OverlayHintPanel.shared.presentedPanel?.hasShadow == true
      let title = OverlayHintPanel.shared.presentedTitle
      let expectedStarTitle = String(localized: "Show transcription")
      move(panel, to: NSPoint(x: -10_000, y: -10_000))
      try await Task.sleep(for: .milliseconds(320))
      if (rich || title == expectedStarTitle), let title, !cards.contains(title) {
        return candidate
      }
    }
    return nil
  }

  private func settle(_ view: NSView, for seconds: TimeInterval = 0.08) {
    view.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(seconds))
    view.layoutSubtreeIfNeeded()
  }

  private func scrollViews(in view: NSView) -> [NSScrollView] {
    (view as? NSScrollView).map { [$0] } ?? view.subviews.flatMap(scrollViews(in:))
  }

  /// FloatingOverlayPanel.sendEvent routes every mouse move to every
  /// registered region; replay that dispatch without touching NSCursor.
  private func move(_ panel: FloatingOverlayPanel, to point: NSPoint) {
    for region in cursorRegions(of: panel) { region.updateHover(at: point) }
  }

  private func cursorRegions(of panel: FloatingOverlayPanel) -> [OverlayMenuCursorRegionView] {
    panel.menuCursorRegions.allObjects.compactMap { $0 as? OverlayMenuCursorRegionView }
      .filter { $0.window === panel }
  }

  /// Z1 root cause: a roster row scrolled above its viewport is invisible but
  /// its cursor region stayed live under the header, ✦ included. Only ✦ may
  /// answer the pointer there, and the agents list it opens keeps the panel.
  func testStarAloneOwnsThePointerAndTheListWhileRosterRowsScrollUnderIt() async throws {
    let (state, agents) = try rosterState()
    let (panel, host) = expandedWindow(state)
    defer {
      move(panel, to: NSPoint(x: -10_000, y: -10_000))
      panel.contentView = nil
      panel.close()
    }
    // The roster: the rightmost scroll view whose rows overflow its frame.
    let roster = try XCTUnwrap(
      scrollViews(in: host).filter {
        ($0.documentView?.frame.height ?? 0) > $0.frame.height + 100
      }.max { $0.convert($0.bounds, to: nil).minX < $1.convert($1.bounds, to: nil).minX },
      "the roster overflows its column")
    let viewport = roster.convert(roster.bounds, to: nil)
    let cardTitles = Set(agents.compactMap { OverlayRosterMessageHint.fullText(in: $0) })
    let starView = try await starRegion(in: panel, above: roster, cards: cardTitles)
    let star = try XCTUnwrap(starView, "the header shows ✦ with its agents list")
    let starFrame = star.convert(star.bounds, to: nil)
    let probe = NSPoint(x: starFrame.midX, y: starFrame.midY)
    XCTAssertFalse(viewport.contains(probe), "✦ \(starFrame) lies above the roster \(viewport)")

    // Scroll until a row's region lies under ✦, as in the recording.
    let clip = roster.contentView
    let document = try XCTUnwrap(roster.documentView)
    var buried: [OverlayMenuCursorRegionView] = []
    var offset: CGFloat = 0
    while buried.isEmpty, offset < document.frame.height {
      offset += 10
      let y = document.isFlipped ? offset : document.frame.height - clip.bounds.height - offset
      clip.scroll(to: NSPoint(x: clip.bounds.minX, y: max(0, y)))
      roster.reflectScrolledClipView(clip)
      settle(host, for: 0.03)
      buried = cursorRegions(of: panel).filter {
        $0.isDescendant(of: roster) && $0.onHover != nil
          && $0.convert($0.bounds, to: nil).contains(probe)
      }
    }
    XCTAssertFalse(
      buried.isEmpty, "fixture: a scrolled-out roster row must lie under ✦ \(starFrame)")

    move(panel, to: probe)
    let hoveredRows = cursorRegions(of: panel).filter { $0.hovered && $0.isDescendant(of: roster) }
    XCTAssertTrue(
      hoveredRows.isEmpty,
      "a row outside the viewport cannot take the pointer: \(hoveredRows.map { $0.convert($0.bounds, to: nil) })"
    )
    XCTAssertTrue(star.hovered, "✦ answers the pointer")
    // No row anywhere under the header band answers it either.
    let header = NSRect(
      x: viewport.minX, y: viewport.maxY + 1, width: viewport.width,
      height: host.bounds.height - viewport.maxY - 1)
    var answering: NSPoint?
    for y in stride(from: header.minY + 2, to: header.maxY, by: 6) where answering == nil {
      for x in stride(from: header.minX + 2, to: header.maxX, by: 6) where answering == nil {
        move(panel, to: NSPoint(x: x, y: y))
        if cursorRegions(of: panel).contains(where: { $0.hovered && $0.isDescendant(of: roster) }) {
          answering = NSPoint(x: x, y: y)
        }
      }
    }
    XCTAssertNil(answering, "a scrolled-out row answers the pointer over the header")

    // The list opens after its pause and keeps the panel for a full second.
    move(panel, to: probe)
    try await Task.sleep(for: .milliseconds(500))
    let listTitle = try XCTUnwrap(
      OverlayHintPanel.shared.presentedTitle, "resting on ✦ opens the agents list")
    XCTAssertFalse(cardTitles.contains(listTitle), "✦ shows the list, not a row's card")
    let presented = OverlayHintPanel.shared.presentedPanel
    for _ in 0..<20 {
      try await Task.sleep(for: .milliseconds(55))
      move(panel, to: probe)
      XCTAssertEqual(OverlayHintPanel.shared.presentedTitle, listTitle, "the list keeps the panel")
      XCTAssertTrue(OverlayHintPanel.shared.presentedPanel === presented)
    }
  }

  /// Z1 re-arm: a card closed because another owner took the panel does not
  /// order itself again while its row still reads as hovered.
  func testRosterCardClosedByAnotherOwnerDoesNotReopenItself() async throws {
    let (_, agents) = try rosterState()
    let target = agents[0]
    let host = NSHostingView(
      rootView: AnyView(
        Text(verbatim: target.name).frame(maxWidth: .infinity, maxHeight: .infinity)
          .modifier(OverlayRosterMessageHint(conversation: target, palette: .dark, textScale: 1))))
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 320, height: 140)
    let other = NSView(frame: NSRect(x: 10, y: 10, width: 20, height: 20))
    host.addSubview(other)
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 300, y: 400, width: 320, height: 140),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.contentView = host
    BackgroundTestWindow.present(panel)
    settle(host, for: 0.06)
    let anchor = OverlayHintAnchor()
    anchor.view = other
    defer {
      anchor.hide()
      move(panel, to: NSPoint(x: -10_000, y: -10_000))
      panel.contentView = nil
      panel.close()
    }
    let row = try XCTUnwrap(
      cursorRegions(of: panel).max { $0.bounds.width < $1.bounds.width },
      "the row registers its hover region")
    let rowFrame = row.convert(row.bounds, to: nil)
    move(panel, to: NSPoint(x: rowFrame.midX, y: rowFrame.midY))
    let card = try XCTUnwrap(OverlayRosterMessageHint.fullText(in: target))
    for _ in 0..<40 where OverlayHintPanel.shared.presentedTitle != card {
      try await Task.sleep(for: .milliseconds(50))
    }
    XCTAssertEqual(OverlayHintPanel.shared.presentedTitle, card, "the hovered row opens its card")

    // Another owner takes the shared panel while the row is still hovered.
    anchor.show("Another owner", palette: .dark)
    XCTAssertEqual(OverlayHintPanel.shared.presentedTitle, "Another owner")
    try await Task.sleep(for: .milliseconds(600))
    XCTAssertEqual(
      OverlayHintPanel.shared.presentedTitle, "Another owner",
      "the closed card must not order itself again")
  }

  /// Z2 (Monika, 2026-11-07): resting on the bar's `(0)` pill opens channel
  /// 0's preview card, the roster's message carousel over the All
  /// conversation, also while All is the selected conversation. The bar
  /// shows no conversation, so the roster's "no duplicate preview for the
  /// open conversation" rule has nothing to duplicate there; it hid the card
  /// for good after one click on `(0)`. The card belongs to the pill, not to
  /// a row of the list under ✦, which has none for channel 0.
  func testBarChannelZeroPillOpensChannelZeroPreviewCard() async throws {
    let agents = try [agent("2", name: "nela"), agent("4", name: "Tola")]
    let body = "Z2 broadcast for everyone on channel zero."
    var broadcastNote = OverlayConversationMessage(
      id: "reply:z2-all", kind: .reply, text: body, order: 1,
      emittedAt: "2026-11-06T22:19:00Z", owner: agents[0].owner, recipients: [],
      deliveryID: nil, replyTo: nil, unsolicited: true, playback: nil, busPath: "")
    broadcastNote.sourceChannel = "2"
    let all = OverlayConversation(
      id: "0", channel: "0", name: "All", owner: nil, messages: [broadcastNote])
    let inventory = [all] + agents
    let state = OverlayState(micAccessProvider: { true })
    optIntoChannelMap(state)
    state.applyConversationSnapshot(
      .init(
        deliveries: agents.map {
          OverlayChannelDelivery(
            channel: $0.channel, agent: $0.name, deliveryID: "z2-\($0.channel)",
            stage: .received, isOpen: false)
        }, conversations: inventory))
    state.applyChannelRoster(
      inventory.map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
      })
    // A click on (0) selects All, as in Monika's running build.
    state.selectConversation(all.id, expand: false)
    state.setPresentationMode(.midi)
    XCTAssertTrue(state.channelMapInHeader, "the bar shows its pills")
    XCTAssertEqual(state.selectedConversationID, all.id, "fixture: All is selected")
    let zero = try XCTUnwrap(
      DictationOverlayView(state: state).channelStatusView.channelMapModel.chips.first)
    XCTAssertEqual(zero.digit, "0", "(0) leads the pills")
    XCTAssertTrue(zero.selected, "fixture: the bar draws (0) as the selected pill")

    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 200, y: 200, width: 410, height: 46),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.setChannelMapPresentation(enabled: state.channelMapInMini)
    let geometry = DictationOverlayWindow.midiBarGeometry(
      map: state.channelMapLayoutModel, textScale: 1,
      visibleWidth: NSScreen.main?.visibleFrame.width)
    panel.setMidiBarGeometry(geometry)
    let host = NSHostingView(
      rootView: AnyView(
        DictationOverlayView(state: state)
          .symbolEffectsRemoved()
          .transaction { $0.disablesAnimations = true }))
    host.sizingOptions = []
    panel.contentView = OverlayContentContainer(hosting: host)
    BackgroundTestWindow.present(panel)
    panel.setFrame(NSRect(origin: NSPoint(x: 200, y: 200), size: panel.midiSize), display: true)
    for _ in 0..<3 { settle(host, for: 0.1) }
    defer {
      move(panel, to: NSPoint(x: -10_000, y: -10_000))
      panel.contentView = nil
      panel.close()
    }

    // (0) leads the pills: the leftmost pill-sized region inside the bar.
    // Header controls are 22 pt, the close dot 9 pt; a pill is wider.
    let zeroRegion = try XCTUnwrap(
      cursorRegions(of: panel).filter {
        let frame = $0.convert($0.bounds, to: nil)
        return $0.onHover != nil && frame.width >= 40 && host.bounds.intersects(frame)
      }.min { $0.convert($0.bounds, to: nil).minX < $1.convert($1.bounds, to: nil).minX },
      "the bar registers the (0) pill's pointer region")
    let frame = zeroRegion.convert(zeroRegion.bounds, to: nil)
    move(panel, to: NSPoint(x: frame.midX, y: frame.midY))
    XCTAssertTrue(zeroRegion.hovered, "(0) answers the pointer")
    // The carousel opens after its 350 ms pause.
    for _ in 0..<30 where OverlayHintPanel.shared.presentedTitle?.contains(body) != true {
      try await Task.sleep(for: .milliseconds(50))
    }
    XCTAssertEqual(
      OverlayHintPanel.shared.presentedPanel?.hasShadow, true,
      "resting on (0) at \(frame) opens a card")
    XCTAssertTrue(
      OverlayHintPanel.shared.presentedTitle?.contains(body) == true,
      "the card is channel 0's message carousel: \(OverlayHintPanel.shared.presentedTitle ?? "none")"
    )
  }
}
