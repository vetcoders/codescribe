import Foundation
import Observation

/// Presentation-only lease for dictation into an overlay conversation draft.
/// The shared controller owns capture and the reducer owns the delivered text.
@MainActor @Observable
final class OverlayDraftCapture {
  private(set) var conversationID: String?
  private(set) var handle: CsCaptureHandle?
  private(set) var transitioning = false
  private(set) var error: String?
  private(set) var errorConversationID: String?
  @ObservationIgnored var onStartResolved: (() -> Void)?
  @ObservationIgnored private var controller: (any ComposerCaptureControlling)?
  @ObservationIgnored private var destinations: [String: String] = [:]
  /// Sync members linked when each take was requested. Speech spoken before a
  /// participant joined is never theirs, however late the document arrives.
  @ObservationIgnored private var audiences: [String: Set<String>] = [:]
  @ObservationIgnored private var requestedAudience: Set<String> = []
  @ObservationIgnored private var earlyTerminals: Set<String> = []
  @ObservationIgnored private var errorCaptureID: String?

  init(controller: (any ComposerCaptureControlling)? = nil) {
    self.controller = controller
  }

  var hasRequest: Bool { conversationID != nil }
  var awaitingStart: Bool { hasRequest && handle == nil }

  func destination(for sessionID: String) -> String? { destinations[sessionID] }

  /// Conversations linked to the destination when this take was requested.
  func audience(for sessionID: String) -> Set<String> {
    audiences[sessionID] ?? destinations[sessionID].map { [$0] } ?? []
  }

  func ended(sessionID: String) {
    if awaitingStart { earlyTerminals.insert(sessionID) }
    guard handle?.captureId == sessionID else { return }
    conversationID = nil
    handle = nil
  }

  func completed(sessionID: String) {
    if errorCaptureID == sessionID {
      error = nil
      errorConversationID = nil
      errorCaptureID = nil
    }
    ended(sessionID: sessionID)
  }

  func toggle(
    conversationID destination: String, busy: Bool, audience: Set<String> = []
  ) async {
    guard !transitioning else { return }
    if hasRequest {
      guard conversationID == destination, let owned = handle else { return }
      transitioning = true
      defer { transitioning = false }
      do {
        let outcome = try await captureController.stopComposerTurnRecording(handle: owned)
        guard handle == owned else { return }
        if outcome == .admissionUnavailable {
          error = String(localized: "Recording is busy. Try stopping again.")
        } else if outcome == .foreignCapture || outcome == .noLiveCapture {
          // The conditional owner check proves this handle no longer owns
          // capture. Its destination remains retained for any late document.
          error = String(localized: "Waiting for this recording to finish.")
          ended(sessionID: owned.captureId)
        } else {
          error = nil
        }
      } catch {
        self.error = error.userFacingMessage
      }
      errorConversationID = error == nil ? nil : destination
      errorCaptureID = error == nil ? nil : owned.captureId
      return
    }
    guard !busy else {
      error = String(localized: "Another recording is still running.")
      errorConversationID = destination
      return
    }
    transitioning = true
    defer { transitioning = false }
    // The previous take's teardown can outlive its Stop by a breath, and a
    // click in that window used to vanish without a word (Maciej,
    // 2026-10-22: the bus field's microphone "races"). The shared
    // controller gets a short grace; when it stays busy the person is told,
    // instead of a silently dead button.
    var grace = 10
    while await captureController.isRecording() {
      grace -= 1
      guard grace >= 0 else {
        error = String(localized: "Another recording is still running.")
        errorConversationID = destination
        return
      }
      try? await Task.sleep(nanoseconds: 50_000_000)
    }
    // Install the destination before the asynchronous start: projections may
    // beat the start reply, but cannot nominate themselves as the owner.
    conversationID = destination
    requestedAudience = audience.union([destination])
    error = nil
    errorConversationID = nil
    errorCaptureID = nil
    do {
      let admitted = try await captureController.startComposerTurnRecording()
      handle = admitted
      destinations[admitted.captureId] = destination
      audiences[admitted.captureId] = requestedAudience
      if earlyTerminals.remove(admitted.captureId) != nil {
        conversationID = nil
        handle = nil
      }
      earlyTerminals.removeAll()
      onStartResolved?()
    } catch {
      self.error = error.userFacingMessage
      errorConversationID = destination
      conversationID = nil
      handle = nil
      earlyTerminals.removeAll()
      onStartResolved?()
    }
  }

  /// Error callbacks carry no identity. Only an already admitted projection
  /// can associate one with this take; Stop still names the controller handle.
  func failed(sessionID: String, message: String) async {
    guard handle?.captureId == sessionID, let destination = conversationID else { return }
    error = message
    errorConversationID = destination
    errorCaptureID = sessionID
    await toggle(conversationID: destination, busy: false)
    if handle?.captureId == sessionID, error == nil {
      error = message
      errorConversationID = destination
      errorCaptureID = sessionID
    }
  }

  private var captureController: any ComposerCaptureControlling {
    if let controller { return controller }
    let created = CodescribeHotkeys()
    controller = created
    return created
  }
}
