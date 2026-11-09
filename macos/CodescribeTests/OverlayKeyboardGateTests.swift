import AppKit
import XCTest

@testable import Codescribe

/// Maciej, 2026-11-08 14:25Z: "Composer w oknie Agents nie focusuje się mimo
/// kliknięcia i pozostawia focus na innej aplikacji w niektórych sytuacjach."
/// The overlay's keyboard gate stops trusting AppKit's key record: a click
/// asks the window server again when the record outlived the gate, and the
/// user leaving for another app closes the gate so the record follows.
///
/// The panel is never shown and its key calls are recorded, not made: the
/// probe runs on the Founder's desktop and must not take the keyboard.
@MainActor
final class OverlayKeyboardGateTests: XCTestCase {
  /// AppKit as these tests model it: resign-key also posts the delegate call.
  private final class KeyRecord {
    var key = false
    var calls: [String] = []
  }

  private final class OutsideSource: OverlayOutsideInteractionSource {
    private var handler: (@MainActor (OverlayOutsideInteraction) -> Void)?
    private(set) var starts = 0
    var isWatching: Bool { handler != nil }

    func start(_ handler: @escaping @MainActor (OverlayOutsideInteraction) -> Void) {
      starts += 1
      self.handler = handler
    }

    func stop() { handler = nil }

    func send(_ interaction: OverlayOutsideInteraction) { handler?(interaction) }
  }

  private func hiddenPanel() -> (FloatingOverlayPanel, NSTextView, KeyRecord, OutsideSource) {
    let panel = FloatingOverlayPanel(
      contentRect: CGRect(x: 0, y: 0, width: 400, height: 200),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.delegate = panel
    let editor = NSTextView(frame: CGRect(x: 0, y: 0, width: 400, height: 200))
    panel.contentView = editor
    let record = KeyRecord()
    let outside = OutsideSource()
    panel.keyCalls = OverlayKeyCalls(
      isKey: { _ in record.key },
      makeKey: { _ in
        record.calls.append("makeKey")
        record.key = true
      },
      resignKey: { window in
        record.calls.append("resignKey")
        record.key = false
        (window as? FloatingOverlayPanel)?.windowDidResignKey(
          Notification(name: NSWindow.didResignKeyNotification, object: window))
      })
    panel.outsideInteractions = outside
    return (panel, editor, record, outside)
  }

  func testOpeningAsksTheWindowServerUnlessAnOpenGateAlreadyHoldsTheKey() {
    XCTAssertEqual(OverlayKeyboardRequest.opening(gateOpen: false, panelIsKey: false), .makeKey)
    XCTAssertEqual(OverlayKeyboardRequest.opening(gateOpen: true, panelIsKey: false), .makeKey)
    XCTAssertEqual(OverlayKeyboardRequest.opening(gateOpen: true, panelIsKey: true), .none)
    XCTAssertEqual(
      OverlayKeyboardRequest.opening(gateOpen: false, panelIsKey: true), .reclaimKey)
  }

  /// The panel still reports key behind a closed gate; the old click saw
  /// `isKeyWindow` and never asked, so the keyboard stayed in the other app.
  func testAClickReclaimsAKeyRecordTheClosedGateNoLongerOwns() {
    let (panel, editor, record, _) = hiddenPanel()
    defer { panel.invalidatePresence() }
    XCTAssertTrue(panel.makeFirstResponder(editor))
    record.key = true
    XCTAssertFalse(panel.canBecomeKey)

    panel.takeKeyForTranscript()

    XCTAssertEqual(record.calls, ["resignKey", "makeKey"])
    XCTAssertTrue(record.key)
    XCTAssertTrue(panel.canBecomeKey, "the reclaimed key keeps its gate open")
    XCTAssertTrue(panel.firstResponder === editor, "resigning the stale record keeps the editor")
  }

  /// Click the composer, go on in another app, click the composer again: the
  /// second click asks the window server again.
  func testLeavingForAnotherAppClosesTheGateSoTheNextClickAsksAgain() {
    for interaction in [OverlayOutsideInteraction.click, .activation] {
      let (panel, editor, record, outside) = hiddenPanel()
      defer { panel.invalidatePresence() }
      XCTAssertTrue(panel.makeFirstResponder(editor))
      panel.takeKeyForTranscript()
      XCTAssertEqual(record.calls, ["makeKey"])
      XCTAssertTrue(outside.isWatching, "an open gate watches for the user leaving")

      outside.send(interaction)

      XCTAssertFalse(panel.canBecomeKey, "\(interaction)")
      XCTAssertFalse(record.key, "\(interaction): the record follows the window server")
      XCTAssertEqual(record.calls, ["makeKey", "resignKey"], "\(interaction)")
      XCTAssertFalse(
        panel.firstResponder === editor, "\(interaction): the editor resigns with the key")
      XCTAssertFalse(outside.isWatching, "\(interaction)")

      XCTAssertTrue(panel.makeFirstResponder(editor))
      panel.takeKeyForTranscript()
      XCTAssertEqual(record.calls, ["makeKey", "resignKey", "makeKey"], "\(interaction)")
      XCTAssertTrue(panel.canBecomeKey, "\(interaction)")
      XCTAssertTrue(panel.firstResponder === editor, "\(interaction)")
    }
  }

  /// One click takes the gate from the panel and again from the editor; the
  /// window server is asked once and one watch is installed.
  func testAnOpenGateOnAKeyPanelIsNotAskedAgain() {
    let (panel, _, record, outside) = hiddenPanel()
    defer { panel.invalidatePresence() }
    panel.takeKeyForTranscript()
    panel.takeKeyForTranscript()
    panel.takeKeyForTranscript()
    XCTAssertEqual(record.calls, ["makeKey"])
    XCTAssertEqual(outside.starts, 1)
  }

  /// Every other way the gate closes ends the watch, and a report arriving
  /// after that changes nothing.
  func testEveryClosedGateStopsWatching() {
    let (panel, _, record, outside) = hiddenPanel()
    defer { panel.invalidatePresence() }
    panel.takeKeyForTranscript()
    panel.releaseKeyAfterTranscript()
    XCTAssertFalse(outside.isWatching, "the canvas released the gate")
    XCTAssertEqual(record.calls, ["makeKey", "resignKey"])

    panel.takeKeyForTranscript()
    XCTAssertTrue(outside.isWatching)
    panel.windowDidResignKey(
      Notification(name: NSWindow.didResignKeyNotification, object: panel))
    XCTAssertFalse(outside.isWatching, "AppKit reported resign-key")
    XCTAssertFalse(panel.canBecomeKey)

    panel.noteInteractionOutside(.click)
    XCTAssertEqual(record.calls, ["makeKey", "resignKey", "makeKey"])
    XCTAssertFalse(panel.canBecomeKey)

    panel.takeKeyForTranscript()
    XCTAssertTrue(outside.isWatching)
    panel.invalidatePresence()
    XCTAssertFalse(outside.isWatching, "the panel's teardown removes the watch")
  }
}
