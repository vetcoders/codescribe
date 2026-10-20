import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Mounts the real Agent window composition. Symbols here exist on the
/// baseline, so root can run this file there: hidden title, unified toolbar,
/// full-height sidebar material and clear titlebar are measured on the live
/// window, with toolbar controls still clear of the traffic lights.
@MainActor
final class AgentWindowChromeTests: XCTestCase {
  private let sidebarKey = "AgentChat.sidebarExpanded.v1"
  private let panelKey = AgentContextPanelPreference.defaultsKey

  func testIntegratedHeaderAtMinimumAndNormalWidths() throws {
    let previous = UserDefaults.standard.object(forKey: sidebarKey)
    let previousPanel = UserDefaults.standard.object(forKey: panelKey)
    UserDefaults.standard.set(false, forKey: panelKey)
    defer {
      restoreSidebarDefault(previous)
      restore(previousPanel, forKey: panelKey)
    }

    for width in [AgentWindowMetrics.minWidth, 1120 as CGFloat] {
      for expanded in [true, false] {
        UserDefaults.standard.set(expanded, forKey: sidebarKey)
        let window = mount(contentWidth: width)
        defer {
          window.orderOut(nil)
          window.close()
        }
        let theme = window.contentView?.superview
        XCTAssertNotNil(theme, "width \(width) expanded \(expanded)")

        pump(window) {
          (window.toolbar?.items.compactMap(\.view).count ?? 0) >= 2
            && (!expanded || self.sidebarMaterial(in: window) != nil)
            && window.title == "Agent"
            && window.titleVisibility == .hidden
            && window.toolbarStyle == .unified
        }

        XCTAssertEqual(window.title, "Agent", "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.titleVisibility, .hidden, "width \(width) expanded \(expanded)")
        XCTAssertTrue(window.titlebarAppearsTransparent, "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.toolbarStyle, .unified, "width \(width) expanded \(expanded)")

        let split = splitView(in: window)
        let controller = split?.delegate as? NSSplitViewController
        let sidebar = controller?.splitViewItems.first { $0.behavior == .sidebar }
        XCTAssertEqual(sidebar?.behavior, .sidebar, "width \(width) expanded \(expanded)")
        XCTAssertEqual(
          sidebar?.minimumThickness, CSSidebarWidth.navigation.minimum,
          "width \(width) expanded \(expanded)")
        XCTAssertEqual(sidebar?.isCollapsed, !expanded, "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.contentMinSize.width, 640)
        if expanded, let sidebar {
          XCTAssertEqual(
            frame(sidebar.viewController.view).maxY, window.frame.height, accuracy: 1,
            "sidebar must extend through the titlebar at \(width)")
          XCTAssertEqual(
            frame(sidebar.viewController.view).minY, 0, accuracy: 1,
            "sidebar must reach the bottom edge at \(width)")
          let search = try XCTUnwrap(first(in: try XCTUnwrap(theme)) { $0 is NSSearchField })
          XCTAssertLessThanOrEqual(
            frame(search).maxY, window.contentLayoutRect.maxY + 1,
            "full-height material must not move search under the toolbar")
        }

        if expanded {
          let material = try XCTUnwrap(sidebarMaterial(in: window))
          XCTAssertEqual(material.material, .sidebar, "width \(width) expanded \(expanded)")
          XCTAssertEqual(
            material.blendingMode, .behindWindow, "width \(width) expanded \(expanded)")
          XCTAssertEqual(frame(material).maxY, window.frame.height, accuracy: 1)
          XCTAssertEqual(frame(material).minY, 0, accuracy: 1)
        }

        // SwiftUI toolbar children are not projected through AppKit AX in
        // XCTest. Measure the public NSToolbarItem views instead of treating
        // absent AX children as absent production controls.
        // Round 4 (UI-59): the conversation title moved to the header row of
        // the conversation column, so the bar holds brand and actions only.
        let controls = try XCTUnwrap(window.toolbar).items.compactMap(\.view)
        XCTAssertGreaterThanOrEqual(
          controls.count, 2, "brand and actions stay native toolbar clusters")
        XCTAssertGreaterThanOrEqual(
          controls.filter { frame($0).height >= 51 }.count, 2,
          "brand and actions share the three-line native header rhythm")
        let bounds = NSRect(origin: .zero, size: window.frame.size)
        for control in controls {
          let rect = frame(control)
          XCTAssertGreaterThan(rect.width, 1)
          XCTAssertGreaterThan(rect.height, 1)
          XCTAssertTrue(bounds.contains(rect), "toolbar cluster leaves window: \(rect)")
          XCTAssertGreaterThanOrEqual(rect.midY, window.contentLayoutRect.maxY - 1)
          for light in trafficLights(in: window) {
            XCTAssertFalse(rect.intersects(frame(light)), "cluster overlaps traffic lights")
          }
          let parent = try XCTUnwrap(control.superview)
          let point = parent.convert(NSPoint(x: rect.midX, y: rect.midY), from: nil)
          XCTAssertNotNil(control.hitTest(point), "cluster must retain native hit testing")
        }

      }
    }
  }

  func testSidebarMaterialFollowsReduceTransparencyAndIncreaseContrast() throws {
    for (reduced, contrast) in [(false, false), (true, false), (false, true)] {
      let host = NSHostingView(
        rootView: CSSidebarMaterial(
          reduceTransparency: reduced, contrast: contrast ? .increased : .standard))
      let window = NSWindow(
        contentRect: NSRect(x: 80, y: 80, width: 300, height: 400),
        styleMask: [.titled], backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      window.orderFrontRegardless()
      defer { window.close() }
      pump(window) { host.fittingSize.height > 0 }
      if reduced || contrast {
        XCTAssertNil(sidebarMaterial(in: window), "shared policy uses an opaque system canvas")
      } else {
        let effect = try XCTUnwrap(sidebarMaterial(in: window))
        XCTAssertEqual(effect.material, .sidebar)
        XCTAssertEqual(effect.blendingMode, .behindWindow)
        XCTAssertNil(effect.hitTest(NSPoint(x: 10, y: 10)))
      }
    }
  }

  func testMessagesExtendUnderTheFloatingComposer() throws {
    let previous = UserDefaults.standard.object(forKey: sidebarKey)
    let previousPanel = UserDefaults.standard.object(forKey: panelKey)
    UserDefaults.standard.set(true, forKey: sidebarKey)
    UserDefaults.standard.set(false, forKey: panelKey)
    defer {
      restoreSidebarDefault(previous)
      restore(previousPanel, forKey: panelKey)
    }
    let messages = (0..<30).map {
      ChatMessage(
        role: .assistant, timestamp: nil,
        text: "Turn \($0). " + String(repeating: "words ", count: 25))
    }
    let store = AgentChatStore(
      engine: nil, threads: [ChatThread(title: "Scroll fixture", meta: "", messages: messages)])
    let window = mount(contentWidth: 1120, store: store)
    defer { window.close() }
    let root = try XCTUnwrap(window.contentView)
    pump(window) {
      self.first(in: root) {
        ($0 as? NSTextView)?.accessibilityIdentifier() == "agent-composer-text"
      }
        != nil
    }
    let input = try XCTUnwrap(
      first(in: root) { ($0 as? NSTextView)?.accessibilityIdentifier() == "agent-composer-text" })
    let scroll = try XCTUnwrap(
      first(in: root) { view in
        guard let scroll = view as? NSScrollView, scroll.frame.height > 200 else { return false }
        return self.first(in: scroll) { $0 is NSTableView } == nil
      } as? NSScrollView)
    XCTAssertGreaterThan(scroll.documentView?.bounds.height ?? 0, scroll.contentSize.height)
    XCTAssertLessThan(
      frame(scroll).minY, frame(input).maxY,
      "conversation scroll surface must continue beneath the input, not stop above its strip")
    XCTAssertGreaterThanOrEqual(
      frame(scroll).maxY, window.contentLayoutRect.maxY,
      "messages must continue under the native header")
  }

  /// Maciej, 2026-10-14: the title bar sits only above the messages, never over
  /// the sidebar, as the overlay header's glass with a hard lower edge, and the
  /// same in Settings. An AppKit-hosted window draws no SwiftUI scroll edge
  /// effect under its toolbar (probe 2026-10-14), so the glass is the
  /// `CSDetailHeaderGlass` plane on each detail column. Window chrome does not
  /// render in an XCTest mount, so the structure is pinned at the source.
  func testHeaderGlassCoversOnlyTheDetailColumns() throws {
    let codescribe = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent()
      .appendingPathComponent("Codescribe")
    func source(_ path: String) throws -> String {
      try String(contentsOf: codescribe.appendingPathComponent(path), encoding: .utf8)
    }
    let agent = try source("Screens/AgentChat/AgentChatView.swift")
    let threadDetail = try XCTUnwrap(
      agent.range(of: "private struct ThreadDetail").map { String(agent[$0.lowerBound...]) })
    XCTAssertTrue(threadDetail.contains(".modifier(CSDetailHeaderGlass())"))
    XCTAssertEqual(agent.components(separatedBy: "CSDetailHeaderGlass").count - 1, 1)

    let settings = try source("Screens/Settings/SettingsView.swift")
    let detail = try XCTUnwrap(settings.range(of: "} detail: {"))
    let toolbar = try XCTUnwrap(
      settings.range(of: ".toolbar {", range: detail.upperBound..<settings.endIndex))
    XCTAssertTrue(settings[detail.upperBound..<toolbar.lowerBound].contains("CSDetailHeaderGlass"))
    XCTAssertFalse(
      settings[..<detail.lowerBound].contains("CSDetailHeaderGlass"), "never over the sidebar")

    let components = try source("DesignSystem/Components.swift")
    XCTAssertFalse(components.contains("scrollEdgeEffectStyle(.soft, for: .top)"))
  }

  // MARK: - Context panel (round 4, UI-59)

  /// Founder, 2026-10-20: optional, collapsible, closed by default, a fixed
  /// 260 pt column. The conversation keeps its own floor; an open panel
  /// raises the window floor by its footprint instead of squeezing the turns.
  func testContextPanelIsClosedByDefaultAndAFixedColumn() {
    XCTAssertEqual(AgentContextPanelPreference.defaultsKey, "AgentChat.contextPanelOpen.v1")
    XCTAssertFalse(AgentContextPanelPreference.defaultIsOpen)
    let defaults = UserDefaults(suiteName: UUID().uuidString)!
    XCTAssertFalse(
      defaults.bool(forKey: AgentContextPanelPreference.defaultsKey),
      "a missing flag reads as closed")

    XCTAssertEqual(AgentWindowMetrics.contextPanelWidth, 260)
    XCTAssertEqual(
      AgentWindowMetrics.contextPanelFootprint,
      AgentWindowMetrics.contextPanelWidth + AgentWindowMetrics.contextPanelDividerWidth)
    XCTAssertEqual(
      AgentWindowMetrics.windowFloorWidth(contextPanelOpen: false), AgentWindowMetrics.minWidth)
    XCTAssertEqual(
      AgentWindowMetrics.detailFloorWidth(contextPanelOpen: false),
      AgentWindowMetrics.detailMinWidth)
    XCTAssertEqual(
      AgentWindowMetrics.detailFloorWidth(contextPanelOpen: true)
        - AgentWindowMetrics.contextPanelFootprint,
      AgentWindowMetrics.detailMinWidth,
      "the conversation keeps the same floor with the panel open")
    XCTAssertEqual(
      AgentWindowMetrics.windowFloorWidth(contextPanelOpen: true)
        - AgentWindowMetrics.detailFloorWidth(contextPanelOpen: true),
      CSSidebarWidth.navigation.ideal,
      "window floor minus the opening rail is still exactly the detail floor")
    XCTAssertGreaterThanOrEqual(
      AgentWindowMetrics.detailMinWidth,
      ChatLayoutPolicy.minimumReadable + 2 * ChatLayoutPolicy.listPadding,
      "the conversation floor fits a readable bubble inside the list padding")
  }

  /// Opening and closing is view state: the thread, the draft, the model, the
  /// staged attachments, the recording and the speech state stay as they were.
  /// The Context and close buttons write the same persisted flag this test
  /// writes (pinned at the source below).
  func testTogglingTheContextPanelChangesOnlyItsFlag() throws {
    let previous = UserDefaults.standard.object(forKey: sidebarKey)
    let previousPanel = UserDefaults.standard.object(forKey: panelKey)
    UserDefaults.standard.set(true, forKey: sidebarKey)
    UserDefaults.standard.set(false, forKey: panelKey)
    defer {
      restoreSidebarDefault(previous)
      restore(previousPanel, forKey: panelKey)
    }
    let messages = [
      ChatMessage(role: .you, timestamp: nil, text: "Plan the project"),
      ChatMessage(role: .assistant, timestamp: nil, text: "Start from the goal."),
    ]
    let store = AgentChatStore(
      engine: nil,
      threads: [
        ChatThread(title: "Project plan", meta: "", messages: messages, model: "grok-4.5")
      ])
    store.draft = "half-written reply"
    let window = mount(contentWidth: 1120, store: store, renameTo: nil)
    defer { window.close() }
    let root = try XCTUnwrap(window.contentView)
    pump(window) { self.conversationScroll(in: root) != nil }
    let closedWidth = try XCTUnwrap(conversationScroll(in: root)).frame.width

    func snapshot() -> [String] {
      [
        String(describing: store.selectedThreadID),
        store.draft,
        "\(store.pendingAttachments.count)",
        String(describing: store.dictationPhase),
        String(describing: store.speakingMessageID),
      ]
        + store.threads.map {
          "\($0.id)|\($0.title)|\($0.model ?? "")|\($0.messages.count)|\($0.isFavorite)"
        }
    }
    let before = snapshot()

    UserDefaults.standard.set(true, forKey: panelKey)
    pump(window) {
      (self.conversationScroll(in: root)?.frame.width ?? closedWidth) < closedWidth - 100
    }
    let openWidth = try XCTUnwrap(conversationScroll(in: root)).frame.width
    XCTAssertEqual(
      openWidth, closedWidth - AgentWindowMetrics.contextPanelFootprint, accuracy: 2,
      "the open panel takes exactly its own column from the conversation")
    XCTAssertEqual(snapshot(), before, "opening the panel must not touch conversation state")

    UserDefaults.standard.set(false, forKey: panelKey)
    pump(window) {
      (self.conversationScroll(in: root)?.frame.width ?? 0) > closedWidth - 2
    }
    XCTAssertEqual(
      try XCTUnwrap(conversationScroll(in: root)).frame.width, closedWidth, accuracy: 2)
    XCTAssertEqual(snapshot(), before, "closing the panel must not touch conversation state")

    let codescribe = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent()
      .appendingPathComponent("Codescribe")
    let panel = try String(
      contentsOf: codescribe.appendingPathComponent("Screens/AgentChat/AgentContextPanel.swift"),
      encoding: .utf8)
    let toggle = try XCTUnwrap(
      panel.range(of: "struct AgentContextToggle").map { String(panel[$0.lowerBound...]) })
    XCTAssertTrue(toggle.contains("Button {\n      isOpen.toggle()\n    } label:"))
    let agent = try String(
      contentsOf: codescribe.appendingPathComponent("Screens/AgentChat/AgentChatView.swift"),
      encoding: .utf8)
    XCTAssertTrue(agent.contains("close: { isContextOpen = false }"))
  }

  /// At the window floor with the rail open, opening the panel widens the
  /// window to the raised floor: the composer stays whole inside the window and
  /// the conversation never drops below its own floor.
  func testOpenPanelAtTheWindowFloorKeepsComposerAndConversationFloor() throws {
    let previous = UserDefaults.standard.object(forKey: sidebarKey)
    let previousPanel = UserDefaults.standard.object(forKey: panelKey)
    UserDefaults.standard.set(true, forKey: sidebarKey)
    UserDefaults.standard.set(true, forKey: panelKey)
    defer {
      restoreSidebarDefault(previous)
      restore(previousPanel, forKey: panelKey)
    }
    let openFloor = AgentWindowMetrics.windowFloorWidth(contextPanelOpen: true)
    let window = mount(contentWidth: AgentWindowMetrics.minWidth)
    defer { window.close() }
    let root = try XCTUnwrap(window.contentView)
    pump(window) {
      window.contentRect(forFrameRect: window.frame).width >= openFloor - 1
        && self.composerInput(in: root) != nil
        && self.conversationScroll(in: root) != nil
    }
    XCTAssertEqual(window.contentMinSize.width, openFloor)
    XCTAssertGreaterThanOrEqual(
      window.contentRect(forFrameRect: window.frame).width, openFloor - 1,
      "a window below the raised floor widens when the panel opens")

    let input = try XCTUnwrap(composerInput(in: root))
    let bounds = NSRect(origin: .zero, size: window.contentRect(forFrameRect: window.frame).size)
    XCTAssertTrue(
      bounds.insetBy(dx: -1, dy: -1).contains(frame(input)),
      "composer must stay fully inside the window: \(frame(input))")
    let scroll = try XCTUnwrap(conversationScroll(in: root))
    XCTAssertGreaterThanOrEqual(scroll.frame.width, AgentWindowMetrics.detailMinWidth - 1)
    XCTAssertLessThanOrEqual(
      frame(input).maxX, frame(scroll).maxX + 1, "the panel never sits over the composer")
  }

  /// The panel is a fixed column: a short and a very long conversation title
  /// leave it exactly as wide. The long title wraps inside it.
  func testContextPanelDoesNotGrowWithItsContent() throws {
    let previous = UserDefaults.standard.object(forKey: sidebarKey)
    let previousPanel = UserDefaults.standard.object(forKey: panelKey)
    UserDefaults.standard.set(true, forKey: sidebarKey)
    UserDefaults.standard.set(true, forKey: panelKey)
    defer {
      restoreSidebarDefault(previous)
      restore(previousPanel, forKey: panelKey)
    }
    var panelWidths: [CGFloat] = []
    for title in ["Plan", String(repeating: "A very long conversation title ", count: 12)] {
      let store = AgentChatStore(
        engine: nil,
        threads: [ChatThread(title: title, meta: "", messages: [], model: "grok-4.5")])
      let window = mount(contentWidth: 1120, store: store, renameTo: nil)
      defer { window.close() }
      let root = try XCTUnwrap(window.contentView)
      pump(window) {
        self.conversationScroll(in: root) != nil && self.detailColumn(in: window) != nil
      }
      let detail = try XCTUnwrap(detailColumn(in: window))
      let scroll = try XCTUnwrap(conversationScroll(in: root))
      panelWidths.append(frame(detail).width - frame(scroll).width)
    }
    XCTAssertEqual(panelWidths.count, 2)
    for width in panelWidths {
      XCTAssertEqual(width, AgentWindowMetrics.contextPanelFootprint, accuracy: 2)
    }
    XCTAssertEqual(panelWidths[0], panelWidths[1], accuracy: 1, "a long title never widens it")
  }

  /// One identity plane: the toolbar holds no principal item, the header row
  /// in the conversation column carries title, "Agent · model" and Context,
  /// and the Voice Lab mark lives in that row instead of a corner overlay.
  func testConversationIdentityHasOneHome() throws {
    let codescribe = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent()
      .appendingPathComponent("Codescribe")
    func source(_ path: String) throws -> String {
      try String(contentsOf: codescribe.appendingPathComponent(path), encoding: .utf8)
    }
    let agent = try source("Screens/AgentChat/AgentChatView.swift")
    XCTAssertFalse(agent.contains("placement: .principal"))
    XCTAssertFalse(agent.contains("developerPowerCorner"))
    let header = try source("Screens/AgentChat/AgentContextPanel.swift")
    let row = try XCTUnwrap(
      header.range(of: "struct AgentConversationHeader").map { String(header[$0.lowerBound...]) })
    XCTAssertTrue(row.contains("DeveloperPowerMark()"))
    XCTAssertTrue(row.contains("AgentContextToggle(isOpen: $isContextOpen)"))
    XCTAssertTrue(row.contains("AgentLiveStatusPill(activity: store.selectedHeaderActivity)"))
    XCTAssertEqual(AgentConversationHeader.identityLine(model: " grok-4.5 "), "Agent · grok-4.5")
    XCTAssertEqual(AgentConversationHeader.identityLine(model: nil), "Agent")
    XCTAssertEqual(AgentConversationHeader.identityLine(model: "  "), "Agent")
  }

  /// Round 4 (UI-58): the "· streaming · attach file / image" strip under the
  /// field repeated what the controls already say; their tooltips and
  /// accessibility labels carry it now.
  func testComposerCarriesNoStaticHintStrip() throws {
    let composer = try String(
      contentsOf: URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Codescribe/Screens/AgentChat/Composer.swift"),
      encoding: .utf8)
    XCTAssertFalse(composer.contains("Composer hint"))
    XCTAssertFalse(composer.contains("ForEach(affordances"))
    XCTAssertTrue(composer.contains(".help(\"Attach an image (PNG, JPEG, GIF, WebP)\")"))
    XCTAssertTrue(
      composer.contains(".accessibilityLabel(Text(\"Attach an image (PNG, JPEG, GIF, WebP)\"))"))
    XCTAssertTrue(composer.contains(".help(micState.accessibilityLabel)"))
    XCTAssertTrue(composer.contains(".help(primaryAction.accessibilityLabel)"))
  }

  private func mount(
    contentWidth: CGFloat, store: AgentChatStore? = nil, renameTo title: String? = "Probe"
  )
    -> NSWindow
  {
    let store = store ?? AgentChatStore(engine: nil)
    if let title, let thread = store.currentThread {
      store.rename(thread, to: title)
    }
    let window = NSWindow(
      contentRect: NSRect(x: 80, y: 80, width: contentWidth, height: 720),
      styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
      backing: .buffered,
      defer: false
    )
    window.isReleasedWhenClosed = false
    window.contentViewController = NSHostingController(
      rootView: AgentChatView(store: store)
        .environment(\.csReduceTransparencyOverride, false)
        .environment(\.csContrastOverride, .standard)
    )
    window.setContentSize(NSSize(width: contentWidth, height: 720))
    window.setFrameOrigin(NSPoint(x: 80, y: 80))
    window.orderFrontRegardless()
    return window
  }

  private func restoreSidebarDefault(_ previous: Any?) {
    restore(previous, forKey: sidebarKey)
  }

  private func restore(_ previous: Any?, forKey key: String) {
    if let previous {
      UserDefaults.standard.set(previous, forKey: key)
    } else {
      UserDefaults.standard.removeObject(forKey: key)
    }
  }

  private func pump(_ window: NSWindow, until predicate: () -> Bool) {
    let deadline = Date().addingTimeInterval(2)
    while Date() < deadline {
      RunLoop.current.run(until: Date().addingTimeInterval(0.02))
      window.displayIfNeeded()
      if predicate() { return }
    }
  }

  private func splitView(in window: NSWindow) -> NSSplitView? {
    guard let content = window.contentView else { return nil }
    return first(in: content) { $0 is NSSplitView } as? NSSplitView
  }

  /// The detail column of the native split view (everything right of the rail).
  private func detailColumn(in window: NSWindow) -> NSView? {
    let controller = splitView(in: window)?.delegate as? NSSplitViewController
    return controller?.splitViewItems.first { $0.behavior != .sidebar }?.viewController.view
  }

  /// The conversation's scroll surface: tall, and not the rail's table.
  private func conversationScroll(in root: NSView) -> NSScrollView? {
    first(in: root) { view in
      guard let scroll = view as? NSScrollView, scroll.frame.height > 200 else { return false }
      return self.first(in: scroll) { $0 is NSTableView } == nil
    } as? NSScrollView
  }

  private func composerInput(in root: NSView) -> NSView? {
    first(in: root) {
      ($0 as? NSTextView)?.accessibilityIdentifier() == "agent-composer-text"
    }
  }

  private func sidebarMaterial(in window: NSWindow) -> NSVisualEffectView? {
    guard let content = window.contentView else { return nil }
    return first(in: content) { view in
      view is CSSidebarVisualEffectNSView
    } as? NSVisualEffectView
  }

  private func trafficLights(in window: NSWindow) -> [NSView] {
    [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton].compactMap {
      window.standardWindowButton($0)
    }
  }

  private func frame(_ view: NSView) -> NSRect {
    view.convert(view.bounds, to: nil)
  }

  private func first(in root: NSView, where matches: (NSView) -> Bool) -> NSView? {
    if matches(root) { return root }
    for child in root.subviews {
      if let found = first(in: child, where: matches) { return found }
    }
    return nil
  }
}
