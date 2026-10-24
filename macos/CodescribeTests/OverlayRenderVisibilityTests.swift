import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Native notifications with synthetic visibility properties. These windows
/// are never ordered onto the Founder's desktop and do not open capture.
@MainActor
final class OverlayRenderVisibilityTests: XCTestCase {
  private final class VisibilityWindow: NSWindow {
    var presented = true
    var exposed = true
    override var isVisible: Bool { presented }
    override var occlusionState: NSWindow.OcclusionState { exposed ? [.visible] : [] }

    func publish() {
      NotificationCenter.default.post(
        name: NSWindow.didChangeOcclusionStateNotification, object: self)
    }
  }

  private func fixture() -> (VisibilityWindow, NSView, OverlayRenderVisibility.VisibilityView) {
    let window = VisibilityWindow(
      contentRect: NSRect(x: -10000, y: -10000, width: 80, height: 40),
      styleMask: [.borderless], backing: .buffered, defer: false)
    let parent = NSView(frame: NSRect(x: 0, y: 0, width: 80, height: 40))
    let observer = OverlayRenderVisibility.VisibilityView(frame: parent.bounds)
    parent.addSubview(observer)
    window.contentView = parent
    return (window, parent, observer)
  }

  private func settle() async throws {
    try await Task.sleep(for: .milliseconds(10))
  }

  func testOcclusionStopsMicrophoneMotionWithoutDismissingTheWidget() async throws {
    let (window, _, observer) = fixture()
    defer { window.contentView = nil }
    var visible = false
    var hidden = 0
    observer.onChange = { visible = $0 }
    observer.onHidden = { hidden += 1 }
    try await settle()
    XCTAssertTrue(visible)
    XCTAssertTrue(
      OverlayAgentAudioControls.microphoneWavesActive(
        open: true, renderVisible: visible, reduceMotion: false))
    window.exposed = false
    window.publish()
    try await settle()
    XCTAssertFalse(visible)
    XCTAssertEqual(hidden, 0, "Occlusion suppresses drawing, not widget state")
    XCTAssertFalse(
      OverlayAgentAudioControls.microphoneWavesActive(
        open: true, renderVisible: visible, reduceMotion: false))
    window.exposed = true
    window.publish()
    try await settle()
    XCTAssertTrue(visible)
  }

  func testHiddenAncestorStopsAndUnhidingResumesTheSameObserver() async throws {
    let (window, parent, observer) = fixture()
    defer { window.contentView = nil }
    var visible = false
    observer.onChange = { visible = $0 }
    try await settle()
    XCTAssertTrue(visible)
    parent.isHidden = true
    try await settle()
    XCTAssertFalse(visible)
    parent.isHidden = false
    try await settle()
    XCTAssertTrue(visible)
    XCTAssertNil(observer.hitTest(NSPoint(x: 20, y: 20)), "The observer never owns clicks")
  }

  func testWindowHidePublishesHiddenAndDetachCannotReviveMotion() async throws {
    let (window, _, observer) = fixture()
    defer { window.contentView = nil }
    var visible = false
    var hidden = 0
    observer.onChange = { visible = $0 }
    observer.onHidden = { hidden += 1 }
    try await settle()
    window.presented = false
    window.publish()
    try await settle()
    XCTAssertFalse(visible)
    XCTAssertGreaterThan(hidden, 0)
    observer.removeFromSuperview()
    window.presented = true
    window.publish()
    try await settle()
    XCTAssertFalse(visible)
    XCTAssertNil(observer.window)
  }

  func testDismantleWithdrawsCallbacksBeforeQueuedVisibilityNotification() async throws {
    let (window, _, observer) = fixture()
    defer { window.contentView = nil }
    var callbacks = 0
    observer.onChange = { _ in callbacks += 1 }
    observer.onHidden = { callbacks += 1 }
    try await settle()
    let before = callbacks
    window.publish()
    OverlayRenderVisibility.dismantleNSView(observer, coordinator: ())
    try await settle()
    XCTAssertEqual(callbacks, before)
    XCTAssertNil(observer.onChange)
    XCTAssertNil(observer.onHidden)
  }

  func testReducedMotionAndClosedMicrophoneNeverRequestWavesInAnExposedHost() async throws {
    let (window, _, observer) = fixture()
    defer { window.contentView = nil }
    var visible = false
    observer.onChange = { visible = $0 }
    try await settle()
    XCTAssertTrue(visible)
    XCTAssertFalse(
      OverlayAgentAudioControls.microphoneWavesActive(
        open: true, renderVisible: visible, reduceMotion: true))
    XCTAssertFalse(
      OverlayAgentAudioControls.microphoneWavesActive(
        open: false, renderVisible: visible, reduceMotion: false))
  }

  func testMicrophoneStaticCueReviewSheetUsesTheActualControlInBothAppearances() throws {
    func row(_ label: String, open: Bool, dark: Bool) -> some View {
      let palette: OverlayAppearancePalette = dark ? .dark : .light
      let shade: SheetShade = dark ? .nightlyDark : .nightlyLight
      return HStack(spacing: 24) {
        Text(verbatim: label).font(.system(size: 13)).frame(width: 140, alignment: .leading)
        OverlayAgentAudioControls(
          open: open, muted: false, microphoneEnabled: true, playbackEnabled: true,
          palette: palette, onMicrophone: {}, onPlayback: {},
          broadcastEmphasis: false, chrome: false, diameter: 30)
      }
      .foregroundStyle(palette.primaryText.color)
      .padding(16)
      .background(OverlayColorToken(shade.hex).color)
      .environment(\.colorScheme, dark ? .dark : .light)
      .transaction { $0.disablesAnimations = true }
    }
    let content = VStack(spacing: 0) {
      row("Light · closed", open: false, dark: false)
      row("Light · open", open: true, dark: false)
      row("Dark · closed", open: false, dark: true)
      row("Dark · open", open: true, dark: true)
    }
    // ImageRenderer substitutes warning tiles for the real NSView-backed
    // cursor/visibility backgrounds. Render through the actual AppKit host.
    let host = NSHostingView(rootView: content)
    let size = host.fittingSize
    let window = VisibilityWindow(
      contentRect: NSRect(origin: NSPoint(x: -10000, y: -10000), size: size),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.contentView = host
    defer { window.contentView = nil }
    host.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let image = NSImage(size: host.bounds.size)
    image.addRepresentation(bitmap)
    XCTAssertGreaterThan(image.size.width, 0)
    XCTAssertGreaterThan(image.size.height, 0)
    let attachment = XCTAttachment(image: image)
    attachment.name = "Microphone static open cue — actual shared control, offscreen"
    attachment.lifetime = .keepAlways
    add(attachment)
  }
}
