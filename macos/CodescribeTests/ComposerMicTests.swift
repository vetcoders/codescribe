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

    func testFinalPassRegressionKeepsLongerLiveTranscriptAndPreservesAlternative() {
        let store = AgentChatStore()
        store.beginDictationPreviewSession()
        store.updateDictationPreview("one two three four five six seven eight nine ten")

        let result = store.resolveDictationDelivery(final: "one two three", autoSend: true)

        XCTAssertEqual(result.text, "one two three four five six seven eight nine ten")
        XCTAssertTrue(result.autoSend)
        XCTAssertEqual(store.dictationFinalPreview, "one two three")
        XCTAssertTrue(store.dictationFinalChangedText)
        XCTAssertEqual(store.dictationDeliverySource, .live)
    }

    func testUserEditedPreviewWinsAndCancelsAssistiveAutoSend() {
        let store = AgentChatStore()
        store.beginDictationPreviewSession()
        store.updateDictationPreview("live machine text")
        store.editDictationPreview("human owned text")
        store.updateDictationPreview("later callback must not overwrite the edit")

        let result = store.resolveDictationDelivery(final: "final machine text", autoSend: true)

        XCTAssertEqual(result.text, "human owned text")
        XCTAssertFalse(result.autoSend)
        XCTAssertTrue(store.dictationPreviewUserEdited)
        XCTAssertEqual(store.dictationDeliverySource, .edited)
    }

    func testPendingAttachmentPreviewUsesExactStagedURL() {
        let url = URL(fileURLWithPath: "/tmp/exact-staged-preview.png")
        let pending = PendingAttachment(url: url)

        XCTAssertEqual(pending.previewAttachment.url, url)
        XCTAssertEqual(pending.previewAttachment.name, url.lastPathComponent)
        XCTAssertEqual(pending.previewAttachment.type, "image/png")
    }
}
