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
    coordinator.userScrollBegan()
    coordinator.userScrollEnded(isAtLiveEdge: false, hasSelection: true)

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

  func testManualScrollKeepsViewportAndResponderDuringStreamAppend() async throws {
    let fixture = makeScrollFixture()
    defer {
      fixture.coordinator.detach()
      fixture.window.close()
    }

    let prefix = String(repeating: "Recorded words stay above the viewport.\n", count: 400)
    LiveTranscriptTextView(text: prefix, appearance: .dark).update(
      fixture.textView, coordinator: fixture.coordinator)
    fixture.scrollView.layoutSubtreeIfNeeded()
    fixture.textView.layoutSubtreeIfNeeded()

    let maximumOrigin = fixture.textView.bounds.maxY - fixture.scrollView.contentView.bounds.height
    XCTAssertGreaterThan(maximumOrigin, 100)
    scroll(fixture.scrollView, toY: min(100, maximumOrigin / 2))
    XCTAssertTrue(fixture.window.makeFirstResponder(fixture.textView))
    let responder = fixture.window.firstResponder
    XCTAssertNotNil(responder)

    // Model the real AppKit notification synchronously while a reveal queued by
    // the initial render is still pending on the main queue.
    fixture.coordinator.scheduleTailReveal(for: fixture.textView)
    NotificationCenter.default.post(
      name: NSScrollView.willStartLiveScrollNotification,
      object: fixture.scrollView
    )
    let originBeforeAppend = fixture.scrollView.contentView.bounds.origin
    let appended = String(repeating: "New streamed words.\n", count: 12)
    LiveTranscriptTextView(text: prefix + appended, appearance: .dark).update(
      fixture.textView, coordinator: fixture.coordinator)

    await nextMainQueueTurn()

    XCTAssertEqual(fixture.textView.string, prefix + appended)
    XCTAssertEqual(fixture.scrollView.contentView.bounds.origin, originBeforeAppend)
    XCTAssertTrue(fixture.window.firstResponder === responder)
  }

  func testCaretAtEndSelectionNotificationCannotResumeDetachedViewport() async throws {
    let fixture = makeScrollFixture()
    defer {
      fixture.coordinator.detach()
      fixture.window.close()
    }

    let prefix = String(repeating: "Caret remains at the transcript end.\n", count: 400)
    LiveTranscriptTextView(text: prefix, appearance: .dark).update(
      fixture.textView, coordinator: fixture.coordinator)
    fixture.scrollView.layoutSubtreeIfNeeded()
    fixture.textView.layoutSubtreeIfNeeded()
    await nextMainQueueTurn()

    let end = (prefix as NSString).length
    XCTAssertEqual(fixture.textView.selectedRange(), NSRange(location: end, length: 0))
    NotificationCenter.default.post(
      name: NSScrollView.willStartLiveScrollNotification,
      object: fixture.scrollView
    )
    scroll(fixture.scrollView, toY: 80)
    NotificationCenter.default.post(
      name: NSScrollView.didEndLiveScrollNotification,
      object: fixture.scrollView
    )
    let originBeforeAppend = fixture.scrollView.contentView.bounds.origin
    let distanceFromBottom =
      fixture.textView.bounds.maxY - fixture.scrollView.contentView.documentVisibleRect.maxY
    XCTAssertGreaterThan(distanceFromBottom, 40)
    XCTAssertFalse(fixture.coordinator.followsTail)

    fixture.coordinator.textViewDidChangeSelection(
      Notification(name: NSTextView.didChangeSelectionNotification, object: fixture.textView)
    )
    XCTAssertFalse(fixture.coordinator.followsTail)

    let appended = "A live delta after the caret notification."
    LiveTranscriptTextView(text: prefix + appended, appearance: .dark).update(
      fixture.textView, coordinator: fixture.coordinator)
    await nextMainQueueTurn()

    XCTAssertEqual(fixture.textView.string, prefix + appended)
    XCTAssertFalse(fixture.coordinator.followsTail)
    XCTAssertEqual(fixture.scrollView.contentView.bounds.origin, originBeforeAppend)
  }

  func testReturningToTheLiveBottomResumesTailFollow() async throws {
    let fixture = makeScrollFixture()
    defer {
      fixture.coordinator.detach()
      fixture.window.close()
    }
    fixture.scrollView.automaticallyAdjustsContentInsets = false
    fixture.scrollView.contentInsets = NSEdgeInsetsZero
    fixture.scrollView.contentView.automaticallyAdjustsContentInsets = false
    let prefix = String(repeating: "Recorded line remains available.\n", count: 400)
    LiveTranscriptTextView(text: prefix, appearance: .dark).update(
      fixture.textView, coordinator: fixture.coordinator)
    fixture.scrollView.layoutSubtreeIfNeeded()
    fixture.textView.layoutSubtreeIfNeeded()
    await nextMainQueueTurn()
    let contentInsets = NSEdgeInsets(top: 50, left: 0, bottom: 52, right: 0)
    fixture.scrollView.contentView.contentInsets = contentInsets
    fixture.scrollView.layoutSubtreeIfNeeded()
    fixture.scrollView.setFrameSize(NSSize(width: 420, height: 161))
    fixture.scrollView.layoutSubtreeIfNeeded()
    fixture.scrollView.setFrameSize(NSSize(width: 420, height: 160))
    fixture.scrollView.layoutSubtreeIfNeeded()
    XCTAssertEqual(fixture.scrollView.contentView.contentInsets.top, contentInsets.top)
    XCTAssertEqual(fixture.scrollView.contentView.contentInsets.bottom, contentInsets.bottom)

    NotificationCenter.default.post(
      name: NSScrollView.willStartLiveScrollNotification,
      object: fixture.scrollView
    )
    scroll(fixture.scrollView, toY: 80)
    NotificationCenter.default.post(
      name: NSScrollView.didEndLiveScrollNotification,
      object: fixture.scrollView
    )
    XCTAssertFalse(fixture.coordinator.followsTail)

    NotificationCenter.default.post(
      name: NSScrollView.willStartLiveScrollNotification,
      object: fixture.scrollView
    )
    let maximumOrigin = LiveTranscriptNativeTextView.liveBottomScrollOrigin(
      documentMaxY: fixture.textView.bounds.maxY,
      clipHeight: fixture.scrollView.contentView.bounds.height,
      contentInsets: contentInsets
    )
    let insetAwayFromLiveBottom = max(4, contentInsets.bottom / 2)
    scroll(fixture.scrollView, toY: maximumOrigin - insetAwayFromLiveBottom)
    NotificationCenter.default.post(
      name: NSScrollView.didEndLiveScrollNotification,
      object: fixture.scrollView
    )
    XCTAssertFalse(
      fixture.coordinator.followsTail,
      "The footer inset remains scrollable space; follow resumes only at the real live edge"
    )

    NotificationCenter.default.post(
      name: NSScrollView.willStartLiveScrollNotification,
      object: fixture.scrollView
    )
    scroll(fixture.scrollView, toY: maximumOrigin)
    NotificationCenter.default.post(
      name: NSScrollView.didEndLiveScrollNotification,
      object: fixture.scrollView
    )
    XCTAssertTrue(fixture.coordinator.followsTail)

    let appended = String(repeating: "Newest streamed line.\n", count: 12)
    LiveTranscriptTextView(text: prefix + appended, appearance: .dark).update(
      fixture.textView, coordinator: fixture.coordinator)
    await nextMainQueueTurn()

    let visibleDocumentRect = fixture.scrollView.contentView.documentVisibleRect
    let liveBottomAfterAppend = LiveTranscriptNativeTextView.liveBottomScrollOrigin(
      documentMaxY: fixture.textView.bounds.maxY,
      clipHeight: fixture.scrollView.contentView.bounds.height,
      contentInsets: fixture.scrollView.contentView.contentInsets
    )
    let stringLength = (fixture.textView.string as NSString).length
    let lastCharacterRect = fixture.textView.firstRect(
      forCharacterRange: NSRange(location: max(0, stringLength - 1), length: min(1, stringLength)),
      actualRange: nil
    )
    let visibleRectOnScreen = fixture.window.convertToScreen(
      fixture.textView.convert(visibleDocumentRect, to: nil)
    )
    XCTAssertEqual(
      fixture.scrollView.contentView.bounds.origin.y,
      liveBottomAfterAppend,
      accuracy: 2,
      "tail follow must settle at the shared inset-aware bottom; "
        + "document=\(fixture.textView.bounds), clip=\(fixture.scrollView.contentView.bounds), "
        + "visible=\(visibleDocumentRect), insets=\(fixture.scrollView.contentView.contentInsets), "
        + "lastCharacter=\(lastCharacterRect)"
    )
    XCTAssertTrue(
      visibleRectOnScreen.contains(lastCharacterRect),
      "last character \(lastCharacterRect) is outside visible transcript area \(visibleRectOnScreen)"
    )
  }

  func testKeyboardScrollCommandClaimsViewportBeforeStreamingUpdate() async throws {
    let fixture = makeScrollFixture()
    defer {
      fixture.coordinator.detach()
      fixture.window.close()
    }

    let prefix = String(repeating: "Recorded keyboard test line.\n", count: 400)
    LiveTranscriptTextView(text: prefix, appearance: .dark).update(
      fixture.textView, coordinator: fixture.coordinator)
    fixture.scrollView.layoutSubtreeIfNeeded()
    fixture.textView.layoutSubtreeIfNeeded()
    scroll(fixture.scrollView, toY: 100)
    XCTAssertTrue(fixture.window.makeFirstResponder(fixture.textView))

    XCTAssertFalse(
      fixture.coordinator.textView(
        fixture.textView,
        doCommandBy: #selector(NSResponder.scrollPageUp(_:))
      )
    )
    let originBeforeAppend = fixture.scrollView.contentView.bounds.origin
    let appended = "A keyboard initiated stream delta."
    LiveTranscriptTextView(text: prefix + appended, appearance: .dark).update(
      fixture.textView, coordinator: fixture.coordinator)
    await nextMainQueueTurn()

    XCTAssertFalse(fixture.coordinator.followsTail)
    XCTAssertEqual(fixture.textView.string, prefix + appended)
    XCTAssertEqual(fixture.scrollView.contentView.bounds.origin, originBeforeAppend)
    XCTAssertTrue(fixture.window.firstResponder === fixture.textView)
  }

  func testDiscreteWheelInputClaimsViewport() throws {
    let fixture = makeScrollFixture()
    defer {
      fixture.coordinator.detach()
      fixture.window.close()
    }

    let prefix = String(repeating: "Recorded wheel test line.\n", count: 400)
    LiveTranscriptTextView(text: prefix, appearance: .dark).update(
      fixture.textView, coordinator: fixture.coordinator)
    fixture.scrollView.layoutSubtreeIfNeeded()
    fixture.textView.layoutSubtreeIfNeeded()
    scroll(fixture.scrollView, toY: 100)
    let cgEvent = try XCTUnwrap(
      CGEvent(
        scrollWheelEvent2Source: nil,
        units: .pixel,
        wheelCount: 2,
        wheel1: -24,
        wheel2: 0,
        wheel3: 0
      )
    )
    let event = try XCTUnwrap(NSEvent(cgEvent: cgEvent))

    fixture.scrollView.scrollWheel(with: event)

    XCTAssertFalse(fixture.coordinator.followsTail)
    XCTAssertGreaterThan(fixture.scrollView.contentView.bounds.origin.y, 0)
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

  private func makeScrollFixture() -> (
    window: NSWindow,
    scrollView: LiveTranscriptScrollView,
    textView: LiveTranscriptNativeTextView,
    coordinator: LiveTranscriptTextView.Coordinator
  ) {
    let frame = NSRect(x: 0, y: 0, width: 420, height: 160)
    let window = NSWindow(
      contentRect: frame,
      styleMask: [.titled],
      backing: .buffered,
      defer: true
    )
    window.isReleasedWhenClosed = false
    let scrollView = LiveTranscriptScrollView(frame: frame)
    scrollView.hasVerticalScroller = true
    scrollView.autohidesScrollers = false
    window.contentView = scrollView

    let textView = LiveTranscriptTextView.makeTextView()
    let coordinator = LiveTranscriptTextView.Coordinator()
    textView.delegate = coordinator
    textView.setFrameSize(NSSize(width: frame.width, height: 800))
    scrollView.documentView = textView
    scrollView.followCoordinator = coordinator
    coordinator.attach(to: scrollView)
    return (window, scrollView, textView, coordinator)
  }

  private func scroll(_ scrollView: NSScrollView, toY originY: CGFloat) {
    scrollView.contentView.scroll(to: NSPoint(x: 0, y: max(0, originY)))
    scrollView.reflectScrolledClipView(scrollView.contentView)
  }

  private func nextMainQueueTurn() async {
    await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
      DispatchQueue.main.async {
        continuation.resume()
      }
    }
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
