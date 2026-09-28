import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class LiveTranscriptTextViewTests: XCTestCase {
  func testAppendingLiveWordsDoesNotInvalidateTheRecordedPrefix() throws {
    let prefix = String(repeating: "Already recorded words.\n", count: 2_000)
    let textView = LiveTranscriptTextView.makeTextView()
    let coordinator = LiveTranscriptTextView.Coordinator()
    textView.delegate = coordinator
    LiveTranscriptTextView(text: prefix, appearance: .dark).update(
      textView, coordinator: coordinator)
    let recorder = TranscriptStorageEditRecorder()
    let storage = try XCTUnwrap(textView.textStorage)
    storage.delegate = recorder
    let selection = NSRange(location: 5, length: 8)
    textView.setSelectedRange(selection)
    coordinator.followsTail = false

    let appended = "New words."
    LiveTranscriptTextView(text: prefix + appended, appearance: .dark).update(
      textView, coordinator: coordinator)

    XCTAssertEqual(Array(textView.string.utf8), Array((prefix + appended).utf8))
    XCTAssertEqual(textView.selectedRange(), selection)
    XCTAssertFalse(recorder.characterRanges.isEmpty)
    XCTAssertTrue(
      recorder.characterRanges.allSatisfy { $0.location >= (prefix as NSString).length },
      "An append must not invalidate the entire recorded document: \(recorder.characterRanges)")
  }

  func testTailRevisionPreservesExactUnicodeBytesAndSelection() {
    let textView = LiveTranscriptTextView.makeTextView()
    let coordinator = LiveTranscriptTextView.Coordinator()
    textView.delegate = coordinator
    let texts = ["kept 👩🏽‍⚕️ café", "kept 👩🏽‍⚕️ cafe\u{301}", "kept 👨‍⚕️ new", "kept", ""]
    for text in texts {
      LiveTranscriptTextView(text: text, appearance: .dark).update(
        textView, coordinator: coordinator)
      XCTAssertEqual(Array(textView.string.utf8), Array(text.utf8))
      XCTAssertLessThanOrEqual(NSMaxRange(textView.selectedRange()), (text as NSString).length)
    }
  }

  func testNativeEditInvalidatesCachedStyleWithoutInterruptingEditing() throws {
    let view = LiveTranscriptTextView.makeTextView()
    let coordinator = LiveTranscriptTextView.Coordinator()
    view.delegate = coordinator
    let projection = LiveTranscriptTextView(text: "recorded words", appearance: .dark)
    projection.update(view, coordinator: coordinator)
    let storage = try XCTUnwrap(view.textStorage)
    let originalColor = try XCTUnwrap(
      storage.attribute(.foregroundColor, at: 0, effectiveRange: nil))
    coordinator.isEditing = true
    storage.addAttribute(
      .foregroundColor, value: NSColor.red, range: NSRange(location: 0, length: 1))
    coordinator.textDidChange(Notification(name: NSText.didChangeNotification, object: view))
    projection.update(view, coordinator: coordinator)
    XCTAssertEqual(
      storage.attribute(.foregroundColor, at: 0, effectiveRange: nil) as? NSColor, .red)
    coordinator.isEditing = false
    projection.update(view, coordinator: coordinator)
    XCTAssertEqual(
      storage.attribute(.foregroundColor, at: 0, effectiveRange: nil) as? NSColor,
      originalColor as? NSColor)
  }

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
    let pasteboard = NSPasteboard(
      name: NSPasteboard.Name("codescribe.tests.live-transcript.\(UUID().uuidString)")
    )

    let textView = LiveTranscriptTextView.makeTextView()
    textView.string = "alpha beta gamma"
    textView.setSelectedRange(NSRange(location: 6, length: 4))
    XCTAssertTrue(textView.copySelection(to: pasteboard))

    XCTAssertEqual(pasteboard.string(forType: .string), "beta")
  }
}

@MainActor
private final class TranscriptStorageEditRecorder: NSObject, NSTextStorageDelegate {
  var characterRanges: [NSRange] = []

  nonisolated func textStorage(
    _ textStorage: NSTextStorage, didProcessEditing editedMask: NSTextStorageEditActions,
    range editedRange: NSRange, changeInLength delta: Int
  ) {
    MainActor.assumeIsolated {
      if editedMask.contains(.editedCharacters) { characterRanges.append(editedRange) }
    }
  }
}
