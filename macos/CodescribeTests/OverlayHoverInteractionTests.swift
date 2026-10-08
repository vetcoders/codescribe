import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayHoverInteractionTests: XCTestCase {
  func testHoverEntryExitAndReentryDoNotOpenTheTranscriptOrChangeCapture() throws {
    let state = OverlayState.previewListening()
    state.setPresentationMode(.mini)
    let text = state.activeText
    let generation = state.captureGeneration
    state.setPointerHovering(true)
    let entry = try XCTUnwrap(state.widgetHoverDeadline)
    state.expireWidgetHover(at: entry.advanced(by: .milliseconds(-1)))
    XCTAssertEqual(state.presentationMode, .mini)
    state.setPointerHovering(false)
    state.expireWidgetHover(at: entry)
    XCTAssertEqual(state.presentationMode, .mini, "a cancelled entry must not open controls")
    state.setPointerHovering(true)
    state.expireWidgetHover(at: try XCTUnwrap(state.widgetHoverDeadline))
    XCTAssertEqual(state.presentationMode, .midi)
    state.setPointerHovering(false)
    let exit = try XCTUnwrap(state.widgetHoverDeadline)
    state.setPointerHovering(true)
    state.expireWidgetHover(at: exit)
    XCTAssertEqual(state.presentationMode, .midi, "returning to the strip cancels its exit")
    state.setPointerHovering(false)
    state.expireWidgetHover(at: try XCTUnwrap(state.widgetHoverDeadline))
    XCTAssertEqual(state.presentationMode, .mini)
    XCTAssertEqual(state.activeText, text)
    XCTAssertEqual(state.captureGeneration, generation)
  }

  func testControlsAndDragHoldTheWidgetAndExplicitExpansionCancelsHover() throws {
    for interaction in [OverlayWidgetInteraction.primaryControls, .closeControl, .dragging] {
      let state = OverlayState()
      state.setPointerHovering(true)
      let entry = try XCTUnwrap(state.widgetHoverDeadline)
      state.setWidgetInteraction(interaction, held: true)
      state.expireWidgetHover(at: entry)
      XCTAssertEqual(state.presentationMode, .mini, "never move a pressed or hovered control")
      state.setWidgetInteraction(interaction, held: false)
      state.expireWidgetHover(at: try XCTUnwrap(state.widgetHoverDeadline))
      XCTAssertEqual(state.presentationMode, .midi)
      state.setPointerHovering(false)
      let exit = try XCTUnwrap(state.widgetHoverDeadline)
      state.toggleCollapsed()
      state.expireWidgetHover(at: exit)
      XCTAssertEqual(state.presentationMode, .expanded)
    }
  }

  func testExplicitCollapseWaitsForPointerExitAndHidingCancelsTheTimer() throws {
    let state = OverlayState()
    state.setPointerHovering(true)
    state.toggleCollapsed()
    state.toggleCollapsed()
    XCTAssertEqual(state.presentationMode, .mini)
    XCTAssertNil(state.widgetHoverDeadline, "folding under the pointer must stay folded")
    state.setWidgetInteraction(.primaryControls, held: false)
    XCTAssertNil(state.widgetHoverDeadline)
    state.setPointerHovering(false)
    state.setPointerHovering(true)
    let entry = try XCTUnwrap(state.widgetHoverDeadline)
    state.clearWidgetHover()
    state.expireWidgetHover(at: entry)
    XCTAssertEqual(state.presentationMode, .mini)
    state.setPointerHovering(true)
    XCTAssertNotNil(state.widgetHoverDeadline, "a newly shown widget can hover again")
  }

  func testHeaderRecordingEndsAutomaticHoverAndUsesTheTakePreference() throws {
    let state = OverlayState(micAccessProvider: { true })
    state.engine = OverlayChromePolicyEngine()
    state.attach()
    state.setPointerHovering(true)
    state.expireWidgetHover(at: try XCTUnwrap(state.widgetHoverDeadline))
    state.requestHeaderRecording(.startRecording)
    state.setPointerHovering(false)
    XCTAssertNil(state.widgetHoverDeadline)
    state.handleRecordingPreparing()
    XCTAssertEqual(state.presentationMode, state.expandedByDefault ? .expanded : .mini)
    state.finishControllerRecording()
    state.handleRecordingPreparing()
    XCTAssertEqual(state.presentationMode, state.expandedByDefault ? .expanded : .mini)
    state.finishControllerRecording()
  }

  func testNativeMenuTrackingHoldsMidiUntilTheLastMenuCloses() async throws {
    let state = OverlayState()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "Hover.menu")) as? FloatingOverlayPanel)
    panel.orderFrontRegardless()
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    try await Task.sleep(for: .milliseconds(100))
    state.setPointerHovering(true)
    state.expireWidgetHover(at: try XCTUnwrap(state.widgetHoverDeadline))
    let menus = [NSMenu(title: "Placement"), NSMenu(title: "Position")]
    for menu in menus {
      NotificationCenter.default.post(name: NSMenu.didBeginTrackingNotification, object: menu)
    }
    await Task.yield()
    state.setPointerHovering(false)
    XCTAssertNil(state.widgetHoverDeadline)
    NotificationCenter.default.post(name: NSMenu.didEndTrackingNotification, object: menus[0])
    await Task.yield()
    XCTAssertNil(state.widgetHoverDeadline, "nested tracking still holds the strip")
    NotificationCenter.default.post(name: NSMenu.didEndTrackingNotification, object: menus[1])
    await Task.yield()
    state.expireWidgetHover(at: try XCTUnwrap(state.widgetHoverDeadline))
    XCTAssertEqual(state.presentationMode, .mini)
    panel.invalidatePresence()
    NotificationCenter.default.post(name: NSMenu.didBeginTrackingNotification, object: menus[0])
    await Task.yield()
    state.setPointerHovering(true)
    XCTAssertNotNil(state.widgetHoverDeadline, "a hidden cached panel owns no menu observers")
  }

  func testMountedNativeWidgetRunsItsActualHoverTimerAndFrameAnimation() async throws {
    let state = OverlayState.previewFormatted()
    state.setPresentationMode(.mini)
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "Hover.native")) as? FloatingOverlayPanel)
    panel.setFrameOrigin(NSPoint(x: 900, y: 500))
    panel.orderFrontRegardless()
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.contentView?.layoutSubtreeIfNeeded()
    try await Task.sleep(for: .milliseconds(100))
    let right = panel.frame.maxX
    let top = panel.frame.maxY
    let text = state.activeText
    let generation = state.captureGeneration
    XCTAssertFalse(
      state.isEditingTranscript, "the retained hidden canvas cannot acquire edit focus")
    state.setPointerHovering(true)
    try await waitForMode(.midi, state: state, panel: panel)
    XCTAssertEqual(panel.frame.width, DictationOverlayWindow.midiSize.width, accuracy: 0.5)
    XCTAssertEqual(panel.frame.maxX, right, accuracy: 0.5)
    XCTAssertEqual(panel.frame.maxY, top, accuracy: 0.5)
    state.setPointerHovering(false)
    try await waitForMode(.mini, state: state, panel: panel)
    XCTAssertEqual(panel.frame.width, DictationOverlayWindow.collapsedSize.width, accuracy: 0.5)
    XCTAssertEqual(panel.frame.maxX, right, accuracy: 0.5)
    XCTAssertEqual(state.activeText, text)
    XCTAssertEqual(state.captureGeneration, generation)
  }

  private func waitForMode(
    _ mode: OverlayPresentationMode, state: OverlayState, panel: FloatingOverlayPanel
  ) async throws {
    let deadline = ContinuousClock.now.advanced(by: .seconds(2))
    while state.presentationMode != mode || panel.isFrameTransitioning,
      ContinuousClock.now < deadline
    {
      panel.contentView?.layoutSubtreeIfNeeded()
      try await Task.sleep(for: .milliseconds(20))
    }
    XCTAssertEqual(
      state.presentationMode, mode,
      "deadline \(String(describing: state.widgetHoverDeadline))"
    )
    XCTAssertFalse(panel.isFrameTransitioning)
  }

  func testToolPanelKeepsRailOpenUntilItCloses() {
    var actions = OverlayActionsPresentation()
    actions.pointerChanged(true)
    XCTAssertEqual(actions.phase, .idle)
    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
    actions.panelChanged(true)
    actions.pointerChanged(false)
    actions.expire(at: .now.advanced(by: .seconds(20)))
    XCTAssertEqual(actions.phase, .open)
    XCTAssertNil(actions.hideDeadline)
    actions.panelChanged(false)
    XCTAssertNotNil(actions.hideDeadline)
    actions.expire(at: .now.advanced(by: .seconds(4)))
    XCTAssertEqual(actions.phase, .idle)
  }

  func testKeyboardFocusDoesNotOpenToolsBeforeActivation() {
    var actions = OverlayActionsPresentation()
    actions.focusChanged(true)
    XCTAssertEqual(actions.phase, .idle)
    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
    XCTAssertNil(actions.hideDeadline)
    actions.focusChanged(false)
    XCTAssertNotNil(actions.hideDeadline)
  }

  func testHoverPanelDoesNotResizeItsAnchorOrPerformAnAction() throws {
    let model = HarnessModel()
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 400, y: 300, width: 320, height: 120),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    let host = FirstMouseHostingView(rootView: Harness(model: model))
    host.sizingOptions = []
    panel.contentView = host
    panel.orderFrontRegardless()
    defer {
      model.presented = nil
      panel.orderOut(nil)
    }
    settle(host)
    let before = model.anchor
    XCTAssertGreaterThan(before.width, 0)
    click(panel, host: host, anchor: before)
    settle(host)
    XCTAssertEqual(model.actions, 1, "Baseline synthetic click must reach the button")
    model.actions = 0
    let keyBefore = NSApp.keyWindow
    model.presented = "hover-probe"
    settle(host)
    XCTAssertEqual(model.anchor, before, "A long tooltip must not move the icon")
    XCTAssertEqual(model.actions, 0, "Presentation is never execution")
    XCTAssertFalse(panel.canBecomeKey)
    XCTAssertTrue(NSApp.keyWindow === keyBefore, "Hover must not steal the typing focus")
    let popup = try XCTUnwrap(
      NSApp.windows.first {
        $0.isVisible && String(describing: type(of: $0)).contains("Popover")
      })
    let anchorInWindow = host.convert(before, to: nil)
    let anchorOnScreen = panel.convertToScreen(anchorInWindow)
    XCTAssertGreaterThan(popup.frame.midY, anchorOnScreen.midY, "Tooltip belongs above its button")
    let content = try XCTUnwrap(popup.contentView)
    let bitmap = try XCTUnwrap(content.bitmapImageRepForCachingDisplay(in: content.bounds))
    content.cacheDisplay(in: content.bounds, to: bitmap)
    try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(
      to: FileManager.default.temporaryDirectory.appendingPathComponent(
        "codescribe-hover-panel.png"))

    // Route a paired click through the real non-activating panel, not the closure.
    // A visible tooltip must not consume the first click intended for its button.
    click(panel, host: host, anchor: before)
    settle(host)
    XCTAssertEqual(model.actions, 1, "One click executes once even with a tooltip visible")
  }

  func testDetailsRequireClickAndRemainOpenWithoutHover() throws {
    let model = HarnessModel()
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 400, y: 300, width: 320, height: 120),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    let host = FirstMouseHostingView(rootView: Harness(model: model, opensDetails: true))
    host.sizingOptions = []
    panel.contentView = host
    panel.orderFrontRegardless()
    defer {
      model.presented = nil
      panel.orderOut(nil)
    }
    settle(host)
    XCTAssertNil(model.presented)
    let anchor = model.anchor
    click(panel, host: host, anchor: anchor)
    settle(host)
    XCTAssertEqual(model.presented, "hover-probe")
    RunLoop.main.run(until: Date().addingTimeInterval(0.3))
    XCTAssertEqual(
      model.presented, "hover-probe", "Leaving the anchor must not dismiss clicked details")
    XCTAssertEqual(model.actions, 0, "Opening choices must not execute a command")
    XCTAssertEqual(model.anchor, anchor)
  }

  func testMiniTooltipClearsButtonAndRailWithoutResizingAnchor() {
    for title in ["Copy", "Previous take", "Copy the full transcript"] {
      let model = HarnessModel()
      let host = NSHostingView(
        rootView:
          OverlayTooltipLayout {
            Color.clear.frame(width: 24, height: 24)
              .onGeometryChange(for: CGRect.self) {
                $0.frame(in: .global)
              } action: {
                model.anchor = $0
              }
            Text(title).font(.system(size: 10)).fixedSize()
              .padding(.horizontal, 7).padding(.vertical, 4)
              .onGeometryChange(for: CGRect.self) {
                $0.frame(in: .global)
              } action: {
                model.hint = $0
              }
          }
          .padding(4)
          .frame(width: 320, height: 120)
      )
      host.frame = NSRect(x: 0, y: 0, width: 320, height: 120)
      settle(host)
      XCTAssertEqual(model.anchor.size, CGSize(width: 24, height: 24))
      XCTAssertGreaterThan(model.hint.width, 0)
      XCTAssertLessThanOrEqual(model.hint.maxY, model.anchor.minY - 12)
      XCTAssertEqual(model.hint.midX, model.anchor.midX, accuracy: 0.5)
    }
  }

  private func click(_ panel: NSPanel, host: NSView, anchor: CGRect) {
    let rect = host.convert(anchor, to: nil)
    let location = NSPoint(x: rect.midX, y: rect.midY)
    for (type, number) in [(NSEvent.EventType.leftMouseDown, 1), (.leftMouseUp, 2)] {
      let event = NSEvent.mouseEvent(
        with: type, location: location, modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime,
        windowNumber: panel.windowNumber, context: nil, eventNumber: number, clickCount: 1,
        pressure: 1)!
      panel.sendEvent(event)
    }
  }

  private final class FirstMouseHostingView<Content: View>: NSHostingView<Content> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  }

  private func settle(_ view: NSView) {
    view.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.12))
    view.layoutSubtreeIfNeeded()
  }

  @Observable final class HarnessModel {
    var presented: String?
    var actions = 0
    var anchor: CGRect = .zero
    var hint: CGRect = .zero
  }

  private struct Harness: View {
    @Bindable var model: HarnessModel
    var opensDetails = false

    var body: some View {
      HStack {
        OverlayHoverControl(
          id: "hover-probe", title: "Copy transcript", palette: .dark,
          presented: $model.presented, action: opensDetails ? nil : { model.actions += 1 }
        ) {
          Image(systemName: "doc.on.doc").frame(width: 24, height: 24)
        } detail: { _ in
          Text("Copy this transcript without changing the original recording")
        }
        .onGeometryChange(for: CGRect.self) {
          $0.frame(in: .global)
        } action: {
          model.anchor = $0
        }
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
  }
}
