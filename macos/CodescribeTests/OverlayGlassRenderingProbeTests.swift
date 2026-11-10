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
            Text("x9bus").font(.system(size: 15, weight: .semibold))
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

  func testBackgroundFixedShort() throws { try runWorkload(.fixedShort, visible: false) }
  func testBackgroundFixedLong() throws { try runWorkload(.fixedLong, visible: false) }
  func testBackgroundAppend() throws { try runWorkload(.append, visible: false) }
  func testBackgroundPendingRevision() throws { try runWorkload(.pendingRevision, visible: false) }

  func testVisibleFixedShort() throws { try runWorkload(.fixedShort, visible: true) }
  func testVisibleFixedLong() throws { try runWorkload(.fixedLong, visible: true) }
  func testVisibleAppend() throws { try runWorkload(.append, visible: true) }
  func testVisiblePendingRevision() throws { try runWorkload(.pendingRevision, visible: true) }

  func testFullExpandedFixed() throws { try runFullPanel(mini: false, append: false, bus: false) }
  func testFullExpandedAppend() throws { try runFullPanel(mini: false, append: true, bus: false) }
  func testFullExpandedFixedBus() throws { try runFullPanel(mini: false, append: false, bus: true) }
  func testFullExpandedAppendBus() throws { try runFullPanel(mini: false, append: true, bus: true) }
  func testFullMiniFixed() throws { try runFullPanel(mini: true, append: false, bus: false) }
  func testFullMiniAppend() throws { try runFullPanel(mini: true, append: true, bus: false) }
  func testFullMiniFixedBus() throws { try runFullPanel(mini: true, append: false, bus: true) }
  func testFullMiniAppendBus() throws { try runFullPanel(mini: true, append: true, bus: true) }
  func testFullExpandedFixedHeartbeat() throws {
    try runFullPanel(mini: false, append: false, bus: true, heartbeat: true)
  }
  func testFullExpandedAppendHeartbeat() throws {
    try runFullPanel(mini: false, append: true, bus: true, heartbeat: true)
  }
  func testFullMiniFixedHeartbeat() throws {
    try runFullPanel(mini: true, append: false, bus: true, heartbeat: true)
  }
  func testFullMiniAppendHeartbeat() throws {
    try runFullPanel(mini: true, append: true, bus: true, heartbeat: true)
  }

  /// Current production panel, header, mounted text and controller-owned growth.
  /// Synthetic state-boundary input only: no engine, audio, file reader or tray.
  /// The Bus factor repeats equal roster/history snapshots; it does not model
  /// native observation, bridge I/O or playback-mute refresh. The heartbeat
  /// variant changes only each channel's timestamp, retaining all message data.
  private func runFullPanel(mini: Bool, append: Bool, bus: Bool, heartbeat: Bool = false) throws {
    let suite = "CodescribeTests.full-glass-probe.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let state = OverlayState(
      autoSendEnabled: { false }, micAccessProvider: { false },
      autoCollapseScheduler: { _, _ in {} })
    state.channelMapPreference = OverlayChannelMapPreference(defaults: defaults)
    state.sharedChannelNameSetting = { "" }
    state.agentChannelModifierSetting = { "control" }
    // Lifecycle callbacks installed by OverlayController also drive the real
    // AppModel/VoiceLab. Enter listening BEFORE constructing that controller.
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.setPresentationMode(.expanded)
    let scaleKey = suite + ".scale"
    defer { UserDefaults.standard.removeObject(forKey: scaleKey) }
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: scaleKey), sizeDefaults: defaults)
        as? FloatingOverlayPanel)
    let controller = OverlayController(
      state: state, intercomDesktopStore: OverlayIntercomDesktopStore(defaults: defaults),
      overlayEnabledProvider: { true }, assistiveStatusProvider: { false },
      panelFactory: { _, _ in panel }, orderPanelFront: { _ in }, orderPanelOut: { _ in })
    defer {
      controller.hide()
      BackgroundTestWindow.tearDown(panel, state: state)
    }
    controller.show()
    if mini { state.setPresentationMode(.mini) }
    panel.settleFrameTransition()
    BackgroundTestWindow.present(panel)
    settle(panel)
    RunLoop.main.run(until: Date().addingTimeInterval(0.15))
    if QualityCaptureHost.interactiveWindows() {
      XCTAssertTrue(panel.isVisible && panel.alphaValue == 1)
      XCTAssertTrue(
        panel.occlusionState.contains(.visible), "Full-panel compositor probe needs a real GUI")
    }
    let initialHeight = panel.frame.height
    XCTAssertEqual(state.presentationMode, mini ? .mini : .expanded)
    XCTAssertFalse(panel.isFrameTransitioning)

    let roster = (0..<10).map { digit in
      CsChannelRosterState(
        channel: String(digit), audience: "fixture-\(digit)", provider: "codex",
        providerSessionId: "fixture-session-\(digit)", open: false, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
    }
    let snapshot = OverlayChannelDeliverySnapshot(
      deliveries: [],
      conversations: (0..<10).map { digit in
        OverlayConversation(
          id: "fixture-conversation-\(digit)", channel: String(digit), name: "fixture-\(digit)",
          owner: nil,
          messages: (0..<40).map { row in
            OverlayConversationMessage(
              id: "fixture-\(digit)-\(row)", kind: .reply,
              text: String(repeating: "Synthetic retained message. ", count: 8),
              order: UInt64(row), emittedAt: "2026-11-09T00:00:00Z", owner: nil,
              recipients: [], deliveryID: nil, replyTo: nil, unsolicited: true,
              playback: nil, busPath: "", sourceChannel: String(digit))
          })
      })
    // Baseline history outside the timed section; no fresh-reply admission.
    state.applyChannelRoster(roster)
    state.applyConversationSnapshot(snapshot)
    let session = "full-glass-fixture"
    let corpus = String(
      repeating: "A recorded phrase grows without changing earlier words. ", count: 160)
    let ticks = 16
    let inputs = (1...ticks).map { tick in
      String(corpus.prefix(append ? 400 + 7600 * tick / ticks : 400))
    }
    settle(panel)
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    let before = cpuSeconds()
    let started = CACurrentMediaTime()
    let wallStarted = Date().timeIntervalSince1970
    var latencies: [Double] = []
    for (index, text) in inputs.enumerated() {
      let updateStarted = CACurrentMediaTime()
      let sequence = UInt64(index + 1)
      state.applyCompactProjection(
        CsCompactProjection(
          sessionId: session, captureEpoch: 1, sequence: sequence,
          text: text, degraded: false, evidence: []))
      state.applyTranscriptProjection(
        transcriptProjection(
          sequence: sequence, emittedAt: "2026-11-09T00:00:00Z", sessionId: session,
          renderedText: text, phase: "listening", terminal: false,
          reducerAction: "record_ledger_projection"))
      if bus {
        let nextRoster = roster.map { row in
          var changed = row
          if heartbeat { changed.lastHeartbeatUnixMs = 1_794_240_000_000 + Int64(index) * 100 }
          return changed
        }
        state.applyChannelRoster(nextRoster)
        state.applyConversationSnapshot(snapshot)
      }
      settle(panel)
      RunLoop.main.run(until: Date().addingTimeInterval(0.01))
      settle(panel)
      latencies.append((CACurrentMediaTime() - updateStarted) * 1000)
      RunLoop.main.run(until: Date().addingTimeInterval(0.09))
    }
    let cpu = cpuSeconds() - before
    let wall = CACurrentMediaTime() - started
    XCTAssertTrue(cpu.isFinite && cpu >= 0)
    XCTAssertEqual(state.activeText, inputs.last)
    XCTAssertEqual(state.compactProjection?.sequence, UInt64(ticks))
    XCTAssertEqual(state.presentationMode, mini ? .mini : .expanded)
    XCTAssertTrue(state.showsMyDictation)
    if QualityCaptureHost.interactiveWindows() {
      XCTAssertTrue(panel.isVisible && panel.alphaValue == 1)
      XCTAssertTrue(panel.occlusionState.contains(.visible))
    }
    let native = try XCTUnwrap(panel.contentView.flatMap { firstText(in: $0) })
    XCTAssertEqual(native.string, inputs.last, "Even mini must actually exercise the mounted text")
    if append && !mini { XCTAssertGreaterThan(panel.frame.height, initialHeight) }
    if mini { XCTAssertEqual(panel.frame.height, initialHeight) }
    let sorted = latencies.sorted()
    let payload: [String: Any] = [
      "schema": "codescribe.full-overlay-probe.v1",
      "scope": QualityCaptureHost.interactiveWindows()
        ? "visible-isolated-gui" : "background-update-layout-only",
      "mode": mini ? "mini" : "expanded", "workload": append ? "append" : "fixed",
      "equal_bus_snapshots": bus && !heartbeat, "bus_snapshots": bus,
      "heartbeat_changes": heartbeat, "conversation_count": 10, "message_count": 400,
      "ticks": ticks, "final_utf16": native.string.utf16.count,
      "cpu_seconds": cpu, "wall_seconds": wall,
      "start_unix_ms": wallStarted * 1000,
      "end_unix_ms": Date().timeIntervalSince1970 * 1000,
      "initial_height": initialHeight, "final_height": panel.frame.height,
      "window_number": panel.windowNumber,
      "panel_visible": panel.isVisible,
      "occlusion_visible": panel.occlusionState.contains(.visible),
      "update_p50_ms": sorted[sorted.count / 2], "update_p95_ms": sorted[sorted.count - 1],
      "microphone": false, "asr": false, "file_observer": false,
    ]
    let data = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
    print("FULL_OVERLAY_PROBE_RESULT \(String(decoding: data, as: UTF8.self))")
    withExtendedLifetime(controller) {}
  }

  private func runWorkload(_ workload: Workload, visible: Bool) throws {
    if visible { try BackgroundTestWindow.requireIsolatedGUI() }
    // A matched, deterministic text stream; never reads audio or user content.
    let corpus = String(
      repeating: "A recorded phrase grows without changing its earlier words. ", count: 150)
    // Three material cells must remain below the suite's 10 s per-test limit.
    let ticks = 16
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
      if visible {
        XCTAssertTrue(panel.isVisible && panel.alphaValue == 1)
        XCTAssertTrue(panel.occlusionState.contains(.visible), "A visible probe needs a real GUI")
      }

      let wallStarted = Date().timeIntervalSince1970
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
        "start_unix_ms": wallStarted * 1000,
        "end_unix_ms": Date().timeIntervalSince1970 * 1000,
        "window_number": panel.windowNumber,
        "panel_visible": panel.isVisible,
        "occlusion_visible": panel.occlusionState.contains(.visible),
        "update_p50_ms": sorted[sorted.count / 2],
        "update_p95_ms": sorted[min(sorted.count - 1, Int(Double(sorted.count) * 0.95))],
        "compositor_power_measured": false,
      ]
      let data = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
      print("GLASS_PROBE_RESULT \(String(decoding: data, as: UTF8.self))")
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
