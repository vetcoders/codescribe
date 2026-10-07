import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

// Offscreen contact sheet of the production Overlay, Agent, Settings, and Tray
// views. The host is an unordered NSWindow: nothing is ordered front, and a
// visible window fails the cell.
//
// Appearance is resolved per cell by the SwiftUI color-scheme environment and
// by that window's NSAppearance. preferredColorScheme alone does not reach
// NSHostingView or dynamic NSColor, which is why the first sheet was dark in
// both columns.
//
// The bitmap proves layout, clip, opaque ink, and approximate color. It does
// not prove Liquid Glass, vibrancy, or system compositor blur.
//
// Artifacts use CODESCRIBE_VISUAL_EVIDENCE_DIR in the test process, otherwise
// NSTemporaryDirectory()/codescribe-visual-consistency. When launching Xcode,
// pass TEST_RUNNER_CODESCRIBE_VISUAL_EVIDENCE_DIR to forward that value.
//
// Integrator, after sibling surfaces settle and bindings exist:
//   make test-swift SWIFT_TEST_ARGS='-only-testing:CodescribeTests/VisualConsistencyContactSheetTests'

@MainActor
final class VisualConsistencyContactSheetTests: XCTestCase {
  func testProductionSurfacesRenderContactSheet() throws {
    let run = SheetRun()
    let directory = try run.prepareDirectory()

    for (sizeName, size) in [("floor", overlayFloor), ("wide", overlayWide)] {
      for scheme in [EvidenceScheme.dark, .light] {
        run.takeOverlay(
          stateName: "formatted", state: OverlayState.previewFormatted(),
          sizeName: sizeName, scheme: scheme, size: size)
      }
    }
    for scheme in [EvidenceScheme.dark, .light] {
      run.takeOverlay(
        stateName: "error", state: OverlayState.previewError(),
        sizeName: "floor", scheme: scheme, size: overlayFloor)
    }
    for (sizeName, size) in [("floor", agentFloor), ("ideal", agentIdeal)] {
      for scheme in [EvidenceScheme.dark, .light] {
        run.takeAgent(sizeName: sizeName, scheme: scheme, size: size)
      }
    }
    for (sizeName, size) in [("floor", settingsFloor), ("wide", settingsWide)] {
      for scheme in [EvidenceScheme.dark, .light] {
        run.takeSettings(sizeName: sizeName, scheme: scheme, size: size)
      }
    }

    #if DEBUG
      run.takeTray(
        stateName: "idle", recording: false, kind: .idle, tone: .neutral,
        scheme: .dark)
      run.takeTray(
        stateName: "idle", recording: false, kind: .idle, tone: .neutral,
        scheme: .light)
      run.takeTray(
        stateName: "recording", recording: true, kind: .idle, tone: .neutral,
        scheme: .dark)
      run.takeTray(
        stateName: "error",
        recording: false,
        kind: .error,
        tone: .critical,
        scheme: .light
      )
    #else
      run.note("tray cells omitted: TrayStatusStore.preview is DEBUG-only")
    #endif

    run.judgeAppearance()
    do {
      try run.writeArtifacts(to: directory)
    } catch {
      run.failures.append("artifact write failed: \(error)")
    }
    print("VISUAL_CONSISTENCY_EVIDENCE \(directory.url.path)")
    print("VISUAL_CONSISTENCY_EVIDENCE_SOURCE \(directory.source)")

    XCTAssertTrue(
      run.failures.isEmpty,
      "Visual consistency contact sheet failures:\n\(run.failures.joined(separator: "\n"))\nArtifacts: \(directory.url.path) (\(directory.source))"
    )
  }

  func testContactSheetPreservesCellAspect() throws {
    let wide = try solidPNG(
      width: 400, height: 100, color: NSColor(srgbRed: 0.92, green: 0.08, blue: 0.08, alpha: 1))
    let tall = try solidPNG(
      width: 100, height: 400, color: NSColor(srgbRed: 0.08, green: 0.12, blue: 0.92, alpha: 1))
    let cells = [
      RenderedCell(
        measurement: fixtureMeasurement(id: "wide", width: 400, height: 100),
        png: wide
      ),
      RenderedCell(
        measurement: fixtureMeasurement(id: "tall", width: 100, height: 400),
        png: tall
      ),
    ]
    let sheet = try contactSheet(from: cells)
    guard let bitmap = NSBitmapImageRep(data: sheet) else {
      XCTFail("contact sheet produced no bitmap")
      return
    }
    let red = colorBounds(in: bitmap) { $0.redComponent > 0.7 && $0.blueComponent < 0.3 }
    let blue = colorBounds(in: bitmap) { $0.blueComponent > 0.7 && $0.redComponent < 0.3 }
    XCTAssertGreaterThan(red.width, 0)
    XCTAssertGreaterThan(blue.height, 0)
    XCTAssertEqual(Double(red.width) / Double(red.height), 4, accuracy: 0.15)
    XCTAssertEqual(Double(blue.width) / Double(blue.height), 0.25, accuracy: 0.04)
    let tileAspect = contactTileSize.width / contactTileSize.height
    XCTAssertNotEqual(Double(red.width) / Double(red.height), Double(tileAspect), accuracy: 0.2)
  }
}

// Production floors. Wide overlay/settings cells are larger sheets, not a second
// minimum: Settings content minimum is 880×620 with the sidebar open
// (SettingsView.detailMinWidth plus the sidebar). Agent floors come from AgentWindowMetrics. Overlay floor is
// DictationOverlayWindow.minSize.
private let overlayFloor = CGSize(
  width: DictationOverlayWindow.minSize.width,
  height: DictationOverlayWindow.minSize.height
)
private let overlayWide = CGSize(width: 680, height: 420)
private let agentFloor = CGSize(
  width: AgentWindowMetrics.minWidth, height: AgentWindowMetrics.minHeight)
private let agentIdeal = CGSize(
  width: AgentWindowMetrics.idealWidth, height: AgentWindowMetrics.idealHeight)
private let settingsFloor = CGSize(width: 880, height: 620)
private let settingsWide = CGSize(width: 1120, height: 740)
private let trayWidth: CGFloat = 300
private let trayProbeHeight: CGFloat = 760
private let frameSlack: CGFloat = 2
private let appearanceDeltaFloor = 0.04
private let inkAlphaFloor = 16.0 / 255.0
private let inkRatioFloor = 0.10
private let layoutPulse = 0.03
private let layoutTurns = 4
private let contactTileSize = CGSize(width: 280, height: 168)

private enum EvidenceScheme: String, Codable {
  case light
  case dark

  var colorScheme: ColorScheme {
    self == .light ? .light : .dark
  }

  var appearanceName: NSAppearance.Name {
    self == .light ? .aqua : .darkAqua
  }
}

private struct EvidencePoint: Codable {
  var x: Double
  var y: Double
  var width: Double
  var height: Double

  init(_ rect: CGRect) {
    x = Double(rect.origin.x)
    y = Double(rect.origin.y)
    width = Double(rect.size.width)
    height = Double(rect.size.height)
  }
}

private struct EvidenceView: Codable {
  var className: String
  var identifier: String?
  var frame: EvidencePoint
}

private struct CellMeasurement: Codable {
  var id: String
  var surface: String
  var state: String
  var sizeName: String
  var scheme: EvidenceScheme
  var requestedSize: EvidencePoint
  var settledSize: EvidencePoint
  var pixelWidth: Int
  var pixelHeight: Int
  var scale: Double
  var interiorLuminance: Double? = nil
  var inkRatio: Double
  var inkSampleCount: Int
  var maxCornerAlpha: Double
  var resolvedAppearance: String
  var appearanceMatchesRequest: Bool
  var hostFlipped: Bool = false
  var schemeDelta: Double? = nil
  var schemeRequestHonored: Bool? = nil
  var headerDragRegionInsideTopBand: Bool? = nil
  var transcriptScrollsUnderHeader: Bool? = nil
  var headerAndBodyHitsDiffer: Bool? = nil
  var headerHitClass: String? = nil
  var bodyHitClass: String? = nil
  var bottomControlWidthRatio: Double? = nil
  var bottomControlsObserved: Bool = false
  var compositorGlassProven: Bool = false
  var views: [EvidenceView] = []
  var notes: [String] = []
}

private struct EvidenceManifest: Codable {
  var harness: String
  var evidenceDirectory: String
  var evidenceDirectorySource: String
  var hostOrderedFront: Bool
  var bitmapProves: [String]
  var bitmapDoesNotProve: [String]
  var windowCaptureSupplement: String
  var cells: [CellMeasurement]
}

private struct RenderedCell {
  var measurement: CellMeasurement
  var png: Data
}

private struct EvidenceDirectory {
  var url: URL
  var source: String
}

@MainActor
private final class SheetRun {
  var failures: [String] = []
  private(set) var rendered: [RenderedCell] = []
  private var notes: [String] = []
  private var directorySource = "temporary-directory"

  func note(_ message: String) {
    notes.append(message)
  }

  func expect(_ condition: @autoclosure () -> Bool, _ message: @autoclosure () -> String) {
    if !condition() {
      failures.append(message())
    }
  }

  func prepareDirectory() throws -> EvidenceDirectory {
    let resolved = resolveEvidenceDirectory()
    directorySource = resolved.source
    try FileManager.default.createDirectory(at: resolved.url, withIntermediateDirectories: true)
    let cells = resolved.url.appendingPathComponent("cells", isDirectory: true)
    if FileManager.default.fileExists(atPath: cells.path) {
      try FileManager.default.removeItem(at: cells)
    }
    try FileManager.default.createDirectory(at: cells, withIntermediateDirectories: true)
    let receipt = Data("\(resolved.source)\n".utf8)
    try receipt.write(to: resolved.url.appendingPathComponent("evidence-source.txt"))
    return resolved
  }

  func takeOverlay(
    stateName: String,
    state: OverlayState,
    sizeName: String,
    scheme: EvidenceScheme,
    size: CGSize
  ) {
    take(
      id: "overlay-\(stateName)-\(sizeName)-\(scheme.rawValue)",
      surface: "overlay",
      state: stateName,
      sizeName: sizeName,
      scheme: scheme,
      size: size,
      root: DictationOverlayView(state: state)
    )
  }

  func takeAgent(sizeName: String, scheme: EvidenceScheme, size: CGSize) {
    let suite = "codescribe.visual-consistency.\(UUID().uuidString)"
    guard let defaults = UserDefaults(suiteName: suite) else {
      failures.append(
        "agent-\(sizeName)-\(scheme.rawValue) could not open an isolated defaults suite")
      return
    }
    defaults.removePersistentDomain(forName: suite)
    defer { defaults.removePersistentDomain(forName: suite) }

    let thread = ChatThread(
      title: "Layout fixture",
      meta: "local",
      isRestored: false,
      messagesLoaded: true,
      messages: [
        ChatMessage(
          role: .you,
          timestamp: "18:40",
          text: "Where does the header keep its drag region?"
        ),
        ChatMessage(
          role: .assistant,
          timestamp: "18:41",
          text:
            "The header drag region is an inert AppKit view. The transcript stays selectable underneath it, and the toolbelt floats instead of becoming a full-width footer. This sentence is long enough to wrap inside the agent window floor."
        ),
      ]
    )
    let store = AgentChatStore(
      engine: nil,
      threads: [thread],
      loadsThreadIndexEagerly: false,
      persistenceDefaults: defaults
    )
    take(
      id: "agent-thread-\(sizeName)-\(scheme.rawValue)",
      surface: "agent",
      state: "thread",
      sizeName: sizeName,
      scheme: scheme,
      size: size,
      root: AgentChatView(store: store)
    )
  }

  func takeSettings(sizeName: String, scheme: EvidenceScheme, size: CGSize) {
    let model = isolatedSettingsModel()
    model.onQuickStartDictation = {}
    take(
      id: "settings-creator-\(sizeName)-\(scheme.rawValue)",
      surface: "settings",
      state: "creator",
      sizeName: sizeName,
      scheme: scheme,
      size: size,
      root: SettingsView(model: model)
    )
  }

  #if DEBUG
    func takeTray(
      stateName: String,
      recording: Bool,
      kind: CsTrayStatusKind,
      tone: CsTrayStatusTone,
      scheme: EvidenceScheme
    ) {
      let model = TrayViewModel(engine: nil, isRecording: recording)
      model.onQuit = {}
      model.onCheckForUpdates = {}
      model.onOpenSetupWizard = {}
      model.onHelp = {}
      model.onAbout = {}
      model.onIntent = { _ in }
      model.onDictationStartRequested = {}
      model.onSaveLastTranscript = {}
      model.onSaveSelection = {}
      model.onOpenNotesFolder = {}
      model.onOpenTodayNote = {}
      model.onOpenLogFolder = {}
      model.onCopyDebugInfo = {}
      let status = TrayStatusStore.preview(kind: kind, tone: tone)
      take(
        id: "tray-\(stateName)-identity-\(scheme.rawValue)",
        surface: "tray",
        state: stateName,
        sizeName: "identity",
        scheme: scheme,
        size: CGSize(width: trayWidth, height: trayProbeHeight),
        fitHeight: true,
        root: TrayMenuView(viewModel: model, trayStatus: status)
      )
    }
  #endif

  func judgeAppearance() {
    var groups: [String: [Int]] = [:]
    for (index, cell) in rendered.enumerated() {
      let key = "\(cell.measurement.surface)|\(cell.measurement.state)|\(cell.measurement.sizeName)"
      groups[key, default: []].append(index)
    }
    for (key, indexes) in groups {
      let light = indexes.first { rendered[$0].measurement.scheme == .light }
      let dark = indexes.first { rendered[$0].measurement.scheme == .dark }
      guard let light, let dark else { continue }
      let lightCell = rendered[light].measurement
      let darkCell = rendered[dark].measurement
      guard lightCell.appearanceMatchesRequest, darkCell.appearanceMatchesRequest else {
        failures.append(
          "\(key) appearance did not resolve (light \(lightCell.resolvedAppearance), dark \(darkCell.resolvedAppearance))"
        )
        continue
      }
      guard let lightLum = lightCell.interiorLuminance, let darkLum = darkCell.interiorLuminance
      else {
        failures.append("\(key) resolved both appearances but has no opaque luminance sample")
        continue
      }
      let delta = abs(lightLum - darkLum)
      rendered[light].measurement.schemeDelta = delta
      rendered[dark].measurement.schemeDelta = delta
      let honored = delta >= appearanceDeltaFloor
      rendered[light].measurement.schemeRequestHonored = honored
      rendered[dark].measurement.schemeRequestHonored = honored
      if !honored {
        failures.append(
          "\(key) painted the same ink under resolved aqua and darkAqua (luminance delta \(delta) < \(appearanceDeltaFloor))"
        )
      }
    }
  }

  func writeArtifacts(to directory: EvidenceDirectory) throws {
    let cells = directory.url.appendingPathComponent("cells", isDirectory: true)
    for cell in rendered {
      let url = cells.appendingPathComponent("\(cell.measurement.id).png")
      try cell.png.write(to: url)
      expect(cell.png.count > 800, "\(cell.measurement.id) PNG is \(cell.png.count) bytes")
    }
    if !rendered.isEmpty {
      let sheet = try contactSheet(from: rendered)
      try sheet.write(to: directory.url.appendingPathComponent("contact-sheet.png"))
      expect(sheet.count > 800, "contact sheet PNG is \(sheet.count) bytes")
    }
    let manifest = EvidenceManifest(
      harness: "VisualConsistencyContactSheetTests",
      evidenceDirectory: directory.url.path,
      evidenceDirectorySource: directory.source,
      hostOrderedFront: false,
      bitmapProves: [
        "requested pixel size",
        "full-frame ink above alpha \(inkAlphaFloor)",
        "overlay rounded-corner clip",
        "visible AppKit frame after ancestor clipping",
        "overlay header drag region on the visual top edge",
        "overlay transcript insets",
        "opaque-pixel luminance under the cell NSAppearance",
      ],
      bitmapDoesNotProve: [
        "Liquid Glass refraction",
        "NSVisualEffectView behind-window sampling",
        "system compositor blur",
        "vibrancy",
        "keyboard focus ring",
        "hover hint",
        "window shadow",
        "real pointer clicks",
      ],
      windowCaptureSupplement:
        "Each cell is an unordered NSWindow with an explicit aqua or darkAqua appearance. cacheDisplay does not sample the desktop. A screenshot of the installed app is the compositor proof. Do not orderFront during this XCTest.",
      cells: rendered.map(\.measurement) + noteCells
    )
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    try encoder.encode(manifest).write(
      to: directory.url.appendingPathComponent("measurements.json"))
  }

  private var noteCells: [CellMeasurement] {
    guard !notes.isEmpty else { return [] }
    return [
      CellMeasurement(
        id: "harness-notes",
        surface: "harness",
        state: "notes",
        sizeName: "none",
        scheme: .dark,
        requestedSize: EvidencePoint(.zero),
        settledSize: EvidencePoint(.zero),
        pixelWidth: 0,
        pixelHeight: 0,
        scale: 0,
        inkRatio: 0,
        inkSampleCount: 0,
        maxCornerAlpha: 0,
        resolvedAppearance: "none",
        appearanceMatchesRequest: false,
        notes: notes
      )
    ]
  }

  private func take<V: View>(
    id: String,
    surface: String,
    state: String,
    sizeName: String,
    scheme: EvidenceScheme,
    size: CGSize,
    fitHeight: Bool = false,
    root: V
  ) {
    do {
      let appearance =
        NSAppearance(named: scheme.appearanceName) ?? NSAppearance(named: .aqua)!
      let window = NSWindow(
        contentRect: NSRect(origin: .zero, size: size),
        styleMask: surface == "overlay" || surface == "tray"
          ? [.borderless] : [.titled, .closable, .miniaturizable, .resizable],
        backing: .buffered,
        defer: false
      )
      window.isReleasedWhenClosed = false
      window.isRestorable = false
      window.isExcludedFromWindowsMenu = true
      window.appearance = appearance
      window.titleVisibility = .hidden
      window.titlebarAppearsTransparent = true
      defer {
        window.orderOut(nil)
        window.contentView = nil
        window.close()
      }

      let host = NSHostingView(
        rootView:
          root
          .environment(\.colorScheme, scheme.colorScheme)
          .preferredColorScheme(scheme.colorScheme)
          .transaction { transaction in
            transaction.disablesAnimations = true
          }
          .frame(width: size.width, height: fitHeight ? nil : size.height)
          // NSWindow paints this canvas in the app; cacheDisplay captures only its content view.
          .background(surface == "settings" ? Color(nsColor: .windowBackgroundColor) : .clear)
      )
      host.appearance = appearance
      // The window owns the size. Hosting constraints on a resizable window chase
      // fittingSize; DictationOverlayWindow keeps sizingOptions empty for that reason.
      host.sizingOptions = surface == "tray" ? [.intrinsicContentSize] : []
      host.safeAreaRegions = []
      host.translatesAutoresizingMaskIntoConstraints = true
      host.frame = NSRect(origin: .zero, size: size)
      host.autoresizingMask = [.width, .height]
      window.contentView = host
      window.setContentSize(size)

      var settled = size
      var fittingNote: String?
      // Budget stays layoutTurns × layoutPulse. Host frame is assigned above,
      // so it is not a witness. Stop when mounted content is in the tree.
      // A missing witness spends the whole budget, then capture still runs.
      let layoutDeadline = Date().addingTimeInterval(layoutPulse * Double(layoutTurns))
      while true {
        host.layoutSubtreeIfNeeded()
        window.layoutIfNeeded()
        if contentMounted(host, surface: surface, state: state) {
          break
        }
        if Date() >= layoutDeadline {
          break
        }
        let sliceEnd = min(layoutDeadline, Date().addingTimeInterval(layoutPulse))
        RunLoop.main.run(mode: .default, before: sliceEnd)
      }
      if surface == "tray" {
        let fitted = host.fittingSize
        expect(fitted.height > 160, "\(id) intrinsic tray height was not resolved: \(fitted)")
        let height = min(max(ceil(fitted.height), 160), 900)
        settled = CGSize(width: trayWidth, height: height > 1 ? height : trayProbeHeight)
        fittingNote =
          "fittingSize \(fitted.width)×\(fitted.height); settled \(settled.width)×\(settled.height)"
      }
      window.setContentSize(settled)
      host.frame = NSRect(origin: .zero, size: settled)
      host.layoutSubtreeIfNeeded()

      expect(!window.isVisible, "\(id) window became visible; capture stays unordered")
      expect(
        abs(host.bounds.width - settled.width) <= frameSlack
          && abs(host.bounds.height - settled.height) <= frameSlack,
        "\(id) host settled at \(host.bounds.size), requested \(settled)"
      )

      let resolved = appearanceToken(host.effectiveAppearance)
      let appearanceMatches =
        host.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua])
        == scheme.appearanceName
      expect(
        appearanceMatches,
        "\(id) effective appearance \(resolved) did not resolve to \(scheme.appearanceName.rawValue)"
      )

      let bitmap = try XCTUnwrap(
        host.bitmapImageRepForCachingDisplay(in: host.bounds),
        "\(id) produced no bitmap"
      )
      appearance.performAsCurrentDrawingAppearance {
        host.cacheDisplay(in: host.bounds, to: bitmap)
      }
      let png = try XCTUnwrap(
        bitmap.representation(using: .png, properties: [:]),
        "\(id) produced no PNG"
      )

      let scale = settled.width > 0 ? Double(bitmap.pixelsWide) / Double(settled.width) : 0
      let scaleY = settled.height > 0 ? Double(bitmap.pixelsHigh) / Double(settled.height) : 0
      expect(bitmap.pixelsWide > 0 && bitmap.pixelsHigh > 0, "\(id) bitmap is empty")
      expect(abs(scale - scaleY) < 0.05, "\(id) pixel scale x \(scale) != y \(scaleY)")

      let stats = scanPixels(bitmap)
      expect(
        stats.inkRatio > inkRatioFloor,
        "\(id) ink ratio \(stats.inkRatio) over \(stats.sampleCount) samples looks blank (\(stats.alphaSource))"
      )
      if surface == "overlay" {
        expect(
          stats.maxCornerAlpha < 0.2,
          "\(id) corner alpha \(stats.maxCornerAlpha) — rounded clip missing"
        )
      }

      let geometry = inspect(host, surface: surface, state: state, id: id)
      var measurement = CellMeasurement(
        id: id,
        surface: surface,
        state: state,
        sizeName: sizeName,
        scheme: scheme,
        requestedSize: EvidencePoint(CGRect(origin: .zero, size: size)),
        settledSize: EvidencePoint(CGRect(origin: .zero, size: settled)),
        pixelWidth: bitmap.pixelsWide,
        pixelHeight: bitmap.pixelsHigh,
        scale: scale,
        interiorLuminance: stats.luminance,
        inkRatio: stats.inkRatio,
        inkSampleCount: stats.sampleCount,
        maxCornerAlpha: stats.maxCornerAlpha,
        resolvedAppearance: resolved,
        appearanceMatchesRequest: appearanceMatches,
        hostFlipped: host.isFlipped,
        headerDragRegionInsideTopBand: geometry.headerDragInTopBand,
        transcriptScrollsUnderHeader: geometry.transcriptUnderHeader,
        headerAndBodyHitsDiffer: geometry.hitsDiffer,
        headerHitClass: geometry.headerHitClass,
        bodyHitClass: geometry.bodyHitClass,
        bottomControlWidthRatio: geometry.bottomControlWidthRatio,
        bottomControlsObserved: geometry.bottomControlsObserved,
        views: geometry.views,
        notes: geometry.notes
      )
      if let fittingNote {
        measurement.notes.append(fittingNote)
      }
      measurement.notes.append("pixel alpha via \(stats.alphaSource)")
      rendered.append(RenderedCell(measurement: measurement, png: png))
    } catch {
      failures.append("\(id) capture failed: \(error)")
    }
  }

  /// Real mounted content. Frame size is set before the first turn.
  private func contentMounted(_ host: NSView, surface: String, state: String) -> Bool {
    switch surface {
    case "overlay":
      let headerReady = descendants(of: host, where: {
        ($0 as? OverlayWindowDragRegionView)?.accessibilityIdentifier()
          == "overlay-header-drag-region"
      }).contains { view in
        guard let frame = visibleBounds(view, in: host) else { return false }
        return frame.width > 20 && frame.height > 8
      }
      if state == "formatted" {
        let transcript = descendants(of: host, where: { $0 is LiveTranscriptNativeTextView })
        return headerReady && !transcript.isEmpty
      }
      return headerReady
    case "agent":
      return descendants(of: host, where: { $0 is NSSplitView })
        .contains { $0.bounds.width > 100 && $0.bounds.height > 100 }
    case "settings":
      return descendants(of: host, where: { $0 is NSControl && !($0 is NSTextView) })
        .contains { $0.bounds.width > 40 && $0.bounds.height > 8 }
    case "tray":
      return host.fittingSize.height > 160
    default:
      return false
    }
  }

  private func inspect(
    _ host: NSView, surface: String, state: String, id: String
  ) -> GeometryReport {
    var report = GeometryReport()
    let controls = descendants(of: host, where: { $0 is NSControl && !($0 is NSTextView) })
    let textViews = descendants(of: host, where: { $0 is NSTextView })
    let dragRegions = descendants(of: host, where: { $0 is OverlayWindowDragRegionView })
    let effects = descendants(of: host) {
      if #available(macOS 26, *), $0 is OverlayDesktopGlassView { return true }
      return $0 is OverlayDesktopEffectView
    }

    for view in controls + dragRegions + effects {
      guard let placed = place(view, in: host) else { continue }
      switch placed {
      case .clippedAway:
        continue
      case .titlebarChrome(let frame):
        report.notes.append(
          "\(type(of: view)) \(frame) is titlebar chrome outside the content view")
        report.views.append(evidence(view, frame: frame))
      case .inside(let frame):
        report.views.append(evidence(view, frame: frame))
      case .escapes(let frame):
        report.views.append(evidence(view, frame: frame))
        failures.append(
          "\(id) \(type(of: view)) visible \(frame) escapes \(host.bounds.size)"
        )
      }
    }

    for view in textViews {
      guard let placed = place(view, in: host) else { continue }
      switch placed {
      case .clippedAway, .titlebarChrome:
        continue
      case .inside(let frame), .escapes(let frame):
        report.views.append(evidence(view, frame: frame))
        let outsideX = frame.minX < -frameSlack || frame.maxX > host.bounds.width + frameSlack
        if outsideX {
          failures.append("\(id) text view visible \(frame) escapes horizontally")
        }
      }
    }

    if surface == "overlay" {
      let header = dragRegions.filter {
        $0.accessibilityIdentifier() == "overlay-header-drag-region"
      }
      let headerFrame = header.first.flatMap { visibleBounds($0, in: host) }
      let inBand = headerFrame.map { frame in
        frame.width > 20 && frame.height > 8
          && distanceFromVisualTop(frame, host: host) < host.bounds.height * 0.45
          && frame.minX >= -frameSlack && frame.maxX <= host.bounds.width + frameSlack
      }
      report.headerDragInTopBand = inBand ?? false
      expect(
        report.headerDragInTopBand == true,
        "\(id) header drag region missing from the visual top band (flipped=\(host.isFlipped))"
      )

      let transcript = textViews.compactMap { $0 as? LiveTranscriptNativeTextView }
      if state == "formatted" {
        expect(!transcript.isEmpty, "\(id) formatted canvas has no live transcript text view")
      }
      if let text = transcript.first, let scroll = text.enclosingScrollView {
        let under = scroll.contentView.contentInsets.top > 8
        report.transcriptUnderHeader = under
        expect(
          under,
          "\(id) transcript content inset \(scroll.contentView.contentInsets.top) does not clear the header"
        )
      } else if state == "formatted" {
        report.transcriptUnderHeader = false
        failures.append("\(id) transcript has no enclosing scroll view")
      }

      let headerHit = host.hitTest(NSPoint(x: host.bounds.midX, y: visualY(0.92, host: host)))
      let bodyHit = host.hitTest(NSPoint(x: host.bounds.midX, y: visualY(0.50, host: host)))
      report.headerHitClass = headerHit.map { String(describing: type(of: $0)) }
      report.bodyHitClass = bodyHit.map { String(describing: type(of: $0)) }
      if let headerHit, let bodyHit, headerHit !== host, bodyHit !== host {
        report.hitsDiffer = headerHit !== bodyHit
        expect(
          headerHit !== bodyHit,
          "\(id) header and transcript hit the same view \(type(of: headerHit))"
        )
      } else {
        report.notes.append(
          "hitTest did not reach a subview offscreen; click routing was not proved")
      }

      let bottom = controls.filter { view in
        guard let frame = visibleBounds(view, in: host), frame.height >= 10, frame.height <= 44
        else { return false }
        return distanceFromVisualBottom(frame, host: host) < host.bounds.height * 0.32
      }
      let widths = bottom.compactMap { visibleBounds($0, in: host)?.width }
      if let widest = widths.max(), host.bounds.width > 0 {
        let ratio = widest / host.bounds.width
        report.bottomControlsObserved = true
        report.bottomControlWidthRatio = Double(ratio)
        expect(
          ratio < 0.85,
          "\(id) bottom control covers \(ratio) of the width"
        )
      } else {
        report.bottomControlsObserved = false
        report.notes.append(
          "no sized NSControl in the visual bottom band"
        )
      }
    }

    return report
  }

  private func evidence(_ view: NSView, frame: CGRect) -> EvidenceView {
    EvidenceView(
      className: String(describing: type(of: view)),
      identifier: view.accessibilityIdentifier(),
      frame: EvidencePoint(frame)
    )
  }

  private func descendants(of root: NSView, where predicate: (NSView) -> Bool) -> [NSView] {
    var found: [NSView] = []
    func walk(_ view: NSView) {
      if view !== root, predicate(view), !view.isHidden {
        found.append(view)
      }
      for child in view.subviews {
        walk(child)
      }
    }
    walk(root)
    return found
  }
}

private enum ViewPlacement {
  case clippedAway
  case inside(CGRect)
  case escapes(CGRect)
  case titlebarChrome(CGRect)
}

private func place(_ view: NSView, in host: NSView) -> ViewPlacement? {
  let raw = host.convert(view.bounds, from: view)
  guard raw.width >= 1, raw.height >= 1 else { return nil }
  var frame = raw
  var ancestor = view.superview
  while let current = ancestor {
    if current is NSClipView || current.clipsToBounds {
      let clip = host.convert(current.bounds, from: current)
      frame = frame.intersection(clip)
      if frame.isNull || frame.width < 0.5 || frame.height < 0.5 {
        return .clippedAway
      }
    }
    if current === host { break }
    ancestor = current.superview
  }
  let escapes =
    frame.minX < -frameSlack || frame.maxX > host.bounds.width + frameSlack
    || frame.minY < -frameSlack || frame.maxY > host.bounds.height + frameSlack
  if escapes {
    if mostlyAboveVisualTop(frame, host: host), isTitlebarControl(view) {
      return .titlebarChrome(frame)
    }
    return .escapes(frame)
  }
  return .inside(frame)
}

private func visibleBounds(_ view: NSView, in host: NSView) -> CGRect? {
  switch place(view, in: host) {
  case .inside(let frame), .escapes(let frame), .titlebarChrome(let frame):
    return frame
  case .clippedAway, nil:
    return nil
  }
}

private func isTitlebarControl(_ view: NSView) -> Bool {
  let name = String(describing: type(of: view))
  return name.contains("Button") || name.contains("Segment") || name.contains("Toolbar")
    || name.contains("Titlebar")
}

private func mostlyAboveVisualTop(_ frame: CGRect, host: NSView) -> Bool {
  let outside: CGFloat =
    host.isFlipped ? max(0, -frame.minY) : max(0, frame.maxY - host.bounds.height)
  return outside > frame.height * 0.5
}

private func distanceFromVisualTop(_ frame: CGRect, host: NSView) -> CGFloat {
  host.isFlipped ? frame.midY : host.bounds.height - frame.midY
}

private func distanceFromVisualBottom(_ frame: CGRect, host: NSView) -> CGFloat {
  host.isFlipped ? host.bounds.height - frame.midY : frame.midY
}

/// `fraction` is 0 at the visual top and 1 at the visual bottom.
private func visualY(_ fractionFromTop: CGFloat, host: NSView) -> CGFloat {
  host.isFlipped
    ? host.bounds.height * fractionFromTop
    : host.bounds.height * (1 - fractionFromTop)
}

private struct GeometryReport {
  var views: [EvidenceView] = []
  var notes: [String] = []
  var headerDragInTopBand: Bool?
  var transcriptUnderHeader: Bool?
  var hitsDiffer: Bool?
  var headerHitClass: String?
  var bodyHitClass: String?
  var bottomControlWidthRatio: Double?
  var bottomControlsObserved = false
}

private struct PixelScan {
  var inkRatio: Double
  var sampleCount: Int
  var luminance: Double?
  var maxCornerAlpha: Double
  var alphaSource: String
}

private func scanPixels(_ bitmap: NSBitmapImageRep) -> PixelScan {
  let width = bitmap.pixelsWide
  let height = bitmap.pixelsHigh
  guard width > 1, height > 1 else {
    return PixelScan(
      inkRatio: 0, sampleCount: 0, luminance: nil, maxCornerAlpha: 1, alphaSource: "empty")
  }
  if let oriented = orientedRawScan(bitmap) {
    return oriented
  }
  return colorAtScan(bitmap)
}

private func orientedRawScan(_ bitmap: NSBitmapImageRep) -> PixelScan? {
  let width = bitmap.pixelsWide
  let height = bitmap.pixelsHigh
  let x = width / 2
  let y = height / 2
  guard let probed = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { return nil }
  let candidates = [false, true]
  guard
    let flipY = candidates.first(where: { flip in
      guard let alpha = rawAlpha(bitmap, x: x, y: y, flipY: flip) else { return false }
      return abs(alpha - Double(probed.alphaComponent)) < 0.2
    })
  else { return nil }
  return rawScan(bitmap, flipY: flipY)
}

private func rawScan(_ bitmap: NSBitmapImageRep, flipY: Bool) -> PixelScan? {
  let width = bitmap.pixelsWide
  let height = bitmap.pixelsHigh
  let strideX = max(1, width / 160)
  let strideY = max(1, height / 120)
  var samples = 0
  var opaque = 0
  var luminances: [Double] = []
  var y = 0
  while y < height {
    var x = 0
    while x < width {
      guard let alpha = rawAlpha(bitmap, x: x, y: y, flipY: flipY) else { return nil }
      samples += 1
      if alpha > inkAlphaFloor {
        opaque += 1
        // Every 32nd opaque sample, including the first. A prefix of the frame
        // is chrome; the stride still reaches the body on a full-frame scan.
        if opaque == 1 || opaque % 32 == 0, let color = bitmap.colorAt(x: x, y: y),
          let luminance = relativeLuminance(color)
        {
          luminances.append(luminance)
        }
      }
      x += strideX
    }
    y += strideY
  }
  return PixelScan(
    inkRatio: samples == 0 ? 0 : Double(opaque) / Double(samples),
    sampleCount: samples,
    luminance: median(luminances),
    maxCornerAlpha: cornerAlpha(bitmap),
    alphaSource: flipY ? "bitmap-bytes-flipped" : "bitmap-bytes"
  )
}

private func rawAlpha(_ bitmap: NSBitmapImageRep, x: Int, y: Int, flipY: Bool) -> Double? {
  guard bitmap.bitsPerSample == 8, let data = bitmap.bitmapData else { return nil }
  let bpp = bitmap.bitsPerPixel / 8
  guard bpp >= 1 else { return nil }
  let row = flipY ? bitmap.pixelsHigh - 1 - y : y
  guard row >= 0, row < bitmap.pixelsHigh, x >= 0, x < bitmap.pixelsWide else { return nil }
  let offset = row * bitmap.bytesPerRow + x * bpp
  guard offset >= 0, offset + bpp <= bitmap.bytesPerRow * bitmap.pixelsHigh else { return nil }
  if !bitmap.hasAlpha { return 1 }
  let alphaIndex = bitmap.bitmapFormat.contains(.alphaFirst) ? 0 : bpp - 1
  return Double(data[offset + alphaIndex]) / 255
}

private func colorAtScan(_ bitmap: NSBitmapImageRep) -> PixelScan {
  let width = bitmap.pixelsWide
  let height = bitmap.pixelsHigh
  let strideX = max(1, width / 120)
  let strideY = max(1, height / 80)
  var samples = 0
  var opaque = 0
  var luminances: [Double] = []
  var y = 0
  while y < height {
    var x = 0
    while x < width {
      if let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) {
        samples += 1
        if color.alphaComponent > CGFloat(inkAlphaFloor) {
          opaque += 1
          if let luminance = relativeLuminance(color) {
            luminances.append(luminance)
          }
        }
      }
      x += strideX
    }
    y += strideY
  }
  return PixelScan(
    inkRatio: samples == 0 ? 0 : Double(opaque) / Double(samples),
    sampleCount: samples,
    luminance: median(luminances),
    maxCornerAlpha: cornerAlpha(bitmap),
    alphaSource: "colorAt"
  )
}

private func cornerAlpha(_ bitmap: NSBitmapImageRep) -> Double {
  let points = [
    (0, 0),
    (max(bitmap.pixelsWide - 1, 0), 0),
    (0, max(bitmap.pixelsHigh - 1, 0)),
    (max(bitmap.pixelsWide - 1, 0), max(bitmap.pixelsHigh - 1, 0)),
  ]
  let alphas = points.map { point in
    Double(bitmap.colorAt(x: point.0, y: point.1)?.alphaComponent ?? 1)
  }
  return alphas.max() ?? 1
}

private func appearanceToken(_ appearance: NSAppearance) -> String {
  switch appearance.bestMatch(from: [.aqua, .darkAqua]) {
  case .aqua?:
    return "aqua"
  case .darkAqua?:
    return "darkAqua"
  default:
    return "unresolved"
  }
}

private func relativeLuminance(_ color: NSColor) -> Double? {
  guard let rgb = color.usingColorSpace(.sRGB) else { return nil }
  func channel(_ value: CGFloat) -> Double {
    let component = Double(value)
    return component <= 0.04045
      ? component / 12.92
      : pow((component + 0.055) / 1.055, 2.4)
  }
  return 0.2126 * channel(rgb.redComponent) + 0.7152 * channel(rgb.greenComponent)
    + 0.0722 * channel(rgb.blueComponent)
}

private func median(_ values: [Double]) -> Double? {
  guard !values.isEmpty else { return nil }
  let sorted = values.sorted()
  return sorted[sorted.count / 2]
}

private func contactSheet(from cells: [RenderedCell]) throws -> Data {
  let columns = 4
  let tileSize = contactTileSize
  let labelHeight: CGFloat = 32
  let pad: CGFloat = 12
  let rows = Int(ceil(Double(cells.count) / Double(columns)))
  let canvas = CGSize(
    width: pad + CGFloat(columns) * (tileSize.width + pad),
    height: pad + CGFloat(rows) * (tileSize.height + labelHeight + pad)
  )
  return try renderPNG(size: canvas) {
    NSColor(srgbRed: 0.12, green: 0.12, blue: 0.13, alpha: 1).setFill()
    NSBezierPath(rect: CGRect(origin: .zero, size: canvas)).fill()
    let attributes: [NSAttributedString.Key: Any] = [
      .font: NSFont.monospacedSystemFont(ofSize: 9, weight: .regular),
      .foregroundColor: NSColor.white,
    ]
    for (index, cell) in cells.enumerated() {
      let column = index % columns
      let row = index / columns
      let x = pad + CGFloat(column) * (tileSize.width + pad)
      let y = canvas.height - pad - CGFloat(row + 1) * (tileSize.height + labelHeight + pad)
      let tileRect = CGRect(
        x: x, y: y + labelHeight, width: tileSize.width, height: tileSize.height)
      if let cellImage = NSImage(data: cell.png) {
        let aspect = CGSize(
          width: max(cell.measurement.settledSize.width, 1),
          height: max(cell.measurement.settledSize.height, 1)
        )
        cellImage.draw(
          in: aspectFit(aspect, in: tileRect),
          from: .zero,
          operation: .sourceOver,
          fraction: 1,
          respectFlipped: false,
          hints: nil
        )
      }
      let luminance = cell.measurement.interiorLuminance.map { String(format: "%.3f", $0) } ?? "n/a"
      let caption = "\(cell.measurement.id)\nL \(luminance)"
      (caption as NSString).draw(
        in: CGRect(x: x, y: y, width: tileSize.width, height: labelHeight),
        withAttributes: attributes
      )
    }
  }
}

private func aspectFit(_ imageSize: CGSize, in tile: CGRect) -> CGRect {
  guard imageSize.width > 0, imageSize.height > 0 else { return tile }
  let scale = min(tile.width / imageSize.width, tile.height / imageSize.height)
  let size = CGSize(width: imageSize.width * scale, height: imageSize.height * scale)
  return CGRect(
    x: tile.midX - size.width / 2,
    y: tile.midY - size.height / 2,
    width: size.width,
    height: size.height
  )
}

private func solidPNG(width: Int, height: Int, color: NSColor) throws -> Data {
  try renderPNG(size: NSSize(width: width, height: height)) {
    color.setFill()
    NSBezierPath(rect: NSRect(x: 0, y: 0, width: width, height: height)).fill()
  }
}

private func renderPNG(size: CGSize, draw: () -> Void) throws -> Data {
  guard
    let bitmap = NSBitmapImageRep(
      bitmapDataPlanes: nil,
      pixelsWide: Int(size.width),
      pixelsHigh: Int(size.height),
      bitsPerSample: 8,
      samplesPerPixel: 4,
      hasAlpha: true,
      isPlanar: false,
      colorSpaceName: .deviceRGB,
      bytesPerRow: 0,
      bitsPerPixel: 0
    ),
    let context = NSGraphicsContext(bitmapImageRep: bitmap)
  else {
    throw CocoaError(.coderInvalidValue)
  }
  bitmap.size = size
  NSGraphicsContext.saveGraphicsState()
  NSGraphicsContext.current = context
  draw()
  NSGraphicsContext.restoreGraphicsState()
  guard let png = bitmap.representation(using: .png, properties: [:]) else {
    throw CocoaError(.coderInvalidValue)
  }
  return png
}

private func fixtureMeasurement(id: String, width: Double, height: Double) -> CellMeasurement {
  let size = EvidencePoint(CGRect(x: 0, y: 0, width: width, height: height))
  return CellMeasurement(
    id: id,
    surface: "fixture",
    state: "solid",
    sizeName: "aspect",
    scheme: .dark,
    requestedSize: size,
    settledSize: size,
    pixelWidth: Int(width),
    pixelHeight: Int(height),
    scale: 1,
    inkRatio: 1,
    inkSampleCount: 1,
    maxCornerAlpha: 1,
    resolvedAppearance: "fixture",
    appearanceMatchesRequest: true,
  )
}

private func colorBounds(
  in bitmap: NSBitmapImageRep, where predicate: (NSColor) -> Bool
) -> (width: Int, height: Int) {
  var minX = bitmap.pixelsWide
  var minY = bitmap.pixelsHigh
  var maxX = 0
  var maxY = 0
  var hits = 0
  for y in 0..<bitmap.pixelsHigh {
    for x in 0..<bitmap.pixelsWide {
      guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB), predicate(color) else {
        continue
      }
      hits += 1
      minX = min(minX, x)
      minY = min(minY, y)
      maxX = max(maxX, x)
      maxY = max(maxY, y)
    }
  }
  guard hits > 0 else { return (0, 0) }
  return (maxX - minX + 1, maxY - minY + 1)
}

private func resolveEvidenceDirectory() -> EvidenceDirectory {
  if let path = ProcessInfo.processInfo.environment["CODESCRIBE_VISUAL_EVIDENCE_DIR"],
    !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  {
    return EvidenceDirectory(
      url: URL(fileURLWithPath: path, isDirectory: true), source: "process-environment")
  }
  return EvidenceDirectory(
    url: FileManager.default.temporaryDirectory.appendingPathComponent(
      "codescribe-visual-consistency", isDirectory: true),
    source: "temporary-directory"
  )
}

@MainActor
private func isolatedSettingsModel() -> SettingsViewModel {
  // A non-nil engine makes SettingsView.onAppear call whisperModelStatus().
  // Init already seeds the sample snapshot, so the fixture leaves the engine nil.
  let model = SettingsViewModel(
    creatorAgentBridge: OfflineAgentBridge(),
    permissionProbe: MockPermissionProbe(.allGranted),
    agentStatus: MockAgentStatusEngine(),
    mcpAdmin: MockMCPAdminEngine(),
    hotkeys: MockHotkeysEngine(),
    licenseService: .preview,
    runtimeLlmLaneProvider: { lane in
      CsRuntimeLlmLane(
        lane: lane,
        providerId: "fixture",
        providerDisplayName: "Fixture",
        wire: "responses",
        endpoint: "https://fixture.invalid/v1/responses",
        model: "fixture",
        keyAccount: "FIXTURE_KEY",
        keyPresent: false,
        accountAuth: false,
        available: false,
        unavailableReason: "visual fixture"
      )
    },
    servingStatusProvider: { nil }
  )
  model.section = .creator
  model.reloadMcpServers()
  model.loadHotkeys()
  return model
}

private struct OfflineAgentBridge: AgentBridgeInstalling {
  func status() -> AgentBridgeInstallationStatus {
    AgentBridgeInstallationStatus(
      payloadAvailable: true,
      bundleVersion: "visual-fixture",
      installedClients: [],
      installedPaths: [],
      detail: "Offline fixture"
    )
  }

  func install(selectedClients _: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
    throw AgentBridgeInstallationError.payloadUnavailable
  }

  func adoptManualSkill(client _: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
}
