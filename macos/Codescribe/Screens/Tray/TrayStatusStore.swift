import SwiftUI

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
        if status.assistive {
            return CSColor.assistive
        }
        switch status.tone {
        case .neutral:
            return CSColor.oliveLight
        case .active:
            return CSColor.terracotta
        case .success:
            return CSColor.oliveLight
        case .warning:
            return CSColor.terracotta
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

    /// Colored status dot drawn beside the (always-static) menu bar icon.
    /// `nil` = no dot (idle / starting / success). The glyph never changes;
    /// only this dot's color signals the mode. Recording / processing /
    /// assistive hues mirror the caret hold-badge (`app/os/hold_badge.rs`:
    /// red / orange / purple) so the tray and the cursor speak one language.
    /// Warning states (error / thermal / hotkey conflict) fall back to a
    /// system red / yellow dot — the tooltip and menu status row carry the
    /// specifics, the dot only asks for attention.
    var menuBarDotColor: Color? {
        switch status.kind {
        case .starting, .idle, .success:
            return nil
        case .listening:
            return status.assistive
                ? Color(red: 0.6, green: 0.2, blue: 0.9)   // assistive — purple
                : Color(red: 1.0, green: 0.0, blue: 0.0)   // recording — red
        case .processing:
            return Color(red: 1.0, green: 0.5, blue: 0.0)  // processing — orange
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
        assistive: Bool = false,
        label: String = "Status: Idle"
    ) -> TrayStatusStore {
        TrayStatusStore(status: CsTrayStatusPayload(
            kind: kind,
            tone: tone,
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
