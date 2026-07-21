import Foundation

/// Cross-surface observation of the persisted thread set (NotificationCenter
/// enum-bus, same shape as `ConfigChangeBus`).
///
/// Rail live refresh (wave S, cut C): a turn finished by the assistive/overlay
/// path persists a thread on disk (thread JSON + index top) while an already
/// open Agent window still renders the list read at launch — the reply looked
/// "gone" (incident 2026-07-21). Publishers post on a turn-completed
/// persistence edge; `AgentChatStore` observes and re-reads disk truth through
/// its threads provider. Strictly event-driven — this bus must never grow a
/// polling producer.
///
/// Publishers cover the full `CsAgentDeliveryListener` lifecycle: `onDone`,
/// `onError` and `onCancelled` (errored/cancelled turns persist their user
/// half too). Known seam: turns that persist WITHOUT emitting any delivery
/// event have no Swift-side publisher — the Rust-side publish is a separate
/// wave. Until then, window activation (`NSWindow.didBecomeKeyNotification`,
/// observed in `AgentChatStore`) covers discoverability on the next activation.
enum ThreadsChangeBus {
    static let threadsDidChange = Notification.Name("codescribe.threads.threadsDidChange")

    static func postThreadsChanged() {
        NotificationCenter.default.post(name: threadsDidChange, object: nil)
    }
}
