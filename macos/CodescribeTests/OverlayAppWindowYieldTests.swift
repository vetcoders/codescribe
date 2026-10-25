import AppKit
import XCTest

@testable import Codescribe

// Falsifier for the expanded overlay stepping aside for our own windows
// (Maciej, 2026-10-17: Settings open, the full overlay still covers it).
//
// `OverlayAppWindowPolicy` is a pure predicate. `OverlayState.yieldToAppWindow`
// only folds expanded to midi and does not restore on close. `OverlayAppWindowYield`
// is the watcher: one fold per window appearance, cleared on close and when that
// window's occlusion drops `.visible` (the agent chat orders out on minimise).

@MainActor
final class OverlayAppWindowYieldTests: XCTestCase {
  func testIntercomYieldsEveryCardAndRestoresFloatingLevelsWithoutPresentingWindows() {
    let state = makeState()
    let header = OverlayIntercomPanel()
    let card = OverlayIntercomPanel()
    let detached = OverlayIntercomPanel()
    detached.floatingLevel = OverlayIntercomPanel.detachedLevel
    let panels = [header, card, detached]
    let settings = titledWindow("Settings")
    defer {
      for panel in panels { panel.close() }
      settings.close()
    }
    let observer = OverlayAppWindowYield(panel: header, state: state) { window in
      for panel in panels { panel.yieldToAppWindow(window) }
    }
    observer.start()
    defer { observer.invalidate() }
    let generation = state.captureGeneration
    deliver(NSWindow.didBecomeKeyNotification, settings)
    XCTAssertTrue(panels.allSatisfy { $0.level == .normal })
    // A subsequent click must still lower the cards even after the normal
    // overlay has already yielded once to this same window.
    card.level = .floating
    deliver(NSWindow.didBecomeKeyNotification, settings)
    XCTAssertEqual(card.level, .normal)
    deliver(NSWindow.willCloseNotification, settings)
    XCTAssertEqual(header.level, .floating)
    XCTAssertEqual(card.level, .floating)
    XCTAssertEqual(detached.level, OverlayIntercomPanel.detachedLevel)
    XCTAssertTrue(panels.allSatisfy { !$0.isVisible && !$0.isKeyWindow })
    XCTAssertFalse(settings.isVisible)
    XCTAssertEqual(state.captureGeneration, generation)
  }

  private struct Rig {
    let state: OverlayState
    let panel: FloatingOverlayPanel
    let yield: OverlayAppWindowYield
  }

  private func makeState() -> OverlayState {
    OverlayState(
      autoSendEnabled: { false },
      micAccessProvider: { true },
      autoCollapseScheduler: { _, _ in {} })
  }

  /// A live borderless overlay panel plus the watcher the panel owns in
  /// `DictationOverlayWindow.make`. The test holds the watcher itself
  /// (`appWindowYield` is fileprivate). Ordering front happens before `start`,
  /// so the panel's own key notification is not the appearance under test.
  private func makeRig(mode: OverlayPresentationMode, visible: Bool) -> Rig {
    let state = makeState()
    state.setPresentationMode(mode)
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 470, height: 280),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered,
      defer: false)
    panel.isReleasedWhenClosed = false
    panel.setFrameOrigin(NSPoint(x: 80, y: 80))
    if visible {
      BackgroundTestWindow.present(panel)
    }
    let yield = OverlayAppWindowYield(panel: panel, state: state)
    yield.start()
    return Rig(state: state, panel: panel, yield: yield)
  }

  private func titledWindow(_ title: String) -> NSWindow {
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 480, height: 320),
      styleMask: [.titled, .closable, .miniaturizable, .resizable],
      backing: .buffered,
      defer: false)
    window.isReleasedWhenClosed = false
    window.title = title
    window.level = .normal
    window.setFrameOrigin(NSPoint(x: 240, y: 180))
    return window
  }

  private func label(_ mode: OverlayPresentationMode) -> String {
    switch mode {
    case .mini: "mini"
    case .midi: "midi"
    case .expanded: "expanded"
    }
  }

  private func expectMode(
    _ state: OverlayState, _ mode: OverlayPresentationMode, _ situation: String,
    file: StaticString = #filePath, line: UInt = #line
  ) {
    XCTAssertEqual(
      state.presentationMode, mode, "\(situation), got \(label(state.presentationMode))",
      file: file, line: line)
  }

  /// The watcher is queued on the main queue, so a post returns before the fold.
  private func deliver(_ name: Notification.Name, _ window: NSWindow) {
    NotificationCenter.default.post(name: name, object: window)
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
  }

  private func retire(_ rig: Rig, windows: [NSWindow] = []) {
    rig.yield.invalidate()
    rig.panel.orderOut(nil)
    for window in windows {
      if let parent = window.sheetParent {
        parent.endSheet(window)
      }
      window.orderOut(nil)
    }
  }

  func testShouldYieldOnlyForATitledNormalWindow() {
    let settings = titledWindow("Settings")
    XCTAssertTrue(settings.styleMask.contains(.titled))
    XCTAssertEqual(settings.level, .normal)
    XCTAssertNil(settings.sheetParent)
    XCTAssertTrue(
      OverlayAppWindowPolicy.shouldYield(to: settings),
      "Settings-like titled window: titled at normal level should yield, got false")

    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 470, height: 280),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered,
      defer: false)
    panel.isReleasedWhenClosed = false
    XCTAssertFalse(
      OverlayAppWindowPolicy.shouldYield(to: panel),
      "FloatingOverlayPanel: should not yield, got true")

    let tray = NSPanel(
      contentRect: NSRect(x: 0, y: 0, width: 280, height: 120),
      styleMask: [.borderless, .nonactivatingPanel],
      backing: .buffered,
      defer: false)
    tray.isReleasedWhenClosed = false
    tray.level = .normal
    XCTAssertFalse(tray.styleMask.contains(.titled))
    XCTAssertFalse(
      OverlayAppWindowPolicy.shouldYield(to: tray),
      "borderless panel (tray/popover-like): should not yield, got true")

    let parent = titledWindow("Settings")
    BackgroundTestWindow.present(parent)
    let sheet = titledWindow("Confirm")
    parent.beginSheet(sheet)
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    defer {
      if sheet.sheetParent != nil {
        parent.endSheet(sheet)
      }
      sheet.orderOut(nil)
      parent.orderOut(nil)
      panel.orderOut(nil)
      tray.orderOut(nil)
      settings.orderOut(nil)
    }
    XCTAssertTrue(
      sheet.sheetParent === parent,
      "sheet attached to a parent: sheetParent is nil, so this does not test the sheet rule")
    XCTAssertTrue(sheet.styleMask.contains(.titled))
    XCTAssertEqual(sheet.level, .normal)
    XCTAssertFalse(
      OverlayAppWindowPolicy.shouldYield(to: sheet),
      "sheet attached to a parent: should not yield, got true")

    let modal = titledWindow("Alert")
    modal.level = .modalPanel
    defer { modal.orderOut(nil) }
    XCTAssertTrue(modal.styleMask.contains(.titled))
    XCTAssertNil(modal.sheetParent)
    XCTAssertEqual(modal.level, .modalPanel)
    XCTAssertFalse(
      OverlayAppWindowPolicy.shouldYield(to: modal),
      "titled window at modalPanel level: should not yield, got true")
  }

  func testExpandedYieldsToMidi() {
    let state = makeState()
    state.setPresentationMode(.expanded)
    state.yieldToAppWindow()
    expectMode(state, .midi, "Settings-like titled window: expanded should yield to midi")
  }

  func testMiniYieldStaysMini() {
    let state = makeState()
    state.setPresentationMode(.mini)
    state.yieldToAppWindow()
    expectMode(state, .mini, "Settings-like titled window: mini should stay mini")
  }

  func testMidiYieldStaysMidi() {
    let state = makeState()
    state.setPresentationMode(.midi)
    state.yieldToAppWindow()
    expectMode(state, .midi, "Settings-like titled window: midi should stay midi")
  }

  func testClosingTheYieldedWindowLeavesMidi() {
    let rig = makeRig(mode: .expanded, visible: true)
    let settings = titledWindow("Settings")
    defer { retire(rig, windows: [settings]) }
    XCTAssertTrue(rig.panel.isVisible, "FloatingOverlayPanel: ordered front before the yield")
    deliver(NSWindow.didBecomeKeyNotification, settings)
    expectMode(rig.state, .midi, "Settings-like titled window: expanded should yield to midi")
    deliver(NSWindow.willCloseNotification, settings)
    expectMode(
      rig.state, .midi, "Settings-like titled window: closing it after a yield should leave midi")
  }

  func testOncePerWindowAppearance() {
    let rig = makeRig(mode: .expanded, visible: true)
    let settings = titledWindow("Settings")
    defer { retire(rig, windows: [settings]) }
    XCTAssertTrue(rig.panel.isVisible, "FloatingOverlayPanel: ordered front before the yield")

    deliver(NSWindow.didBecomeKeyNotification, settings)
    expectMode(rig.state, .midi, "Settings-like titled window: expanded should yield to midi")

    rig.state.setPresentationMode(.expanded)
    deliver(NSWindow.didBecomeKeyNotification, settings)
    expectMode(
      rig.state, .expanded,
      "Settings-like titled window: a second key of the same appearance should stay expanded")

    deliver(NSWindow.willCloseNotification, settings)
    expectMode(
      rig.state, .expanded,
      "Settings-like titled window: close alone should leave the re-expanded overlay expanded")
    deliver(NSWindow.didBecomeKeyNotification, settings)
    expectMode(
      rig.state, .midi,
      "Settings-like titled window: a fresh appearance after close should yield expanded to midi")
  }

  func testFirstKeyWhileMiniIsRemembered() {
    let rig = makeRig(mode: .mini, visible: true)
    let settings = titledWindow("Settings")
    defer { retire(rig, windows: [settings]) }
    deliver(NSWindow.didBecomeKeyNotification, settings)
    expectMode(rig.state, .mini, "Settings-like titled window: mini should stay mini")
    rig.state.setPresentationMode(.expanded)
    deliver(NSWindow.didBecomeKeyNotification, settings)
    expectMode(
      rig.state, .expanded,
      "Settings-like titled window: a key seen while mini should stay expanded on the next key")
  }

  func testOverlayPanelBecomingKeyDoesNotFold() {
    let rig = makeRig(mode: .expanded, visible: true)
    defer { retire(rig) }
    deliver(NSWindow.didBecomeKeyNotification, rig.panel)
    expectMode(
      rig.state, .expanded, "FloatingOverlayPanel: becoming key itself should stay expanded")
  }

  func testInvisibleOverlayDoesNotFoldAndRemembersTheAppearance() {
    let rig = makeRig(mode: .expanded, visible: false)
    let settings = titledWindow("Settings")
    defer { retire(rig, windows: [settings]) }
    XCTAssertFalse(rig.panel.isVisible, "FloatingOverlayPanel: hidden before the key")
    deliver(NSWindow.didBecomeKeyNotification, settings)
    expectMode(
      rig.state, .expanded, "Settings-like titled window: hidden overlay should stay expanded")
    BackgroundTestWindow.present(rig.panel)
    XCTAssertTrue(
      rig.panel.isVisible, "FloatingOverlayPanel: shown again for the remembered appearance")
    deliver(NSWindow.didBecomeKeyNotification, settings)
    expectMode(
      rig.state, .expanded,
      "Settings-like titled window: an appearance seen while hidden should stay expanded")
  }

  func testOcclusionWithoutVisibleClearsTheAppearance() {
    let rig = makeRig(mode: .expanded, visible: true)
    let settings = titledWindow("Settings")
    defer { retire(rig, windows: [settings]) }
    XCTAssertTrue(rig.panel.isVisible, "FloatingOverlayPanel: ordered front before the yield")
    XCTAssertFalse(
      settings.occlusionState.contains(.visible),
      "Settings-like titled window: occlusion case needs a window without .visible")

    deliver(NSWindow.didBecomeKeyNotification, settings)
    expectMode(rig.state, .midi, "Settings-like titled window: expanded should yield to midi")
    rig.state.setPresentationMode(.expanded)
    deliver(NSWindow.didBecomeKeyNotification, settings)
    expectMode(
      rig.state, .expanded,
      "Settings-like titled window: a second key of the same appearance should stay expanded")

    deliver(NSWindow.didChangeOcclusionStateNotification, settings)
    expectMode(
      rig.state, .expanded,
      "Settings-like titled window: occlusion without .visible should not itself change expanded")
    deliver(NSWindow.didBecomeKeyNotification, settings)
    expectMode(
      rig.state, .midi,
      "Settings-like titled window: after occlusion without .visible, "
        + "a new key should yield to midi")
  }
}
