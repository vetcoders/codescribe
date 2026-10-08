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
    // Creation starts in mini; these contracts exercise the explicitly opened canvas.
    state.setPresentationMode(.expanded)
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
    let backdrop = try XCTUnwrap(desktopEffect(in: root))

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
    XCTAssertTrue(backdrop === desktopEffect(in: root), "Editing keeps the same native material")

    // Leaving the canvas gives the keyboard back and ends the edit.
    XCTAssertTrue(panel.makeFirstResponder(nil))
    XCTAssertFalse(panel.canBecomeKey)
    XCTAssertFalse(state.isEditingTranscript)
    state.discardRevisionDraft()
    XCTAssertEqual(state.canvasText, "final text")
  }

  func testKeyLostFromOutsideClosesTheGateAndResignsTheCanvas() throws {
    let state = OverlayState()
    state.setPresentationMode(.expanded)
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
    // AppKit may preselect the canvas before the take becomes editable. Force
    // a real responder transition for this resign-key contract.
    XCTAssertTrue(panel.makeFirstResponder(nil))
    XCTAssertTrue(panel.makeFirstResponder(canvas))
    XCTAssertTrue(panel.canBecomeKey)

    // Click in another app / panel ordered out: AppKit reports resign-key.
    panel.windowDidResignKey(
      Notification(name: NSWindow.didResignKeyNotification, object: panel))

    XCTAssertFalse(panel.canBecomeKey)
    XCTAssertFalse(panel.firstResponder === canvas, "the canvas resigns with the key")
    XCTAssertFalse(state.isEditingTranscript)
  }

  func testClickOpensEditGateForPreselectedCanvas() throws {
    let state = OverlayState()
    state.setPresentationMode(.expanded)
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayEditKeyGateTests.preselected.textScale")
      ) as? FloatingOverlayPanel
    )
    defer {
      panel.makeFirstResponder(nil)
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.orderFrontRegardless()
    let root = try XCTUnwrap(panel.contentView)
    project("live text", phase: "listening", terminal: false, sequence: 1, to: state)
    settle(root)
    let canvas = try XCTUnwrap(descendant(of: LiveTranscriptNativeTextView.self, in: root))
    // Establish the preselected read-only canvas explicitly; action buttons
    // also participate in AppKit focus ordering.
    XCTAssertTrue(panel.makeFirstResponder(canvas))
    project("final text", phase: "formatted", terminal: true, sequence: 2, to: state)
    settle(root)
    XCTAssertTrue(panel.firstResponder === canvas)
    XCTAssertTrue(canvas.isEditable)
    XCTAssertFalse(panel.canBecomeKey)

    // The same edit activation used after an actual AppKit mouseDown. A
    // synthetic NSTextView mouseDown waits for a matching system mouse-up.
    canvas.beginEditingIfNeeded()
    XCTAssertTrue(panel.canBecomeKey)
    XCTAssertTrue(state.isEditingTranscript)
  }

  // MARK: Helpers

  private func desktopEffect(in root: NSView) -> NSView? {
    if #available(macOS 26, *) {
      return descendant(of: OverlayDesktopGlassView.self, in: root)
    }
    return descendant(of: OverlayDesktopEffectView.self, in: root)
  }

  private func settle(_ root: NSView) {
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    root.layoutSubtreeIfNeeded()
  }

  private func descendant<View: NSView>(of type: View.Type, in root: NSView) -> View? {
    if let root = root as? View { return root }
    return root.subviews.lazy.compactMap { self.descendant(of: type, in: $0) }.first
  }

  func testLive1656SelectedReadOnlyWordsCanReceiveNativeCopyCommands() throws {
    let state = OverlayState()
    state.setPresentationMode(.expanded)
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "Live1656.copySelection"))
        as? FloatingOverlayPanel)
    defer {
      panel.makeFirstResponder(nil)
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.orderFrontRegardless()
    let root = try XCTUnwrap(panel.contentView)
    project("alpha beta gamma", phase: "listening", terminal: false, sequence: 1, to: state)
    settle(root)
    let canvas = try XCTUnwrap(descendant(of: LiveTranscriptNativeTextView.self, in: root))
    XCTAssertTrue(panel.makeFirstResponder(canvas))
    let point = canvas.convert(NSPoint(x: 25, y: 10), to: nil)
    let down = try XCTUnwrap(
      NSEvent.mouseEvent(
        with: .leftMouseDown, location: point, modifierFlags: [], timestamp: 0,
        windowNumber: panel.windowNumber, context: nil, eventNumber: 1, clickCount: 1,
        pressure: 1))
    let up = try XCTUnwrap(
      NSEvent.mouseEvent(
        with: .leftMouseUp, location: point, modifierFlags: [], timestamp: 0.01,
        windowNumber: panel.windowNumber, context: nil, eventNumber: 2, clickCount: 1,
        pressure: 0))
    NSApp.postEvent(up, atStart: true)
    canvas.mouseDown(with: down)
    canvas.setSelectedRange(NSRange(location: 6, length: 4))
    XCTAssertFalse(canvas.isEditable)
    XCTAssertTrue(panel.canBecomeKey, "explicit selection needs native command routing")
    XCTAssertTrue(panel.firstResponder === canvas)
    let menuItem = NSMenuItem(title: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
    XCTAssertTrue(canvas.validateMenuItem(menuItem))
    let board = NSPasteboard(name: .init("codescribe.tests.live1656.\(UUID().uuidString)"))
    XCTAssertTrue(canvas.copySelection(to: board))
    XCTAssertEqual(board.string(forType: .string), "beta")
    XCTAssertFalse(state.isEditingTranscript)
    XCTAssertFalse(state.canInsert)
  }

  func testLive1656EphemeralPaintReachesCanvasWithoutCommittingDelivery() throws {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.applyCompactProjection(
      CsCompactProjection(
        sessionId: "edit-key-gate-fixture", captureEpoch: 1, sequence: 1,
        text: "", degraded: false, evidence: []))
    state.handleRecordingStarted()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "Live1656.ephemeral"))
        as? FloatingOverlayPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.orderFrontRegardless()
    let root = try XCTUnwrap(panel.contentView)
    state.applyCompactProjection(
      CsCompactProjection(
        sessionId: "edit-key-gate-fixture", captureEpoch: 1, sequence: 2,
        text: "Ostatnie słowa są widoczne od razu.", degraded: false, evidence: []))
    settle(root)
    let canvas = try XCTUnwrap(descendant(of: LiveTranscriptNativeTextView.self, in: root))
    XCTAssertEqual(canvas.string, "Ostatnie słowa są widoczne od razu.")
    XCTAssertTrue(state.formattedText.isEmpty)
    XCTAssertNil(state.latestTranscriptProjection)
    XCTAssertFalse(state.canInsert)
    XCTAssertFalse(state.canPaste)
    state.applyCompactProjection(
      CsCompactProjection(
        sessionId: "foreign", captureEpoch: 1, sequence: 99,
        text: "Stale text", degraded: false, evidence: []))
    settle(root)
    XCTAssertEqual(canvas.string, "Ostatnie słowa są widoczne od razu.")
  }

  private func project(
    _ text: String, phase: String, terminal: Bool, sequence: UInt64, to state: OverlayState
  ) {
    let isFormatted = phase == "formatted"
    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: sequence,
        emittedAt: "2026-09-08T00:00:00Z",
        sessionId: "edit-key-gate-fixture",
        renderedText: text,
        phase: phase,
        terminal: terminal,
        reducerAction: terminal ? "record_ledger_terminal_seal" : "record_ledger_projection",
        canPaste: isFormatted,
        canInsert: isFormatted,
        canRetranscribe: isFormatted,
        canFormat: isFormatted
      )
    )
  }
}
