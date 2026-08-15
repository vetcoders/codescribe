import Foundation
import OSLog

/// Diagnostic breadcrumbs for Agent voice capture. Audio, STT, corrections,
/// transcript publication, and delivery are all owned by RecordingController.
private let dictationLog = Logger(
  subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
  category: "composer-dictation"
)

/// Thin UI gesture adapter over the shared controller. The composer never owns
/// a recorder or transcript reducer; it merely requests the Agent toggle route.
@MainActor
final class RealComposerDictation: ComposerDictating {
  private let hotkeys = CodescribeHotkeys()
  private weak var store: AgentChatStore?
  private var transitioning = false

  init(store: AgentChatStore) {
    self.store = store
  }

  func toggle() {
    guard let store, !transitioning else { return }
    transitioning = true
    // Optimistic beat at click latency; both start and stop are non-actionable
    // while in flight, so this also swallows the double-tap.
    store.setDictationPhase(.preparing)
    Task { @MainActor in
      defer { transitioning = false }
      // Direction comes from the controller, not from the cached `dictationBlocked`
      // flag. A flag left stale by a lifecycle event that never arrived used to
      // route every press into a stop that no-ops against an idle controller —
      // a mic that looks busy forever with no way back short of a relaunch.
      let live = await hotkeys.isRecording()
      store.dictationBlocked = live
      do {
        if live {
          try await hotkeys.stopRecording()
          dictationLog.info("Agent voice capture stop requested on shared controller")
        } else {
          try await hotkeys.startAssistiveRecording()
          dictationLog.info("Agent voice capture start requested on shared controller")
        }
      } catch {
        dictationLog.error(
          "Agent voice capture gesture failed: \(error.localizedDescription, privacy: .public)")
        store.reportDictationFailure("Couldn't change recording: \(error.localizedDescription)")
        return
      }
      // Terminal reconcile against the controller. The lifecycle hooks own the
      // happy path; this only catches a gesture that left the controller idle
      // without ever broadcasting a terminal event.
      if await hotkeys.isRecording() == false {
        store.endDictationSession()
      }
    }
  }
}
