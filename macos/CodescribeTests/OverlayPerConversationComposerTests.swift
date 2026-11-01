import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Independent contract tests. All publishers and capture controllers are fixtures;
/// native editors are hosted in background windows without claiming keyboard focus.
@MainActor
final class OverlayPerConversationComposerTests: XCTestCase {
  @MainActor
  private final class Fixture {
    let state = OverlayState()
    let a: OverlayConversation
    let b: OverlayConversation
    let all = OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let suite = "CodescribeTests.per-conversation.\(UUID().uuidString)"
    let directory: URL
    let defaults: UserDefaults
    let keyBefore = NSApp.keyWindow
    let windowsBefore = Set(NSApp.windows.map(ObjectIdentifier.init))
    var hosts: [NSView] = []
    var windows: [NSWindow] = []
    var intercom: OverlayIntercomWidgetController?

    init() throws {
      a = try Self.agent("2", session: "draft-a")
      b = try Self.agent("3", session: "draft-b")
      defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
      directory = ConversationAttachmentPointer.configDirectory.resolvingSymlinksInPath()
        .appendingPathComponent("attachments/\(suite)")
      try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      apply([a, b, all])
      state.publishConversationText = { _, _, _, _ in XCTFail("Unexpected publication") }
      state.publishCaptureAttachments = { _, _ in XCTFail("Unexpected capture publication") }
    }

    static func agent(_ digit: String, session: String) throws -> OverlayConversation {
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": session,
          "lease_id": AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: session),
          "channel": digit, "name": session,
        ]))
      return .init(id: owner.id, channel: digit, name: owner.name, owner: owner, messages: [])
    }

    func apply(_ conversations: [OverlayConversation], open: Set<String> = []) {
      state.applyChannelRoster(
        conversations.map {
          CsChannelRosterState(
            channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
            providerSessionId: $0.owner?.providerSessionID, open: open.contains($0.channel),
            loud: false, autosealDeadlineUnixMs: nil,
            followerAlive: $0.owner == nil ? nil : true, unreadSinceUnixMs: nil)
        })
      state.applyConversationSnapshot(.init(deliveries: [], conversations: conversations))
    }

    func view(_ conversation: OverlayConversation) {
      state.selectConversation(conversation.id)
      settle()
    }

    func mainWindow() -> NSView {
      let host = NSHostingView(
        rootView: DictationOverlayView(state: state)
          .frame(width: 1000, height: 680)
          .environment(\.colorScheme, .dark)
          .symbolEffectsRemoved()
          .transaction { $0.disablesAnimations = true })
      host.sizingOptions = []
      host.frame = NSRect(x: 0, y: 0, width: 1000, height: 680)
      let window = NSWindow(
        contentRect: NSRect(x: -10000, y: -10000, width: 1000, height: 680),
        styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      windows.append(window)
      hosts.append(host)
      BackgroundTestWindow.present(window)
      settle()
      return host
    }

    func card(for conversation: OverlayConversation) throws -> NSView {
      let controller = OverlayIntercomWidgetController(
        state: state, textScale: TextScaleController(key: suite),
        desktopStore: OverlayIntercomDesktopStore(defaults: defaults))
      intercom = controller
      XCTAssertTrue(
        controller.detach(.init(digit: conversation.channel, conversation: conversation)))
      let panels = NSApp.windows.filter {
        $0 is OverlayIntercomPanel && !windowsBefore.contains(ObjectIdentifier($0))
      }
      XCTAssertEqual(panels.count, 1, "Only the requested hidden fiszka is materialized")
      let panel = try XCTUnwrap(panels.first)
      let host = try XCTUnwrap(panel.contentView)
      XCTAssertFalse(panel.isVisible)
      XCTAssertFalse(panel.isKeyWindow)
      hosts.append(host)
      settle()
      return host
    }

    func file(_ name: String) throws -> String {
      let url = directory.appendingPathComponent(name + ".txt")
      try Data("physical attachment \(name)".utf8).write(to: url)
      return url.path
    }

    func settle() {
      for host in hosts { host.layoutSubtreeIfNeeded() }
      RunLoop.main.run(until: Date().addingTimeInterval(0.08))
      for host in hosts { host.layoutSubtreeIfNeeded() }
    }

    func cleanup() {
      if let intercom {
        XCTAssertEqual(intercom.interactionWindowCounts.visible, 0)
        XCTAssertEqual(intercom.interactionWindowCounts.key, 0)
        intercom.invalidate()
      }
      for window in windows {
        window.contentView = nil
        window.close()
      }
      hosts.removeAll()
      defaults.removePersistentDomain(forName: suite)
      XCTAssertNoThrow(try FileManager.default.removeItem(at: directory))
      XCTAssertTrue(NSApp.keyWindow === keyBefore, "Fixtures must not take the Founder's focus")
    }
  }

  private func editor(in view: NSView) throws -> NSTextView {
    func editors(_ view: NSView) -> [NSTextView] {
      if let text = view as? NSTextView, text.isEditable { return [text] }
      return view.subviews.flatMap(editors)
    }
    let found = editors(view)
    XCTAssertEqual(
      found.count, 1, "Select the actual conversation composer, not a synthetic binding")
    return try XCTUnwrap(found.first)
  }

  private func reply(_ id: String) throws -> OverlayUserReplyReference {
    try XCTUnwrap(
      OverlayUserReplyReference(row: ["message_id": id, "author": id, "text": "Question \(id)"]))
  }

  private func append(_ text: String, in host: NSView, fixture: Fixture) throws {
    let editor = try editor(in: host)
    editor.insertText(
      text, replacementRange: NSRange(location: editor.string.utf16.count, length: 0))
    fixture.settle()
  }

  func testABAndBroadcastRestoreTextAttachmentsAndRepliesAfterNavigation() throws {
    let f = try Fixture()
    defer { f.cleanup() }
    let conversations = [f.a, f.b, f.all]
    let paths = try ["a", "b", "broadcast"].map { try f.file($0) }
    let replies = try ["reply-a", "reply-b", "reply-all"].map(reply)
    for (index, conversation) in conversations.enumerated() {
      f.state.setConversationDraft(
        ConversationDraft.compose(text: "Private \(index)", attachments: [paths[index]]),
        for: conversation.id)
      f.state.conversationReplies[conversation.id] = replies[index]
    }
    f.view(f.a)
    let host = f.mainWindow()
    for (index, conversation) in conversations.enumerated() {
      f.view(conversation)
      XCTAssertTrue(try editor(in: host).string.contains("Private \(index)"))
      try append(" edited-\(index)", in: host, fixture: f)
    }
    let saved = f.state.conversationDrafts
    for conversation in [f.b, f.a, f.all, f.a] {
      f.view(conversation)
      let index = try XCTUnwrap(conversations.firstIndex { $0.id == conversation.id })
      let draft = ConversationDraft.split(f.state.conversationDraft(for: conversation.id))
      XCTAssertEqual(draft.text, "Private \(index) edited-\(index)")
      XCTAssertEqual(draft.attachments, [paths[index]])
      XCTAssertTrue(FileManager.default.fileExists(atPath: paths[index]))
      XCTAssertEqual(f.state.conversationReplies[conversation.id], replies[index])
      XCTAssertTrue(try editor(in: host).string.contains("edited-\(index)"))
      XCTAssertEqual(f.state.conversationDrafts, saved, "Navigation is not a draft edit")
    }
  }

  /// The open conversation is always the recipient (Maciej, 2026-10-30 13:58):
  /// typing in A's view edits only A's draft and A stays A's recipient however
  /// the Founder navigates.
  func testEditingTheViewedDraftLeavesOthersAloneAndEachViewIsItsOwnRecipient() throws {
    let f = try Fixture()
    defer { f.cleanup() }
    let attachment = try f.file("only-a")
    f.state.setConversationDraft(
      ConversationDraft.compose(text: "A private words", attachments: [attachment]), for: f.a.id)
    f.state.setConversationDraft("B private words", for: f.b.id)
    f.state.setConversationDraft("Broadcast private words", for: f.all.id)
    f.view(f.a)
    let host = f.mainWindow()
    XCTAssertTrue(try editor(in: host).string.contains("A private words"))
    try append(" more", in: host, fixture: f)
    XCTAssertEqual(
      ConversationDraft.split(f.state.conversationDraft(for: f.a.id)).text, "A private words more")
    XCTAssertEqual(
      ConversationDraft.split(f.state.conversationDraft(for: f.a.id)).attachments, [attachment])
    XCTAssertEqual(f.state.conversationDraft(for: f.b.id), "B private words")
    for conversation in [f.b, f.all, f.a, f.b] {
      f.view(conversation)
      XCTAssertEqual(f.state.resolvedComposerTarget(viewing: conversation)?.id, conversation.id)
    }
    XCTAssertEqual(f.state.conversationDraft(for: f.all.id), "Broadcast private words")
  }

  func testMainAndCardShareDraftAndCardSendGoesToItsOwnAgent() async throws {
    let f = try Fixture()
    defer { f.cleanup() }
    let attachment = try f.file("shared-a")
    f.state.setConversationDraft(
      ConversationDraft.compose(text: "Shared A", attachments: [attachment]), for: f.a.id)
    f.state.setConversationDraft("Separate B", for: f.b.id)
    f.view(f.a)
    let main = f.mainWindow()
    let card = try f.card(for: f.a)
    XCTAssertTrue(try editor(in: main).string.contains("Shared A"))
    XCTAssertTrue(try editor(in: card).string.contains("Shared A"))
    try append(" from main", in: main, fixture: f)
    XCTAssertTrue(try editor(in: card).string.contains("Shared A from main"))
    try append(" from card", in: card, fixture: f)
    XCTAssertTrue(try editor(in: main).string.contains("Shared A from main from card"))
    let shared = f.state.conversationDraft(for: f.a.id)
    XCTAssertEqual(ConversationDraft.split(shared).attachments, [attachment])
    f.view(f.b)
    XCTAssertTrue(try editor(in: main).string.contains("Separate B"))
    XCTAssertTrue(try editor(in: card).string.contains("Shared A from main from card"))
    try append(" stays B", in: main, fixture: f)
    XCTAssertEqual(f.state.conversationDraft(for: f.a.id), shared)
    XCTAssertEqual(f.state.conversationDraft(for: f.b.id), "Separate B stays B")
    f.view(f.a)
    XCTAssertEqual(f.state.resolvedComposerTarget(viewing: f.a)?.id, f.a.id)
    XCTAssertTrue(try editor(in: main).string.contains("Shared A from main from card"))
    let published = expectation(description: "card A sends to A")
    f.state.publishConversationText = { owner, text, _, _ in
      XCTAssertEqual(owner?.id, f.a.id)
      XCTAssertEqual(text, shared)
      published.fulfill()
    }
    let text = try editor(in: card)
    let enter = try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
        windowNumber: text.window?.windowNumber ?? 0, context: nil,
        characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36))
    text.keyDown(with: enter)
    await fulfillment(of: [published], timeout: 2)
    f.settle()
    XCTAssertEqual(f.state.conversationDraft(for: f.a.id), "")
    XCTAssertEqual(f.state.conversationDraft(for: f.b.id), "Separate B stays B")
    XCTAssertEqual(f.state.resolvedComposerTarget(viewing: f.a)?.id, f.a.id)
    XCTAssertEqual(try editor(in: main).string, "")
    XCTAssertEqual(try editor(in: card).string, "")
  }

  /// A reused digit is a new owner: the retired conversation has no recipient,
  /// its successor is its own, and neither inherits a draft or a link.
  func testReusedDigitNeverBorrowsTheRetiredOwnersRecipientDraftOrLink() throws {
    let f = try Fixture()
    defer { f.cleanup() }
    f.state.setConversationDraft("Old B words", for: f.b.id)
    XCTAssertTrue(f.state.toggleDraftSync(from: f.a.id, with: f.b.id))
    let replacement = try Fixture.agent(f.b.channel, session: "replacement-b")
    f.apply([f.a, replacement, f.all])
    f.view(f.a)
    XCTAssertEqual(f.state.resolvedComposerTarget(viewing: f.a)?.id, f.a.id)
    XCTAssertNil(
      f.state.resolvedComposerTarget(viewing: f.b), "the retired owner cannot be sent to")
    XCTAssertFalse(f.state.hasSynchronizedDraft(f.a.id), "retirement dissolves the link")
    f.view(replacement)
    XCTAssertEqual(f.state.resolvedComposerTarget(viewing: replacement)?.id, replacement.id)
    XCTAssertEqual(f.state.conversationDraft(for: replacement.id), "")
    XCTAssertFalse(f.state.hasSynchronizedDraft(replacement.id))
    XCTAssertEqual(f.state.conversationDraft(for: f.b.id), "Old B words")
    f.view(f.all)
    XCTAssertEqual(f.state.resolvedComposerTarget(viewing: f.all)?.id, f.all.id)
    let roster = DictationOverlayView(state: f.state).channelStatusView
    let write = try XCTUnwrap(roster.agentPillMenuActions(for: f.a.channel)?.write)
    XCTAssertTrue(write.enabled)
    write.perform()
    XCTAssertEqual(f.state.selectedConversationID, f.a.id)
    XCTAssertEqual(f.state.resolvedComposerTarget(viewing: f.a)?.id, f.a.id)
  }

  func testHoldReturnAfterNavigationRestoresOriginalDraftWithoutStealingFocus() throws {
    let f = try Fixture()
    defer { f.cleanup() }
    f.state.setConversationDraft("Original A", for: f.a.id)
    f.state.setConversationDraft("Keep B", for: f.b.id)
    f.view(f.a)
    let revision = f.state.prepareHeldComposerEdit(f.a)
    f.view(f.b)
    XCTAssertFalse(f.state.returnHeldComposerText("Held A", to: f.a, revision: revision))
    XCTAssertEqual(f.state.selectedConversationID, f.b.id)
    XCTAssertEqual(f.state.resolvedComposerTarget(viewing: f.b)?.id, f.b.id)
    XCTAssertEqual(f.state.conversationDraft(for: f.a.id), "Original A\n\nHeld A")
    XCTAssertEqual(f.state.conversationDraft(for: f.b.id), "Keep B")
    f.view(f.a)
    let secondRevision = f.state.prepareHeldComposerEdit(f.a)
    f.view(f.b)
    f.view(f.a)
    XCTAssertFalse(f.state.returnHeldComposerText("Later A", to: f.a, revision: secondRevision))
    XCTAssertEqual(f.state.conversationDraft(for: f.a.id), "Original A\n\nHeld A\n\nLater A")
  }

  func testWaitingCaptureAttachmentStaysInAThroughNavigation() throws {
    let f = try Fixture()
    defer { f.cleanup() }
    let path = try f.file("waiting-capture-a")
    f.apply([f.a, f.b, f.all], open: [f.a.channel])
    f.state.setConversationDraft("A beside its waiting attachment", for: f.a.id)
    f.state.setConversationDraft("B without A's attachment", for: f.b.id)
    f.view(f.a)
    XCTAssertTrue(f.state.stageConversationAttachments([path], in: f.a))
    let host = f.mainWindow()
    f.settle()
    XCTAssertTrue(try editor(in: host).string.contains("A beside its waiting attachment"))
    f.view(f.b)
    XCTAssertTrue(try editor(in: host).string.contains("B without A's attachment"))
    XCTAssertNil(f.state.captureAttachmentDrafts[f.b.id])
    f.view(f.all)
    XCTAssertNil(f.state.captureAttachmentDrafts[f.all.id])
    f.view(f.a)
    XCTAssertEqual(f.state.captureAttachmentDrafts[f.a.id]?.paths, [path])
    XCTAssertNil(f.state.captureAttachmentDrafts[f.a.id]?.target)
    XCTAssertTrue(FileManager.default.fileExists(atPath: path))
    XCTAssertEqual(f.state.resolvedComposerTarget(viewing: f.a)?.id, f.a.id)
  }

  private final class MicrophoneFixture: ComposerCaptureControlling, @unchecked Sendable {
    func isRecording() async -> Bool { false }
    func startComposerTurnRecording() async throws -> CsCaptureHandle {
      CsCaptureHandle(captureId: "per-conversation-microphone-a")
    }
    func stopComposerTurnRecording(handle: CsCaptureHandle) async throws -> CsConditionalStop {
      .noLiveCapture
    }
  }

  func testDraftMicrophoneAndItsErrorStayWithAThroughNavigation() async throws {
    let f = try Fixture()
    defer { f.cleanup() }
    f.state.draftCapture = OverlayDraftCapture(controller: MicrophoneFixture())
    f.state.setConversationDraft("Microphone A", for: f.a.id)
    f.state.setConversationDraft("Private B", for: f.b.id)
    f.state.setConversationDraft("Private zero", for: f.all.id)
    f.view(f.a)
    let host = f.mainWindow()
    await f.state.toggleDraftMicrophone(in: f.a)
    XCTAssertEqual(f.state.draftCapture.conversationID, f.a.id)
    for conversation in [f.b, f.all, f.a] {
      f.view(conversation)
      XCTAssertTrue(
        try editor(in: host).string.contains(f.state.conversationDraft(for: conversation.id)))
      XCTAssertEqual(f.state.draftCapture.conversationID, f.a.id)
    }
    f.view(f.b)
    await f.state.draftCapture.failed(
      sessionID: "per-conversation-microphone-a", message: "Only A's microphone failed")
    XCTAssertFalse(f.state.draftCapture.hasRequest)
    XCTAssertEqual(f.state.draftCapture.errorConversationID, f.a.id)
    XCTAssertNotNil(f.state.draftCapture.error)
    XCTAssertEqual(f.state.conversationDraft(for: f.b.id), "Private B")
    XCTAssertTrue(try editor(in: host).string.contains("Private B"))
    f.view(f.a)
    XCTAssertEqual(f.state.draftCapture.errorConversationID, f.a.id)
  }

  /// A releasable barrier keeps timeout/failure cleanup from stranding an async task.
  @MainActor
  private final class PublicationGate {
    private var continuation: CheckedContinuation<Void, Never>?
    private var released = false

    func wait(entered: XCTestExpectation) async {
      guard !released else { return }
      await withCheckedContinuation {
        continuation = $0
        entered.fulfill()
      }
    }

    func release() {
      released = true
      continuation?.resume()
      continuation = nil
    }
  }

  func testSuspendedSendFreezesPayloadAndClearsOnlyUnchangedSourceAfterNavigation() async throws {
    let f = try Fixture()
    defer { f.cleanup() }
    let path = try f.file("frozen-a")
    let payload = ConversationDraft.compose(text: "Send A", attachments: [path])
    let reference = try reply("frozen-reply")
    f.state.setConversationDraft(payload, for: f.a.id)
    f.state.conversationReplies[f.a.id] = reference
    f.state.setConversationDraft("Keep B", for: f.b.id)
    f.state.setConversationDraft("Keep zero", for: f.all.id)
    f.view(f.a)
    let entered = expectation(description: "frozen source publication is suspended")
    let gate = PublicationGate()
    defer { gate.release() }
    var owners: [String?] = []
    f.state.publishConversationText = { owner, text, reply, peers in
      owners.append(owner?.id)
      XCTAssertEqual(text, payload)
      XCTAssertEqual(reply, reference)
      XCTAssertTrue(peers.isEmpty)
      await gate.wait(entered: entered)
    }
    let send = Task { await f.state.sendConversationText(f.a) }
    await fulfillment(of: [entered], timeout: 2)
    XCTAssertEqual(f.state.pendingTextMessages, [f.a.id])
    f.view(f.all)
    gate.release()
    await send.value
    XCTAssertEqual(owners, [f.a.id])
    XCTAssertEqual(f.state.conversationDraft(for: f.a.id), "")
    XCTAssertNil(f.state.conversationReplies[f.a.id])
    XCTAssertEqual(f.state.conversationDraft(for: f.b.id), "Keep B")
    XCTAssertEqual(f.state.conversationDraft(for: f.all.id), "Keep zero")
    XCTAssertEqual(f.state.selectedConversationID, f.all.id)
    XCTAssertTrue(f.state.pendingTextMessages.isEmpty)
  }

  /// A's own digit is reused while A's send is in flight: the frozen send was
  /// A's, the successor inherits nothing, and a retired owner cannot send again.
  func testOwnerRetirementDuringSuspendedSendPreservesDraftAndRefusesSuccessor() async throws {
    let f = try Fixture()
    defer { f.cleanup() }
    let reference = try reply("retired-reply")
    f.state.setConversationDraft("Frozen A", for: f.a.id)
    f.state.conversationReplies[f.a.id] = reference
    f.state.setConversationDraft("Private B", for: f.b.id)
    f.view(f.a)
    let entered = expectation(description: "the retiring owner owns the in-flight send")
    let gate = PublicationGate()
    defer { gate.release() }
    var owners: [String?] = []
    f.state.publishConversationText = { owner, text, reply, _ in
      owners.append(owner?.id)
      XCTAssertEqual(text, "Frozen A")
      XCTAssertEqual(reply, reference)
      await gate.wait(entered: entered)
    }
    let send = Task { await f.state.sendConversationText(f.a) }
    await fulfillment(of: [entered], timeout: 2)
    let replacement = try Fixture.agent(f.a.channel, session: "retired-during-send")
    f.apply([replacement, f.b, f.all])
    f.view(replacement)
    f.state.setConversationDraft("Successor private draft", for: replacement.id)
    // Even an ABA edit is a new source generation.
    f.state.setConversationDraft("Intervening A", for: f.a.id)
    f.state.setConversationDraft("Frozen A", for: f.a.id)
    gate.release()
    await send.value
    XCTAssertEqual(owners, [f.a.id])
    XCTAssertEqual(f.state.conversationDraft(for: f.a.id), "Frozen A")
    XCTAssertEqual(f.state.conversationReplies[f.a.id], reference)
    XCTAssertEqual(f.state.conversationDraft(for: f.b.id), "Private B")
    XCTAssertEqual(f.state.conversationDraft(for: replacement.id), "Successor private draft")
    XCTAssertNil(f.state.resolvedComposerTarget(viewing: f.a))
    XCTAssertEqual(f.state.resolvedComposerTarget(viewing: replacement)?.id, replacement.id)
    XCTAssertTrue(f.state.pendingTextMessages.isEmpty)
    await f.state.sendConversationText(f.a)
    XCTAssertEqual(owners, [f.a.id], "A retired owner never retries onto its reused digit")
  }

  func testSuspendedSendKeepsNewerSourceGenerationAndErrorsOutOfViewedB() async throws {
    for refusal in [false, true] {
      let f = try Fixture()
      defer { f.cleanup() }
      f.state.setConversationDraft("Original A", for: f.a.id)
      f.state.setConversationDraft("Private B", for: f.b.id)
      f.view(f.a)
      let entered = expectation(description: "suspended publication, refusal=\(refusal)")
      let gate = PublicationGate()
      defer { gate.release() }
      var owners: [String?] = []
      f.state.publishConversationText = { owner, text, _, _ in
        owners.append(owner?.id)
        XCTAssertEqual(text, "Original A")
        await gate.wait(entered: entered)
        if refusal { throw CocoaError(.fileWriteNoPermission) }
      }
      let send = Task { await f.state.sendConversationText(f.a) }
      await fulfillment(of: [entered], timeout: 2)
      XCTAssertEqual(f.state.pendingTextMessages, [f.a.id])
      f.view(f.b)
      f.state.setConversationDraft("Intervening A", for: f.a.id)
      f.state.setConversationDraft("Original A", for: f.a.id)
      gate.release()
      await send.value
      XCTAssertEqual(owners, [f.a.id])
      XCTAssertEqual(f.state.conversationDraft(for: f.a.id), "Original A")
      XCTAssertEqual(f.state.conversationDraft(for: f.b.id), "Private B")
      XCTAssertEqual(f.state.selectedConversationID, f.b.id)
      XCTAssertTrue(f.state.pendingTextMessages.isEmpty)
      XCTAssertEqual(f.state.textMessageErrors[f.a.id] != nil, refusal)
      XCTAssertNil(f.state.textMessageErrors[f.b.id])
      f.view(f.a)
      XCTAssertEqual(f.state.textMessageErrors[f.a.id] != nil, refusal)
    }
  }

  private enum SuspendedSendReplyChange {
    case replacement
    case aba
    case unchanged
  }

  private func assertSuspendedSendReplyGeneration(
    _ change: SuspendedSendReplyChange, refusal: Bool,
    file: StaticString = #filePath, line: UInt = #line
  ) async throws {
    let f = try Fixture()
    defer { f.cleanup() }
    let payload = "A's unchanged reply text"
    let first = try reply("suspended-parent-1")
    let second = try reply("suspended-parent-2")
    let bReply = try reply("private-b-parent")
    f.state.setConversationDraft(payload, for: f.a.id)
    f.state.conversationReplies[f.a.id] = first
    f.state.setConversationDraft("B's private text", for: f.b.id)
    f.state.conversationReplies[f.b.id] = bReply
    f.state.setConversationDraft("Zero's private text", for: f.all.id)
    f.view(f.a)
    let recipient = try XCTUnwrap(
      f.state.resolvedComposerTarget(viewing: f.a), file: file, line: line)
    XCTAssertEqual(recipient.id, f.a.id, file: file, line: line)
    XCTAssertFalse(f.state.hasSynchronizedDraft(f.a.id), file: file, line: line)
    let draftRevision = f.state.conversationDraftRevisions[f.a.id]
    let entered = expectation(description: "ordinary send with a frozen reply is suspended")
    let gate = PublicationGate()
    defer { gate.release() }
    var owners: [OverlayConversationOwner?] = []
    var texts: [String] = []
    var parents: [OverlayUserReplyReference?] = []
    f.state.publishConversationText = { owner, text, parent, peers in
      owners.append(owner)
      texts.append(text)
      parents.append(parent)
      XCTAssertTrue(peers.isEmpty, file: file, line: line)
      await gate.wait(entered: entered)
      if refusal { throw CocoaError(.fileWriteNoPermission) }
    }
    // The main window's real send seam: the open conversation, no recipient argument.
    let send = Task { await f.state.sendConversationText(f.a) }
    await fulfillment(of: [entered], timeout: 2)
    XCTAssertEqual(owners.count, 1, file: file, line: line)
    XCTAssertEqual(f.state.pendingTextMessages, [f.a.id], file: file, line: line)
    XCTAssertTrue(f.state.textMessageErrors.isEmpty, file: file, line: line)

    // Mutate the actual reply-selection property while sendConversationText awaits.
    // No text edit can mask a missing reply-generation check on completion.
    var expectedReply = first
    switch change {
    case .replacement:
      f.state.conversationReplies[f.a.id] = second
      expectedReply = second
    case .aba:
      f.state.conversationReplies[f.a.id] = second
      f.state.conversationReplies[f.a.id] = first
    case .unchanged:
      break
    }
    XCTAssertEqual(f.state.conversationDraft(for: f.a.id), payload, file: file, line: line)
    XCTAssertEqual(
      f.state.conversationDraftRevisions[f.a.id], draftRevision, file: file, line: line)
    XCTAssertEqual(
      f.state.conversationReplies[f.a.id], expectedReply, file: file, line: line)
    f.view(f.b)
    XCTAssertEqual(f.state.pendingTextMessages, [f.a.id], file: file, line: line)
    XCTAssertTrue(f.state.textMessageErrors.isEmpty, file: file, line: line)

    gate.release()
    await send.value
    await Task.yield()
    XCTAssertEqual(
      owners.count, 1, "Reply edits must not automatically publish again",
      file: file, line: line)
    XCTAssertEqual(owners, [f.a.owner], file: file, line: line)
    XCTAssertEqual(texts, [payload], file: file, line: line)
    XCTAssertEqual(parents, [first], file: file, line: line)
    var shouldClear = false
    if case .unchanged = change { shouldClear = !refusal }
    XCTAssertEqual(
      f.state.conversationDraft(for: f.a.id), shouldClear ? "" : payload, file: file, line: line)
    XCTAssertEqual(
      f.state.conversationReplies[f.a.id], shouldClear ? nil : expectedReply,
      file: file, line: line)
    XCTAssertTrue(f.state.pendingTextMessages.isEmpty, file: file, line: line)
    XCTAssertEqual(f.state.textMessageErrors[f.a.id] != nil, refusal, file: file, line: line)
    XCTAssertNil(f.state.textMessageErrors[f.b.id], file: file, line: line)
    XCTAssertNil(f.state.textMessageErrors[f.all.id], file: file, line: line)
    XCTAssertEqual(f.state.selectedConversationID, f.b.id, file: file, line: line)
    XCTAssertEqual(
      f.state.conversationDraft(for: f.b.id), "B's private text", file: file, line: line)
    XCTAssertEqual(f.state.conversationReplies[f.b.id], bReply, file: file, line: line)
    XCTAssertEqual(
      f.state.conversationDraft(for: f.all.id), "Zero's private text", file: file, line: line)
    f.view(f.a)
    XCTAssertEqual(f.state.textMessageErrors[f.a.id] != nil, refusal, file: file, line: line)
    XCTAssertEqual(
      f.state.resolvedComposerTarget(viewing: f.a)?.id, f.a.id, file: file, line: line)
  }

  func testSuspendedSendReplyChangePreservesTextOnSuccess() async throws {
    try await assertSuspendedSendReplyGeneration(.replacement, refusal: false)
  }

  func testSuspendedSendReplyABAPreservesTextOnSuccess() async throws {
    try await assertSuspendedSendReplyGeneration(.aba, refusal: false)
  }

  func testSuspendedSendUnchangedReplyClearsOnSuccess() async throws {
    try await assertSuspendedSendReplyGeneration(.unchanged, refusal: false)
  }

  func testSuspendedSendReplyChangeRetainsTextAndSourceErrorOnRefusal() async throws {
    try await assertSuspendedSendReplyGeneration(.replacement, refusal: true)
  }

  func testSuspendedSendReplyABARetainsTextAndSourceErrorOnRefusal() async throws {
    try await assertSuspendedSendReplyGeneration(.aba, refusal: true)
  }

  func testSuspendedSendUnchangedReplyRetainsTextAndSourceErrorOnRefusal() async throws {
    try await assertSuspendedSendReplyGeneration(.unchanged, refusal: true)
  }

  /// Linked drafts with the same words: each agent receives its own
  /// publication and is told who else got it (Maciej, 2026-10-30 14:24: further
  /// recipients are the sync mechanism moved out of "+").
  func testLinkedIdenticalDraftsPublishToEachOwnerAndCoAddressTheOther() async throws {
    let f = try Fixture()
    defer { f.cleanup() }
    let path = try f.file("shared-publication")
    let payload = ConversationDraft.compose(text: "Same publication", attachments: [path])
    let reference = try reply("same-reply")
    f.state.setConversationDraft(payload, for: f.a.id)
    f.state.setConversationDraft(payload, for: f.b.id)
    f.state.conversationReplies[f.a.id] = reference
    f.state.conversationReplies[f.b.id] = reference
    f.view(f.a)
    XCTAssertTrue(f.state.toggleDraftSync(from: f.a.id, with: f.b.id))
    var owners: [String?] = []
    var peersByOwner: [String: [String]] = [:]
    f.state.publishConversationText = { owner, text, reply, peers in
      owners.append(owner?.id)
      if let owner { peersByOwner[owner.id] = peers.map(\.id) }
      XCTAssertEqual(text, payload)
      XCTAssertEqual(reply, reference)
    }
    await f.state.sendConversationText(f.a)
    XCTAssertEqual(owners, [f.a.id, f.b.id], "Each linked agent gets its own publication")
    XCTAssertEqual(peersByOwner[f.a.id], [f.b.id])
    XCTAssertEqual(peersByOwner[f.b.id], [f.a.id])
    for conversation in [f.a, f.b] {
      XCTAssertEqual(f.state.conversationDraft(for: conversation.id), "")
      XCTAssertNil(f.state.conversationReplies[conversation.id])
    }
    XCTAssertTrue(f.state.hasSynchronizedDraft(f.a.id))
    XCTAssertTrue(f.state.pendingTextMessages.isEmpty)
  }

  func testSyncKeepsPrivatePrefixesAttachmentsAndReplyParentsPerConversation() async throws {
    let f = try Fixture()
    defer { f.cleanup() }
    let aPath = try f.file("sync-private-a")
    let bPath = try f.file("sync-private-b")
    let aReply = try reply("sync-parent-a")
    let bReply = try reply("sync-parent-b")
    f.state.setConversationDraft(
      ConversationDraft.compose(text: "Private prefix A", attachments: [aPath]), for: f.a.id)
    f.state.setConversationDraft(
      ConversationDraft.compose(text: "Private prefix B", attachments: [bPath]), for: f.b.id)
    f.state.conversationReplies[f.a.id] = aReply
    f.state.conversationReplies[f.b.id] = bReply
    XCTAssertTrue(f.state.toggleDraftSync(from: f.a.id, with: f.b.id))
    f.view(f.a)
    let main = f.mainWindow()
    f.view(f.b)
    XCTAssertEqual(try editor(in: main).string, "Private prefix B")
    try append(" shared future", in: main, fixture: f)
    let aPayload = f.state.conversationDraft(for: f.a.id)
    let bPayload = f.state.conversationDraft(for: f.b.id)
    XCTAssertEqual(ConversationDraft.split(aPayload).text, "Private prefix A shared future")
    XCTAssertEqual(ConversationDraft.split(bPayload).text, "Private prefix B shared future")
    XCTAssertEqual(ConversationDraft.split(aPayload).attachments, [aPath])
    XCTAssertEqual(ConversationDraft.split(bPayload).attachments, [bPath])
    f.view(f.a)
    var owners: [String?] = []
    f.state.publishConversationText = { owner, text, parent, peers in
      owners.append(owner?.id)
      if owner?.id == f.a.id {
        XCTAssertEqual(text, aPayload)
        XCTAssertEqual(parent, aReply)
      } else {
        XCTAssertEqual(owner?.id, f.b.id)
        XCTAssertEqual(text, bPayload)
        XCTAssertEqual(parent, bReply)
      }
      XCTAssertTrue(peers.isEmpty, "Different private payloads are not co-addressed")
    }
    await f.state.sendConversationText(f.a)
    XCTAssertEqual(owners, [f.a.id, f.b.id])
    XCTAssertEqual(f.state.conversationDraft(for: f.a.id), "")
    XCTAssertEqual(f.state.conversationDraft(for: f.b.id), "")
    XCTAssertNil(f.state.conversationReplies[f.a.id])
    XCTAssertNil(f.state.conversationReplies[f.b.id])
  }
}
