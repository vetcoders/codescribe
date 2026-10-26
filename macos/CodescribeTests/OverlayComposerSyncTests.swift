import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayComposerSyncTests: XCTestCase {
  func testNativeComposerActionsRenderAlignedAndMuted() throws {
    try assertNativeComposerActions(compact: false, width: 480)
  }

  func testNarrowInlineComposerKeepsAllThreeActionsAlignedAndMuted() throws {
    try assertNativeComposerActions(compact: true, width: 220)
  }

  func testLinkedSyncUsesAccentWhileOtherActionsStayMuted() throws {
    try assertNativeComposerActions(compact: true, width: 220, linked: true)
  }

  func testDisabledSendIsDimmerThanAvailableActions() throws {
    try assertNativeComposerActions(compact: true, width: 220, sendEnabled: false)
  }

  private func assertNativeComposerActions(
    compact: Bool, width: CGFloat, linked: Bool = false, sendEnabled: Bool = true
  ) throws {
    let palette = OverlayAppearancePalette.dark
    let composer = OverlayConversationComposer(
      palette: palette, draft: .constant(sendEnabled ? "A shared draft" : ""),
      maximumEditorHeight: compact ? 28 : nil, sending: false, onSubmit: {},
      onToggleDraftMicrophone: {},
      draftSyncChoices: [.init(id: "peer", title: "Peer", selected: linked, enabled: true)],
      draftSyncNames: linked ? ["Agent", "Peer"] : [])
    let host = NSHostingView(rootView: composer
      .padding(12).background(palette.desktopBackground.color)
      .environment(\.colorScheme, .dark))
    host.sizingOptions = []
    let height: CGFloat = compact ? 60 : 100
    host.frame = NSRect(x: 0, y: 0, width: width, height: height)
    let window = NSWindow(
      contentRect: NSRect(x: -10000, y: -10000, width: width, height: height),
      styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    BackgroundTestWindow.present(window)
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    host.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let image = NSImage(cgImage: try XCTUnwrap(bitmap.cgImage), size: host.bounds.size)
    let screenshot = XCTAttachment(image: image)
    screenshot.name = linked ? "Compact-linked-sync-accent"
      : !sendEnabled ? "Compact-disabled-send"
      : compact ? "Compact-narrow-composer-actions" : "Muted-aligned-composer-actions"
    screenshot.lifetime = .keepAlways
    add(screenshot)
    // Measure visible ink, rather than assuming equal layout metrics imply
    // equal native Menu/Button output. AX is unavailable in this background host.
    let pixelScale = CGFloat(bitmap.pixelsWide) / host.bounds.width
    let controlSize = ComposerControlMetrics.hitTargetSize
    let stride = controlSize + (compact
      ? ComposerControlMetrics.intercomSpacing : ComposerControlMetrics.spacing)
    let rightInset: CGFloat = 12 + (compact
      ? ComposerControlMetrics.intercomHorizontalPadding : ComposerControlMetrics.horizontalPadding)
    let textView = try XCTUnwrap(editor(in: host))
    XCTAssertGreaterThan(textView.enclosingScrollView?.bounds.width ?? 0, 60)
    XCTAssertEqual(textView.font?.pointSize ?? 0, 14)
    var centers: [CGFloat] = []
    var heights: [CGFloat] = []
    var peaks: [CGFloat] = []
    for slot in 0..<3 {
      let right = host.bounds.width - rightInset - CGFloat(slot) * stride
      let left = right - controlSize
      var minY = bitmap.pixelsHigh
      var maxY = -1
      var inkCount = 0
      var peak = CGFloat.zero
      for y in 0..<bitmap.pixelsHigh {
        for x in Int(left * pixelScale)..<Int(right * pixelScale) {
          guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
          let brightness = max(color.redComponent, color.greenComponent, color.blueComponent)
          guard brightness > (!sendEnabled && slot == 0 ? 0.12 : 0.35) else { continue }
          inkCount += 1
          minY = min(minY, y)
          maxY = max(maxY, y)
          peak = max(peak, brightness)
        }
      }
      XCTAssertGreaterThan(inkCount, 20, "Each action must actually render its glyph")
      if !linked || slot != 2 {
        XCTAssertLessThan(peak, 0.8, "Idle action must not render bright white/accent ink")
      }
      centers.append(CGFloat(minY + maxY) / (2 * pixelScale))
      heights.append(CGFloat(maxY - minY + 1) / pixelScale)
      peaks.append(peak)
    }
    for index in 1..<3 {
      XCTAssertEqual(centers[index], centers[0], accuracy: 1.5, "Visible glyph centers must align")
      XCTAssertEqual(heights[index], heights[0], accuracy: 2, "Glyphs must have the same optical size")
      if !linked && sendEnabled {
        XCTAssertEqual(peaks[index], peaks[0], accuracy: 0.05, "Idle glyphs must use the same ink")
      }
    }
    if linked { XCTAssertGreaterThan(peaks[2], max(peaks[0], peaks[1]) + 0.1) }
    if !sendEnabled { XCTAssertLessThan(peaks[0], min(peaks[1], peaks[2]) * 0.6) }
  }

  func testForwardSyncJoinKeepsDifferentPrivateDraftsUnchanged() throws {
    let (state, agents) = try fixture(count: 2)
    state.setConversationDraft("Private source", for: agents[0].id)
    state.setConversationDraft("Private target", for: agents[1].id)

    XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: agents[1].id))
    XCTAssertEqual(state.conversationDraft(for: agents[0].id), "Private source")
    XCTAssertEqual(state.conversationDraft(for: agents[1].id), "Private target")
  }

  func testForwardSyncJoinNeverCopiesExistingProseIntoEmptyTarget() throws {
    let (state, agents) = try fixture(count: 2)
    state.setConversationDraft("Private source", for: agents[0].id)

    XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: agents[1].id))
    XCTAssertEqual(state.conversationDraft(for: agents[0].id), "Private source")
    XCTAssertEqual(state.conversationDraft(for: agents[1].id), "")
  }

  func testTypedPublisherCarriesOnlyExplicitOwnerCoordinatesAsSyncContext() throws {
    let (_, agents) = try fixture(count: 2)
    let owner = try XCTUnwrap(agents[0].owner)
    let peer = try XCTUnwrap(agents[1].owner)
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(
      at: root.appendingPathComponent("leases"), withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let receipt: [String: String] = [
      "schema": "codescribe.agent-bridge.lease.v1", "provider": owner.provider,
      "provider_session_id": owner.providerSessionID, "lease_id": owner.leaseID,
      "bus": root.appendingPathComponent("bus.jsonl").path,
    ]
    try JSONSerialization.data(withJSONObject: receipt).write(
      to: root.appendingPathComponent("leases/\(owner.leaseID).json"))
    let args = try RealAgentBridgeInstaller.busTextArguments(
      holdSeconds: 0, owner: owner, root: root, alsoSentTo: [peer])
    let index = try XCTUnwrap(args.firstIndex(of: "--also-sent-to"))
    let metadata = try XCTUnwrap(
      JSONSerialization.jsonObject(with: Data(args[index + 1].utf8)) as? [[String: String]])
    XCTAssertEqual(metadata, [[
      "channel": peer.channel, "provider": peer.provider,
      "provider_session_id": peer.providerSessionID, "lease_id": peer.leaseID,
    ]])
    let single = try RealAgentBridgeInstaller.busTextArguments(
      holdSeconds: 0, owner: owner, root: root)
    let broadcast = try RealAgentBridgeInstaller.busTextArguments(
      holdSeconds: 0, owner: nil, root: root, alsoSentTo: [peer])
    XCTAssertFalse(single.contains("--also-sent-to"))
    XCTAssertFalse(broadcast.contains("--also-sent-to"))
  }

  func testTypingAndAttachmentsHaveOneSharedDraftAndUnlinkRetainsBoth() throws {
    let (state, agents) = try fixture()
    state.setConversationDraft("Shared", for: agents[0].id)
    XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: agents[1].id))
    XCTAssertTrue(state.toggleDraftSync(from: agents[1].id, with: agents[2].id))
    let path = ConversationAttachmentPointer.configDirectory
      .appendingPathComponent("attachments/sync-fixture.txt").path
    let text = ConversationDraft.compose(text: "Edited in the second composer", attachments: [path])
    state.setConversationDraft(text, for: agents[1].id)
    XCTAssertEqual(state.conversationDrafts.count, 1, "The group owns one text entry")
    for agent in agents { XCTAssertEqual(state.conversationDraft(for: agent.id), text) }
    state.stopDraftSync(for: agents[0].id)
    state.setConversationDraft("Independent", for: agents[0].id)
    XCTAssertEqual(state.conversationDraft(for: agents[1].id), text)
    XCTAssertEqual(state.conversationDraft(for: agents[2].id), text)
    state.stopDraftSync(for: agents[1].id)
    XCTAssertTrue(state.conversationDraftGroups.isEmpty)
    XCTAssertEqual(state.conversationDrafts[agents[1].id], text)
    XCTAssertEqual(state.conversationDrafts[agents[2].id], text)
  }

  func testJoiningDoesNotOverwriteDifferentDraftOrIncludeBroadcastAndInbox() throws {
    let (state, agents) = try fixture()
    state.setConversationDraft("Source", for: agents[0].id)
    state.setConversationDraft("Retain this private draft", for: agents[1].id)
    XCTAssertFalse(state.toggleDraftSync(from: agents[0].id, with: agents[1].id))
    XCTAssertEqual(state.conversationDraft(for: agents[1].id), "Retain this private draft")
    XCTAssertFalse(state.toggleDraftSync(from: agents[0].id, with: "0"))
    XCTAssertFalse(state.toggleDraftSync(from: agents[0].id, with: "10"))
    let choices = state.draftSyncChoices(for: agents[0])
    XCTAssertEqual(Set(choices.map(\.id)), Set(agents.dropFirst().map(\.id)))
    XCTAssertEqual(choices.first(where: { $0.id == agents[1].id })?.enabled, false)
    state.setConversationDraft("", for: agents[0].id)
    XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: agents[1].id))
    XCTAssertEqual(state.conversationDraft(for: agents[0].id), "Retain this private draft")
  }

  func testPartialPublicationRetriesOnlyMissingExactOwnersAndKeepsGroupForNextDraft() async throws {
    let (state, agents) = try fixture()
    for target in agents.dropFirst() {
      XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: target.id))
    }
    state.setConversationDraft("First", for: agents[0].id)
    let reply = try XCTUnwrap(OverlayUserReplyReference(row: [
      "message_id": "parent", "author": "Agent 2", "text": "Question",
    ]))
    state.conversationReplies[agents[1].id] = reply
    var calls: [(String, String, OverlayUserReplyReference?)] = []
    var refuseSecond = true
    var expectedText = "First"
    state.publishConversationText = { owner, text, reference, peers in
      let owner = try XCTUnwrap(owner)
      XCTAssertEqual(Set(peers.map(\.id)), Set(agents.map(\.id)).subtracting([owner.id]))
      XCTAssertEqual(text, expectedText, "Co-addressing must never become user text")
      calls.append((owner.id, text, reference))
      if owner.id == agents[1].id && refuseSecond { throw CocoaError(.fileWriteNoPermission) }
    }
    await state.sendConversationText(agents[0])
    XCTAssertEqual(calls.map { $0.0 }, agents.map(\.id))
    XCTAssertNotNil(state.draftSyncSendError(for: agents[0].id))
    XCTAssertEqual(state.conversationDraft(for: agents[2].id), "First")
    XCTAssertEqual(calls[1].2, reply)
    refuseSecond = false
    await state.sendConversationText(agents[2])
    XCTAssertEqual(calls.map { $0.0 }, [agents[0].id, agents[1].id, agents[2].id, agents[1].id])
    XCTAssertEqual(calls.last?.2, reply)
    XCTAssertNil(state.conversationReplies[agents[1].id])
    for agent in agents { XCTAssertEqual(state.conversationDraft(for: agent.id), "") }
    XCTAssertEqual(state.conversationDraftGroups.count, 1, "Sync remains enabled after Send")
    XCTAssertTrue(state.pendingTextMessages.isEmpty)
    expectedText = "Second"
    state.setConversationDraft("Second", for: agents[1].id)
    await state.sendConversationText(agents[1])
    XCTAssertEqual(calls.suffix(3).map { $0.1 }, ["Second", "Second", "Second"])
  }

  func testEditsDuringPublicationAreNotClearedOrSentAsPartOfOlderBatch() async throws {
    let (state, agents) = try fixture(count: 2)
    XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: agents[1].id))
    state.setConversationDraft("Frozen outgoing text", for: agents[0].id)
    var texts: [String] = []
    state.publishConversationText = { _, text, _, _ in
      texts.append(text)
      if texts.count == 1 {
        state.setConversationDraft("New draft", for: agents[1].id)
        await state.sendConversationText(agents[1])
      }
    }
    await state.sendConversationText(agents[0])
    XCTAssertEqual(texts, ["Frozen outgoing text", "Frozen outgoing text"])
    for agent in agents { XCTAssertEqual(state.conversationDraft(for: agent.id), "New draft") }
    await state.sendConversationText(agents[1])
    XCTAssertEqual(texts.suffix(2), ["New draft", "New draft"])
    XCTAssertEqual(state.conversationDraft(for: agents[0].id), "")
  }

  func testSyncCoAddressesStayFrozenWhenAChannelIsReboundDuringSend() async throws {
    let (state, agents) = try fixture(count: 2)
    XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: agents[1].id))
    state.setConversationDraft("Frozen group", for: agents[0].id)
    let successor = try conversation(channel: agents[1].channel, suffix: "new-owner")
    var targets: [String] = []
    state.publishConversationText = { owner, text, _, peers in
      let owner = try XCTUnwrap(owner)
      targets.append(owner.id)
      XCTAssertEqual(text, "Frozen group")
      XCTAssertEqual(peers.map(\.id), agents.filter { $0.id != owner.id }.map(\.id))
      XCTAssertFalse(peers.contains { $0.id == successor.id })
      if targets.count == 1 { self.apply([agents[0], successor], to: state) }
    }
    await state.sendConversationText(agents[0])
    XCTAssertEqual(targets, agents.map(\.id))
    XCTAssertFalse(state.hasSynchronizedDraft(successor.id))
  }

  func testChannelReuseDoesNotJoinSuccessorAndRetiredDraftIsPreserved() throws {
    let (state, agents) = try fixture()
    for target in agents.dropFirst() {
      XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: target.id))
    }
    state.setConversationDraft("Old shared text", for: agents[0].id)
    let successor = try conversation(channel: agents[1].channel, suffix: "replacement")
    let projected = [agents[0], successor, agents[2]]
    apply(projected, to: state)
    XCTAssertFalse(state.hasSynchronizedDraft(agents[1].id))
    XCTAssertFalse(state.hasSynchronizedDraft(successor.id))
    state.setConversationDraft("Next text", for: agents[0].id)
    XCTAssertEqual(state.conversationDraft(for: agents[2].id), "Next text")
    XCTAssertEqual(state.conversationDraft(for: agents[1].id), "Old shared text")
    XCTAssertEqual(state.conversationDraft(for: successor.id), "")
  }

  func testLinkedPasteRemainsManualInsteadOfBeingStagedOnLiveCapture() throws {
    let (state, agents) = try fixture(count: 2)
    XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: agents[1].id))
    XCTAssertFalse(state.stageConversationAttachments(["/fixture/file"], in: agents[0]))
    XCTAssertTrue(state.captureAttachmentDrafts.isEmpty)
    XCTAssertTrue(state.pendingCaptureAttachmentSends.isEmpty)
  }

  func testNativePasteUpdatesThreeEditorsAndStoresLongAttachmentOnce() throws {
    let (state, agents) = try fixture()
    for target in agents.dropFirst() {
      XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: target.id))
    }
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    let file = ConversationAttachmentPointer.configDirectory
      .appendingPathComponent("attachments/sync-\(UUID().uuidString).txt")
    try FileManager.default.createDirectory(
      at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: file) }
    var saves = 0
    var hosts: [NSHostingView<OverlayConversationComposer>] = []
    var windows: [NSWindow] = []
    let keyBefore = NSApp.keyWindow
    for agent in agents {
      let host = NSHostingView(rootView: OverlayConversationComposer(
        palette: .dark,
        draft: Binding(get: { state.conversationDraft(for: agent.id) },
          set: { state.setConversationDraft($0, for: agent.id) }),
        maximumEditorHeight: agent.id == agents[1].id ? 28 : nil,
        sending: false, onSubmit: { XCTFail("Editing must not publish") }, clipboard: board,
        saveFile: { data, _ in
          saves += 1
          try data.write(to: file)
          return file
        },
        draftSyncChoices: state.draftSyncChoices(for: agent),
        draftSyncNames: state.draftSyncNames(for: agent.id)))
      host.sizingOptions = []
      host.frame = NSRect(x: 0, y: 0, width: 480, height: 180)
      let window = NSWindow(
        contentRect: NSRect(x: -10000, y: -10000, width: 480, height: 180),
        styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      hosts.append(host)
      windows.append(window)
      host.layoutSubtreeIfNeeded()
    }
    defer { for window in windows { window.close() } }
    let editors = try hosts.map { try XCTUnwrap(editor(in: $0)) }
    board.setString("Shared native paste", forType: .string)
    editors[0].paste(nil)
    settle(hosts)
    XCTAssertEqual(editors.map(\.string), Array(repeating: "Shared native paste", count: 3))
    board.clearContents()
    board.setString(String(repeating: "long text ", count: 600), forType: .string)
    editors[1].paste(nil)
    settle(hosts)
    XCTAssertEqual(saves, 1)
    for agent in agents {
      let paths = ConversationDraft.split(state.conversationDraft(for: agent.id)).attachments
      XCTAssertEqual(paths.count, 1)
      let attributes = try FileManager.default.attributesOfItem(atPath: XCTUnwrap(paths.first))
      let saved = try FileManager.default.attributesOfItem(atPath: file.path)
      XCTAssertEqual(attributes[.systemNumber] as? NSNumber, saved[.systemNumber] as? NSNumber)
      XCTAssertEqual(attributes[.systemFileNumber] as? NSNumber, saved[.systemFileNumber] as? NSNumber)
    }
    editors[2].insertText("New shared text", replacementRange: NSRange(location: 0, length: editors[2].string.utf16.count))
    settle(hosts)
    XCTAssertEqual(editors.map(\.string), Array(repeating: "New shared text", count: 3))
    XCTAssertFalse(editors[0].undoManager?.canUndo ?? false, "External replacement invalidates stale local undo")
    XCTAssertTrue(state.pendingTextMessages.isEmpty)
    XCTAssertTrue(NSApp.keyWindow === keyBefore)
    let bitmap = try XCTUnwrap(hosts[0].bitmapImageRepForCachingDisplay(in: hosts[0].bounds))
    hosts[0].cacheDisplay(in: hosts[0].bounds, to: bitmap)
    let attachment = XCTAttachment(image: NSImage(cgImage: try XCTUnwrap(bitmap.cgImage), size: hosts[0].bounds.size))
    attachment.name = "Three-linked-composers"
    attachment.lifetime = .keepAlways
    add(attachment)
  }

  private func editor(in view: NSView) -> NSTextView? {
    if let text = view as? NSTextView, text.isEditable { return text }
    return view.subviews.lazy.compactMap { self.editor(in: $0) }.first
  }

  private func settle(_ hosts: [NSHostingView<OverlayConversationComposer>]) {
    RunLoop.main.run(until: Date().addingTimeInterval(0.08))
    for host in hosts { host.layoutSubtreeIfNeeded() }
  }

  private func fixture(count: Int = 3) throws -> (OverlayState, [OverlayConversation]) {
    let agents = try (1...count).map { try conversation(channel: String($0), suffix: "initial") }
    let state = OverlayState()
    apply(agents, to: state)
    return (state, agents)
  }

  private func conversation(channel: String, suffix: String) throws -> OverlayConversation {
    let owner = try XCTUnwrap(OverlayConversationOwner(row: [
      "provider": "codex", "provider_session_id": "sync-\(channel)-\(suffix)",
      "lease_id": String(repeating: channel, count: 32), "channel": channel,
      "name": "Agent \(channel)",
    ]))
    return OverlayConversation(id: owner.id, channel: channel, name: owner.name, owner: owner, messages: [])
  }

  private func apply(_ agents: [OverlayConversation], to state: OverlayState) {
    state.applyChannelRoster(agents.map { agent in
      .init(channel: agent.channel, audience: agent.name, provider: "codex",
        providerSessionId: agent.owner?.providerSessionID ?? "", open: false, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
    })
    state.applyConversationSnapshot(.init(deliveries: [], conversations: agents))
  }
}
