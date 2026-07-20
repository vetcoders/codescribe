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
}
