import AppKit
import Darwin
import QuartzCore
import SwiftUI
import XCTest

@testable import Codescribe

/// Material-only historical fixtures, with the same current native transcript
/// and SwiftUI header in every cell. This is not a whole-app branch comparison.
/// Baselines: develop 6738730ca; Readable Floor ec605c02f; candidate 5459b4ffa.
/// Background measurements cover app-process update/layout work, not compositor
/// power or heat. The visible variant requires the existing isolated-GUI gate.
@MainActor
final class OverlayGlassRenderingProbeTests: XCTestCase {
  private enum Backing: String, CaseIterable {
    case develop, readableFloor, glassBoundary
  }

  private enum Workload: String, CaseIterable {
    case fixedShort, fixedLong, append, pendingRevision
  }

  private struct Subject: View {
    let backing: Backing
    let text: String
    let pending: [NSRange]
    let tick: Int
    private let palette = OverlayAppearancePalette.dark

    var body: some View {
      GlassEffectContainer(spacing: 0) {
        ZStack(alignment: .top) {
          LiveTranscriptTextView(
            text: text, pendingRanges: pending, isEditable: false,
            appearance: .dark, showsActivityUnderline: true,
            contentInsets: NSEdgeInsets(top: 50, left: 20, bottom: 24, right: 20))
          HStack {
            Text("codescribe").font(.system(size: 15, weight: .semibold))
            Spacer()
            Text(verbatim: "\(tick)").monospacedDigit().frame(width: 50)
          }
          .padding(.horizontal, 16)
          .frame(height: 46)
          .modifier(OverlayControlGlass(cornerRadius: 22, role: .chrome))
        }
      }
      .background {
        switch backing {
        case .develop:
          ZStack {
            DevelopMaterial()
            Rectangle().fill(Color(nsColor: OverlayCanvasBackdrop.glassVeil(palette)))
          }
        case .readableFloor:
          ReadableFloorMaterial()
        case .glassBoundary:
          OverlayCanvasBackdrop(
            palette: palette, reduceTransparency: false,
            role: .readingCanvas, showsBorder: true, topInset: 46)
        }
      }
      .clipShape(RoundedRectangle(cornerRadius: 22, style: .continuous))
      .overlay {
        if backing == .develop {
          RoundedRectangle(cornerRadius: 22, style: .continuous)
            .strokeBorder(palette.border.color, lineWidth: 1)
        }
      }
      .shadow(color: .black.opacity(palette.shadowOpacity), radius: 20, x: 0, y: 9)
      .environment(\.colorScheme, .dark)
      .environment(\.csReduceTransparencyOverride, false)
      .environment(\.csContrastOverride, .standard)
    }
  }

  /// The direct native material used by develop, before the native wrapper.
  private struct DevelopMaterial: NSViewRepresentable {
    func makeNSView(context: Context) -> OverlayDesktopGlassView {
      let glass = OverlayDesktopGlassView()
      glass.style = .regular
      glass.cornerRadius = 22
      glass.appearance = NSAppearance(named: .darkAqua)
      return glass
    }

    func updateNSView(_ glass: OverlayDesktopGlassView, context: Context) {
      if glass.cornerRadius != 22 { glass.cornerRadius = 22 }
      if glass.appearance?.name != .darkAqua {
        glass.appearance = NSAppearance(named: .darkAqua)
      }
      if glass.tintColor != nil { glass.tintColor = nil }
    }
  }

  /// Native material, veil, clip and border placement at ec605c02f. The
  /// fixture has no configurable inputs; production's configuration guard
  /// also returns immediately when its reading palette/role stay unchanged.
  private struct ReadableFloorMaterial: NSViewRepresentable {
    func makeNSView(context: Context) -> ReadableFloorBackdrop {
      ReadableFloorBackdrop(frame: .zero)
    }
    func updateNSView(_ view: ReadableFloorBackdrop, context: Context) {}
  }

  private final class ReadableFloorBackdrop: NSView {
    let material = OverlayDesktopGlassView()
    let veil = NSView()

    override init(frame: NSRect) {
      super.init(frame: frame)
      let palette = OverlayAppearancePalette.dark
      appearance = NSAppearance(named: .darkAqua)
      wantsLayer = true
      layer?.cornerRadius = 22
      layer?.masksToBounds = true
      veil.wantsLayer = true
      veil.layer?.backgroundColor = OverlayCanvasBackdrop.glassVeil(palette).cgColor
      veil.layer?.cornerCurve = .continuous
      veil.layer?.cornerRadius = 22
      veil.layer?.borderWidth = 1
      veil.layer?.borderColor = palette.border.nsColor.cgColor
      material.style = .regular
      material.cornerRadius = 22
      material.appearance = appearance
      addSubview(veil)
      addSubview(material, positioned: .below, relativeTo: veil)
    }

    required init?(coder: NSCoder) { nil }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override func setFrameSize(_ size: NSSize) {
      super.setFrameSize(size)
      material.frame = bounds
      veil.frame = bounds
    }
    override func layout() {
      super.layout()
      material.frame = bounds
      veil.frame = bounds
    }
  }

  func testBackgroundUpdateAndLayoutMatrix() throws {
    try runMatrix(visible: false)
  }

  func testVisibleCompositorMatrixRequiresIsolatedGUI() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    try runMatrix(visible: true)
  }

  private func runMatrix(visible: Bool) throws {
    // A matched, deterministic text stream; never reads audio or user content.
    let corpus = String(
      repeating: "A recorded phrase grows without changing its earlier words. ", count: 150)
    let ticks = 24
    for workload in Workload.allCases {
      // Reverse the order in alternate workloads to expose gross order effects.
      let backings =
        workload == .fixedLong || workload == .pendingRevision
        ? Array(Backing.allCases.reversed()) : Backing.allCases
      for backing in backings {
        let initialLength = workload == .fixedLong ? 8000 : 400
        let host = NSHostingView(
          rootView: Subject(
            backing: backing, text: String(corpus.prefix(initialLength)), pending: [], tick: 0))
        host.sizingOptions = []
        host.autoresizingMask = []
        let panel = FloatingOverlayPanel(
          contentRect: NSRect(x: 80, y: 80, width: 640, height: 500),
          styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.contentView = OverlayContentContainer(hosting: host)
        BackgroundTestWindow.present(panel)
        defer { panel.close() }
        settle(panel)
        RunLoop.main.run(until: Date().addingTimeInterval(0.15))
        settle(panel)

        let before = cpuSeconds()
        let started = CACurrentMediaTime()
        var latencies: [Double] = []
        var expected = ""
        for tick in 0..<ticks {
          let length: Int
          switch workload {
          case .fixedShort: length = 400
          case .fixedLong: length = 8000
          case .append, .pendingRevision: length = 400 + 7600 * (tick + 1) / ticks
          }
          expected = String(corpus.prefix(length))
          let pending =
            workload == .pendingRevision
            ? [NSRange(location: max(0, length - 80), length: min(80, length))] : []
          let updateStarted = CACurrentMediaTime()
          host.rootView = Subject(
            backing: backing, text: expected, pending: pending, tick: tick + 1)
          settle(panel)
          RunLoop.main.run(until: Date().addingTimeInterval(0.01))
          settle(panel)
          latencies.append((CACurrentMediaTime() - updateStarted) * 1000)
          RunLoop.main.run(until: Date().addingTimeInterval(0.09))
        }
        let cpu = cpuSeconds() - before
        let wall = CACurrentMediaTime() - started
        XCTAssertTrue(cpu.isFinite && cpu >= 0, "Process CPU accounting must be available")
        let text = try XCTUnwrap(firstText(in: host))
        XCTAssertEqual(text.string, expected, "The workload must reach the real native text view")
        XCTAssertEqual(panel.frame.size, NSSize(width: 640, height: 500))
        let sorted = latencies.sorted()
        let payload: [String: Any] = [
          "schema": "codescribe.glass-rendering-probe.v1",
          "scope": visible ? "visible-isolated-gui" : "background-update-layout-only",
          "backing": backing.rawValue, "workload": workload.rawValue,
          "ticks": ticks, "final_utf16": (text.string as NSString).length,
          "cpu_seconds": cpu, "wall_seconds": wall,
          "update_p50_ms": sorted[sorted.count / 2],
          "update_p95_ms": sorted[min(sorted.count - 1, Int(Double(sorted.count) * 0.95))],
          "compositor_power_measured": false,
        ]
        let data = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
        print("GLASS_PROBE_RESULT \(String(decoding: data, as: UTF8.self))")
      }
    }
  }

  private func settle(_ panel: NSWindow) {
    panel.contentView?.layoutSubtreeIfNeeded()
    panel.displayIfNeeded()
    CATransaction.flush()
  }

  private func cpuSeconds() -> Double {
    var value = rusage()
    guard getrusage(RUSAGE_SELF, &value) == 0 else { return .nan }
    return Double(value.ru_utime.tv_sec + value.ru_stime.tv_sec)
      + Double(value.ru_utime.tv_usec + value.ru_stime.tv_usec) / 1_000_000
  }

  private func firstText(in view: NSView) -> LiveTranscriptNativeTextView? {
    if let text = view as? LiveTranscriptNativeTextView { return text }
    return view.subviews.lazy.compactMap { self.firstText(in: $0) }.first
  }
}
