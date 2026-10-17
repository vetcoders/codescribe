import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

// Falsifiers for the header while the overlay window morphs (2026-10-17).
//
// The window frame is animated by AppKit (NSAnimationContext) while the header
// cross-fade and layout run on SwiftUI's own clock. Every frame of the window
// animation re-lays the header out at the CURRENT, intermediate width. These
// tests sample that layout and demand that the header controls never overlap,
// never leave the window and are never drawn twice.

@MainActor
private final class MorphProbeRecorder {
  var frames = OverlayHeaderControlFrames()
}

@MainActor
private struct MorphProbe: View {
  let state: OverlayState
  let recorder: MorphProbeRecorder

  var body: some View {
    DictationOverlayView(state: state)
      .onPreferenceChange(OverlayHeaderControlFramesPreferenceKey.self) {
        recorder.frames = $0
      }
  }
}

/// One header observation: the probe rects, every mounted hover region of a
/// header control (AppKit truth, one per mounted control) and the width the
/// header was actually laid out in.
private struct MorphSample {
  var label: String
  var width: CGFloat
  var probe: [(name: String, rect: CGRect)]
  var regions: [CGRect]
}

@MainActor
final class OverlayHeaderMorphTests: XCTestCase {
  private struct Rig {
    let state: OverlayState
    let panel: FloatingOverlayPanel
    let host: NSHostingView<MorphProbe>
    let recorder: MorphProbeRecorder
    var animated: Bool
  }

  private final class AnimatedFlag {
    var value = false
  }

  /// A panel assembled the way `DictationOverlayWindow.make` assembles it
  /// (container pinning the hosting view to the window, mode callback that
  /// morphs the frame and publishes the morph duration), with the probe on top.
  private func makeRig(start: OverlayPresentationMode) -> (Rig, AnimatedFlag) {
    let state = OverlayState.previewListening()
    let recorder = MorphProbeRecorder()
    let host = NSHostingView(rootView: MorphProbe(state: state, recorder: recorder))
    host.sizingOptions = []
    host.translatesAutoresizingMaskIntoConstraints = true
    host.autoresizingMask = []
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(origin: .zero, size: DictationOverlayWindow.defaultSize),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered, defer: false)
    panel.contentView = OverlayContentContainer(hosting: host)
    panel.isOpaque = false
    panel.backgroundColor = .clear
    let flag = AnimatedFlag()
    state.onPresentationModeChanged = { [weak panel, weak state] mode in
      guard let panel else { return }
      panel.setPresentationMode(mode, animated: flag.value)
      state?.presentationMorphDuration = panel.lastMorphDuration
    }
    panel.setFrameOrigin(NSPoint(x: 700, y: 500))
    panel.orderFrontRegardless()
    // Reach the starting form without animation.
    switch start {
    case .mini:
      state.setPresentationMode(.mini)
    case .midi:
      state.setPresentationMode(.midi)
    case .expanded:
      state.setPresentationMode(.expanded)
    }
    settle(host)
    flag.value = true
    return (Rig(state: state, panel: panel, host: host, recorder: recorder, animated: true), flag)
  }

  private func settle(_ host: NSView, seconds: TimeInterval = 0.4) {
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(seconds))
    host.layoutSubtreeIfNeeded()
  }

  // MARK: Observation

  private func observe(_ rig: Rig, label: String) -> MorphSample {
    rig.host.layoutSubtreeIfNeeded()
    let frames = rig.recorder.frames
    var probe: [(String, CGRect)] = []
    if let stop = frames.stop { probe.append(("record/stop", stop)) }
    if let preview = frames.preview { probe.append(("preview", preview)) }
    if let waveform = frames.waveform { probe.append(("waveform", waveform)) }
    let host = rig.host
    let headerBand: CGFloat = 46
    let regions: [CGRect] = rig.panel.menuCursorRegions.allObjects.compactMap {
      guard let region = $0 as? OverlayMenuCursorRegionView, region.onHover != nil else {
        return nil
      }
      let rect = region.convert(region.bounds, to: host)
      let top = host.isFlipped ? rect.minY : host.bounds.height - rect.maxY
      guard rect.width <= 30, rect.height <= 30, top < headerBand else { return nil }
      return rect
    }
    return MorphSample(
      label: label, width: host.bounds.width, probe: probe, regions: regions)
  }

  private func overlaps(_ a: CGRect, _ b: CGRect) -> Bool {
    let i = a.intersection(b)
    return !i.isNull && i.width > 0.5 && i.height > 0.5
  }

  /// (a) no overlap, (b) containment, per sample.
  private func geometryViolations(_ sample: MorphSample) -> [String] {
    var out: [String] = []
    let probe = sample.probe
    for i in probe.indices {
      for j in probe.indices where j > i && overlaps(probe[i].rect, probe[j].rect) {
        out.append("\(probe[i].name) \(probe[i].rect) overlaps \(probe[j].name) \(probe[j].rect)")
      }
      let rect = probe[i].rect
      if rect.minX < -0.5 || rect.maxX > sample.width + 0.5 {
        out.append("\(probe[i].name) \(rect) leaves the \(sample.width) pt window")
      }
    }
    for i in sample.regions.indices {
      for j in sample.regions.indices where j > i && overlaps(sample.regions[i], sample.regions[j]) {
        out.append("control hit area \(sample.regions[i]) overlaps \(sample.regions[j])")
      }
      let rect = sample.regions[i]
      if rect.minX < -0.5 || rect.maxX > sample.width + 0.5 {
        out.append("control hit area \(rect) leaves the \(sample.width) pt window")
      }
    }
    return out
  }

  /// (c) one header on screen. Both cross-faded headers mount their own close
  /// dot, recording button and preview button, each with its own hover area, so a
  /// second header shows up as more mounted control hit areas than the settled
  /// form has (the accessibility tree of the hosting view is empty in the test host).
  private func duplicateViolations(_ sample: MorphSample, expectedRegions: Int) -> [String] {
    var out: [String] = []
    if sample.regions.count > expectedRegions {
      out.append(
        "\(sample.regions.count) control hit areas mounted, a single header has \(expectedRegions)")
    }
    return out
  }

  // MARK: Real-time morph

  private func runMorph(
    from start: OverlayPresentationMode, to end: OverlayPresentationMode, name: String
  ) -> (samples: [MorphSample], restRegions: Int) {
    let (rig, _) = makeRig(start: start)
    defer {
      rig.panel.orderOut(nil)
      rig.panel.invalidatePresence()
    }
    let before = observe(rig, label: "\(name) rest before")
    let endRig = rig  // same objects; end rest is read after the morph
    rig.state.setPresentationMode(end)
    let began = Date()
    let duration = rig.panel.lastMorphDuration
    var samples: [MorphSample] = []
    for fraction in [0.25, 0.5, 0.75] {
      let target = began.addingTimeInterval(duration * fraction)
      let wait = target.timeIntervalSinceNow
      if wait > 0 { RunLoop.main.run(until: target) }
      let at = Date().timeIntervalSince(began) / duration
      samples.append(
        observe(
          rig, label: "\(name) @\(Int(fraction * 100))% (measured \(Int(at * 100))% of \(duration) s)"
        ))
    }
    settle(rig.host)
    let after = observe(endRig, label: "\(name) rest after")
    XCTAssertTrue(
      geometryViolations(before).isEmpty && geometryViolations(after).isEmpty,
      "\(name): the probe itself reports a violation at rest, the falsifier is unsound: "
        + "\(geometryViolations(before) + geometryViolations(after))")
    return (samples, max(before.regions.count, after.regions.count))
  }

  private func assertMorph(
    from start: OverlayPresentationMode, to end: OverlayPresentationMode, name: String,
    file: StaticString = #filePath, line: UInt = #line
  ) {
    let run = runMorph(from: start, to: end, name: name)
    for sample in run.samples {
      let geometry = geometryViolations(sample)
      XCTAssertTrue(
        geometry.isEmpty,
        "\(sample.label), window \(sample.width) pt: header controls collide: \(geometry)",
        file: file, line: line)
      let duplicates = duplicateViolations(sample, expectedRegions: run.restRegions)
      XCTAssertTrue(
        duplicates.isEmpty,
        "\(sample.label), window \(sample.width) pt: header drawn twice: \(duplicates)",
        file: file, line: line)
    }
  }

  func testHeaderControlsNeverCollideOrDuplicateMidMorphMiniToMidi() {
    assertMorph(from: .mini, to: .midi, name: "mini->midi")
  }

  func testHeaderControlsNeverCollideOrDuplicateMidMorphMidiToExpanded() {
    assertMorph(from: .midi, to: .expanded, name: "midi->expanded")
  }

  func testHeaderControlsNeverCollideOrDuplicateMidMorphExpandedToMidi() {
    assertMorph(from: .expanded, to: .midi, name: "expanded->midi")
  }

  func testHeaderControlsNeverCollideOrDuplicateMidMorphMidiToMini() {
    assertMorph(from: .midi, to: .mini, name: "midi->mini")
  }

  // MARK: Deterministic intermediate widths

  /// The window animator hands the hosting view every width between the two
  /// forms. Walk those widths by hand, no clock involved: whatever form the
  /// state is in must lay its controls out without overlap inside the width.
  func testHeaderControlsFitEveryIntermediateWindowWidth() {
    let cases: [(OverlayPresentationMode, ClosedRange<CGFloat>)] = [
      (.midi, 200...410), (.expanded, 320...470), (.midi, 320...470),
    ]
    for (mode, widths) in cases {
      let (rig, flag) = makeRig(start: mode)
      flag.value = false
      defer {
        rig.panel.orderOut(nil)
        rig.panel.invalidatePresence()
      }
      let height = rig.panel.frame.height
      var bad: [String] = []
      var width = widths.lowerBound
      while width <= widths.upperBound {
        // The animator resizes the window; the container pins the hosting view.
        var frame = rig.panel.frame
        frame.origin.x = frame.maxX - width
        frame.size = NSSize(width: width, height: height)
        rig.panel.setFrame(frame, display: true)
        settle(rig.host, seconds: 0.03)
        let sample = observe(rig, label: "\(mode) at \(width) pt")
        let violations = geometryViolations(sample)
        if !violations.isEmpty { bad.append("\(sample.label): \(violations)") }
        width += 15
      }
      XCTAssertTrue(
        bad.isEmpty,
        "\(mode) header cannot lay out at the widths the morph passes through: \(bad)")
    }
  }
}
