import AppKit
import XCTest
@testable import Codescribe

@MainActor
final class ComposerTextViewTests: XCTestCase {
    func testHeightGrowsFromOneLineAndClampsAtEight() {
        let lineHeight: CGFloat = 20

        XCTAssertEqual(
            ComposerTextLayout.resolve(contentHeight: 1, lineHeight: lineHeight),
            ComposerTextLayout(height: 26, isVerticallyScrollable: false)
        )
        XCTAssertEqual(
            ComposerTextLayout.resolve(contentHeight: 86, lineHeight: lineHeight),
            ComposerTextLayout(height: 86, isVerticallyScrollable: false)
        )
        XCTAssertEqual(
            ComposerTextLayout.resolve(contentHeight: 300, lineHeight: lineHeight),
            ComposerTextLayout(height: 166, isVerticallyScrollable: true)
        )
    }

    func testLongNativePayloadMeasuresBeyondCapAndEnablesScrolling() {
        let fontSize: CGFloat = 13.5
        let text = (1...12).map { "Zażółć gęślą jaźń — line \($0)" }.joined(separator: "\n")
        let contentHeight = ComposerTextLayout.contentHeight(
            text: text,
            width: 320,
            fontSize: fontSize
        )
        let layout = ComposerTextLayout.resolve(
            contentHeight: contentHeight,
            lineHeight: ComposerTextLayout.lineHeight(fontSize: fontSize)
        )

        XCTAssertTrue(layout.isVerticallyScrollable)
        XCTAssertEqual(
            layout.height,
            ComposerTextLayout.lineHeight(fontSize: fontSize) * 8
                + ComposerTextLayout.verticalPadding
        )
    }

    func testNativeMeasurementRespondsToVisualWrappingWidth() {
        let text = String(repeating: "Zażółć gęślą jaźń payload ", count: 12)
        let narrow = ComposerTextLayout.contentHeight(text: text, width: 180, fontSize: 13.5)
        let wide = ComposerTextLayout.contentHeight(text: text, width: 520, fontSize: 13.5)

        XCTAssertGreaterThan(narrow, wide)
    }

    func testReturnSendsAndShiftReturnInsertsNewline() {
        XCTAssertEqual(
            ComposerTextKeyDisposition.resolve(keyCode: 36, modifiers: [], hasMarkedText: false),
            .send
        )
        XCTAssertEqual(
            ComposerTextKeyDisposition.resolve(keyCode: 36, modifiers: .shift, hasMarkedText: false),
            .insertNewline
        )
        XCTAssertEqual(
            ComposerTextKeyDisposition.resolve(keyCode: 76, modifiers: [], hasMarkedText: false),
            .send
        )
    }

    func testIMEAndModifiedReturnRemainNative() {
        XCTAssertEqual(
            ComposerTextKeyDisposition.resolve(keyCode: 36, modifiers: [], hasMarkedText: true),
            .native
        )
        for modifier in [NSEvent.ModifierFlags.command, .control, .option] {
            XCTAssertEqual(
                ComposerTextKeyDisposition.resolve(
                    keyCode: 36,
                    modifiers: modifier,
                    hasMarkedText: false
                ),
                .native
            )
        }
        XCTAssertEqual(
            ComposerTextKeyDisposition.resolve(keyCode: 0, modifiers: [], hasMarkedText: false),
            .native
        )
    }

    func testTextSurfaceHasStableAccessibilityIdentifier() {
        XCTAssertEqual(ComposerAccessibility.textViewIdentifier, "agent-composer-text")
    }
}
