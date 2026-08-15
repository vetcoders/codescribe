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
    store.setDictationPhase(.preparing)
    Task { @MainActor in
      defer { transitioning = false }
      do {
        if store.dictationBlocked {
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
      }
    }
  }
}
