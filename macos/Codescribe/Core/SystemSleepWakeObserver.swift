import AppKit

/// Bridges real NSWorkspace sleep/wake notifications into one coalesced,
/// payload-free recording lifecycle callback.
///
/// AppKit can emit notifications from inside window/system operations. Both
/// observers are therefore bound to the one shared workspace object and their
/// callouts only schedule one next-main-queue callback. No bridge, disk,
/// formatting, layout, model, network, or blocking work occurs in the callout.
@MainActor
final class SystemSleepWakeObserver {
    private let center: NotificationCenter
    private weak var workspace: NSWorkspace?
    private let onBoundary: () -> Void
    private var tokens: [NSObjectProtocol] = []
    private var boundaryScheduled = false

    init(
        center: NotificationCenter = NSWorkspace.shared.notificationCenter,
        workspace: NSWorkspace = .shared,
        onBoundary: @escaping () -> Void
    ) {
        self.center = center
        self.workspace = workspace
        self.onBoundary = onBoundary
    }

    func start() {
        guard tokens.isEmpty, let workspace else { return }
        let handler: (Notification) -> Void = { [weak self] _ in
            MainActor.assumeIsolated { self?.scheduleBoundary() }
        }
        tokens = [
            center.addObserver(
                forName: NSWorkspace.willSleepNotification,
                object: workspace,
                queue: .main,
                using: handler
            ),
            center.addObserver(
                forName: NSWorkspace.didWakeNotification,
                object: workspace,
                queue: .main,
                using: handler
            ),
        ]
    }

    func invalidate() {
        tokens.forEach(center.removeObserver)
        tokens.removeAll()
        boundaryScheduled = false
    }

    private func scheduleBoundary() {
        guard !boundaryScheduled else { return }
        boundaryScheduled = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.boundaryScheduled = false
            self.onBoundary()
        }
    }
}
