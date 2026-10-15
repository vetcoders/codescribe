import AppKit
import SwiftUI

/// Renders the status dot INTO the menu bar glyph's bottom-right corner
/// (screen #72) instead of as a separate character beside it. The result is a
/// flattened, non-template image: the glyph is tinted at draw time (label
/// color resolves against the menu bar appearance), a thin ring is punched
/// out so the dot stays legible over glyph strokes, and the dot fills the
/// corner.
enum TrayStatusDotIcon {
  /// Dot diameter as a fraction of the glyph's smaller dimension.
  static let dotFraction: CGFloat = 0.42
  /// Punched-out separation ring around the dot, in points.
  static let ringWidth: CGFloat = 1.5
  /// Colored rim drawn inside the dot's edge, in points.
  static let rimWidth: CGFloat = 1.25

  /// The dot's frame: flush with the glyph's bottom-right corner.
  static func dotRect(in bounds: CGRect) -> CGRect {
    let diameter = min(bounds.width, bounds.height) * dotFraction
    return CGRect(
      x: bounds.maxX - diameter,
      y: bounds.minY,
      width: diameter,
      height: diameter
    )
  }

  static func composite(
    base: NSImage,
    dot: NSColor,
    rim: NSColor? = nil,
    glyphTint: NSColor? = nil
  ) -> NSImage {
    let image = NSImage(size: base.size, flipped: false) { rect in
      base.draw(in: rect)
      // Tint like a template image would; the draw-time handler makes
      // label color follow the destination (menu bar) appearance.
      (glyphTint ?? NSColor.labelColor).set()
      rect.fill(using: .sourceAtop)

      let dotRect = dotRect(in: rect)
      if let cg = NSGraphicsContext.current?.cgContext {
        cg.saveGState()
        cg.setBlendMode(.destinationOut)
        cg.fillEllipse(in: dotRect.insetBy(dx: -ringWidth, dy: -ringWidth))
        cg.restoreGState()
      }
      dot.setFill()
      NSBezierPath(ovalIn: dotRect).fill()
      if let rim {
        rim.setStroke()
        let edge = NSBezierPath(ovalIn: dotRect.insetBy(dx: rimWidth / 2, dy: rimWidth / 2))
        edge.lineWidth = rimWidth
        edge.stroke()
      }
      return true
    }
    image.isTemplate = false
    return image
  }
}

/// Menu-bar image for one tray status. Idle and settled states stay the brand
/// mark and its indicator as a real template image. Live activity colors the
/// indicator while the mark continues to follow the menu bar appearance.
enum MenuBarStatusImage {
  static func make(base: NSImage, dot: NSColor?, rim: NSColor? = nil) -> NSImage {
    guard let dot else {
      let image = TrayStatusDotIcon.composite(base: base, dot: .black, glyphTint: .black)
      image.isTemplate = true
      return image
    }
    return TrayStatusDotIcon.composite(base: base, dot: dot, rim: rim, glyphTint: .labelColor)
  }
}

@MainActor
final class TrayStatusStore: ObservableObject {
  @Published private(set) var status: CsTrayStatusPayload

  var onChange: ((CsTrayStatusPayload) -> Void)?

  private let bridge: CodescribeTrayStatus?
  private var listener: TrayStatusListener?
  private var lastAppliedGeneration: UInt64

  init() {
    let bridge = CodescribeTrayStatus()
    let initialStatus = bridge.currentStatus()
    self.bridge = bridge
    self.status = initialStatus
    self.lastAppliedGeneration = initialStatus.generation

    let listener = TrayStatusListener { [weak self] status in
      self?.apply(status)
    }
    self.listener = listener
    bridge.setListener(listener: listener)
  }

  private init(status: CsTrayStatusPayload) {
    self.bridge = nil
    self.status = status
    self.lastAppliedGeneration = status.generation
  }

  private func apply(_ status: CsTrayStatusPayload) {
    guard status.generation > lastAppliedGeneration else { return }
    lastAppliedGeneration = status.generation
    self.status = status
    onChange?(status)
  }

  func invalidate() {
    listener?.invalidate()
    listener = nil
  }

  /// Short status phrase for the header pill.
  ///
  /// Derived from the payload's machine-readable `kind` plus the `assistive`
  /// lane flag. Rust sends no wording: every `(kind, assistive)` pair maps to
  /// exactly one phrase here.
  var compactLabel: String {
    switch status.kind {
    case .starting:
      return String(localized: "Starting...", comment: "Tray status: the runtime is still booting")
    case .idle:
      return String(localized: "Idle", comment: "Tray status: ready, not recording")
    case .listening:
      return status.assistive
        ? String(
          localized: "Agent listening...",
          comment: "Tray status: the assistive (agent) lane is recording")
        : String(localized: "Recording...", comment: "Tray status: dictation is recording")
    case .processing:
      return String(localized: "Processing...", comment: "Tray status: transcribing")
    case .success:
      return String(localized: "Done!", comment: "Tray status: the last session finished")
    case .error:
      return String(localized: "Error", comment: "Tray status: the backend is unavailable")
    case .thermal:
      return String(
        localized: "Thermal throttling",
        comment: "Tray status: the host is too hot to run speech-to-text at full speed")
    case .hotkeyConflict:
      return String(
        localized: "Hotkey conflict",
        comment: "Tray status: a recording shortcut was detected but blocked")
    }
  }

  /// Full status line for the detail row under the pill.
  var detailLabel: String {
    String(
      localized: "Status: \(compactLabel)",
      comment: "Tray detail row; %@ is a status phrase such as Error or Hotkey conflict"
    )
  }

  /// Menu bar tooltip.
  var tooltip: String {
    String(
      localized: "Codescribe - \(tooltipPhrase)",
      comment: "Menu bar tooltip; Codescribe is the product name, %@ is a status phrase"
    )
  }

  /// The tooltip reads differently from the pill for two kinds: it says what the
  /// app can do ("Ready") and what actually broke ("Backend unavailable!").
  private var tooltipPhrase: String {
    switch status.kind {
    case .idle:
      return String(localized: "Ready", comment: "Menu bar tooltip: ready to record")
    case .error:
      return String(
        localized: "Backend unavailable!",
        comment: "Menu bar tooltip: the Codescribe core is not running")
    case .starting, .listening, .processing, .success, .thermal, .hotkeyConflict:
      return compactLabel
    }
  }

  var color: Color {
    if status.indicatorMode == .assistive {
      return CSColor.modeAgent
    }
    switch status.tone {
    case .neutral:
      return CSColor.oliveLight
    case .active:
      return status.indicatorMode == .processing
        ? CSColor.modeProcessing
        : CSColor.modeRecording
    case .success:
      return CSColor.oliveLight
    case .warning:
      return CSColor.modeProcessing
    case .critical:
      return CSColor.terracottaDeep
    }
  }

  var icon: CSIcon {
    switch status.kind {
    case .starting:
      return .more
    case .idle:
      return .success
    case .listening, .processing:
      return .mic
    case .success:
      return .success
    case .error:
      return .error
    case .thermal:
      return .warning
    case .hotkeyConflict:
      return .shortcuts
    }
  }

  var shouldRipple: Bool {
    switch status.kind {
    case .starting, .listening, .processing:
      return true
    case .idle, .success, .error, .thermal, .hotkeyConflict:
      return false
    }
  }

  /// Compact extra row under the wordmark + pill. The pill already carries
  /// idle / success / listening / processing (and starting); only
  /// warning / critical kinds keep a detail row.
  var showsDetailStatusRow: Bool {
    switch status.kind {
    case .error, .thermal, .hotkeyConflict:
      return true
    case .starting, .idle, .listening, .processing, .success:
      return false
    }
  }

  /// Nil means a monochrome template indicator, never an absent indicator.
  /// Recording keeps its red and processing stays orange. The Agent dot follows
  /// the voice like the pointer dot: purple while speaking, white between
  /// phrases, amber while words are owed (Maciej, 2026-10-14). Other status
  /// details live in the tooltip and menu row.
  var menuBarDotColor: Color? {
    switch status.kind {
    case .starting, .idle, .success, .error, .thermal, .hotkeyConflict:
      return nil
    case .listening:
      guard status.indicatorMode == .assistive else { return CSColor.indicatorRecording }
      switch status.voice {
      case .lagging: return CSColor.indicatorLagging
      case .quiet: return CSColor.indicatorAgentQuiet
      case .speaking, .unobserved: return CSColor.modeAgent
      }
    case .processing:
      return CSColor.modeProcessing
    }
  }

  /// A white dot vanishes on a light menu bar; the quiet Agent dot keeps a purple rim.
  var menuBarDotRim: Color? {
    isAgentListening && status.voice == .quiet ? CSColor.modeAgent : nil
  }

  /// Owed Agent words pulse the glyph's dot, as they pulse the pointer dot.
  var menuBarDotPulses: Bool {
    isAgentListening && status.voice == .lagging
  }

  private var isAgentListening: Bool {
    status.kind == .listening && status.indicatorMode == .assistive
  }

  #if DEBUG
    func testingApply(
      kind: CsTrayStatusKind,
      tone: CsTrayStatusTone,
      indicatorMode: CsIndicatorMode = .hold,
      assistive: Bool = false,
      voice: CsCursorVoice = .unobserved,
      generation: UInt64
    ) {
      apply(
        CsTrayStatusPayload(
          kind: kind,
          tone: tone,
          indicatorMode: indicatorMode,
          assistive: assistive,
          voice: voice,
          generation: generation
        ))
    }

    static func preview(
      kind: CsTrayStatusKind = .idle,
      tone: CsTrayStatusTone = .neutral,
      indicatorMode: CsIndicatorMode = .hold,
      assistive: Bool = false,
      voice: CsCursorVoice = .unobserved
    ) -> TrayStatusStore {
      TrayStatusStore(
        status: CsTrayStatusPayload(
          kind: kind,
          tone: tone,
          indicatorMode: indicatorMode,
          assistive: assistive,
          voice: voice,
          generation: 0
        ))
    }
  #endif
}

final class TrayStatusListener: CsTrayStatusListener, Sendable {
  private let continuation: AsyncStream<CsTrayStatusPayload>.Continuation
  private let consumer: Task<Void, Never>

  @MainActor
  init(onStatus: @escaping @MainActor @Sendable (CsTrayStatusPayload) -> Void) {
    let channel = AsyncStream<CsTrayStatusPayload>.makeStream()
    continuation = channel.continuation
    consumer = Task { @MainActor in
      for await status in channel.stream {
        onStatus(status)
      }
    }
  }

  func onTrayStatus(status: CsTrayStatusPayload) {
    continuation.yield(status)
  }

  func invalidate() {
    continuation.finish()
    consumer.cancel()
  }
}
