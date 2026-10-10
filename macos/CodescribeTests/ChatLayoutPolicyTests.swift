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
    // The titlebar carries only the app identity; the model reads in the
    // chrome beside the thread title (Founder brief 2026-10-10).
    pumpUntil(ceiling: 1.0) {
      find(host.view) != nil && window.title == "Agent"
    }
    XCTAssertEqual(window.title, "Agent")
    let split = try XCTUnwrap(find(host.view), "Native split must be reachable after attachment")
    let item = try XCTUnwrap(split.splitViewItems.first(where: { $0.behavior == .sidebar }))
    XCTAssertEqual(item.minimumThickness, 267)
    XCTAssertEqual(item.maximumThickness, 360)

    store.threads[0].title = "Short"
    pumpUntil(ceiling: 1.0) { item.maximumThickness == 267 }
    XCTAssertEqual(item.maximumThickness, 267)
    XCTAssertLessThanOrEqual(item.viewController.view.frame.width, 268)
    let shortWidth = item.viewController.view.frame.width
    store.threads[0].title = "Moderately descriptive thread title"
    pumpUntil(ceiling: 1.0) {
      item.maximumThickness > 267 && item.maximumThickness < 360
    }
    XCTAssertGreaterThan(item.maximumThickness, 267)
    XCTAssertLessThan(
      item.maximumThickness, 360,
      "Intrinsic measurement must produce intermediate widths, not only floor/ceiling buckets")
    store.threads[0].title = String(repeating: "Long thread title ", count: 8)
    pumpUntil(ceiling: 1.0) { item.maximumThickness == 360 }
    XCTAssertEqual(item.maximumThickness, 360)
    XCTAssertEqual(
      item.viewController.view.frame.width, shortWidth, accuracy: 1,
      "A wider content cap must not expand the user's divider")
    store.threads[0].title = "Short again"
    pumpUntil(ceiling: 1.0) { item.maximumThickness == 267 }
    XCTAssertEqual(item.maximumThickness, 267)
    store.threads[0].model = String(repeating: "model-name-", count: 10)
    pumpUntil(ceiling: 1.0) { item.maximumThickness == 360 }
    XCTAssertEqual(item.maximumThickness, 360, "Metadata participates in intrinsic row width")
    let retainedThreads = store.threads
    let sidebarBeforeEmpty = sidebarSignature(item.viewController.view)
    store.threads = []
    // Thickness is already 360. Leaving on that value would skip the update
    // that this assertion exists to watch. Wait until the rail actually
    // changes, then one more turn inside the same 0.15s ceiling.
    let emptyDeadline = Date().addingTimeInterval(0.15)
    pumpUntil(deadline: emptyDeadline) {
      sidebarSignature(item.viewController.view) != sidebarBeforeEmpty
    }
    if Date() < emptyDeadline {
      RunLoop.main.run(
        mode: .default,
        before: min(emptyDeadline, Date().addingTimeInterval(0.016)))
    }
    XCTAssertEqual(
      item.maximumThickness, 360, "Transient empty search results must not reset the cap")
    store.threads = retainedThreads
    store.threads[0].title = String(repeating: "Long thread title ", count: 8)
    pumpUntil(ceiling: 1.0) { item.maximumThickness == 360 }
    for windowWidth in [1120.0, 640.0, 1800.0, 800.0] {
      window.setContentSize(NSSize(width: windowWidth, height: 720))
      for proposed in [1600.0, 50.0, 300.0, 900.0, 0.0] {
        let beforeWidth = item.viewController.view.frame.width
        split.splitView.setPosition(proposed, ofDividerAt: 0)
        split.splitView.layoutSubtreeIfNeeded()
        let target = min(
          AgentSidebarMetrics.maximumWidth, max(AgentSidebarMetrics.minimumWidth, proposed))
        pumpUntil(ceiling: 0.05) {
          let width = item.viewController.view.frame.width
          return abs(width - beforeWidth) > 0.5 || abs(width - target) <= 1
        }
        let width = item.viewController.view.frame.width
        XCTAssertFalse(item.isCollapsed)
        XCTAssertGreaterThanOrEqual(width, 266)
        XCTAssertLessThanOrEqual(width, 361)
        XCTAssertGreaterThanOrEqual(split.splitViewItems[1].viewController.view.frame.width, 319)
      }
      item.isCollapsed = true
      pumpUntil(ceiling: 0.05) { item.isCollapsed }
      XCTAssertTrue(item.isCollapsed)
      item.isCollapsed = false
      pumpUntil(ceiling: 0.05) {
        let width = item.viewController.view.frame.width
        return !item.isCollapsed && width >= 266 && width <= 361
      }
      XCTAssertGreaterThanOrEqual(item.viewController.view.frame.width, 266)
      XCTAssertLessThanOrEqual(item.viewController.view.frame.width, 361)
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

  /// Sidebar tree identity. Used only so an already-true thickness of 360
  /// cannot satisfy the empty-search wait before the rail updates.
  @MainActor
  private func sidebarSignature(_ view: NSView) -> Int {
    var hasher = Hasher()
    func walk(_ node: NSView) {
      hasher.combine(ObjectIdentifier(type(of: node)))
      hasher.combine(node.subviews.count)
      hasher.combine(Int(node.frame.width.rounded()))
      hasher.combine(Int(node.frame.height.rounded()))
      if let field = node as? NSTextField {
        hasher.combine(field.stringValue)
      }
      for child in node.subviews {
        walk(child)
      }
    }
    walk(view)
    return hasher.finalize()
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
