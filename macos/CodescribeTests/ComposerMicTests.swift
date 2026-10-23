import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class ComposerMicTests: XCTestCase {
  func testEveryStateKeepsTheMicrophoneGlyph() {
    for state in ComposerMicVisualState.allCases {
      guard case .mic = state.icon else {
        return XCTFail("\(state) lost the microphone glyph")
      }
    }
  }

  func testStateAccessibilityLabelsAreExplicit() {
    XCTAssertEqual(ComposerMicVisualState.idle.accessibilityLabel, "Start voice input")
    XCTAssertEqual(ComposerMicVisualState.preparing.accessibilityLabel, "Preparing voice input")
    XCTAssertEqual(ComposerMicVisualState.recording.accessibilityLabel, "Stop voice input")
    XCTAssertEqual(
      ComposerMicVisualState.blocked.accessibilityLabel,
      "Microphone busy with shortcut dictation"
    )
    XCTAssertEqual(ComposerAccessibility.micIdentifier, "agent-composer-mic")
  }

  func testOnlyIdleAndRecordingAreActionable() {
    XCTAssertTrue(ComposerMicVisualState.idle.isEnabled)
    XCTAssertTrue(ComposerMicVisualState.recording.isEnabled)
    XCTAssertFalse(ComposerMicVisualState.preparing.isEnabled)
    XCTAssertFalse(ComposerMicVisualState.blocked.isEnabled)
  }

  func testOverlayComposerUsesLiveGlyphOnlyForTheAddressedConversation() throws {
    func conversation(_ channel: String, session: String, lease: String) throws
      -> OverlayConversation
    {
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": session, "lease_id": lease,
          "name": session, "channel": channel,
        ]))
      return OverlayConversation(
        id: owner.id, channel: channel, name: session, owner: owner, messages: [])
    }
    let addressed = try conversation("2", session: "live", lease: String(repeating: "a", count: 32))
    let elsewhere = try conversation(
      "3", session: "other", lease: String(repeating: "b", count: 32))
    let state = OverlayState()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [addressed, elsewhere]))
    func roster(open: Bool) -> [CsChannelRosterState] {
      [
        .init(
          channel: "0", audience: "*", provider: "", providerSessionId: "", open: false,
          loud: false, autosealDeadlineUnixMs: nil, followerAlive: false, unreadSinceUnixMs: nil),
        .init(
          channel: "2", audience: "live", provider: "codex", providerSessionId: "live", open: open,
          loud: open, autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil),
        .init(
          channel: "3", audience: "other", provider: "codex", providerSessionId: "other",
          open: false,
          loud: false, autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil),
      ]
    }
    func composer(_ conversation: OverlayConversation) -> OverlayConversationComposer {
      OverlayConversationComposer(
        palette: .dark, draft: .constant(""), sending: false, onSubmit: {},
        liveSpeechActive: state.conversationMicrophoneOpen(conversation))
    }
    state.applyChannelRoster(roster(open: true))
    XCTAssertEqual(
      composer(addressed).draftMicrophoneSymbol, OverlayAgentAudioControls.liveSpeechSymbol)
    XCTAssertEqual(
      composer(addressed).draftMicrophoneAccessibilityLabel, String(localized: "Live speech active")
    )
    XCTAssertEqual(composer(elsewhere).draftMicrophoneSymbol, "mic")
    XCTAssertEqual(
      composer(elsewhere).draftMicrophoneAccessibilityLabel, String(localized: "Dictate message"))
    state.applyChannelRoster(roster(open: false))
    XCTAssertEqual(composer(addressed).draftMicrophoneSymbol, "mic")
    XCTAssertEqual(
      composer(addressed).draftMicrophoneAccessibilityLabel, String(localized: "Dictate message"))
  }

  func testOverlayComposerKeepsDraftRecordingGlyphWhenNoLiveTakeIsAddressed() {
    let composer = OverlayConversationComposer(
      palette: .dark, draft: .constant(""), sending: false, onSubmit: {},
      draftMicrophoneRecording: true)
    XCTAssertEqual(composer.draftMicrophoneSymbol, "mic.fill")
    XCTAssertEqual(composer.draftMicrophoneAccessibilityLabel, String(localized: "Stop dictation"))
  }

  func testPendingAttachmentPreviewUsesExactStagedURL() {
    let url = URL(fileURLWithPath: "/tmp/exact-staged-preview.png")
    let pending = PendingAttachment(url: url)

    XCTAssertEqual(pending.previewAttachment.url, url)
    XCTAssertEqual(pending.previewAttachment.name, url.lastPathComponent)
    XCTAssertEqual(pending.previewAttachment.type, "image/png")
  }
}
