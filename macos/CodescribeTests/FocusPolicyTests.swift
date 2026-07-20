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
}
