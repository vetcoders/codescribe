import AppKit
import SwiftUI

/// A Codescribe message field with its own microphone.
@MainActor
protocol ComposerMicrophoneRedirecting: AnyObject {
  /// Point the person at this field's microphone after a refused long Fn.
  func showMicrophoneRedirect()
}

/// The one record of which Codescribe message field owns the focused caret.
///
/// The Agent composer and the Intercom bus composer each record through their
/// own microphone. While one of them holds the caret in the key window, the
/// recording controller refuses a long Fn dictation hold (it would open the
/// dictation overlay over the field and deliver elsewhere) and asks this
/// registry to point that field at its microphone instead.
@MainActor
final class ComposerCaretOwnership {
  static let shared = ComposerCaretOwnership()

  /// The inline hint the owning field shows after a refused long Fn.
  static var microphoneHint: String {
    String(
      localized: "Use the microphone button in the message field",
      comment: "Shown in a message field when Fn is held while typing in it")
  }

  /// How long the hint and the highlighted microphone stay visible.
  static let hintSeconds: Double = 3

  /// Test seam over the Rust flag read by the hold-start gate.
  var publish: (Bool) -> Void = { setComposerCaretOwned(owned: $0) }

  private weak var owner: (any ComposerMicrophoneRedirecting)?
  private var ownerID: ObjectIdentifier?
  private var published: Bool?

  var ownsCaret: Bool { ownerID != nil && owner != nil }

  /// Every field reports its own focus. Only the current owner can release.
  func report(_ field: any ComposerMicrophoneRedirecting, focused: Bool) {
    let id = ObjectIdentifier(field)
    if focused {
      owner = field
      ownerID = id
    } else if ownerID == id {
      owner = nil
      ownerID = nil
    }
    publishIfChanged()
  }

  /// A field that leaves memory cannot keep the caret.
  func release(_ id: ObjectIdentifier) {
    guard ownerID == id else { return }
    owner = nil
    ownerID = nil
    publishIfChanged()
  }

  /// The controller refused a long Fn: the owning field shows its hint.
  func redirectToComposerMicrophone() {
    guard let owner, ownerID != nil else {
      // The caret moved between the refusal and this callback. Nothing owns
      // it now, so the next hold must not be refused on stale truth.
      ownerID = nil
      publishIfChanged()
      return
    }
    owner.showMicrophoneRedirect()
  }

  private func publishIfChanged() {
    let owned = ownsCaret
    guard published != owned else { return }
    published = owned
    publish(owned)
  }
}

/// One field's transient hint after a refused long Fn: a line of copy and a
/// lit microphone, announced to VoiceOver, gone after a few seconds.
@MainActor @Observable
final class ComposerMicrophoneHintState {
  private(set) var visible = false
  @ObservationIgnored private var dismissal: Task<Void, Never>?

  func show(reduceMotion: Bool) {
    dismissal?.cancel()
    let animation: Animation? = reduceMotion ? nil : .easeOut(duration: CSMotion.Tempo.hover)
    withAnimation(animation) { visible = true }
    NSAccessibility.post(
      element: NSApp as Any, notification: .announcementRequested,
      userInfo: [.announcement: ComposerCaretOwnership.microphoneHint])
    dismissal = Task { @MainActor [weak self] in
      try? await Task.sleep(for: .seconds(ComposerCaretOwnership.hintSeconds))
      guard !Task.isCancelled else { return }
      withAnimation(animation) { self?.visible = false }
    }
  }
}
