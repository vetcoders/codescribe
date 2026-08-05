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

        let scrollView = NSScrollView()
        let scrolledTextView = NSTextView()
        scrollView.documentView = scrolledTextView
        let clipView = scrollView.contentView

        XCTAssertFalse(
            CSFocusPolicy.shouldReleaseFocus(for: .pointer, hitView: NSTextField())
        )
        XCTAssertFalse(
            CSFocusPolicy.shouldReleaseFocus(for: .pointer, hitView: textViewChild),
            "clicking inside a text editor must not dismiss its first responder"
        )
        XCTAssertFalse(
            CSFocusPolicy.shouldReleaseFocus(for: .pointer, hitView: clipView),
            "TextEditor scroll/clip layers must preserve the backing text view's focus"
        )
        XCTAssertTrue(
            CSFocusPolicy.shouldReleaseFocus(for: .pointer, hitView: NSView())
        )
    }

    // MARK: - Agent sidebar: two states, never a hole

    func testSidebarToggleSwapsModesInsteadOfRemovingTheRail() {
        XCTAssertEqual(AgentSidebarMode.toggled(.expanded), .compact)
        XCTAssertEqual(AgentSidebarMode.toggled(.compact), .expanded)
        XCTAssertEqual(
            AgentSidebarMode.toggled(AgentSidebarMode.toggled(.expanded)),
            .expanded,
            "toggling twice must return to the starting state"
        )
    }

    func testCompactRailKeepsAPositiveFixedWidthAndExpandedStaysResizable() {
        let compact = AgentSidebarMode.compact
        XCTAssertGreaterThan(
            compact.minimumWidth, 0,
            "a collapsed rail must still occupy the column — width 0 is the empty-band bug"
        )
        XCTAssertEqual(compact.minimumWidth, compact.maximumWidth, "compact strip is not resizable")
        XCTAssertEqual(compact.idealWidth, AgentSidebarMode.compactWidth)

        let expanded = AgentSidebarMode.expanded
        XCTAssertLessThan(
            expanded.minimumWidth, expanded.maximumWidth,
            "expanded rail must keep a drag-resizable range"
        )
        XCTAssertGreaterThan(expanded.minimumWidth, compact.maximumWidth)
    }

    // MARK: - W10-A voice reveal policy

    func testVoiceDeliveryNeverActivates() {
        XCTAssertEqual(AgentRevealPolicy.intent(activating: false), .voiceDelivery)
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
        XCTAssertEqual(AgentRevealPolicy.intent(activating: true), .explicitOpen)
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
        let shiftLabel = ArmGestureCopy.label(for: "shift")
        let cmdLabel = ArmGestureCopy.label(for: "cmd")
        XCTAssertEqual(shiftLabel, "Hold Fn+Shift")
        XCTAssertEqual(cmdLabel, "Hold Fn+Command")
        XCTAssertNotEqual(shiftLabel, cmdLabel)
    }

}
