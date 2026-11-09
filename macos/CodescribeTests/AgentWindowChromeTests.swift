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

  func testHeaderAttachmentRetriesWithoutReplacingAnotherOwner() throws {
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 900, height: 600),
      styleMask: [.titled, .closable], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    let coordinator = CSWindowTopHeader<Text>.Coordinator(header: Text(verbatim: "Header probe"))
    defer {
      coordinator.detach()
      window.close()
    }

    coordinator.attach(to: window)
    XCTAssertTrue(window.titlebarAccessoryViewControllers.isEmpty)
    window.styleMask.insert(.fullSizeContentView)
    let otherOwner = NSTitlebarAccessoryViewController()
    otherOwner.layoutAttribute = .top
    otherOwner.view = NSView(frame: NSRect(x: 0, y: 0, width: 900, height: 52))
    window.addTitlebarAccessoryViewController(otherOwner)
    coordinator.attach(to: window)
    XCTAssertEqual(window.titlebarAccessoryViewControllers.count, 1)
    XCTAssertTrue(window.titlebarAccessoryViewControllers.first === otherOwner)

    window.removeTitlebarAccessoryViewController(at: 0)
    coordinator.attach(to: window)
    coordinator.attach(to: window)
    XCTAssertEqual(window.titlebarAccessoryViewControllers.count, 1)
    XCTAssertTrue(window.titlebarAccessoryViewControllers.first === coordinator.accessory)
    window.titlebarSeparatorStyle = .automatic
    NotificationCenter.default.post(name: NSWindow.didUpdateNotification, object: window)
    XCTAssertEqual(window.titlebarSeparatorStyle, .none, "toolbar adoption cannot restore a seam")
    coordinator.detach()
    XCTAssertTrue(window.titlebarAccessoryViewControllers.isEmpty)
    coordinator.attach(to: window)
    XCTAssertEqual(window.titlebarAccessoryViewControllers.count, 1)
  }

  /// The approved shape is one rectangular real-content glass view across
  /// the entire upper detail region, with the native window controls intact.
  func testTrailingTitlebarOwnerKeepsOneBandAndNativeWindowButtons() throws {
    let previousSidebar = UserDefaults.standard.object(forKey: sidebarKey)
    UserDefaults.standard.set(true, forKey: sidebarKey)
    defer {
      restoreSidebarDefault(previousSidebar)
    }
    let window = mount(contentWidth: 1120)
    defer { window.close() }
    pump(window) {
      guard let glass = self.rectangularHeaderGlass(in: window),
        let sidebar = (self.splitView(in: window)?.delegate as? NSSplitViewController)?
          .splitViewItems.first
      else { return false }
      return !sidebar.isCollapsed
        && abs(self.visibleHeaderFrame(glass).minX - self.frame(sidebar.viewController.view).maxX)
          < 1
        && abs(self.visibleHeaderFrame(glass).maxX - window.frame.width) < 1
    }
    let owners = window.titlebarAccessoryViewControllers.filter { $0.layoutAttribute == .right }
    XCTAssertEqual(owners.count, 1)
    let owner = try XCTUnwrap(owners.first)
    if #available(macOS 26.1, *) {
      XCTAssertEqual(owner.preferredScrollEdgeEffectStyle, .soft)
    }
    let glass = try XCTUnwrap(rectangularHeaderGlass(in: window))
    let rect = visibleHeaderFrame(glass)
    let sidebar = try XCTUnwrap(
      (splitView(in: window)?.delegate as? NSSplitViewController)?.splitViewItems.first)
    let lights = trafficLights(in: window)
    let attachment = XCTAttachment(
      string: "window=\(window.frame) layout=\(window.contentLayoutRect) "
        + "top=\(frame(owner.view)) glass=\(rect) lights=\(lights.map { frame($0) })")
    attachment.name = "Rectangular titlebar owner"
    attachment.lifetime = .keepAlways
    add(attachment)
    let layerEvidence = XCTAttachment(
      string: try headerJoinLayers(
        in: window, boundary: frame(sidebar.viewController.view).maxX))
    layerEvidence.name = "Header join compositing layers"
    layerEvidence.lifetime = .keepAlways
    add(layerEvidence)
    let toolbarEvidence = XCTAttachment(
      string: String(
        decoding: try JSONSerialization.data(
          withJSONObject: window.toolbar?.items.map {
            [
              "identifier": $0.itemIdentifier.rawValue,
              "class": String(describing: type(of: $0)),
              "trackingSeparator": $0 is NSTrackingSeparatorToolbarItem,
            ] as [String: Any]
          } ?? [], options: [.prettyPrinted, .sortedKeys]
        ), as: UTF8.self))
    toolbarEvidence.name = "Public toolbar ownership JSON"
    toolbarEvidence.lifetime = .keepAlways
    add(toolbarEvidence)
    XCTAssertEqual(frame(owner.view).height, 52, accuracy: 1)
    XCTAssertEqual(rect.height, 52, accuracy: 1)
    XCTAssertEqual(rect.maxY, window.frame.height, accuracy: 1)
    XCTAssertEqual(rect.minX, frame(sidebar.viewController.view).maxX, accuracy: 1)
    XCTAssertEqual(rect.maxX, window.frame.width, accuracy: 1)
    XCTAssertEqual(window.contentLayoutRect.maxY, window.frame.height - 52, accuracy: 1)
    XCTAssertEqual(glass.cornerRadius, 0)
    XCTAssertEqual(glass.style, .regular)
    let viewport = try XCTUnwrap(glass.superview)
    XCTAssertTrue(viewport.clipsToBounds)
    XCTAssertLessThan(frame(glass).minY, frame(viewport).minY)
    XCTAssertLessThan(frame(glass).minX, frame(viewport).minX)
    XCTAssertEqual(frame(glass).maxX, frame(viewport).maxX, accuracy: 0.5)
    XCTAssertEqual(frame(glass).maxY, frame(viewport).maxY, accuracy: 0.5)
    let controls = try XCTUnwrap(
      first(in: glass) { $0.identifier?.rawValue == "codescribe-header-content" })
    XCTAssertEqual(frame(controls).minY, rect.minY, accuracy: 0.5)
    XCTAssertEqual(frame(controls).maxY, rect.maxY, accuracy: 0.5)
    XCTAssertEqual(frame(controls).minX, rect.minX, accuracy: 0.5)
    XCTAssertEqual(frame(controls).maxX, rect.maxX, accuracy: 0.5)
    XCTAssertEqual(window.titlebarSeparatorStyle, .none)
    XCTAssertNotNil(glass.contentView, "the real information and actions belong inside glass")
    XCTAssertEqual(lights.count, 3)
    XCTAssertTrue(lights.allSatisfy { !$0.isHiddenOrHasHiddenAncestor })
  }

  func testRectangularHeaderAtMinimumAndNormalWidths() throws {
    let previousSidebar = UserDefaults.standard.object(forKey: sidebarKey)
    defer {
      restoreSidebarDefault(previousSidebar)
    }
    for width in [AgentWindowMetrics.minWidth, 1120 as CGFloat] {
      for expanded in [true, false] {
        UserDefaults.standard.set(expanded, forKey: sidebarKey)
        let store = AgentChatStore(
          engine: nil,
          threads: [
            ChatThread(
              title: String(repeating: "Long conversation title ", count: 6),
              meta: "synthetic", messages: [])
          ])
        let window = mount(contentWidth: width, store: store)
        defer { window.close() }
        pump(window) {
          guard let glass = self.rectangularHeaderGlass(in: window) else { return false }
          return self.visibleHeaderFrame(glass).width > 200
            && self.visibleHeaderFrame(glass).maxX >= width - 1
        }
        let glass = try XCTUnwrap(rectangularHeaderGlass(in: window))
        let rect = visibleHeaderFrame(glass)
        let attachment = XCTAttachment(
          string: "requestedWidth=\(width) expanded=\(expanded) window=\(window.frame) "
            + "layout=\(window.contentLayoutRect) glass=\(rect)")
        attachment.name = "Rectangular header \(width) expanded \(expanded)"
        attachment.lifetime = .keepAlways
        add(attachment)
        XCTAssertEqual(
          window.titlebarAccessoryViewControllers.filter { $0.layoutAttribute == .right }.count, 1)
        XCTAssertEqual(rect.maxX, window.frame.width, accuracy: 1)
        XCTAssertEqual(rect.minY, window.contentLayoutRect.maxY, accuracy: 1)
        XCTAssertEqual(rect.height, 52, accuracy: 1)
        XCTAssertEqual(glass.cornerRadius, 0)
        for light in trafficLights(in: window) {
          XCTAssertFalse(
            rect.intersects(frame(light)), "detail glass covers a native window button")
        }
      }
    }
  }

  private func nativeSidebarGlass(in window: NSWindow) -> NSGlassEffectView? {
    guard let content = window.contentView else { return nil }
    return first(in: content) { $0 is NSGlassEffectView } as? NSGlassEffectView
  }

  private func rectangularHeaderGlass(in window: NSWindow) -> NSGlassEffectView? {
    guard let theme = window.contentView?.superview else { return nil }
    return first(in: theme) { $0.identifier?.rawValue == "codescribe-header-glass" }
      as? NSGlassEffectView
  }

  func testIntegratedHeaderAtMinimumAndNormalWidths() throws {
    let previous = UserDefaults.standard.object(forKey: sidebarKey)
    defer {
      restoreSidebarDefault(previous)
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
          self.rectangularHeaderGlass(in: window) != nil
            && (!expanded || self.nativeSidebarGlass(in: window) != nil)
            && window.title == "Agent"
            && window.titleVisibility == .hidden
            && window.toolbarStyle == .unified
        }

        XCTAssertEqual(window.title, "Agent", "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.titleVisibility, .hidden, "width \(width) expanded \(expanded)")
        XCTAssertTrue(window.titlebarAppearsTransparent, "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.toolbarStyle, .unified, "width \(width) expanded \(expanded)")
        XCTAssertEqual(window.titlebarSeparatorStyle, .none)

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
          let material = try XCTUnwrap(nativeSidebarGlass(in: window))
          XCTAssertNil(
            sidebarMaterial(in: window), "no second material inside system sidebar glass")
          XCTAssertEqual(frame(material).maxY, window.frame.height, accuracy: 1)
          XCTAssertEqual(frame(material).minY, 0, accuracy: 1)
        }

        let glass = try XCTUnwrap(rectangularHeaderGlass(in: window))
        let content = try XCTUnwrap(glass.contentView)
        let rect = visibleHeaderFrame(glass)
        XCTAssertEqual(rect.maxX, window.frame.width, accuracy: 1)
        XCTAssertEqual(rect.midY, window.frame.height - 26, accuracy: 1)
        XCTAssertEqual(rect.height, 52, accuracy: 1)
        for light in trafficLights(in: window) {
          XCTAssertFalse(rect.intersects(frame(light)), "header overlaps native window controls")
        }
        let point = glass.convert(NSPoint(x: rect.midX, y: rect.midY), from: nil)
        XCTAssertNotNil(content.hitTest(point), "real header content must receive pointer events")
        let toggle = window.toolbar?.items.first {
          $0.itemIdentifier.rawValue.localizedCaseInsensitiveContains("sidebar")
        }
        XCTAssertNotNil(toggle, "native sidebar toggle stays in the toolbar")

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
      BackgroundTestWindow.present(window)
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
    UserDefaults.standard.set(true, forKey: sidebarKey)
    defer {
      restoreSidebarDefault(previous)
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

  // MARK: - Context card (replaces the round 4 column, UI-59)

  /// Founders, 2026-11-09: context is a compact card on demand, not a third
  /// column. It starts closed, is never persisted, and never moves the window
  /// floor: the floor is the same constant with the card open or closed.
  func testContextCardStartsClosedAndLeavesTheWindowFloorAlone() throws {
    XCTAssertFalse(AgentContextCardPresenter().isOpen)
    XCTAssertLessThanOrEqual(AgentContextCard.preferredWidth, 290)
    XCTAssertGreaterThanOrEqual(AgentContextCard.preferredWidth, 260)
    XCTAssertGreaterThanOrEqual(
      AgentWindowMetrics.detailMinWidth,
      ChatLayoutPolicy.minimumReadable + 2 * ChatLayoutPolicy.listPadding,
      "the conversation floor fits a readable bubble inside the list padding")

    let previous = UserDefaults.standard.object(forKey: sidebarKey)
    UserDefaults.standard.set(true, forKey: sidebarKey)
    defer { restoreSidebarDefault(previous) }
    let card = AgentContextCardPresenter()
    let window = mount(contentWidth: AgentWindowMetrics.minWidth, contextCard: card)
    defer { window.close() }
    let root = try XCTUnwrap(window.contentView)
    pump(window) { self.conversationScroll(in: root) != nil }
    let closedWidth = window.contentRect(forFrameRect: window.frame).width
    card.open()
    pump(window) { self.cardAnchor(in: root) != nil }
    XCTAssertNotNil(cardAnchor(in: root), "the open card is mounted")
    XCTAssertEqual(window.contentMinSize.width, AgentWindowMetrics.minWidth)
    XCTAssertEqual(
      window.contentRect(forFrameRect: window.frame).width, closedWidth, accuracy: 0.5,
      "opening the card never widens the window")
    let source = try String(
      contentsOf: URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Codescribe/Screens/AgentChat/AgentChatView.swift"),
      encoding: .utf8)
    XCTAssertFalse(source.contains("contextPanelOpen"), "no persisted open flag remains")
    XCTAssertFalse(source.contains("growToFloor"), "no window growth on open remains")
  }

  /// Opening and closing is view state: the conversation keeps its width, and
  /// the thread, the draft, the model, the staged attachments, the recording
  /// and the speech state stay as they were.
  func testTogglingTheContextCardChangesNoConversationState() throws {
    let previous = UserDefaults.standard.object(forKey: sidebarKey)
    UserDefaults.standard.set(true, forKey: sidebarKey)
    defer { restoreSidebarDefault(previous) }
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
    let card = AgentContextCardPresenter()
    let window = mount(contentWidth: 1120, store: store, renameTo: nil, contextCard: card)
    defer { window.close() }
    let root = try XCTUnwrap(window.contentView)
    pump(window) { self.conversationScroll(in: root) != nil }
    let closedScroll = frame(try XCTUnwrap(conversationScroll(in: root)))

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

    card.toggle()
    pump(window) { self.cardAnchor(in: root) != nil }
    let anchor = try XCTUnwrap(cardAnchor(in: root))
    let openScroll = frame(try XCTUnwrap(conversationScroll(in: root)))
    XCTAssertEqual(openScroll.width, closedScroll.width, accuracy: 0.5, "the card takes no width")
    XCTAssertEqual(snapshot(), before, "opening the card must not touch conversation state")

    let cardFrame = frame(anchor)
    XCTAssertLessThanOrEqual(cardFrame.width, AgentContextCard.preferredWidth + 1)
    XCTAssertLessThanOrEqual(
      cardFrame.maxY, window.contentLayoutRect.maxY + 0.5, "the card stays under the header")
    XCTAssertEqual(
      openScroll.maxX - cardFrame.maxX, AgentContextCard.inset, accuracy: 1,
      "the card sits at the conversation's trailing edge")

    card.toggle()
    pump(window) { self.cardAnchor(in: root) == nil }
    XCTAssertNil(cardAnchor(in: root))
    XCTAssertEqual(snapshot(), before, "closing the card must not touch conversation state")
  }

  /// At the window floor with the rail open the column is narrow: the card
  /// narrows to the column instead of overrunning it or widening the window.
  func testContextCardFitsTheNarrowestColumn() throws {
    let previous = UserDefaults.standard.object(forKey: sidebarKey)
    defer { restoreSidebarDefault(previous) }
    for expanded in [true, false] {
      UserDefaults.standard.set(expanded, forKey: sidebarKey)
      let card = AgentContextCardPresenter()
      let window = mount(contentWidth: AgentWindowMetrics.minWidth, contextCard: card)
      defer { window.close() }
      let root = try XCTUnwrap(window.contentView)
      pump(window) { self.conversationScroll(in: root) != nil }
      card.open()
      pump(window) { self.cardAnchor(in: root) != nil }
      let cardFrame = frame(try XCTUnwrap(cardAnchor(in: root)))
      let scroll = frame(try XCTUnwrap(conversationScroll(in: root)))
      XCTAssertGreaterThanOrEqual(
        cardFrame.minX, scroll.minX + AgentContextCard.inset - 1, "expanded \(expanded)")
      XCTAssertLessThanOrEqual(cardFrame.maxX, scroll.maxX, "expanded \(expanded)")
      XCTAssertEqual(
        window.contentRect(forFrameRect: window.frame).width, AgentWindowMetrics.minWidth,
        accuracy: 0.5, "expanded \(expanded)")
    }
  }

  /// Escape closes the card and is consumed; a click outside closes it and
  /// still reaches its target; clicks on the card or on the Context button
  /// (which closes it by its own action) leave it; other windows are ignored.
  func testEscapeAndOutsideClicksCloseTheCard() throws {
    let card = AgentContextCardPresenter()
    let window = mount(contentWidth: 1120, contextCard: card)
    defer { window.close() }
    let root = try XCTUnwrap(window.contentView)
    pump(window) { self.conversationScroll(in: root) != nil }
    let number = window.windowNumber
    func open() throws -> NSRect {
      card.open()
      pump(window) { self.cardAnchor(in: root) != nil }
      return frame(try XCTUnwrap(cardAnchor(in: root)))
    }

    var cardFrame = try open()
    XCTAssertTrue(
      card.handle(
        .init(windowNumber: number, kind: .click(NSPoint(x: cardFrame.midX, y: cardFrame.midY)))))
    XCTAssertTrue(card.isOpen, "a click on the card keeps it")
    XCTAssertTrue(card.handle(.init(windowNumber: number + 1, kind: .escape)))
    XCTAssertTrue(card.isOpen, "another window's Escape is not ours")

    XCTAssertTrue(card.handle(.init(windowNumber: number, kind: .click(NSPoint(x: 300, y: 200)))))
    XCTAssertFalse(card.isOpen, "an outside click closes the card and passes through")

    cardFrame = try open()
    XCTAssertFalse(card.handle(.init(windowNumber: number, kind: .escape)))
    XCTAssertFalse(card.isOpen, "Escape closes the card and is consumed")
    XCTAssertTrue(
      card.handle(.init(windowNumber: number, kind: .escape)), "Escape passes once it is closed")
  }

  /// Showing the Agent window again shows it without the card: closing or
  /// ordering the window out closes it. Re-activating a window that stayed
  /// on screen keeps the card as it was.
  func testHidingTheWindowClosesTheCardAndReactivationKeepsIt() throws {
    let card = AgentContextCardPresenter()
    let window = mount(contentWidth: 1120, contextCard: card)
    defer { window.close() }
    let root = try XCTUnwrap(window.contentView)
    pump(window) { self.conversationScroll(in: root) != nil }
    card.open()
    pump(window) { self.cardAnchor(in: root) != nil }

    NotificationCenter.default.post(name: NSWindow.didBecomeKeyNotification, object: window)
    NotificationCenter.default.post(
      name: NSWindow.didChangeOcclusionStateNotification, object: window)
    XCTAssertTrue(card.isOpen, "a visible window's activation keeps the card")

    window.orderOut(nil)
    pump(window) { !card.isOpen }
    XCTAssertFalse(card.isOpen, "ordering the window out closes the card")

    BackgroundTestWindow.present(window)
    card.open()
    pump(window) { self.cardAnchor(in: root) != nil }
    window.close()
    XCTAssertFalse(card.isOpen, "closing the window closes the card")
  }

  /// One identity plane and one bar: the toolbar holds no principal item, the
  /// leading group carries the wordmark with title and "Agent · model", the
  /// trailing cluster the Voice Lab mark, width, Context, pin and menu, and no
  /// header row sits under the bar (founders, 2026-10-21: one bar, the same
  /// height as Settings).
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
    XCTAssertFalse(agent.contains("safeAreaInset(edge: .top"), "no header row under the bar")
    XCTAssertEqual(agent.components(separatedBy: "AgentConversationHeader(store: store)").count, 2)
    let cluster = try XCTUnwrap(
      agent.range(of: "private var trailingCluster").map { String(agent[$0.lowerBound...]) })
    XCTAssertTrue(cluster.contains("DeveloperPowerMark()"))
    XCTAssertTrue(cluster.contains("AgentWidthModeButton()"))
    XCTAssertTrue(cluster.contains("AgentContextToggle(presenter: contextCard)"))
    let header = try source("Screens/AgentChat/AgentContextPanel.swift")
    let row = try XCTUnwrap(
      header.range(of: "struct AgentConversationHeader").map { String(header[$0.lowerBound...]) })
    XCTAssertTrue(row.contains("AgentLiveStatusPill(activity: store.selectedHeaderActivity)"))
    XCTAssertEqual(AgentConversationHeader.maxWidth, 360)
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
    XCTAssertTrue(composer.contains(".help(\"Attach\")"))
    XCTAssertTrue(
      composer.contains(".accessibilityLabel(Text(\"Attach an image (PNG, JPEG, GIF, WebP)\"))"))
    XCTAssertTrue(composer.contains(".help(micState.accessibilityLabel)"))
    XCTAssertTrue(composer.contains(".help(primaryAction.accessibilityLabel)"))
  }

  private func mount(
    contentWidth: CGFloat, store: AgentChatStore? = nil, renameTo title: String? = "Probe",
    contextCard: AgentContextCardPresenter = AgentContextCardPresenter()
  )
    -> NSWindow
  {
    let store = store ?? AgentChatStore(engine: nil)
    if let title, let thread = store.currentThread {
      store.rename(thread, to: title)
    }
    let window = AgentWindowChrome.makeWindow(
      content: NSHostingController(
        rootView: AgentChatView(store: store, contextCard: contextCard)
          .environment(\.csReduceTransparencyOverride, false)
          .environment(\.csContrastOverride, .standard)
      ))
    window.setContentSize(NSSize(width: contentWidth, height: 720))
    window.setFrameOrigin(NSPoint(x: 80, y: 80))
    BackgroundTestWindow.present(window)
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

  /// Visible geometry stays 52 pt; the native optical rim bleeds below it.
  private func visibleHeaderFrame(_ glass: NSGlassEffectView) -> NSRect {
    guard let viewport = glass.superview,
      viewport.identifier?.rawValue == "codescribe-header-viewport"
    else { return .zero }
    return frame(glass).intersection(frame(viewport))
  }

  /// Public view/layer geometry only, around the join shown in the desktop
  /// captures. Runtime class names are evidence, never behavior selectors.
  private func headerJoinLayers(in window: NSWindow, boundary: CGFloat) throws -> String {
    let root = try XCTUnwrap(window.contentView?.superview)
    let join = NSRect(x: boundary - 16, y: window.frame.height - 64, width: 32, height: 64)
    var rows: [[String: Any]] = []
    func visit(_ view: NSView, depth: Int) {
      guard depth < 32, rows.count < 100 else { return }
      let rect = frame(view)
      if rect.intersects(join), !view.isHiddenOrHasHiddenAncestor {
        var row: [String: Any] = [
          "depth": depth, "class": String(describing: type(of: view)),
          "identifier": view.identifier?.rawValue ?? "",
          "frame": NSStringFromRect(rect), "visibleRect": NSStringFromRect(view.visibleRect),
          "clipsToBounds": view.clipsToBounds, "opaque": view.isOpaque,
          "layerMasksToBounds": view.layer?.masksToBounds ?? false,
          "layerCornerRadius": view.layer?.cornerRadius ?? 0,
        ]
        if let glass = view as? NSGlassEffectView {
          row["glassCornerRadius"] = glass.cornerRadius
          row["glassStyle"] = glass.style.rawValue
        }
        if let material = view as? NSVisualEffectView {
          row["visualEffectMaterial"] = material.material.rawValue
          row["blendingMode"] = material.blendingMode.rawValue
        }
        rows.append(row)
      }
      for child in view.subviews { visit(child, depth: depth + 1) }
    }
    visit(root, depth: 0)
    let data = try JSONSerialization.data(
      withJSONObject: rows, options: [.prettyPrinted, .sortedKeys])
    return String(decoding: data, as: UTF8.self)
  }

  /// The open card's inert frame anchor; nil while the card is closed.
  private func cardAnchor(in root: NSView) -> NSView? {
    first(in: root) { $0 is AgentContextHitAnchor.AnchorView }
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
