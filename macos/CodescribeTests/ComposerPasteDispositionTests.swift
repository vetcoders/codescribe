import XCTest
@testable import Codescribe

/// Full 2³ matrix for the composer's ⌘V routing (`Composer.pasteDisposition`).
/// Semantics: file URLs always win; a bare image (no text) stages as a file;
/// any text present — including image+text from a browser copy — pastes as text.
@MainActor
final class ComposerPasteDispositionTests: XCTestCase {
    func testFileURLsStageFilesRegardlessOfImageAndText() {
        // Finder copy carries the file name as a string too — files still win.
        XCTAssertEqual(
            Composer.pasteDisposition(hasFileURLs: true, hasImage: false, hasText: false),
            .stageFiles
        )
        XCTAssertEqual(
            Composer.pasteDisposition(hasFileURLs: true, hasImage: false, hasText: true),
            .stageFiles
        )
        XCTAssertEqual(
            Composer.pasteDisposition(hasFileURLs: true, hasImage: true, hasText: false),
            .stageFiles
        )
        XCTAssertEqual(
            Composer.pasteDisposition(hasFileURLs: true, hasImage: true, hasText: true),
            .stageFiles
        )
    }

    func testBareImageStagesImage() {
        // Screenshot to clipboard (⌘⇧⌃4): TIFF/PNG payload, no text.
        XCTAssertEqual(
            Composer.pasteDisposition(hasFileURLs: false, hasImage: true, hasText: false),
            .stageImage
        )
    }

    func testImageWithTextPassesThroughAsText() {
        // Browser copy puts image + text on the pasteboard — the user expects TEXT.
        XCTAssertEqual(
            Composer.pasteDisposition(hasFileURLs: false, hasImage: true, hasText: true),
            .passthroughText
        )
    }

    func testTextOnlyPassesThrough() {
        XCTAssertEqual(
            Composer.pasteDisposition(hasFileURLs: false, hasImage: false, hasText: true),
            .passthroughText
        )
    }

    func testEmptyPasteboardPassesThrough() {
        // Nothing usable — let the field editor no-op natively.
        XCTAssertEqual(
            Composer.pasteDisposition(hasFileURLs: false, hasImage: false, hasText: false),
            .passthroughText
        )
    }
}
