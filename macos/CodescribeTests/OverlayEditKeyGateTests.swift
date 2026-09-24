import AppKit
import XCTest

@testable import Codescribe

/// The overlay is a non-activating panel that must never take the keyboard
/// away from the app the user is dictating into — except while the formatted
/// transcript is being edited on the canvas. `FloatingOverlayPanel.canBecomeKey`
/// had no writer after 2b96c343d / 8d836df92, so the editor could never
/// receive a keystroke. These tests pin the gate to the canvas's
/// first-responder transitions on the real panel + hosting hierarchy.
@MainActor
final class OverlayEditKeyGateTests: XCTestCase {
  func testPanelCanBecomeKeyOnlyWhileTheFormattedCanvasIsFirstResponder() throws {
    let state = OverlayState()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayEditKeyGateTests.textScale")
      ) as? FloatingOverlayPanel
    )
    defer {
      panel.makeFirstResponder(nil)
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.orderFrontRegardless()
    let root = try XCTUnwrap(panel.contentView)
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    let canvas = try XCTUnwrap(descendant(of: LiveTranscriptNativeTextView.self, in: root))

    // Live take: read-only; a click into the canvas selects, never edits.
    project("live words", phase: "listening", terminal: false, sequence: 1, to: state)
    settle(root)
    XCTAssertFalse(canvas.isEditable)
    XCTAssertFalse(panel.canBecomeKey)
    XCTAssertTrue(panel.makeFirstResponder(canvas))
    XCTAssertFalse(panel.canBecomeKey, "selection in a live take never takes the keyboard")
    XCTAssertFalse(state.isEditingTranscript)
    XCTAssertTrue(panel.makeFirstResponder(nil))

    // Sealed formatted take: the same canvas is the editor.
    project("final text", phase: "formatted", terminal: true, sequence: 2, to: state)
    settle(root)
    XCTAssertTrue(canvas.isEditable)
    XCTAssertEqual(canvas.string, "final text")
    XCTAssertFalse(panel.canBecomeKey, "nothing is key before the user enters the canvas")

    XCTAssertTrue(panel.makeFirstResponder(canvas))
    XCTAssertTrue(panel.canBecomeKey, "editing is the one window in which the panel is key")
    XCTAssertTrue(state.isEditingTranscript)

    // Keystrokes land in the local draft; the projection and the canvas
    // bytes agree, so SwiftUI's repaint must not throw the caret away.
    canvas.setSelectedRange(NSRange(location: (canvas.string as NSString).length, length: 0))
    canvas.insertText("!", replacementRange: canvas.selectedRange())
    settle(root)
    XCTAssertEqual(state.revisionDraft, "final text!")
    XCTAssertTrue(state.isRevisionDraftDirty)
    XCTAssertEqual(state.formattedText, "final text", "typing never mutates projected truth")
    XCTAssertEqual(canvas.string, "final text!")
    XCTAssertTrue(panel.firstResponder === canvas)

    // Leaving the canvas gives the keyboard back and ends the edit.
    XCTAssertTrue(panel.makeFirstResponder(nil))
    XCTAssertFalse(panel.canBecomeKey)
    XCTAssertFalse(state.isEditingTranscript)
    state.discardRevisionDraft()
    XCTAssertEqual(state.canvasText, "final text")
  }

  func testKeyLostFromOutsideClosesTheGateAndResignsTheCanvas() throws {
    let state = OverlayState()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayEditKeyGateTests.outside.textScale")
      ) as? FloatingOverlayPanel
    )
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.orderFrontRegardless()
    let root = try XCTUnwrap(panel.contentView)
    project("final text", phase: "formatted", terminal: true, sequence: 1, to: state)
    settle(root)
    let canvas = try XCTUnwrap(descendant(of: LiveTranscriptNativeTextView.self, in: root))
    XCTAssertTrue(panel.makeFirstResponder(canvas))
    XCTAssertTrue(panel.canBecomeKey)

    // Click in another app / panel ordered out: AppKit reports resign-key.
    panel.windowDidResignKey(
      Notification(name: NSWindow.didResignKeyNotification, object: panel))

    XCTAssertFalse(panel.canBecomeKey)
    XCTAssertFalse(panel.firstResponder === canvas, "the canvas resigns with the key")
    XCTAssertFalse(state.isEditingTranscript)
  }

  // MARK: Helpers

  private func settle(_ root: NSView) {
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    root.layoutSubtreeIfNeeded()
  }

  private func descendant<View: NSView>(of type: View.Type, in root: NSView) -> View? {
    if let root = root as? View { return root }
    return root.subviews.lazy.compactMap { self.descendant(of: type, in: $0) }.first
  }

  private func project(
    _ text: String, phase: String, terminal: Bool, sequence: UInt64, to state: OverlayState
  ) {
    let isFormatted = phase == "formatted"
    state.applyTranscriptProjection(
      CsTranscriptProjectionEvent(
        schema: "codescribe.transcript_projection.v1",
        sequence: sequence,
        emittedAt: "2026-09-08T00:00:00Z",
        sessionId: "edit-key-gate-fixture",
        mode: "dictation",
        reducerRevision: sequence,
        reducerAction: terminal ? "record_ledger_terminal_seal" : "record_ledger_projection",
        occurrenceSessionId: "edit-key-gate-fixture",
        captureEpoch: 1,
        sampleStart: (sequence - 1) * 16_000,
        sampleEnd: sequence * 16_000,
        documentIndex: sequence - 1,
        label: terminal ? "terminal" : "live",
        renderedText: text,
        deliveryText: nil,
        phase: phase,
        canPaste: isFormatted,
        canInsert: isFormatted,
        canCopy: !text.isEmpty,
        canRetranscribe: isFormatted,
        canFormat: isFormatted,
        canSendToAgent: false,
        terminal: terminal,
        lifecycleTerminal: terminal,
        delivery: .unattempted,
        acousticReceipts: [],
        sealCoverage: nil,
        consultationPresentations: []
      )
    )
  }
}
