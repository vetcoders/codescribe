import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

final class ChatLayoutPolicyTests: XCTestCase {
  func testContentWidthSubtractsListPadding() {
    XCTAssertEqual(ChatLayoutPolicy.contentWidth(for: 800), 760)
    XCTAssertEqual(ChatLayoutPolicy.contentWidth(for: 0), 0)
  }

  func testYouBubbleTracksContainerWithoutFixed760Cap() {
    // Narrow: floor at minimumReadable.
    XCTAssertEqual(
      ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: 320, mode: .wide),
      ChatLayoutPolicy.minimumReadable
    )

    // Mid column (~960): You stays chat-style (~72% of usable), which can be
    // *narrower* than the old fixed 760 cap — that is intentional.
    let mid = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: 960, mode: .wide)
    let usableMid = ChatLayoutPolicy.contentWidth(for: 960)
    XCTAssertEqual(mid, usableMid * ChatWidthMode.wide.youFraction, accuracy: 0.5)
    XCTAssertLessThan(mid, usableMid)

    // Wider viewport: proportional You exceeds the old 760 hard cap.
    let expanded = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: 1200, mode: .wide)
    XCTAssertGreaterThan(expanded, 760)

    // Ultrawide still never exceeds usable column.
    let wide = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: 1800, mode: .wide)
    XCTAssertLessThanOrEqual(wide, ChatLayoutPolicy.contentWidth(for: 1800))
  }

  func testLeadingColumnFillsUsableWidthUntilProseCap() {
    let laptop = ChatLayoutPolicy.leadingColumnMaxWidth(containerWidth: 960, mode: .wide)
    XCTAssertEqual(laptop, ChatLayoutPolicy.contentWidth(for: 960), accuracy: 0.5)
    XCTAssertGreaterThan(laptop, 900)  // old assistant hard cap

    let ultrawide = ChatLayoutPolicy.leadingColumnMaxWidth(containerWidth: 2200, mode: .wide)
    XCTAssertEqual(ultrawide, ChatWidthMode.wide.proseComfortCap)
  }

  func testWidthModesOrderComfortableThenWideThenFull() {
    let container: CGFloat = 1400
    let comfortable = ChatLayoutPolicy.leadingColumnMaxWidth(
      containerWidth: container,
      mode: .comfortable
    )
    let wide = ChatLayoutPolicy.leadingColumnMaxWidth(containerWidth: container, mode: .wide)
    let full = ChatLayoutPolicy.leadingColumnMaxWidth(containerWidth: container, mode: .full)
    XCTAssertLessThan(comfortable, wide)
    XCTAssertLessThan(wide, full)
    XCTAssertEqual(full, ChatLayoutPolicy.contentWidth(for: container), accuracy: 0.5)
  }

  func testYouBubbleModeFractionsScale() {
    let container: CGFloat = 1200
    let c = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: container, mode: .comfortable)
    let w = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: container, mode: .wide)
    let f = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: container, mode: .full)
    XCTAssertLessThan(c, w)
    XCTAssertLessThan(w, f)
  }

  func testResolveUnknownWidthModeFallsBackToWide() {
    XCTAssertEqual(ChatWidthMode.resolve("nope"), .wide)
    XCTAssertEqual(ChatWidthMode.resolve(ChatWidthMode.full.rawValue), .full)
  }

  func testDefaultsKeyIsStableForAppStorage() {
    XCTAssertEqual(ChatLayoutPolicy.defaultsKey, "codescribe.chatWidthMode")
  }

  // MARK: - Agent window floor (chrome density)

  func testAgentWindowFloorFitsExpandedRailAndReadableColumn() {
    XCTAssertEqual(AgentWindowMetrics.minWidth, 640)
    XCTAssertEqual(AgentWindowMetrics.minHeight, 440)
    let detailAtFloor =
      AgentWindowMetrics.minWidth - AgentSidebarMetrics.minimumWidth
    XCTAssertGreaterThanOrEqual(
      detailAtFloor,
      ChatLayoutPolicy.minimumReadable,
      "640pt floor must still fit the expanded rail min plus a readable detail column"
    )
    XCTAssertGreaterThan(
      AgentWindowMetrics.minWidth,
      AgentSidebarMetrics.maximumWidth,
      "window floor must stay wider than a fully dragged rail so native collapse is not the only way to keep detail visible"
    )
  }

  func testSidebarHasReadableFloorAndBoundedExpansion() {
    XCTAssertEqual(AgentSidebarMetrics.minimumWidth, 267)
    XCTAssertEqual(AgentSidebarMetrics.maximumWidth, 360)
  }

  @MainActor
  func testNativeSidebarItemEnforcesBoundsAfterWindowAttachment() throws {
    final class LayoutEngine: ChatEngineFixture {}
    let store = AgentChatStore(
      engine: LayoutEngine(),
      threads: [
        ChatThread(
          title: String(repeating: "Long thread title ", count: 8), meta: "now", model: "gpt-6-sol")
      ])
    let host = NSHostingController(
      rootView: AgentChatView(store: store)
        .transaction { $0.disablesAnimations = true }
    )
    let window = NSWindow(contentViewController: host)
    window.setContentSize(NSSize(width: 1120, height: 720))
    window.orderFrontRegardless()
    defer { window.orderOut(nil) }
    func find(_ view: NSView) -> NSSplitViewController? {
      if let split = view as? NSSplitView { return split.delegate as? NSSplitViewController }
      return view.subviews.lazy.compactMap { find($0) }.first
    }
    // The title string stays "Agent" and is hidden. The model reads beside
    // the thread title in the integrated toolbar.
    pumpUntil(ceiling: 1.0) {
      find(host.view) != nil && window.title == "Agent"
    }
    XCTAssertEqual(window.title, "Agent")
    let split = try XCTUnwrap(find(host.view), "Native split must be reachable after attachment")
    let item = try XCTUnwrap(split.splitViewItems.first(where: { $0.behavior == .sidebar }))
    XCTAssertEqual(item.minimumThickness, 267)
    XCTAssertEqual(item.maximumThickness, 360)

    let initialWidth = item.viewController.view.frame.width
    for title in ["Short", "Moderately descriptive title", String(repeating: "Long title ", count: 8)] {
      store.threads[0].title = title
      RunLoop.main.run(until: Date().addingTimeInterval(0.05))
      XCTAssertEqual(item.maximumThickness, 360, "titles cannot own the divider bounds")
      XCTAssertEqual(item.viewController.view.frame.width, initialWidth, accuracy: 1)
    }
    let retainedThreads = store.threads
    store.threads = []
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    XCTAssertEqual(item.maximumThickness, 360, "empty search keeps the same column bounds")
    store.threads = retainedThreads
    for windowWidth in [1120.0, 640.0, 1800.0, 800.0] {
      window.setContentSize(NSSize(width: windowWidth, height: 720))
      for proposed in [267.0, 300.0, 360.0, 900.0] {
        split.splitView.setPosition(proposed, ofDividerAt: 0)
        split.splitView.layoutSubtreeIfNeeded()
        pumpUntil(ceiling: 0.05) {
          let width = item.viewController.view.frame.width
          return width >= 266 && width <= 361
        }
        XCTAssertFalse(item.isCollapsed)
        XCTAssertGreaterThanOrEqual(item.viewController.view.frame.width, 266)
        XCTAssertLessThanOrEqual(item.viewController.view.frame.width, 361)
        XCTAssertGreaterThanOrEqual(
          split.splitViewItems[1].viewController.view.frame.width, 372)
      }
      item.isCollapsed = true
      pumpUntil(ceiling: 0.05) { item.isCollapsed }
      XCTAssertTrue(item.isCollapsed)
      XCTAssertEqual(window.contentMinSize.width, 640, "collapse retains the window floor")
      item.isCollapsed = false
      pumpUntil(ceiling: 0.05) { !item.isCollapsed }
      XCTAssertFalse(item.isCollapsed)
    }
  }

  /// One short slice, then return as soon as `ready` is true. The ceiling
  /// only caps a run that never becomes ready, so it is sized for a loaded
  /// gate host, not for the typical beat.
  @MainActor
  private func pumpUntil(ceiling: TimeInterval, _ ready: () -> Bool) {
    pumpUntil(deadline: Date().addingTimeInterval(ceiling), ready)
  }

  @MainActor
  private func pumpUntil(deadline: Date, _ ready: () -> Bool) {
    while Date() < deadline {
      let sliceEnd = min(deadline, Date().addingTimeInterval(0.008))
      RunLoop.main.run(mode: .default, before: sliceEnd)
      if ready() { return }
    }
  }

  // MARK: - Empty thread never scrolls (round 18)

  /// The empty block plus everything else in the scroll document must add up
  /// to exactly the viewport: list padding on both ends, one turn gap and the
  /// 1 pt live-edge anchor. The old reservation forgot the gap and the anchor
  /// and overshot by 17 pt — a scrollbar on a thread with no messages.
  func testEmptyStateFillsViewportWithoutOvershoot() {
    let viewport: CGFloat = 520
    let empty = ChatLayoutPolicy.emptyStateHeight(viewportHeight: viewport)
    let document =
      empty + ChatLayoutPolicy.turnSpacing + ChatLayoutPolicy.liveEdgeAnchorHeight
      + 2 * ChatLayoutPolicy.listPadding
    XCTAssertEqual(document, viewport)
    XCTAssertEqual(ChatLayoutPolicy.emptyStateHeight(viewportHeight: 10), 0, "never negative")
  }

  // MARK: - R1 window-collapse clamps

  func testDocumentWidthNeverExceedsContainer() {
    XCTAssertEqual(ChatLayoutPolicy.documentWidth(for: 800), 800)
    XCTAssertEqual(ChatLayoutPolicy.documentWidth(for: 320), 320)
    // Zero / unknown geometry falls back to the readable minimum so a
    // first-layout pass never hands children an unbounded width.
    XCTAssertEqual(ChatLayoutPolicy.documentWidth(for: 0), ChatLayoutPolicy.minimumReadable)
  }

  func testYouAndLeadingCapsStayInsideDocument() {
    let container: CGFloat = 900
    let document = ChatLayoutPolicy.documentWidth(for: container)
    let you = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: container, mode: .wide)
    let leading = ChatLayoutPolicy.leadingColumnMaxWidth(containerWidth: container, mode: .wide)
    XCTAssertLessThanOrEqual(you, document)
    XCTAssertLessThanOrEqual(leading, document)
    XCTAssertLessThanOrEqual(you, ChatLayoutPolicy.contentWidth(for: container))
    XCTAssertLessThanOrEqual(leading, ChatLayoutPolicy.contentWidth(for: container))
  }

  func testOversizedSelectionDispositionMatchesBubblePolicy() {
    // Context-chip selections share the same cap as bubble bodies so a
    // 100k pasted selection cannot expand the You turn unboundedly.
    let huge = String(repeating: "token)\" ", count: 20_000)
    XCTAssertTrue(OversizedBubblePolicy.isOversized(huge))
    XCTAssertFalse(
      OversizedBubblePolicy.disposition(utf8Count: huge.utf8.count)
        .sharesListSelectionOverlay
    )
  }

  func testAttachmentMissingPathDoesNotClaimLocalFile() {
    let missing = MessageAttachment(name: "scan.png", url: nil, type: "image/png")
    XCTAssertNil(missing.url)
    XCTAssertEqual(missing.name, "scan.png")
    XCTAssertEqual(missing.type, "image/png")
  }

  func testAttachmentWithPathKeepsMetadataForPreview() {
    let url = URL(fileURLWithPath: "/tmp/codescribe-preview-fixture.png")
    let att = MessageAttachment(name: "codescribe-preview-fixture.png", url: url)
    XCTAssertEqual(att.url, url)
    XCTAssertEqual(att.type, "image/png")
  }
}
