import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// These deterministic host-window controls exercise native notification routing
/// without presenting a test window over the Founder's live recording.
@MainActor
final class WaveformVisibilityTests: XCTestCase {
  private final class VisibilityWindow: NSWindow {
    var presented = false
    var exposed = false
    override var isVisible: Bool { presented }
    override var occlusionState: NSWindow.OcclusionState { exposed ? [.visible] : [] }

    func publishVisibility(presented: Bool, exposed: Bool) {
      self.presented = presented
      self.exposed = exposed
      NotificationCenter.default.post(
        name: NSWindow.didChangeOcclusionStateNotification, object: self)
    }
  }

  private func fixture(presented: Bool = true, exposed: Bool = true)
    -> (VisibilityWindow, WaveformMotionView, AudioLevelMeter)
  {
    let window = VisibilityWindow(
      contentRect: NSRect(x: -10000, y: -10000, width: 80, height: 30),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.presented = presented
    window.exposed = exposed
    let meter = AudioLevelMeter()
    meter.push(rms: 0.05)
    let view = WaveformMotionView(frame: NSRect(x: 0, y: 0, width: 80, height: 30))
    view.apply(
      .init(
        bars: [.init(sweepDuration: 0.8, sweepDelay: 0, silhouette: 0.4, color: .red)],
        barWidth: 2, gap: 3, maxBarHeight: 18, minScale: 0.35,
        transcribing: false, inactiveColor: .gray, meter: meter))
    window.contentView = view
    return (window, view, meter)
  }

  func testAttachedButUnpresentedWindowDoesNotRunGainTimer() {
    let (window, view, _) = fixture(presented: false, exposed: false)
    defer { window.contentView = nil }
    XCTAssertNotNil(view.window, "Attachment alone does not mean the cached window is visible")
    XCTAssertFalse(view.isPollingGain)
  }

  func testSameCachedViewStopsAndResumesWithoutReapplyingConfiguration() async throws {
    let (window, view, meter) = fixture()
    defer { window.contentView = nil }
    XCTAssertTrue(view.isPollingGain)
    let sweeps = view.sweepsAdded
    let bar = try XCTUnwrap(view.movers.sublayers?.first)
    window.publishVisibility(presented: false, exposed: false)
    XCTAssertFalse(view.isPollingGain)
    let heldScale = view.movers.sublayerTransform.m22
    meter.push(rms: 0.3)
    try await Task.sleep(for: .milliseconds(120))
    XCTAssertEqual(view.movers.sublayerTransform.m22, heldScale, accuracy: 1e-6)
    window.publishVisibility(presented: true, exposed: true)
    XCTAssertTrue(view.isPollingGain)
    XCTAssertEqual(
      view.movers.sublayerTransform.m22,
      0.35 + CGFloat(try XCTUnwrap(meter.gain)) * 0.65, accuracy: 1e-6)
    XCTAssertEqual(view.sweepsAdded, sweeps)
    XCTAssertTrue(view.movers.sublayers?.first === bar)
  }

  func testWindowOcclusionStopsOnlyItsOwnWaveform() {
    let (first, firstView, _) = fixture()
    let (second, secondView, _) = fixture()
    defer {
      first.contentView = nil
      second.contentView = nil
    }
    XCTAssertTrue(firstView.isPollingGain)
    XCTAssertTrue(secondView.isPollingGain)
    first.publishVisibility(presented: true, exposed: false)
    XCTAssertFalse(firstView.isPollingGain)
    XCTAssertTrue(secondView.isPollingGain)
    first.publishVisibility(presented: true, exposed: true)
    XCTAssertTrue(firstView.isPollingGain)
  }

  func testDismantledViewDoesNotRestartFromItsFormerWindow() {
    let (window, view, _) = fixture()
    defer { window.contentView = nil }
    XCTAssertTrue(view.isPollingGain)
    WaveformMotionStrip.dismantleNSView(view, coordinator: ())
    XCTAssertFalse(view.isPollingGain)
    window.publishVisibility(presented: true, exposed: true)
    XCTAssertFalse(view.isPollingGain)
  }

  func testReparentingUnsubscribesThePreviousWindow() {
    let (first, view, _) = fixture()
    let second = VisibilityWindow(
      contentRect: .zero, styleMask: [.borderless], backing: .buffered, defer: false)
    defer {
      first.contentView = nil
      second.contentView = nil
    }
    XCTAssertTrue(view.isPollingGain)
    first.contentView = nil
    XCTAssertFalse(view.isPollingGain)
    second.contentView = view
    XCTAssertFalse(view.isPollingGain)
    first.publishVisibility(presented: true, exposed: true)
    XCTAssertFalse(view.isPollingGain)
    second.publishVisibility(presented: true, exposed: true)
    XCTAssertTrue(view.isPollingGain)
  }
}
