import XCTest

@testable import Codescribe

/// Full 2³ matrix for the composer's ⌘V routing (`Composer.pasteDisposition`).
/// Semantics: file URLs always win; an image stages as a file even with
/// a filename/URL text representation; text-only copies remain editable text.
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

  func testImageWithTextStagesImage() {
    // Image copies can include a filename or URL as their text representation.
    XCTAssertEqual(
      Composer.pasteDisposition(hasFileURLs: false, hasImage: true, hasText: true),
      .stageImage
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
