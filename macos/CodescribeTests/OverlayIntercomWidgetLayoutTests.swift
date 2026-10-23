import AppKit
import XCTest

@testable import Codescribe

final class OverlayIntercomWidgetLayoutTests: XCTestCase {
  private typealias Layout = OverlayIntercomWidgetLayout

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

  func testCardHeightIsOneRowOrTwoNeverAnEmptyBottom() {
    for scale in [CGFloat(0.8), 1, 1.6] {
      let metrics = IntercomContentMetrics(names: ["Lena"], digits: ["2"], scale: scale)
      let stacked = metrics.stackedActionsWidth
      let icon = metrics.iconSide
      for requested in [icon, 1.49 * icon, 0] {
        XCTAssertEqual(
          metrics.fittedCardHeight(requested, width: stacked, count: 3, available: 2000), icon)
      }
      for requested in [1.5 * icon, 2 * icon, 600] {
        XCTAssertEqual(
          metrics.fittedCardHeight(requested, width: stacked, count: 3, available: 2000), 2 * icon)
      }
      for width in [icon, metrics.actionsWidth, 1200] {
        XCTAssertEqual(
          metrics.fittedCardHeight(600, width: width, count: 3, available: 2000), icon,
          "only stacked controls fill a second row")
      }
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

  func testScreenHeightLimitsControlStackingWithoutEnlargingTheGlyphs() {
    let metrics = IntercomContentMetrics(names: ["Lena"], digits: ["2"], scale: 1)
    let minimum = metrics.minimumGroupSize(count: 4)
    let width = metrics.stackedActionsWidth
    let row = metrics.fittedCardHeight(200, width: width, count: 4, available: minimum.height)
    XCTAssertEqual(row, metrics.iconSide)
    XCTAssertEqual(metrics.tier(width: metrics.nameWidth, cardHeight: row), .name)
    let tall = metrics.fittedCardHeight(200, width: width, count: 4, available: 700)
    XCTAssertGreaterThanOrEqual(tall, 2 * metrics.iconSide)
    let screen = CGRect(x: 0, y: 0, width: 1000, height: 700)
    let header = Layout.clampedHeader(
      CGRect(x: 900, y: 10, width: 200, height: 30),
      count: 4, visibleFrame: screen, cardHeight: tall)
    for index in 0..<4 {
      let frame = Layout.rowFrame(
        index: index, header: header, count: 4,
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
            CGRect(x: visible.maxX + 600, y: visible.minY - 200, width: 300, height: 30),
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

  /// Left edge and bottom corners resize; the minimum keeps the composer usable.
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

  /// A row torn off the group becomes a mini under the hand holding it, and
  /// the conversation it unfolds to keeps the mini's top-right corner, so the
  /// window that shows the mini puts it exactly where the hand let go.
  func testATornOffRowLandsUnderThePointerAndRestoresAroundIt() {
    let row = CGRect(x: 1000, y: 600, width: 260, height: 48)
    let size = CGSize(width: 180, height: 30)
    let pointer = CGPoint(x: 1040, y: 620)
    let tile = Layout.tornOffTile(row: row, pointer: pointer, size: size)
    XCTAssertEqual(tile.size, size)
    XCTAssertTrue(tile.contains(pointer), "the pointer holds the tile it tore off")
    XCTAssertEqual(tile.minX, pointer.x - 16)

    let nearRightEdge = Layout.tornOffTile(
      row: row, pointer: CGPoint(x: 1255, y: 640), size: size)
    XCTAssertEqual(nearRightEdge.maxX, row.maxX, "the tile stays inside the row's span")
    XCTAssertEqual(nearRightEdge.maxY, row.maxY, "and never rises above the row")

    let iconRow = CGRect(x: 1200, y: 600, width: 40, height: 40)
    let wide = Layout.tornOffTile(row: iconRow, pointer: CGPoint(x: 1220, y: 620), size: size)
    XCTAssertEqual(wide.maxX, iconRow.maxX, "a mini wider than an icon row grows to the left")

    let beside = Layout.tornOffTile(row: row, pointer: nil, size: size)
    XCTAssertEqual(beside.maxX, row.minX - Layout.gap, "without a hand it lands beside the group")
    XCTAssertEqual(beside.maxY, row.maxY)

    let restored = Layout.restoredFrame(forTile: tile, scale: 1)
    XCTAssertEqual(restored.size, Layout.conversationSize(scale: 1))
    XCTAssertEqual(restored.maxX, tile.maxX)
    XCTAssertEqual(restored.maxY, tile.maxY)
    let screen = CGRect(x: 0, y: 0, width: 1728, height: 1080)
    XCTAssertEqual(
      Layout.cardFrame(
        below: CGRect(
          x: restored.maxX - size.width, y: restored.maxY, width: size.width, height: 0),
        size: size, visibleFrame: screen),
      tile, "the mini drawn from the restored frame is the torn tile")
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
