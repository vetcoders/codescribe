import Foundation
import OSLog

/// Diagnostic breadcrumbs for Agent voice capture. Audio, STT, corrections,
/// transcript publication, and delivery are all owned by RecordingController.
private let dictationLog = Logger(
  subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
  category: "composer-dictation"
)

/// The exact slice of the shared controller the composer gesture speaks to.
///
/// A protocol rather than the concrete bridge type so the gesture policy —
/// which press starts, which stops, and which must refuse — is testable against
/// the production adapter instead of a re-implemented copy of its rules.
///
/// Shape mirrors the generated `CodescribeHotkeysProtocol` (`AnyObject`,
/// `Sendable`, non-isolated) so the bridge object conforms without an adapter
/// and without pulling the FFI surface onto the main actor.
protocol ComposerCaptureControlling: AnyObject, Sendable {
  /// Is the one shared controller capturing right now (any surface)?
  func isRecording() async -> Bool
  /// Start one explicit composer take: one gesture, one turn.
  ///
  /// Returns the controller-admitted identity of the take that was opened. A
  /// start that admits no capture returns no handle to stop with.
  func startComposerTurnRecording() async throws -> CsCaptureHandle
  /// Stop exactly the named capture, or refuse.
  ///
  /// The unconditional `stopRecording` is deliberately absent from this seam:
  /// between the press and this call the microphone can have changed hands, and
  /// a UI gesture may not end a take it did not open.
  func stopComposerTurnRecording(handle: CsCaptureHandle) async throws -> CsConditionalStop
}

/// Thin UI gesture adapter over the shared controller. The composer never owns
/// a recorder or transcript reducer; it requests one explicit Agent take.
@MainActor
final class RealComposerDictation: ComposerDictating {
  private let hotkeys: ComposerCaptureControlling
  private weak var store: AgentChatStore?
  private var transitioning = false
  private(set) var transitionTask: Task<Void, Never>?

  init(store: AgentChatStore, hotkeys: ComposerCaptureControlling = CodescribeHotkeys()) {
    self.store = store
    self.hotkeys = hotkeys
  }

  func toggle() {
    guard let store, !transitioning else { return }
    // Pending settlement is still ownership. Neither another press nor a false
    // recording query can acknowledge delivery or replace its destination.
    guard !store.hasComposerCaptureRequest || store.ownsLiveDictation || store.composerStopRetryAvailable
    else { return }
    let ownedHandle = store.composerCaptureHandle
    let request = store.currentComposerCaptureRequestID
    let destination = store.selectedThreadID
    transitioning = true
    store.prepareDictationGesture()
    transitionTask = Task { @MainActor in
      defer { transitioning = false }
      if let request {
        guard store.isCurrentComposerCaptureRequest(request), let handle = ownedHandle else { return }
        store.awaitComposerCaptureTerminal()
        do {
          let outcome = try await hotkeys.stopComposerTurnRecording(handle: handle)
          store.applyComposerStopOutcome(outcome, requestID: request, handle: handle)
        } catch {
          store.reportComposerStopFailure(
            "Couldn't change recording: \(error.userFacingMessage)", requestID: request, handle: handle)
        }
        // The terminal projection consumer must deliver before releasing the
        // latch. No post-stop isRecording poll can establish that ordering.
        return
      }
      let live = await hotkeys.isRecording()
      guard !store.hasComposerCaptureRequest else { return }
      if live {
        store.releaseUnownedDictationGesture()
        return
      }
      let requestID = store.beginComposerCaptureRequest(threadID: destination)
      do {
        let admitted = try await hotkeys.startComposerTurnRecording()
        guard store.isCurrentComposerCaptureRequest(requestID) else { return }
        store.completeComposerCaptureStart(requestID, live: true, handle: admitted)
        dictationLog.info("Agent composer take start requested on shared controller")
      } catch {
        guard store.isCurrentComposerCaptureRequest(requestID) else { return }
        dictationLog.error(
          "Agent voice capture gesture failed: \(error.localizedDescription, privacy: .public)")
        store.reportDictationFailure("Couldn't change recording: \(error.userFacingMessage)")
      }
    }
  }
}

/// The production capture surface. Declared here rather than on the generated
/// binding so no generated file carries hand-written conformance.
extension CodescribeHotkeys: ComposerCaptureControlling {}
