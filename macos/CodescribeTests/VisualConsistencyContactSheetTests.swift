import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

// Offline contact sheet of the production Overlay, Agent, Settings, and Tray
// views. The host is an offscreen NSHostingView; nothing is ordered front.
// cacheDisplay proves layout, clip, approximate color, and AppKit frames.
// It does not prove Liquid Glass refraction, vibrancy, or behind-window sampling.
//
// Artifacts land in CODESCRIBE_VISUAL_EVIDENCE_DIR, or under NSTemporaryDirectory.
// Integrator, after sibling surfaces settle and bindings exist:
//   make test-swift SWIFT_TEST_ARGS='-only-testing:CodescribeTests/VisualConsistencyContactSheetTests'

@MainActor
final class VisualConsistencyContactSheetTests: XCTestCase {
  func testProductionSurfacesRenderContactSheet() throws {
    let run = SheetRun()
    let directory = try run.prepareDirectory()

    run.takeOverlay(
      stateName: "formatted",
      state: OverlayState.previewFormatted(),
      sizeName: "floor",
      scheme: .dark,
      size: overlayFloor
    )
    run.takeOverlay(
      stateName: "formatted",
      state: OverlayState.previewFormatted(),
      sizeName: "floor",
      scheme: .light,
      size: overlayFloor
    )
    run.takeOverlay(
      stateName: "formatted",
      state: OverlayState.previewFormatted(),
      sizeName: "wide",
      scheme: .dark,
      size: overlayWide
    )
    run.takeOverlay(
      stateName: "formatted",
      state: OverlayState.previewFormatted(),
      sizeName: "wide",
      scheme: .light,
      size: overlayWide
    )
    run.takeOverlay(
      stateName: "error",
      state: OverlayState.previewError(),
      sizeName: "floor",
      scheme: .dark,
      size: overlayFloor
    )
    run.takeOverlay(
      stateName: "error",
      state: OverlayState.previewError(),
      sizeName: "floor",
      scheme: .light,
      size: overlayFloor
    )

    run.takeAgent(sizeName: "floor", scheme: .dark, size: agentFloor)
    run.takeAgent(sizeName: "floor", scheme: .light, size: agentFloor)
    run.takeAgent(sizeName: "ideal", scheme: .dark, size: agentIdeal)
    run.takeAgent(sizeName: "ideal", scheme: .light, size: agentIdeal)

    run.takeSettings(sizeName: "floor", scheme: .dark, size: settingsFloor)
    run.takeSettings(sizeName: "floor", scheme: .light, size: settingsFloor)
    run.takeSettings(sizeName: "wide", scheme: .dark, size: settingsWide)
    run.takeSettings(sizeName: "wide", scheme: .light, size: settingsWide)

    #if DEBUG
      run.takeTray(
        stateName: "idle", recording: false, kind: .idle, tone: .neutral, label: "Status: Idle",
        scheme: .dark)
      run.takeTray(
        stateName: "idle", recording: false, kind: .idle, tone: .neutral, label: "Status: Idle",
        scheme: .light)
      run.takeTray(
        stateName: "recording", recording: true, kind: .idle, tone: .neutral, label: "Status: Idle",
        scheme: .dark)
      run.takeTray(
        stateName: "error",
        recording: false,
        kind: .error,
        tone: .critical,
        label: "Status: Microphone unavailable",
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
    print("VISUAL_CONSISTENCY_EVIDENCE \(directory.path)")

    XCTAssertTrue(
      run.failures.isEmpty,
      "Visual consistency contact sheet failures:\n\(run.failures.joined(separator: "\n"))\nArtifacts: \(directory.path)"
    )
  }
}

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
private let layoutPulse: TimeInterval = 0.05

private enum EvidenceScheme: String, Codable {
  case light
  case dark

  var colorScheme: ColorScheme {
    self == .light ? .light : .dark
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
  var interiorLuminance: Double?
  var inkRatio: Double
  var maxCornerAlpha: Double
  var schemeDelta: Double?
  var schemeRequestHonored: Bool?
  var headerDragRegionInsideTopBand: Bool?
  var transcriptScrollsUnderHeader: Bool?
  var headerAndBodyHitsDiffer: Bool?
  var headerHitClass: String?
  var bodyHitClass: String?
  var bottomControlWidthRatio: Double?
  var bottomControlsObserved: Bool
  var compositorGlassProven: Bool
  var views: [EvidenceView]
  var notes: [String]
}

private struct EvidenceManifest: Codable {
  var harness: String
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

@MainActor
private final class SheetRun {
  var failures: [String] = []
  private(set) var rendered: [RenderedCell] = []
  private var notes: [String] = []

  func note(_ message: String) {
    notes.append(message)
  }

  func expect(_ condition: @autoclosure () -> Bool, _ message: @autoclosure () -> String) {
    if !condition() {
      failures.append(message())
    }
  }

  func prepareDirectory() throws -> URL {
    let root: URL
    if let override = ProcessInfo.processInfo.environment["CODESCRIBE_VISUAL_EVIDENCE_DIR"],
      !override.isEmpty
    {
      root = URL(fileURLWithPath: override, isDirectory: true)
    } else {
      root = FileManager.default.temporaryDirectory.appendingPathComponent(
        "codescribe-visual-consistency", isDirectory: true)
    }
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    let cells = root.appendingPathComponent("cells", isDirectory: true)
    if FileManager.default.fileExists(atPath: cells.path) {
      try FileManager.default.removeItem(at: cells)
    }
    try FileManager.default.createDirectory(at: cells, withIntermediateDirectories: true)
    return root
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
    let model = SettingsViewModel.preview(.creator)
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
      label: String,
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
      let status = TrayStatusStore.preview(kind: kind, tone: tone, label: label)
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
      let lightLum = rendered[light].measurement.interiorLuminance
      let darkLum = rendered[dark].measurement.interiorLuminance
      let overlay = rendered[light].measurement.surface == "overlay"
      guard let lightLum, let darkLum else {
        if overlay {
          failures.append("\(key) is missing an interior luminance sample")
        }
        continue
      }
      let delta = abs(lightLum - darkLum)
      rendered[light].measurement.schemeDelta = delta
      rendered[dark].measurement.schemeDelta = delta
      let honored = delta >= appearanceDeltaFloor
      rendered[light].measurement.schemeRequestHonored = honored
      rendered[dark].measurement.schemeRequestHonored = honored
      if overlay {
        expect(
          honored,
          "\(key) light/dark interior luminance delta \(delta) is below \(appearanceDeltaFloor)"
        )
      }
    }
  }

  func writeArtifacts(to directory: URL) throws {
    let cells = directory.appendingPathComponent("cells", isDirectory: true)
    for cell in rendered {
      let url = cells.appendingPathComponent("\(cell.measurement.id).png")
      try cell.png.write(to: url)
      expect(cell.png.count > 800, "\(cell.measurement.id) PNG is \(cell.png.count) bytes")
    }
    if !rendered.isEmpty {
      let sheet = try contactSheet(from: rendered)
      try sheet.write(to: directory.appendingPathComponent("contact-sheet.png"))
      expect(sheet.count > 800, "contact sheet PNG is \(sheet.count) bytes")
    }
    let manifest = EvidenceManifest(
      harness: "VisualConsistencyContactSheetTests",
      hostOrderedFront: false,
      bitmapProves: [
        "requested pixel size",
        "non-blank ink",
        "overlay rounded-corner clip",
        "AppKit frame containment",
        "overlay header drag region",
        "overlay transcript insets",
        "approximate interior luminance",
      ],
      bitmapDoesNotProve: [
        "Liquid Glass refraction",
        "NSVisualEffectView behind-window sampling",
        "vibrancy",
        "keyboard focus ring",
        "hover hint",
        "window shadow",
        "real pointer clicks",
      ],
      windowCaptureSupplement:
        "One non-activating NSWindow per surface, off the user's active space if possible, captured with CGWindowListCreateImage after a short layout. Still incomplete for some glass. A screenshot of the installed app is the compositor proof. Do not orderFront during this XCTest.",
      cells: rendered.map(\.measurement) + noteCells
    )
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    try encoder.encode(manifest).write(
      to: directory.appendingPathComponent("measurements.json"))
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
        interiorLuminance: nil,
        inkRatio: 0,
        maxCornerAlpha: 0,
        schemeDelta: nil,
        schemeRequestHonored: nil,
        headerDragRegionInsideTopBand: nil,
        transcriptScrollsUnderHeader: nil,
        headerAndBodyHitsDiffer: nil,
        headerHitClass: nil,
        bodyHitClass: nil,
        bottomControlWidthRatio: nil,
        bottomControlsObserved: false,
        compositorGlassProven: false,
        views: [],
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
      let cell = try capture(
        id: id,
        surface: surface,
        state: state,
        sizeName: sizeName,
        scheme: scheme,
        size: size,
        root:
          root
          .transaction { transaction in
            transaction.disablesAnimations = true
          }
          .frame(width: size.width, height: fitHeight ? nil : size.height)
          .preferredColorScheme(scheme.colorScheme)
      )
      rendered.append(cell)
    } catch {
      failures.append("\(id) capture failed: \(error)")
    }
  }

  private func capture<V: View>(
    id: String,
    surface: String,
    state: String,
    sizeName: String,
    scheme: EvidenceScheme,
    size: CGSize,
    root: V
  ) throws -> RenderedCell {
    let host = NSHostingView(rootView: root)
    host.frame = CGRect(origin: .zero, size: size)
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(layoutPulse))
    host.layoutSubtreeIfNeeded()

    var settled = size
    var fittingNote: String?
    if surface == "tray" {
      let fitted = host.fittingSize
      let height = min(max(ceil(fitted.height), 160), 900)
      settled = CGSize(width: trayWidth, height: height > 1 ? height : trayProbeHeight)
      host.frame = CGRect(origin: .zero, size: settled)
      host.layoutSubtreeIfNeeded()
      fittingNote =
        "fittingSize \(fitted.width)×\(fitted.height); settled \(settled.width)×\(settled.height)"
    }

    expect(
      abs(host.bounds.width - settled.width) <= frameSlack
        && abs(host.bounds.height - settled.height) <= frameSlack,
      "\(id) host settled at \(host.bounds.size), requested \(settled)"
    )

    let bitmap = try XCTUnwrap(
      host.bitmapImageRepForCachingDisplay(in: host.bounds),
      "\(id) produced no bitmap"
    )
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let png = try XCTUnwrap(
      bitmap.representation(using: .png, properties: [:]),
      "\(id) produced no PNG"
    )

    let scale = settled.width > 0 ? Double(bitmap.pixelsWide) / Double(settled.width) : 0
    let scaleY = settled.height > 0 ? Double(bitmap.pixelsHigh) / Double(settled.height) : 0
    expect(bitmap.pixelsWide > 0 && bitmap.pixelsHigh > 0, "\(id) bitmap is empty")
    expect(abs(scale - scaleY) < 0.05, "\(id) pixel scale x \(scale) != y \(scaleY)")

    let stats = bitmapStats(bitmap)
    expect(stats.inkRatio > 0.2, "\(id) ink ratio \(stats.inkRatio) looks blank")
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
      maxCornerAlpha: Double(stats.maxCornerAlpha),
      schemeDelta: nil,
      schemeRequestHonored: nil,
      headerDragRegionInsideTopBand: geometry.headerDragInTopBand,
      transcriptScrollsUnderHeader: geometry.transcriptUnderHeader,
      headerAndBodyHitsDiffer: geometry.hitsDiffer,
      headerHitClass: geometry.headerHitClass,
      bodyHitClass: geometry.bodyHitClass,
      bottomControlWidthRatio: geometry.bottomControlWidthRatio,
      bottomControlsObserved: geometry.bottomControlsObserved,
      compositorGlassProven: false,
      views: geometry.views,
      notes: geometry.notes
    )
    if let fittingNote {
      measurement.notes.append(fittingNote)
    }
    return RenderedCell(measurement: measurement, png: png)
  }

  private func inspect(
    _ host: NSView, surface: String, state: String, id: String
  ) -> GeometryReport {
    var report = GeometryReport()
    let controls = descendants(of: host, where: { $0 is NSControl && !($0 is NSTextView) })
    let textViews = descendants(of: host, where: { $0 is NSTextView })
    let dragRegions = descendants(of: host, where: { $0 is OverlayWindowDragRegionView })
    let effects = descendants(of: host, where: { $0 is OverlayDesktopEffectView })

    for view in controls + dragRegions + effects {
      let frame = host.convert(view.bounds, from: view)
      guard frame.width >= 1, frame.height >= 1 else { continue }
      report.views.append(
        EvidenceView(
          className: String(describing: type(of: view)),
          identifier: view.accessibilityIdentifier(),
          frame: EvidencePoint(frame)
        )
      )
      let outside =
        frame.minX < -frameSlack || frame.maxX > host.bounds.width + frameSlack
        || frame.minY < -frameSlack || frame.maxY > host.bounds.height + frameSlack
      if outside {
        failures.append(
          "\(id) \(type(of: view)) \(frame) escapes \(host.bounds.size)"
        )
      }
    }

    for view in textViews {
      let frame = host.convert(view.bounds, from: view)
      guard frame.width >= 1, frame.height >= 1 else { continue }
      report.views.append(
        EvidenceView(
          className: String(describing: type(of: view)),
          identifier: view.accessibilityIdentifier(),
          frame: EvidencePoint(frame)
        )
      )
      let outsideX = frame.minX < -frameSlack || frame.maxX > host.bounds.width + frameSlack
      if outsideX {
        failures.append("\(id) text view \(frame) escapes horizontally")
      }
    }

    if surface == "overlay" {
      let header = dragRegions.filter {
        $0.accessibilityIdentifier() == "overlay-header-drag-region"
      }
      let headerFrame = header.first.map { host.convert($0.bounds, from: $0) }
      let inBand = headerFrame.map { frame in
        frame.width > 20 && frame.height > 8 && frame.midY > host.bounds.height * 0.55
          && frame.minX >= -frameSlack && frame.maxX <= host.bounds.width + frameSlack
      }
      report.headerDragInTopBand = inBand ?? false
      expect(
        report.headerDragInTopBand == true,
        "\(id) header drag region missing from the top band"
      )

      let transcript = textViews.compactMap { $0 as? LiveTranscriptNativeTextView }
      if state == "formatted" {
        expect(!transcript.isEmpty, "\(id) formatted canvas has no live transcript text view")
      }
      if let text = transcript.first, let scroll = text.enclosingScrollView {
        let under = scroll.contentInsets.top > 8
        report.transcriptUnderHeader = under
        expect(
          under,
          "\(id) transcript content inset \(scroll.contentInsets.top) does not clear the header")
      } else if state == "formatted" {
        report.transcriptUnderHeader = false
        failures.append("\(id) transcript has no enclosing scroll view")
      }

      let headerHit = host.hitTest(NSPoint(x: host.bounds.midX, y: host.bounds.maxY - 24))
      let bodyHit = host.hitTest(NSPoint(x: host.bounds.midX, y: host.bounds.midY))
      report.headerHitClass = headerHit.map { String(describing: type(of: $0)) }
      report.bodyHitClass = bodyHit.map { String(describing: type(of: $0)) }
      if let headerHit, let bodyHit {
        report.hitsDiffer = headerHit !== bodyHit
        expect(
          headerHit !== bodyHit,
          "\(id) header and transcript hit the same view \(type(of: headerHit))"
        )
      } else {
        report.notes.append("hitTest returned nil offscreen; click routing was not proved")
      }

      let bottom = controls.filter {
        host.convert($0.bounds, from: $0).midY < host.bounds.height * 0.32
      }
      let widths = bottom.map { host.convert($0.bounds, from: $0).width }
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
          "no NSControl in the bottom band; SwiftUI toolbelt may not materialize offscreen"
        )
      }
    }

    return report
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

  private func bitmapStats(_ bitmap: NSBitmapImageRep) -> BitmapStats {
    let samples = interiorSamples(bitmap)
    var opaque = 0
    var luminances: [Double] = []
    for (x, y) in samples {
      guard let color = bitmap.colorAt(x: x, y: y) else { continue }
      if color.alphaComponent > 0.15 {
        opaque += 1
      }
      if let luminance = relativeLuminance(color) {
        luminances.append(luminance)
      }
    }
    let corners = [
      bitmap.colorAt(x: 0, y: 0)?.alphaComponent ?? 1,
      bitmap.colorAt(x: max(bitmap.pixelsWide - 1, 0), y: 0)?.alphaComponent ?? 1,
      bitmap.colorAt(x: 0, y: max(bitmap.pixelsHigh - 1, 0))?.alphaComponent ?? 1,
      bitmap.colorAt(x: max(bitmap.pixelsWide - 1, 0), y: max(bitmap.pixelsHigh - 1, 0))?
        .alphaComponent ?? 1,
    ]
    let ink = samples.isEmpty ? 0 : Double(opaque) / Double(samples.count)
    return BitmapStats(
      inkRatio: ink,
      luminance: median(luminances),
      maxCornerAlpha: corners.max() ?? 1
    )
  }

  private func interiorSamples(_ bitmap: NSBitmapImageRep) -> [(Int, Int)] {
    let width = bitmap.pixelsWide
    let height = bitmap.pixelsHigh
    guard width > 8, height > 8 else { return [] }
    let x0 = width * 18 / 100
    let y0 = height * 18 / 100
    let x1 = max(x0 + 1, width * 82 / 100)
    let y1 = max(y0 + 1, height * 82 / 100)
    var points: [(Int, Int)] = []
    for row in 0..<4 {
      for column in 0..<6 {
        let x = x0 + (x1 - x0) * column / 5
        let y = y0 + (y1 - y0) * row / 3
        points.append((min(x, width - 1), min(y, height - 1)))
      }
    }
    return points
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
    let tileSize = CGSize(width: 280, height: 168)
    let labelHeight: CGFloat = 32
    let pad: CGFloat = 12
    let rows = Int(ceil(Double(cells.count) / Double(columns)))
    let canvas = CGSize(
      width: pad + CGFloat(columns) * (tileSize.width + pad),
      height: pad + CGFloat(rows) * (tileSize.height + labelHeight + pad)
    )
    let tiles: [(id: String, png: Data, luminance: String)] = cells.map { cell in
      let luminance = cell.measurement.interiorLuminance.map { String(format: "%.3f", $0) } ?? "n/a"
      return (cell.measurement.id, cell.png, luminance)
    }
    guard
      let sheet = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: Int(canvas.width),
        pixelsHigh: Int(canvas.height),
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
      ),
      let context = NSGraphicsContext(bitmapImageRep: sheet)
    else {
      throw CocoaError(.coderInvalidValue)
    }
    sheet.size = canvas
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    NSColor(srgbRed: 0.12, green: 0.12, blue: 0.13, alpha: 1).setFill()
    NSBezierPath(rect: CGRect(origin: .zero, size: canvas)).fill()
    let attributes: [NSAttributedString.Key: Any] = [
      .font: NSFont.monospacedSystemFont(ofSize: 9, weight: .regular),
      .foregroundColor: NSColor.white,
    ]
    for (index, tile) in tiles.enumerated() {
      let column = index % columns
      let row = index / columns
      let x = pad + CGFloat(column) * (tileSize.width + pad)
      let y = canvas.height - pad - CGFloat(row + 1) * (tileSize.height + labelHeight + pad)
      let tileRect = CGRect(
        x: x, y: y + labelHeight, width: tileSize.width, height: tileSize.height)
      if let cellImage = NSImage(data: tile.png) {
        cellImage.draw(
          in: tileRect, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: false,
          hints: nil)
      }
      let caption = "\(tile.id)\nL \(tile.luminance)"
      (caption as NSString).draw(
        in: CGRect(x: x, y: y, width: tileSize.width, height: labelHeight),
        withAttributes: attributes
      )
    }
    NSGraphicsContext.restoreGraphicsState()
    guard let png = sheet.representation(using: .png, properties: [:]) else {
      throw CocoaError(.coderInvalidValue)
    }
    return png
  }
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

private struct BitmapStats {
  var inkRatio: Double
  var luminance: Double?
  var maxCornerAlpha: CGFloat
}
