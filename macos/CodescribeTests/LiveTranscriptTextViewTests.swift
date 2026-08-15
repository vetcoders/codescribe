import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class LiveTranscriptTextViewTests: XCTestCase {
  func testLiveTranscriptIsReadOnlySelectableAndAcceptsFirstClick() {
    let textView = LiveTranscriptTextView.makeTextView()

    XCTAssertFalse(textView.isEditable)
    XCTAssertTrue(textView.isSelectable)
    XCTAssertTrue(textView.acceptsFirstMouse(for: nil))
    XCTAssertEqual(
      textView.accessibilityIdentifier(),
      "overlay-transcript-live"
    )
  }

  func testSelectionSurvivesAnAppendAtTheSameUtf16Range() {
    let selected = NSRange(location: 6, length: 8)

    XCTAssertEqual(
      LiveTranscriptSelectionPolicy.preservedRange(selected, updatedLength: 42),
      selected,
      "new live words must not throw away an earlier selection"
    )
    XCTAssertFalse(
      LiveTranscriptSelectionPolicy.followsTail(selection: selected, textLength: 42),
      "an active selection pauses automatic tail scrolling, not transcription"
    )
  }

  func testSelectionIsClampedWhenTheOpenTailIsReplaced() {
    XCTAssertEqual(
      LiveTranscriptSelectionPolicy.preservedRange(
        NSRange(location: 8, length: 20),
        updatedLength: 15
      ),
      NSRange(location: 8, length: 7)
    )
    XCTAssertTrue(
      LiveTranscriptSelectionPolicy.followsTail(
        selection: NSRange(location: 15, length: 0),
        textLength: 15
      )
    )
  }

  func testNativeCopyUsesOnlyTheCurrentSelection() throws {
    let pasteboard = NSPasteboard.general
    let oldItems: [NSPasteboardItem] = (pasteboard.pasteboardItems ?? []).map { item in
      let copy = NSPasteboardItem()
      for type in item.types {
        if let data = item.data(forType: type) {
          copy.setData(data, forType: type)
        }
      }
      return copy
    }
    defer {
      pasteboard.clearContents()
      pasteboard.writeObjects(oldItems)
    }

    let textView = LiveTranscriptTextView.makeTextView()
    textView.string = "alpha beta gamma"
    textView.setSelectedRange(NSRange(location: 6, length: 4))
    textView.copy(nil)

    XCTAssertEqual(pasteboard.string(forType: .string), "beta")
  }
}
