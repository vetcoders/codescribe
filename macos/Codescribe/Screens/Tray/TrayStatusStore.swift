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
            return true
        }
        image.isTemplate = false
        return image
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

    var compactLabel: String {
        status.menuLabel.replacingOccurrences(of: "Status: ", with: "")
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

    /// Colored status dot composited into the (always-static) menu bar glyph's
    /// bottom-right corner. The glyph never changes; only this dot signals the
    /// mode, 1:1 with the Rust tray-status feed: green = ready (idle/success,
    /// the locked palette's ready tone), red = recording, orange = processing,
    /// purple = assistive — recording/processing/assistive hues mirror the
    /// caret hold-badge (`app/os/hold_badge.rs`) so the tray and the cursor
    /// speak one language. Processing is always orange, including agent
    /// sessions. `nil` = no dot
    /// (starting — the app is not ready yet). Warning states (error / thermal /
    /// hotkey conflict) fall back to a system red / yellow attention dot — the
    /// tooltip and menu status row carry the specifics.
    var menuBarDotColor: Color? {
        switch status.kind {
        case .starting:
            return nil
        case .idle, .success:
            return CSColor.modeReady
        case .listening:
            return status.indicatorMode == .assistive
                ? CSColor.modeAgent
                : CSColor.indicatorRecording
        case .processing:
            return CSColor.modeProcessing
        case .error:
            return .red
        case .thermal, .hotkeyConflict:
            return .yellow
        }
    }

    #if DEBUG
    static func preview(
        kind: CsTrayStatusKind = .idle,
        tone: CsTrayStatusTone = .neutral,
        indicatorMode: CsIndicatorMode = .hold,
        assistive: Bool = false,
        label: String = "Status: Idle"
    ) -> TrayStatusStore {
        TrayStatusStore(status: CsTrayStatusPayload(
            kind: kind,
            tone: tone,
            indicatorMode: indicatorMode,
            assistive: assistive,
            tooltip: "Codescribe - \(label.replacingOccurrences(of: "Status: ", with: ""))",
            menuLabel: label,
            generation: 0
        ))
    }
    #endif
}

final class TrayStatusListener: CsTrayStatusListener, @unchecked Sendable {
    private let onStatus: @MainActor (CsTrayStatusPayload) -> Void

    init(onStatus: @escaping @MainActor (CsTrayStatusPayload) -> Void) {
        self.onStatus = onStatus
    }

    func onTrayStatus(status: CsTrayStatusPayload) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.onStatus(status)
            }
        }
    }
}
