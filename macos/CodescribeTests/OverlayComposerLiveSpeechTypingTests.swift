import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Maciej, 2026-11-08 11:45Z, dictating to channel 4 in the Agents window:
/// "Nie mogę pisać w Composerze w momencie, kiedy dyktuję". The composer of
/// the conversation whose microphone is open keeps the keyboard and every
/// typed character while live speech updates the roster and the conversation.
///
/// The panel is never shown and never takes the keyboard: the probe runs on
/// the Founder's desktop and must not steal focus.
@MainActor
final class OverlayComposerLiveSpeechTypingTests: XCTestCase {
  private func ownerConversation(
    _ name: String, channel: String, session: String, lease: String, replies: Int
  ) throws -> OverlayConversation {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session,
        "lease_id": String(repeating: lease, count: 32), "channel": channel, "name": name,
      ]))
    let messages = (0..<replies).map { index in
      OverlayConversationMessage(
        id: "reply:\(name)-\(index)", kind: .reply, text: "reply \(index)", order: UInt64(index),
        emittedAt: "2026-11-08T11:40:\(String(format: "%02d", index))Z", owner: owner,
        recipients: [], deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil,
        busPath: "")
    }
    return OverlayConversation(
      id: owner.id, channel: channel, name: name, owner: owner, messages: messages)
  }

  private func roster(
    _ conversations: [OverlayConversation], open channel: String?, deadline: Int64? = nil
  ) -> [CsChannelRosterState] {
    conversations.map {
      let live = $0.channel == channel
      return CsChannelRosterState(
        channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
        providerSessionId: $0.owner?.providerSessionID, open: live, loud: live,
        autosealDeadlineUnixMs: live ? deadline : nil,
        followerAlive: $0.owner == nil ? nil : true, unreadSinceUnixMs: nil)
    }
  }

  private func hiddenPanel(_ root: some View) -> (FloatingOverlayPanel, NSView) {
    let host = NSHostingView(rootView: root)
    host.sizingOptions = []
    let panel = FloatingOverlayPanel(
      contentRect: CGRect(x: 0, y: 0, width: 700, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered,
      defer: false)
    panel.isReleasedWhenClosed = false
    panel.delegate = panel
    panel.contentView = host
    settle(host)
    return (panel, host)
  }

  private func settle(_ host: NSView) {
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    host.layoutSubtreeIfNeeded()
  }

  private func composerEditor(in view: NSView) -> NSTextView? {
    if let text = view as? NSTextView,
      String(describing: type(of: text)).contains("MessageTextView")
    {
      return text
    }
    for subview in view.subviews {
      if let text = composerEditor(in: subview) { return text }
    }
    return nil
  }

  private func press(_ character: String, into panel: NSWindow) throws {
    let key = try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
        windowNumber: panel.windowNumber, context: nil, characters: character,
        charactersIgnoringModifiers: character, isARepeat: false, keyCode: 0))
    panel.sendEvent(key)
  }

  func testTypingKeepsEveryCharacterWhileLiveSpeechUpdatesTheConversation() throws {
    let broadcast = OverlayConversation(
      id: "live-all", channel: "0", name: "All", owner: nil, messages: [])
    var andrzej = try ownerConversation(
      "andrzej", channel: "4", session: "andrzej-session", lease: "d", replies: 1)
    let lena = try ownerConversation(
      "lena", channel: "2", session: "lena-session", lease: "a", replies: 1)
    let all = [broadcast, lena, andrzej]
    let state = OverlayState.previewFormatted()
    state.applyChannelRoster(roster(all, open: nil))
    state.applyConversationSnapshot(.init(deliveries: [], conversations: all))
    state.selectConversation(andrzej.id)
    let (panel, host) = hiddenPanel(DictationOverlayView(state: state))
    defer {
      panel.makeFirstResponder(nil)
      panel.close()
    }
    let editor = try XCTUnwrap(composerEditor(in: host), "the Agents canvas shows a composer")
    XCTAssertTrue(panel.makeFirstResponder(editor))
    try press("a", into: panel)
    XCTAssertEqual(editor.string, "a", "typing reaches the composer before any speech")
    XCTAssertEqual(state.conversationDraft(for: andrzej.id), "a")

    // The microphone of this conversation opens: live speech to channel 4.
    state.applyChannelRoster(roster(all, open: "4", deadline: 1_762_602_000_000))
    settle(host)
    var typed = "a"
    for (step, next) in ["b", "c", "d", "e"].enumerated() {
      // Live speech moves the roster (autoseal deadline) and the conversation.
      state.applyChannelRoster(
        roster(all, open: "4", deadline: 1_762_602_000_000 + Int64(step + 1) * 1_000))
      andrzej = try ownerConversation(
        "andrzej", channel: "4", session: "andrzej-session", lease: "d", replies: step + 2)
      state.applyConversationSnapshot(
        .init(deliveries: [], conversations: [broadcast, lena, andrzej]))
      settle(host)
      let current = try XCTUnwrap(
        composerEditor(in: host), "live speech step \(step): the composer stays on screen")
      XCTAssertTrue(
        current === editor, "live speech step \(step): the composer is not rebuilt under the hands")
      XCTAssertTrue(
        panel.firstResponder === editor,
        "live speech step \(step): the composer keeps the keyboard, first responder is "
          + String(describing: panel.firstResponder.map { type(of: $0) }))
      XCTAssertTrue(current.isEditable, "live speech step \(step): the composer stays editable")
      try press(next, into: panel)
      typed += next
      XCTAssertEqual(editor.string, typed, "live speech step \(step): the key reaches the field")
      XCTAssertEqual(
        state.conversationDraft(for: andrzej.id), typed,
        "live speech step \(step): the typed text is the conversation's draft")
    }
  }
}
