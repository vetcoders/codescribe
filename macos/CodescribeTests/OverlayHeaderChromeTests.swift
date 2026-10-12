import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Header controls carry the action pill's hint and a hairline hover ring in
/// the hue of what they open. The hint floats in a child window because the
/// mini and compact header is the whole 46 pt panel.
@MainActor
final class OverlayHeaderChromeTests: XCTestCase {
  func testHintSitsBelowItsControlCenteredInsideTheScreen() {
    let visible = NSRect(x: 0, y: 0, width: 1_440, height: 900)
    let anchor = NSRect(x: 700, y: 800, width: 22, height: 22)
    let frame = OverlayHintPanel.frame(
      size: CGSize(width: 120, height: 20), below: anchor, within: visible)
    XCTAssertEqual(frame.maxY, anchor.minY - OverlayHintPanel.gap)
    XCTAssertEqual(frame.midX, anchor.midX)
  }

  func testHintFlipsAboveWhenTheScreenEndsBelowTheControl() {
    let visible = NSRect(x: 0, y: 0, width: 1_440, height: 900)
    let anchor = NSRect(x: 700, y: 10, width: 22, height: 22)
    let frame = OverlayHintPanel.frame(
      size: CGSize(width: 120, height: 20), below: anchor, within: visible)
    XCTAssertEqual(frame.minY, anchor.maxY + OverlayHintPanel.gap)
  }

  func testHintNeverRunsPastTheScreenEdge() {
    let visible = NSRect(x: 0, y: 0, width: 1_440, height: 900)
    let nearRight = OverlayHintPanel.frame(
      size: CGSize(width: 240, height: 20),
      below: NSRect(x: 1_420, y: 800, width: 22, height: 22), within: visible)
    XCTAssertEqual(nearRight.maxX, visible.maxX - OverlayHintPanel.screenInset)
    let nearLeft = OverlayHintPanel.frame(
      size: CGSize(width: 240, height: 20),
      below: NSRect(x: 0, y: 800, width: 22, height: 22), within: visible)
    XCTAssertEqual(nearLeft.minX, visible.minX + OverlayHintPanel.screenInset)
  }

  /// The hint is a click-through child of the panel that owns the control: it
  /// follows that panel and leaves with it, and only its owner can hide it.
  func testHintIsAClickThroughChildOfTheOverlayPanelAndLeavesWithIt() throws {
    let parent = NSPanel(
      contentRect: NSRect(x: 300, y: 500, width: 400, height: 46),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    let control = NSView(frame: NSRect(x: 300, y: 12, width: 22, height: 22))
    parent.contentView?.addSubview(control)
    parent.orderFrontRegardless()
    defer { parent.orderOut(nil) }
    let owner = OverlayHintAnchor()
    owner.view = control
    let stranger = OverlayHintAnchor()

    owner.show("Show transcription", palette: .dark)
    let hint = try XCTUnwrap(OverlayHintPanel.shared.presentedPanel)
    XCTAssertTrue(hint.parent === parent)
    XCTAssertTrue(hint.ignoresMouseEvents)
    XCTAssertFalse(hint.canBecomeKey)
    let controlOnScreen = parent.convertToScreen(control.convert(control.bounds, to: nil))
    XCTAssertEqual(hint.frame.maxY, controlOnScreen.minY - OverlayHintPanel.gap, accuracy: 0.5)
    XCTAssertEqual(hint.appearance?.name, .darkAqua)

    stranger.hide()
    XCTAssertNotNil(OverlayHintPanel.shared.presentedPanel, "another control cannot hide it")

    owner.hide()
    XCTAssertNil(OverlayHintPanel.shared.presentedPanel)
    XCTAssertNil(hint.parent)

    owner.show("Agents", palette: .light)
    XCTAssertNotNil(OverlayHintPanel.shared.presentedPanel)
    parent.orderOut(nil)
    XCTAssertFalse(hint.isVisible, "no hint outlives its header")
    owner.hide()
  }

  func testHintIsNotShownForAControlOutsideAVisibleWindow() {
    let owner = OverlayHintAnchor()
    let detached = NSView(frame: NSRect(x: 0, y: 0, width: 22, height: 22))
    owner.view = detached
    owner.show("Pin", palette: .light)
    XCTAssertNil(OverlayHintPanel.shared.presentedPanel)
    withExtendedLifetime(detached) {}
  }

  func testAccentsFollowWhatEachControlOpens() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      XCTAssertEqual(
        OverlayHeaderAccent.transcription.color(in: palette), palette.successStatus.color)
      XCTAssertEqual(OverlayHeaderAccent.agents.color(in: palette), palette.agentAccent.color)
      XCTAssertEqual(OverlayHeaderAccent.state.color(in: palette), palette.listeningStatus.color)
    }
  }

  /// Every header control goes through the one hover chrome, in its family's
  /// hue; none relies on the native tooltip that a non-activating panel does
  /// not reliably present.
  func testEveryHeaderControlUsesTheHoverChromeInsteadOfANativeTooltip() throws {
    let view = try source("Overlay/DictationOverlayView.swift")
    XCTAssertTrue(
      view.contains("title: transcriptPreviewHelp, accent: .transcription, palette: palette"))
    XCTAssertTrue(view.contains("accent: recordingDisabled ? nil : .state, palette: palette"))
    XCTAssertTrue(
      view.contains("title: previewAccessibilityLabel, accent: .state, palette: palette"))
    XCTAssertTrue(
      view.contains("title: OverlayIntent.close.helpText, accent: nil, palette: palette"))
    for native in [
      ".help(transcriptPreviewHelp)", ".help(previewAccessibilityLabel)",
      ".help(OverlayIntent.close.helpText)", ".help(recordingLabel",
    ] {
      XCTAssertFalse(view.contains(native), native)
    }

    let roster = try source("Overlay/OverlayChannelStatusView.swift")
    XCTAssertTrue(
      roster.contains("title: String(localized: \"Agents\"), accent: .agents, palette: palette"))
    XCTAssertFalse(roster.contains(".help(\"Agents\")"))

    let menu = try source("Overlay/OverlayHeaderMenu.swift")
    XCTAssertTrue(
      menu.contains(
        ".modifier(OverlayHeaderControlChrome(title: help, accent: .state, palette: palette))"))
    XCTAssertFalse(menu.contains(".help(help)"))
  }

  /// The floating name pill opens the Agents drawer for All as for one agent;
  /// interactive glass gives it the system's hover and press feedback.
  func testConversationNamePillOpensTheAgentsDrawer() throws {
    let conversation = try source("Overlay/OverlayConversationView.swift")
    XCTAssertTrue(conversation.contains("Button(action: onShowAgents) { namePillLabel }"))
    XCTAssertTrue(conversation.contains(".modifier(OverlayNamePillGlass(interactive: true))"))
    XCTAssertTrue(
      conversation.contains(
        "content.glassEffect(interactive ? .regular.interactive() : .regular, in: Capsule())"))
    let overlay = try source("Overlay/DictationOverlayView.swift")
    XCTAssertTrue(overlay.contains("onShowAgents: { state.showAgentMonitor() },"))

    let state = OverlayState()
    let broadcast = OverlayConversation(
      id: "pill-all", channel: "0", name: "All", owner: nil, messages: [])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast]))
    state.selectConversation(broadcast.id)
    state.showAgentMonitor()
    XCTAssertTrue(state.showsAgentMonitor)
    XCTAssertEqual(state.selectedConversationID, broadcast.id, "the drawer keeps the conversation")
  }

  /// Every clickable drawer row carries the tray-like hover wash.
  func testClickableDrawerRowsCarryTheHoverWash() throws {
    let roster = try source("Overlay/OverlayChannelStatusView.swift")
    XCTAssertEqual(
      roster.components(separatedBy: ".modifier(OverlayRowHoverHighlight(").count - 1, 2)
    XCTAssertEqual(roster.components(separatedBy: "OverlayRowHoverHighlight(\n").count - 1, 1)
    XCTAssertTrue(roster.contains("enabled: conversation != nil && onSelectConversation != nil"))
    XCTAssertTrue(roster.contains("opacity(hovered && enabled ? 0.06 : 0)"))
  }

  private func source(_ relative: String) throws -> String {
    let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens")
      .appendingPathComponent(relative)
    return try String(contentsOf: url, encoding: .utf8)
  }
}
