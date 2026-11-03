import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayIntercomProjectionTests: XCTestCase {
  private func controller() -> OverlayIntercomWidgetController {
    OverlayIntercomWidgetController(
      state: OverlayState(), textScale: TextScaleController(key: UUID().uuidString))
  }

  private func identity(session: String = "first", name: String = "Roman") throws
    -> OverlayIntercomCardIdentity
  {
    .init(digit: "3", conversation: try conversation(session: session, name: name))
  }

  private func conversation(
    session: String = "first", name: String = "Roman", channel: String = "3"
  ) throws
    -> OverlayConversation
  {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session,
        "lease_id": AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: session),
        "channel": channel, "name": name,
      ]))
    return .init(id: owner.id, channel: channel, name: name, owner: owner, messages: [])
  }

  private func settle() async throws {
    // Let the coordinator's queued MainActor task and its explicit yield finish.
    try await Task.sleep(for: .milliseconds(30))
  }

  func testGroupFoldLeavesDetachedWindowsAndGlobalDockRestoresThePreviousLayout() async throws {
    let suite = "CodescribeTests.hide-all.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let store = OverlayIntercomDesktopStore(defaults: defaults)
    let agentConversation = try conversation()
    let groupedConversation = try conversation(session: "grouped", name: "Marek", channel: "4")
    let broadcast = OverlayConversation(
      id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let agent = OverlayIntercomCardIdentity(digit: "3", conversation: agentConversation)
    let previousOwner = try identity(session: "previous")
    let inbox = OverlayIntercomCardIdentity(digit: "10", conversation: .humanInbox())
    let state = OverlayState()
    state.applyConversationSnapshot(
      .init(
        deliveries: [],
        conversations: [broadcast, agentConversation, groupedConversation, .humanInbox()]))
    state.applyChannelRoster(
      [broadcast, agentConversation, groupedConversation].map {
        .init(
          channel: $0.channel, audience: $0.name,
          provider: $0.owner?.provider, providerSessionId: $0.owner?.providerSessionID,
          open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
          unreadSinceUnixMs: nil)
      })
    XCTAssertEqual(state.channelMapLayoutModel.agents.map(\.digit), ["3", "4"])
    state.conversationDrafts = [agentConversation.id: "unfinished reply", "10": "private draft"]
    state.publishConversationText = { _, _, _, _ in XCTFail("Hiding windows must not send") }
    var desktop = OverlayIntercomDesktopState()
    desktop.groupWidth = 600
    desktop.groupRowHeight = 240
    for (index, card) in [agent, previousOwner, inbox].enumerated() {
      desktop.upsert(
        .init(
          identity: card,
          frame: CGRect(x: 80 + index * 20, y: 160, width: 380, height: 320),
          minimized: index == 1, closed: index == 2))
    }
    store.save(desktop)
    let controller = OverlayIntercomWidgetController(
      state: state, textScale: TextScaleController(key: suite), desktopStore: store)
    defer { controller.invalidate() }
    let existingWindows = Set(NSApp.windows.map(ObjectIdentifier.init))
    controller.setVisible(true, relativeTo: nil)
    let ownedWindows = NSApp.windows.filter { !existingWindows.contains(ObjectIdentifier($0)) }
    XCTAssertGreaterThan(ownedWindows.filter(\.isVisible).count, 1)
    let drafts = state.conversationDrafts
    let generation = state.captureGeneration
    let selected = state.selectedConversationID
    let conversations = state.conversations.map(\.id)
    let initialDesktop = store.load()
    let visibleBeforeFold = Set(ownedWindows.filter(\.isVisible).map(ObjectIdentifier.init))
    let allRosterRowCount = state.channelMapLayoutModel.agents.count
    let groupRowCount = initialDesktop.groupRows(of: [
      agent,
      .init(digit: "4", conversation: groupedConversation),
    ]).count
    XCTAssertEqual(
      visibleBeforeFold.count,
      1 + groupRowCount + initialDesktop.instances.filter { !$0.closed }.count,
      "Detached conversations leave the group")
    controller.toggleGroup()
    for _ in 0..<50 {
      if ownedWindows.filter(\.isVisible).count == visibleBeforeFold.count - groupRowCount { break }
      try await Task.sleep(for: .milliseconds(20))
    }
    let foldedVisible = Set(ownedWindows.filter(\.isVisible).map(ObjectIdentifier.init))
    XCTAssertEqual(
      foldedVisible.count, visibleBeforeFold.count - groupRowCount,
      "Every roster row folds; detached windows stay visible")
    XCTAssertGreaterThan(foldedVisible.count, 1, "Detached windows remain visible")
    XCTAssertEqual(store.load(), initialDesktop, "Folding the group never docks detached windows")
    let reconciliations = controller.reconciliationRequests
    XCTAssertFalse(controller.canRestoreDockedLayout)
    controller.dockAll()

    let saved = store.load()
    var expected = initialDesktop
    for index in expected.instances.indices { expected.instances[index].closed = true }
    XCTAssertEqual(saved, expected, "Only open presentation records become closed")
    XCTAssertEqual(
      NSApp.windows.filter {
        !existingWindows.contains(ObjectIdentifier($0)) && $0.isVisible
      }.count, 1 + allRosterRowCount,
      "Docking collects conversations into the visible roster")
    XCTAssertTrue(controller.canRestoreDockedLayout)
    let bar = try XCTUnwrap(ownedWindows.first(where: \.isVisible)?.contentView)
    bar.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(bar.bitmapImageRepForCachingDisplay(in: bar.bounds))
    bar.cacheDisplay(in: bar.bounds, to: bitmap)
    let image = try XCTUnwrap(bitmap.cgImage)
    let attachment = XCTAttachment(image: NSImage(cgImage: image, size: bar.bounds.size))
    attachment.name = "Intercom separate group and global docking controls"
    attachment.lifetime = .keepAlways
    add(attachment)
    XCTAssertEqual(controller.reconciliationRequests, reconciliations + 1)
    XCTAssertEqual(state.conversationDrafts, drafts)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertEqual(state.selectedConversationID, selected)
    XCTAssertEqual(state.conversations.map(\.id), conversations)
    XCTAssertTrue(state.pendingTextMessages.isEmpty)

    controller.hideAll()
    XCTAssertEqual(store.load(), saved, "Repeated recovery preserves exact saved windows")
    XCTAssertEqual(ownedWindows.filter(\.isVisible).count, 1)

    controller.toggleGroup()
    try await settle()
    controller.restoreDockedLayout()
    for _ in 0..<50 {
      let visible = Set(
        NSApp.windows.filter {
          !existingWindows.contains(ObjectIdentifier($0)) && $0.isVisible
        }.map(ObjectIdentifier.init))
      if visible == foldedVisible { break }
      try await Task.sleep(for: .milliseconds(20))
    }
    XCTAssertFalse(controller.canRestoreDockedLayout)
    XCTAssertEqual(
      Set(
        NSApp.windows.filter {
          !existingWindows.contains(ObjectIdentifier($0)) && $0.isVisible
        }.map(ObjectIdentifier.init)), foldedVisible,
      "Global restore returns the exact detached windows and the previously folded group")
    XCTAssertEqual(store.load(), initialDesktop, "Closed history and exact geometry are restored")
    XCTAssertEqual(state.conversationDrafts, drafts)
    XCTAssertEqual(state.captureGeneration, generation)

    controller.dockAll()
    XCTAssertTrue(controller.canRestoreDockedLayout)
    XCTAssertEqual(store.load(), saved)
    XCTAssertEqual(
      NSApp.windows.filter {
        !existingWindows.contains(ObjectIdentifier($0)) && $0.isVisible
      }.count, 1 + allRosterRowCount)

    controller.openMailbox("0")
    let newWindow = try XCTUnwrap(store.load().instances.first { $0.conversationID == "0" })
    controller.restoreDockedLayout()
    let restoredWithNewWindow = store.load()
    XCTAssertEqual(restoredWithNewWindow.instances.first { $0.id == newWindow.id }, newWindow)
    XCTAssertEqual(
      restoredWithNewWindow.instances.filter { $0.id != newWindow.id }, initialDesktop.instances,
      "Restoring a snapshot preserves windows created after docking")
    XCTAssertEqual(state.conversationDrafts, drafts)
    XCTAssertEqual(state.captureGeneration, generation)
  }

  func testRepeatedHostingUpdatesDoNotRepeatNativePlacement() async throws {
    let controller = controller()
    let card = try identity()
    let host = NSHostingView(
      rootView: OverlayIntercomWindowProjection(
        chips: [], identities: [card], controller: controller))
    host.frame = CGRect(x: 0, y: 0, width: 40, height: 40)
    let window = NSWindow(
      contentRect: CGRect(x: -10000, y: -10000, width: 40, height: 40),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.contentView = host
    defer { window.contentView = nil }
    host.layoutSubtreeIfNeeded()
    try await settle()
    XCTAssertGreaterThan(controller.reconciliationRequests, 0, "The representable must execute")
    let initial = controller.reconciliationRequests
    for _ in 0..<5 {
      host.rootView = OverlayIntercomWindowProjection(
        chips: [], identities: [card], controller: controller)
      host.layoutSubtreeIfNeeded()
      try await settle()
    }
    XCTAssertEqual(controller.reconciliationRequests, initial)
  }

  func testIdenticalProjectionUpdatesReconcileOnlyOnce() async throws {
    let controller = controller()
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    defer { coordinator.cancel() }
    let first = try identity()
    for _ in 0..<20 {
      coordinator.submit(chips: [], identities: [first], scale: 1, controller: controller)
    }
    try await settle()
    XCTAssertEqual(controller.reconciliationRequests, 1)
    coordinator.submit(chips: [], identities: [first], scale: 1, controller: controller)
    try await settle()
    XCTAssertEqual(controller.reconciliationRequests, 1)
  }

  func testOwnerLabelAndScaleChangesRemainVisibleBehindTheSameDigit() async throws {
    let controller = controller()
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    defer { coordinator.cancel() }
    let original = try identity()
    let replacement = try identity(session: "second")
    let renamed = try identity(session: "second", name: "A much longer name")
    XCTAssertNotEqual(original, replacement)
    XCTAssertEqual(replacement, renamed, "Display names deliberately are not card identity")
    for (index, inputs) in [(original, CGFloat(1)), (replacement, 1), (renamed, 1), (renamed, 1.5)]
      .enumerated()
    {
      coordinator.submit(chips: [], identities: [inputs.0], scale: inputs.1, controller: controller)
      try await settle()
      XCTAssertEqual(controller.reconciliationRequests, index + 1)
    }
  }

  func testBurstTargetsOnlyTheLatestController() async throws {
    let first = controller()
    let last = controller()
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    defer { coordinator.cancel() }
    coordinator.submit(chips: [], identities: [], scale: 1, controller: first)
    coordinator.submit(chips: [], identities: [], scale: 1.5, controller: last)
    try await settle()
    XCTAssertEqual(first.reconciliationRequests, 0)
    XCTAssertEqual(last.reconciliationRequests, 1)
    coordinator.submit(chips: [], identities: [], scale: 1.5, controller: last)
    try await settle()
    XCTAssertEqual(last.reconciliationRequests, 1)
  }

  func testDismantleCancelsPendingWorkAndAllowsAFreshSubmission() async throws {
    let controller = controller()
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    coordinator.submit(chips: [], identities: [], scale: 1, controller: controller)
    OverlayIntercomWindowProjection.dismantleNSView(NSView(), coordinator: coordinator)
    try await settle()
    XCTAssertEqual(controller.reconciliationRequests, 0)
    coordinator.submit(chips: [], identities: [], scale: 1, controller: controller)
    try await settle()
    XCTAssertEqual(controller.reconciliationRequests, 1)
    coordinator.cancel()
  }

  func testPendingProjectionDoesNotRetainTheWindowOwner() async throws {
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    var owner: OverlayIntercomWidgetController? = controller()
    weak let weakOwner = owner
    coordinator.submit(chips: [], identities: [], scale: 1, controller: owner)
    owner = nil
    XCTAssertNil(weakOwner)
    try await settle()
    coordinator.cancel()
  }

  func testScreenVisibleFrameChangesStillReconcileWithIdenticalCards() async throws {
    let controller = controller()
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    defer { coordinator.cancel() }
    let card = try identity()
    for (index, height) in [CGFloat(900), 850, 850].enumerated() {
      coordinator.submit(
        chips: [], identities: [card], scale: 1, controller: controller,
        visibleFrames: [CGRect(x: 0, y: 0, width: 1440, height: height)])
      try await settle()
      XCTAssertEqual(controller.reconciliationRequests, min(index + 1, 2))
    }
  }

  func testChangedChipStateReachesExistingController() async throws {
    let controller = controller()
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    defer { coordinator.cancel() }
    for (index, open) in [false, true, true].enumerated() {
      let chip = OverlayChannelMapChip(
        digit: "3", name: "Roman", disconnected: false, microphoneOpen: open,
        unread: false, selectable: true, selected: false)
      coordinator.submit(chips: [chip], identities: [], scale: 1, controller: controller)
      try await settle()
      XCTAssertEqual(controller.reconciliationRequests, min(index + 1, 2))
    }
  }
}
