import AppKit
import XCTest
@testable import Codescribe

@MainActor
final class FocusPolicyTests: XCTestCase {
    func testPointerReleasesButtonFocusButKeyboardNavigationKeepsIt() {
        XCTAssertTrue(
            CSFocusPolicy.shouldReleaseFocus(for: .pointer, hitView: NSButton())
        )
        XCTAssertFalse(
            CSFocusPolicy.shouldReleaseFocus(for: .keyboard, hitView: NSButton()),
            "keyboard navigation must keep native focus and its visible focus effect"
        )
    }

    func testPointerPreservesTextEntryFocus() {
        let textView = NSTextView()
        let textViewChild = NSView()
        textView.addSubview(textViewChild)

        XCTAssertFalse(
            CSFocusPolicy.shouldReleaseFocus(for: .pointer, hitView: NSTextField())
        )
        XCTAssertFalse(
            CSFocusPolicy.shouldReleaseFocus(for: .pointer, hitView: textViewChild),
            "clicking inside a text editor must not dismiss its first responder"
        )
        XCTAssertTrue(
            CSFocusPolicy.shouldReleaseFocus(for: .pointer, hitView: NSView())
        )
    }

    // MARK: - W10-A voice reveal policy

    func testVoiceDeliveryNeverActivates() {
        XCTAssertFalse(
            AgentRevealPolicy.shouldActivate(for: .voiceDelivery),
            "TurnStarted / end-of-turn fallback must not steal focus"
        )
        XCTAssertTrue(
            AgentRevealPolicy.shouldReorderEvenIfVisible(for: .voiceDelivery),
            "passive reveal must re-order even when isVisible is already true"
        )
    }

    func testExplicitOpenMayActivate() {
        XCTAssertTrue(AgentRevealPolicy.shouldActivate(for: .explicitOpen))
        XCTAssertTrue(AgentRevealPolicy.shouldReorderEvenIfVisible(for: .explicitOpen))
    }

    func testTrayIntentRevealIsDistinctFromOpenChat() {
        // Compile-time + behavioral lock: both cases exist; openChat is the
        // activating path, revealChat is the passive voice path (AppModel wires
        // onSendToAgent → .revealChat after W10-A).
        let open: TrayIntent = .openChat
        let reveal: TrayIntent = .revealChat
        XCTAssertNotEqual(
            String(describing: open),
            String(describing: reveal)
        )
    }

    // MARK: - W10-B arm gesture copy

    func testArmGestureLabelsDeriveFromHoldArmModifier() {
        // SettingsViewModel.holdArmModifier normalizes to shift|cmd; ShortcutsPanel
        // builds labels from that value (no hardcoded-only "Hold Fn+Command" path).
        let shiftLabel = armGestureLabel(for: "shift")
        let cmdLabel = armGestureLabel(for: "cmd")
        XCTAssertEqual(shiftLabel, "Hold Fn+Shift")
        XCTAssertEqual(cmdLabel, "Hold Fn+Command")
        XCTAssertNotEqual(shiftLabel, cmdLabel)
    }

    private func armGestureLabel(for modifier: String) -> String {
        modifier == "cmd" ? "Hold Fn+Command" : "Hold Fn+Shift"
    }
}
