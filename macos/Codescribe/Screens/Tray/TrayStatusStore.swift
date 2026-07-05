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

    var menuBarTint: Color? {
        switch status.kind {
        case .listening, .processing:
            return color
        case .error, .thermal, .hotkeyConflict:
            return color
        case .starting, .idle, .success:
            return nil
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

    var menuBarSymbolNames: [String] {
        switch status.kind {
        case .starting:
            return ["ellipsis.circle"]
        case .idle:
            return []
        case .listening:
            return ["waveform"]
        case .processing:
            return ["waveform.circle", "waveform"]
        case .success:
            return ["checkmark.circle"]
        case .error:
            return ["exclamationmark.triangle.fill", "exclamationmark.triangle"]
        case .thermal:
            return ["thermometer.high", "thermometer.medium"]
        case .hotkeyConflict:
            return ["keyboard.badge.exclamationmark", "keyboard"]
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
