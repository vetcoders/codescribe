import AppKit
import Foundation
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class AgentSendHoldTests: XCTestCase {
  func testStepperRoundTripsThroughTheSettingsViewModelAndReloads() {
    var stored = CsSettings.sample
    var writes: [(String, String)] = []
    let engine = MockSettingsEngine(
      settingsLoader: { stored },
      updateConfigObserver: { key, value in
        writes.append((key, value))
        if key == "CODESCRIBE_AGENT_SEND_HOLD_SEC", let seconds = UInt32(value) {
          stored.agentSendHoldSec = seconds
        }
      })
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
    XCTAssertEqual(model.agentSendHoldSeconds.wrappedValue, 10)
    for seconds in [0, 7, 10, 15, 30] {
      model.agentSendHoldSeconds.wrappedValue = seconds
      XCTAssertEqual(model.settings.agentSendHoldSec, UInt32(seconds))
      XCTAssertEqual(writes.last?.0, "CODESCRIBE_AGENT_SEND_HOLD_SEC")
      XCTAssertEqual(writes.last?.1, String(seconds))
    }
    let reopened = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
    XCTAssertEqual(reopened.agentSendHoldSeconds.wrappedValue, 30)
    model.agentSendHoldSeconds.wrappedValue = 31
    XCTAssertEqual(stored.agentSendHoldSec, 30)
    model.agentSendHoldSeconds.wrappedValue = -1
    XCTAssertEqual(stored.agentSendHoldSec, 0)
  }

  func testRejectedSettingWriteDoesNotInventANewSnapshot() {
    let model = SettingsViewModel(
      engine: MockSettingsEngine(updateConfigObserver: { _, _ in
        throw CocoaError(.fileWriteNoPermission)
      }), permissionProbe: MockPermissionProbe())
    model.agentSendHoldSeconds.wrappedValue = 0
    XCTAssertEqual(model.settings.agentSendHoldSec, 10)
    XCTAssertNotNil(model.lastError)
  }

  func testThreeControlCommandsCarryTheVerifiedOwnerAndOriginalBus() throws {
    let fixture = try Fixture()
    defer { try? FileManager.default.removeItem(at: fixture.home) }
    for control in OverlayDeliveryControl.allCases {
      let arguments = try RealAgentBridgeInstaller.deliveryControlArguments(
        control, identity: fixture.messageID, owner: fixture.owner,
        busPath: fixture.identity.bus, root: fixture.installer.bridgeRoot)
      XCTAssertEqual(
        arguments,
        [
          "--" + control.rawValue, fixture.messageID,
          "--channel", "1", "--provider", "junie", "--session", "held-fixture",
          "--lease", fixture.identity.leaseID, "--bus", fixture.identity.bus,
          "--bridge-home", fixture.installer.bridgeRoot.path,
        ])
      XCTAssertEqual(
        try RealAgentBridgeInstaller.deliveryControlArguments(
          control, identity: fixture.messageID, owner: nil,
          busPath: fixture.identity.bus, root: fixture.installer.bridgeRoot),
        [
          "--" + control.rawValue, fixture.messageID, "--channel", "0",
          "--bridge-home", fixture.installer.bridgeRoot.path,
        ])
    }
    for seconds in [UInt32(0), 7, 30] {
      let arguments = try RealAgentBridgeInstaller.busTextArguments(
        holdSeconds: seconds, owner: fixture.owner, root: fixture.installer.bridgeRoot)
      // The owner block stays right behind --send-text and the hold trails as
      // the last option, so publisher receipts keep their argument prefix.
      XCTAssertEqual(arguments.first, "--send-text")
      XCTAssertEqual(Array(arguments.suffix(2)), ["--hold-seconds", String(seconds)])
      XCTAssertEqual(Array(arguments.dropFirst().prefix(2)), ["--channel", "1"])
    }
    XCTAssertThrowsError(
      try RealAgentBridgeInstaller.deliveryControlArguments(
        .release, identity: fixture.messageID, owner: fixture.owner,
        busPath: fixture.home.appendingPathComponent("other.jsonl").path,
        root: fixture.installer.bridgeRoot))
    try fixture.writeLease(session: "foreign-session")
    XCTAssertThrowsError(
      try RealAgentBridgeInstaller.deliveryControlArguments(
        .withdraw, identity: fixture.messageID, owner: fixture.owner,
        busPath: fixture.identity.bus, root: fixture.installer.bridgeRoot))
  }

  func testComposerEscapeExitsRevisionModeWithoutSubmitting() throws {
    var cancelled = false
    var submitted = false
    let composer = OverlayConversationComposer(
      palette: .dark, draft: .constant("Poprawiona wiadomość"), sending: false,
      onSubmit: { submitted = true }, onCancel: { cancelled = true })
    let host = NSHostingView(rootView: composer)
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 480, height: 120)
    let window = NSWindow(
      contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    host.layoutSubtreeIfNeeded()
    func editor(in root: NSView) -> NSTextView? {
      if let text = root as? NSTextView, text.isEditable { return text }
      return root.subviews.lazy.compactMap { editor(in: $0) }.first
    }
    let text = try XCTUnwrap(editor(in: host))
    window.makeFirstResponder(text)
    let escape = try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
        windowNumber: window.windowNumber, context: nil,
        characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53))
    text.keyDown(with: escape)
    XCTAssertTrue(cancelled)
    XCTAssertFalse(submitted)
  }

  func testEnterRemovesTextFromNativeComposerBeforePublisherFinishesAndRestoresRefusal()
    async throws
  {
    for refuses in [false, true] {
      let fixture = try Fixture()
      defer { try? FileManager.default.removeItem(at: fixture.home) }
      let state = OverlayState()
      let conversation = OverlayConversation(
        id: fixture.owner.id, channel: "1", name: "fixture", owner: fixture.owner, messages: [])
      state.applyChannelRoster([
        CsChannelRosterState(
          channel: "1", audience: "fixture", provider: fixture.owner.provider,
          providerSessionId: fixture.owner.providerSessionID, open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
      ])
      state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
      state.setConversationDraft("Never paint two copies", for: conversation.id)
      let entered = expectation(description: "publisher is blocked")
      let gate = PublicationGate()
      defer { gate.release() }
      var sends = 0
      state.publishConversationText = { _, text, _, _ in
        sends += 1
        XCTAssertEqual(text, "Never paint two copies")
        await gate.wait(entered: entered)
        if refuses { throw CocoaError(.fileWriteNoPermission) }
      }
      let host = NSHostingView(rootView: SendingHarness(state: state, conversation: conversation))
      let window = backgroundWindow(host)
      defer { window.close() }
      let editor = try nativeEditor(in: host)
      editor.keyDown(with: try enter(in: window))
      await fulfillment(of: [entered], timeout: 2)
      settle(host)
      XCTAssertEqual(editor.string, "", "The helper is still blocked: its exit cannot clear the UI")
      XCTAssertEqual(editor.accessibilityLabel(), String(localized: "Sending"))
      XCTAssertFalse(editor.isEditable)
      XCTAssertEqual(sends, 1)
      // Enter while publication is pending must neither duplicate nor release anything.
      editor.keyDown(with: try enter(in: window))
      gate.release()
      let settled = expectation(description: "publication settles")
      Task { @MainActor in
        while state.pendingTextMessages.contains(conversation.id) { await Task.yield() }
        settled.fulfill()
      }
      await fulfillment(of: [settled], timeout: 2)
      settle(host)
      XCTAssertEqual(sends, 1)
      XCTAssertEqual(editor.string, refuses ? "Never paint two copies" : "")
      XCTAssertEqual(state.textMessageErrors[conversation.id] != nil, refuses)
      XCTAssertTrue(editor.isEditable)
    }
  }

  func testEmptyEnterReleasesTheLatestHeldMessageOnceAndNeverSendsAnotherText() async throws {
    let fixture = try Fixture()
    defer { try? FileManager.default.removeItem(at: fixture.home) }
    let until = ISO8601DateFormatter().string(from: Date().addingTimeInterval(60))
    var bus = OverlayChannelDelivery.Bus()
    for index in [1, 2] {
      bus.consume([
        "schema": "codescribe.agent-user-message.v1", "kind": "agent_user_message",
        "source": "typed", "audience": "fixture", "channel": "1",
        "message_id": String(format: "%024x", index), "text": "Held \(index)",
        "emitted_at": ISO8601DateFormatter().string(from: Date()), "hold_until": until,
        "sender": ["name": "founder", "channel": "10"],
        "recipients": [
          [
            "provider": fixture.owner.provider,
            "provider_session_id": fixture.owner.providerSessionID,
            "lease_id": fixture.owner.leaseID, "channel": "1", "name": "fixture",
          ]
        ],
      ])
    }
    let conversation = try XCTUnwrap(
      bus.conversations(busPath: fixture.identity.bus).first {
        $0.owner != nil
      })
    let released = expectation(description: "the latest held bubble releases")
    let gate = PublicationGate()
    defer { gate.release() }
    var controls: [String] = []
    let view = OverlayConversationView(
      conversation: conversation, palette: .dark, topInset: 0, bottomInset: 0,
      pendingControls: [], controlErrors: [:], onControl: { _, _ in },
      draft: .constant(""), sending: false, sendError: nil,
      onSend: { XCTFail("Empty Enter must not publish another message") },
      onDeliveryControl: { control, message, owner, _ in
        XCTAssertEqual(control, .release)
        XCTAssertEqual(owner, fixture.owner)
        controls.append(try XCTUnwrap(message.controlIdentity))
        await gate.wait(entered: released)
      })
    let host = NSHostingView(rootView: view)
    let window = backgroundWindow(host)
    defer { window.close() }
    let editor = try nativeEditor(in: host)
    editor.keyDown(with: try enter(in: window))
    await fulfillment(of: [released], timeout: 2)
    editor.keyDown(with: try enter(in: window))
    editor.keyDown(with: try enter(in: window, repeats: true))
    XCTAssertEqual(controls, [String(format: "%024x", 2)])
    gate.release()
  }

  func testEmptyEnterIsOptInAndKeyRepeatNeverSubmits() throws {
    var sends = 0
    let host = NSHostingView(
      rootView: OverlayConversationComposer(
        palette: .dark, draft: .constant(""), sending: false, onSubmit: { sends += 1 }))
    let window = backgroundWindow(host)
    defer { window.close() }
    let editor = try nativeEditor(in: host)
    editor.keyDown(with: try enter(in: window))
    editor.string = "A fresh message"
    editor.keyDown(with: try enter(in: window, repeats: true))
    XCTAssertEqual(sends, 0)
    editor.keyDown(with: try enter(in: window))
    XCTAssertEqual(sends, 1)
  }

  func testPendingHeldControlDoesNotHideOrLockANewUnsentDraft() async throws {
    let fixture = try Fixture()
    defer { try? FileManager.default.removeItem(at: fixture.home) }
    let conversation = heldConversation(fixture)
    let state = OverlayState()
    let entered = expectation(description: "release is still waiting for its receipt")
    let gate = PublicationGate()
    defer { gate.release() }
    let host = NSHostingView(
      rootView: SendingHarness(
        state: state, conversation: conversation,
        onSend: { XCTFail("Pending control must block another submission") },
        onDeliveryControl: { _, _, _, _ in
          state.setConversationDraft("Unsent draft stays here", for: conversation.id)
          await gate.wait(entered: entered)
        }))
    let window = backgroundWindow(host)
    defer { window.close() }
    let editor = try nativeEditor(in: host)
    editor.keyDown(with: try enter(in: window))
    await fulfillment(of: [entered], timeout: 2)
    settle(host)
    XCTAssertEqual(editor.string, "Unsent draft stays here")
    XCTAssertTrue(editor.isEditable)
    XCTAssertNotEqual(editor.accessibilityLabel(), String(localized: "Sending"))
    editor.keyDown(with: try enter(in: window))
    editor.insertText(
      " edited", replacementRange: NSRange(location: editor.string.utf16.count, length: 0))
    XCTAssertEqual(editor.string, "Unsent draft stays here edited")
    gate.release()
  }

  func testPendingPublicationDoesNotMaterializeAPastedAttachment() throws {
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    let pixel = try XCTUnwrap(
      Data(
        base64Encoded:
          "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a9l8AAAAASUVORK5CYII="
      ))
    XCTAssertTrue(board.setData(pixel, forType: .png))
    var saves = 0
    let host = NSHostingView(
      rootView: OverlayConversationComposer(
        palette: .dark, draft: .constant("Frozen payload"), sending: true,
        onSubmit: { XCTFail("Publication already pending") }, clipboard: board,
        saveImage: { _ in
          saves += 1
          throw CocoaError(.userCancelled)
        }))
    let window = backgroundWindow(host)
    defer { window.close() }
    let editor = try nativeEditor(in: host)
    editor.paste(nil)
    XCTAssertEqual(saves, 0, "The retained outgoing draft must not adopt a new file while hidden")
    XCTAssertEqual(editor.string, "")
  }

  func testEmptyEnterDoesNotReleaseABroadcastMirrorExpiredOrUncontrollableMessage() throws {
    let fixture = try Fixture()
    defer { try? FileManager.default.removeItem(at: fixture.home) }
    let original = heldConversation(fixture)
    for mode in ["broadcast", "expired", "no-identity"] {
      var message = try XCTUnwrap(original.messages.first)
      if mode == "broadcast" { message.sourceChannel = "0" }
      if mode == "expired" { message.holdUntil = Date().addingTimeInterval(-1) }
      if mode == "no-identity" { message.controlIdentity = nil }
      let conversation = OverlayConversation(
        id: original.id, channel: original.channel, name: original.name, owner: original.owner,
        messages: [message])
      let view = OverlayConversationView(
        conversation: conversation, palette: .dark, topInset: 0, bottomInset: 0,
        pendingControls: [], controlErrors: [:], onControl: { _, _ in },
        draft: .constant(""), sending: false, sendError: nil,
        onSend: { XCTFail("Empty composer must not send") },
        onDeliveryControl: { _, _, _, _ in XCTFail("Must not release \(mode)") })
      let host = NSHostingView(rootView: view)
      let window = backgroundWindow(host)
      defer { window.close() }
      try nativeEditor(in: host).keyDown(with: enter(in: window))
      settle(host)
    }
  }

  private func heldConversation(_ fixture: Fixture) -> OverlayConversation {
    let message = OverlayConversationMessage(
      id: "typed:" + fixture.messageID, kind: .user, text: "Held text", order: 1,
      emittedAt: ISO8601DateFormatter().string(from: Date()), owner: nil,
      recipients: [
        .init(owner: fixture.owner, queued: false, accepted: false, acknowledged: false)
      ],
      deliveryID: nil, replyTo: nil, unsolicited: false, busPath: fixture.identity.bus,
      sourceChannel: "1", holdUntil: Date().addingTimeInterval(60),
      controlIdentity: fixture.messageID, deliverySealed: true)
    return OverlayConversation(
      id: fixture.owner.id, channel: "1", name: "fixture", owner: fixture.owner, messages: [message]
    )
  }

  private struct SendingHarness: View {
    let state: OverlayState
    let conversation: OverlayConversation
    var onSend: (() -> Void)? = nil
    var onDeliveryControl:
      @MainActor (
        OverlayDeliveryControl, OverlayConversationMessage, OverlayConversationOwner?, String?
      ) async throws -> Void = { _, _, _, _ in XCTFail("Unexpected control") }
    var body: some View {
      OverlayConversationView(
        conversation: conversation, palette: .dark, topInset: 0, bottomInset: 0,
        pendingControls: [], controlErrors: [:], onControl: { _, _ in },
        draft: Binding(
          get: { state.conversationDraft(for: conversation.id) },
          set: { state.setConversationDraft($0, for: conversation.id) }),
        sending: state.pendingTextMessages.contains(conversation.id),
        sendError: state.textMessageErrors[conversation.id],
        onSend: {
          if let onSend {
            onSend()
          } else {
            Task { await state.sendConversationText(conversation) }
          }
        },
        onDeliveryControl: onDeliveryControl)
    }
  }

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

  private func backgroundWindow<Content: View>(_ host: NSHostingView<Content>) -> NSWindow {
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 480, height: 360)
    let window = NSWindow(
      contentRect: NSRect(x: -10000, y: -10000, width: 480, height: 360),
      styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    BackgroundTestWindow.present(window)
    host.layoutSubtreeIfNeeded()
    return window
  }

  private func settle(_ host: NSView) {
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.02))
    host.layoutSubtreeIfNeeded()
  }

  private func nativeEditor(in root: NSView) throws -> NSTextView {
    if let editor = root as? NSTextView,
      editor.accessibilityIdentifier() == "overlay-conversation-editor"
    {
      return editor
    }
    for child in root.subviews {
      if let editor = try? nativeEditor(in: child) { return editor }
    }
    throw CocoaError(.coderValueNotFound)
  }

  private func enter(in window: NSWindow, repeats: Bool = false) throws -> NSEvent {
    try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
        windowNumber: window.windowNumber, context: nil, characters: "\r",
        charactersIgnoringModifiers: "\r", isARepeat: repeats, keyCode: 36))
  }

  private struct Fixture {
    let home: URL
    let installer: RealAgentBridgeInstaller
    let identity: AgentPlaybackIdentity
    let owner: OverlayConversationOwner
    let messageID = String(repeating: "c", count: 24)

    init() throws {
      let repo = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
      home = repo.appendingPathComponent("build/test-scratch/\(UUID().uuidString)")
      try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
      installer = RealAgentBridgeInstaller(resourceRoot: nil, homeDirectory: home, environment: [:])
      identity = AgentPlaybackIdentity(
        provider: "junie", session: "held-fixture",
        bus: home.appendingPathComponent("fixture.jsonl").path)
      owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": identity.provider, "provider_session_id": identity.session,
          "lease_id": identity.leaseID, "channel": "1", "name": "fixture",
        ]))
      try writeLease(session: identity.session)
    }

    func writeLease(session: String) throws {
      let lease = installer.bridgeRoot.appendingPathComponent("leases/\(identity.leaseID).json")
      try FileManager.default.createDirectory(
        at: lease.deletingLastPathComponent(), withIntermediateDirectories: true)
      try JSONSerialization.data(withJSONObject: [
        "schema": "codescribe.agent-bridge.lease.v1", "provider": identity.provider,
        "provider_session_id": session, "lease_id": identity.leaseID, "bus": identity.bus,
      ]).write(to: lease)
    }
  }
}
