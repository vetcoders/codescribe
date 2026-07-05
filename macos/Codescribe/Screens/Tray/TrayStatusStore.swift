import SwiftUI

@MainActor
final class TrayStatusStore: ObservableObject {
    @Published private(set) var status: CsTrayStatusPayload

    var onChange: ((CsTrayStatusPayload) -> Void)?

    private let bridge: CodescribeTrayStatus?
    private var listener: TrayStatusListener?

    init() {
        let bridge = CodescribeTrayStatus()
        self.bridge = bridge
        self.status = bridge.currentStatus()

        let listener = TrayStatusListener { [weak self] status in
            self?.apply(status)
        }
        self.listener = listener
        bridge.setListener(listener: listener)
    }

    private init(status: CsTrayStatusPayload) {
        self.bridge = nil
        self.status = status
    }

    private func apply(_ status: CsTrayStatusPayload) {
        self.status = status
        onChange?(status)
    }

    var compactLabel: String {
        status.menuLabel.replacingOccurrences(of: "Status: ", with: "")
    }

    var color: Color {
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
        label: String = "Status: Idle"
    ) -> TrayStatusStore {
        TrayStatusStore(status: CsTrayStatusPayload(
            kind: kind,
            tone: tone,
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
