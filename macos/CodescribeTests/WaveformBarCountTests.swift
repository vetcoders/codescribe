import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class WaveformBarCountTests: XCTestCase {
  func testBarCountGrowsWithWidth() {
    let counts = [120, 240, 900].map {
      WaveformView.effectiveBarCount(width: CGFloat($0), barWidth: 2, gap: 3, minimum: 10)
    }
    XCTAssertEqual(counts, [24, 48, 180])
    XCTAssertLessThan(counts[0], counts[1])
    XCTAssertLessThan(counts[1], counts[2])
    XCTAssertEqual(
      WaveformView.effectiveBarCount(width: 0, barWidth: 2, gap: 3, minimum: 34), 34)
  }

  func testBarWidthIsConstantAcrossWidths() throws {
    for width in [120, 240, 900] {
      let renderer = ImageRenderer(
        content: WaveformView(barCount: 10, active: false, inactiveColor: .red, stretches: true)
          .frame(width: CGFloat(width), height: 16)
      )
      renderer.scale = 2
      let bitmap = NSBitmapImageRep(cgImage: try XCTUnwrap(renderer.cgImage))
      var runs: [Int] = []
      var run = 0
      for x in 0..<bitmap.pixelsWide {
        let alpha = try XCTUnwrap(bitmap.colorAt(x: x, y: bitmap.pixelsHigh / 2)).alphaComponent
        if alpha > 0.5 {
          run += 1
        } else if run > 0 {
          runs.append(run)
          run = 0
        }
      }
      if run > 0 { runs.append(run) }
      XCTAssertEqual(runs.count, [120: 24, 240: 48, 900: 180][width])
      XCTAssertTrue(runs.allSatisfy { $0 == 4 }, "Each bar must remain 2 pt: \(runs)")
    }
  }

  func testResampleNeverIndexesOutOfRange() {
    let samples = (0..<34).map { CGFloat($0) / 33 }
    let levels = WaveformView.resampledLevels(samples, count: 60)
    XCTAssertEqual(levels.count, 60)
    XCTAssertEqual(levels.first, 0)
    XCTAssertEqual(levels.last, 1)
    XCTAssertTrue(levels.allSatisfy { $0.isFinite && (0...1).contains($0) })
    XCTAssertEqual(WaveformView.resampledLevels([], count: 60), Array(repeating: 0, count: 60))
    XCTAssertEqual(WaveformView.resampledLevels([0.5], count: 60), Array(repeating: 0.5, count: 60))
    XCTAssertEqual(WaveformView.resampledLevels(samples, count: 0), [])
    XCTAssertEqual(WaveformView.resampledLevels(samples, count: 1), [0])
    XCTAssertEqual(WaveformView.resampledLevels(samples, count: 34), samples)
  }
}

/// UI-88: the moving waveform runs on Core Animation, not a per-frame SwiftUI timeline.
@MainActor
final class WaveformMotionTests: XCTestCase {
  private func configuration(
    bars count: Int = 12, transcribing: Bool = false, meter: AudioLevelMeter?,
    isVisible: Bool = true
  ) -> WaveformMotionStrip.Configuration {
    WaveformMotionStrip.Configuration(
      bars: (0..<count).map {
        WaveformMotionStrip.Bar(
          sweepDuration: 0.7 + Double(($0 * 7) % 9) / 10, sweepDelay: Double($0) / 14,
          silhouette: 0.3 + CGFloat($0) / 40, color: .red)
      },
      barWidth: 1.5, gap: 2, maxBarHeight: 18, minScale: 0.35, transcribing: transcribing,
      inactiveColor: .gray, meter: meter, isVisible: isVisible)
  }

  private func strip(
    transcribing: Bool = false, meter: AudioLevelMeter?
  ) -> WaveformMotionView {
    let view = WaveformMotionView(frame: NSRect(x: 0, y: 0, width: 60, height: 22))
    view.apply(
      WaveformMotionStrip.Configuration(
        bars: (0..<12).map {
          WaveformMotionStrip.Bar(
            sweepDuration: 0.7 + Double(($0 * 7) % 9) / 10, sweepDelay: Double($0) / 14,
            silhouette: 0.3 + CGFloat($0) / 40, color: .red)
        },
        barWidth: 1.5, gap: 2, maxBarHeight: 18, minScale: 0.35, transcribing: transcribing,
        inactiveColor: .gray, meter: meter))
    return view
  }

  func testLiveBarsSweepOnTheRenderServerAndScaleWithTheMeasuredGain() throws {
    let meter = AudioLevelMeter()
    meter.push(rms: 0.05)
    let gain = try XCTUnwrap(meter.gain)
    let view = strip(meter: meter)
    let movers = try XCTUnwrap(view.movers.sublayers)
    XCTAssertEqual(movers.count, 12)
    XCTAssertEqual(view.cores.sublayers?.count, 12)
    for mover in movers {
      let sweep = try XCTUnwrap(mover.animation(forKey: "sweep") as? CABasicAnimation)
      XCTAssertEqual(sweep.keyPath, "transform.scale.y")
      XCTAssertEqual(sweep.repeatCount, .infinity)
      XCTAssertTrue(sweep.autoreverses)
      XCTAssertEqual(sweep.fromValue as? CGFloat, 0.35)
      XCTAssertEqual(sweep.toValue as? CGFloat, 1)
    }
    XCTAssertFalse(view.movers.isHidden)
    XCTAssertEqual(view.movers.sublayerTransform.m22, 0.35 + CGFloat(gain) * 0.65, accuracy: 1e-6)

    meter.push(rms: 0.3)
    view.refreshGain()
    XCTAssertEqual(
      view.movers.sublayerTransform.m22, 0.35 + CGFloat(try XCTUnwrap(meter.gain)) * 0.65,
      accuracy: 1e-6)
  }

  func testWithoutAMeasuredLevelOnlyTheFlatRestingLineShows() throws {
    let view = strip(meter: AudioLevelMeter())
    XCTAssertTrue(view.movers.isHidden, "No RMS yet: motion would be decoration, not evidence")
    let cores = try XCTUnwrap(view.cores.sublayers)
    XCTAssertEqual(cores.count, 12)
    XCTAssertEqual(cores[0].frame.height, 18 * 0.35, accuracy: 1e-6)
  }

  func testTranscribingStripBreathesAsOneOverItsFrozenSilhouette() throws {
    let view = strip(transcribing: true, meter: nil)
    let breath = try XCTUnwrap(view.movers.animation(forKey: "breath") as? CABasicAnimation)
    XCTAssertEqual(breath.keyPath, "sublayerTransform.scale.y")
    XCTAssertEqual(breath.fromValue as? Double, 0.86)
    XCTAssertEqual(breath.repeatCount, .infinity)
    let movers = try XCTUnwrap(view.movers.sublayers)
    XCTAssertEqual(movers[3].transform.m22, 0.3 + 3.0 / 40, accuracy: 1e-6)
    XCTAssertNil(movers[3].animation(forKey: "sweep"))
    XCTAssertEqual(view.cores.sublayers?.count ?? 0, 0)
  }

  func testDismissingCachedOverlayStopsGainPollingAndShowingItRestartsPolling() throws {
    let meter = AudioLevelMeter()
    meter.push(rms: 0.05)
    let view = strip(meter: meter)
    let window = NSWindow(
      contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
    window.contentView = view
    defer {
      view.stopGainPolling()
      window.contentView = nil
    }
    let timer = try XCTUnwrap(view.gainTimer)
    XCTAssertTrue(timer.isValid)
    view.apply(configuration(meter: meter))
    XCTAssertTrue(view.gainTimer === timer, "An unchanged configuration keeps one timer")
    let scale = view.movers.sublayerTransform.m22
    let sweeps = view.sweepsAdded

    window.orderOut(nil)
    view.apply(configuration(meter: meter, isVisible: false))
    XCTAssertTrue(view.window === window, "The cached overlay keeps its content attached")
    XCTAssertFalse(timer.isValid)
    XCTAssertNil(view.gainTimer)
    meter.push(rms: 0.3)
    view.refreshGain()
    XCTAssertEqual(view.movers.sublayerTransform.m22, scale)

    view.apply(configuration(meter: meter))
    XCTAssertTrue(try XCTUnwrap(view.gainTimer).isValid)
    XCTAssertFalse(view.gainTimer === timer)
    view.refreshGain()
    XCTAssertEqual(
      view.movers.sublayerTransform.m22, 0.35 + CGFloat(try XCTUnwrap(meter.gain)) * 0.65,
      accuracy: 1e-6)
    XCTAssertEqual(view.sweepsAdded, sweeps, "Visibility changes keep the running sweeps")
  }

  func testGainPollingRequiresVisibilityWindowMeterAndCapturePhase() throws {
    let meter = AudioLevelMeter()
    let view = WaveformMotionView(frame: NSRect(x: 0, y: 0, width: 60, height: 22))
    view.apply(configuration(meter: meter, isVisible: false))
    XCTAssertNil(view.gainTimer)
    let window = NSWindow(
      contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
    window.contentView = view
    defer {
      view.stopGainPolling()
      window.contentView = nil
    }
    XCTAssertNil(view.gainTimer, "Attaching a hidden overlay must not start polling")
    view.apply(configuration(transcribing: true, meter: meter))
    XCTAssertNil(view.gainTimer)
    view.apply(configuration(meter: nil))
    XCTAssertNil(view.gainTimer)
    view.apply(configuration(meter: meter))
    let timer = try XCTUnwrap(view.gainTimer)
    view.removeFromSuperview()
    XCTAssertFalse(timer.isValid)
    XCTAssertNil(view.gainTimer)
    view.apply(configuration(meter: meter))
    XCTAssertNil(view.gainTimer, "A detached waveform must not restart polling")
  }

  func testAWideningStripKeepsItsBarsAndTheirRunningSweeps() throws {
    let meter = AudioLevelMeter()
    meter.push(rms: 0.05)
    let view = WaveformMotionView(frame: NSRect(x: 0, y: 0, width: 60, height: 22))
    view.apply(configuration(bars: 12, meter: meter))
    XCTAssertEqual(view.sweepsAdded, 12)
    let bars = try XCTUnwrap(view.movers.sublayers).map(ObjectIdentifier.init)

    view.setFrameSize(NSSize(width: 70, height: 22))
    view.layoutSubtreeIfNeeded()
    view.apply(configuration(bars: 14, meter: meter))
    XCTAssertEqual(view.sweepsAdded, 14, "Only the two new bars get a sweep")
    let widened = try XCTUnwrap(view.movers.sublayers)
    XCTAssertEqual(widened.count, 14)
    XCTAssertEqual(Array(widened.prefix(12)).map(ObjectIdentifier.init), bars)
    XCTAssertEqual(view.cores.sublayers?.count, 14)

    view.apply(configuration(bars: 10, meter: meter))
    view.needsLayout = true
    view.layoutSubtreeIfNeeded()
    XCTAssertEqual(view.sweepsAdded, 14, "Narrowing and a same-size layout attach nothing")
    XCTAssertEqual(view.movers.sublayers?.map(ObjectIdentifier.init), Array(bars.prefix(10)))
    XCTAssertEqual(view.cores.sublayers?.count, 10)
  }
}
