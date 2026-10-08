import Foundation

// Single source of truth for the Dictation › Cloud & privacy copy. Every
// sentence here states something the Rust core enforces:
//
// - a live cloud session needs an explicit audio-egress consent record, and a
//   refusal resolves to Apple on-device plus the dictionary without loading
//   local weights (`core/asr_session/consent.rs`);
// - the cloud session's diagnostics are content-free identifiers and counters
//   (`CloudSessionTelemetry` in `core/asr_session/cloud.rs`, `UsageEvent` and
//   `ErrorEvent` in `core/asr_session/events.rs`);
// - the live gateway mint carries no vendor keys and keeps the provider behind
//   the gateway (`core/config/cloud_asr.rs`), while the keys a user does
//   configure live in the macOS Keychain (`core/config/keychain.rs`);
// - an explicit cloud re-transcription of a finished recording is a second,
//   separate egress that does not run through the mode picker
//   (`bridge/src/recording.rs`, `core/asr_session/mod.rs`).
//
// Tests pin the copy to that contract so UI text cannot drift from the
// enforced behavior.
enum CloudPrivacyCopy {
  /// Tab title, reused as the tab headline.
  static let title = String(localized: "Cloud & privacy")

  /// One rendered section: a heading and the sentences under it.
  struct Block: Identifiable {
    let id: String
    let heading: String
    let lines: [String]
  }

  // MARK: - Cloud status

  static let statusHeading = String(
    localized: "Cloud status", comment: "Privacy section: current cloud lane and stored consent")

  /// Row label for the selected recognition mode. The value is the mode label
  /// the Engine picker shows, read from the view model.
  static let currentModeLabel = String(
    localized: "Current mode", comment: "Privacy row: the selected recognition mode")

  /// Row label for the stored consent record, deliberately a separate row:
  /// a granted record is not the same thing as audio leaving right now.
  static let savedConsentLabel = String(
    localized: "Saved consent", comment: "Privacy row: the stored audio-egress consent record")

  static let consentGranted = String(
    localized: "Granted", comment: "Stored audio-egress consent state")
  static let consentNotGranted = String(
    localized: "Not granted", comment: "Stored audio-egress consent state")

  /// The point of splitting the two rows, said out loud.
  static let consentIsNotLiveEgress = String(
    localized:
      "A saved consent does not mean audio is being sent now. Audio is sent only while Cloud mode is selected, or during a cloud re-transcription you start yourself."
  )

  // MARK: - What can leave this Mac

  static let egressHeading = String(
    localized: "What can leave this Mac", comment: "Privacy section: outbound data")

  static let egressAudio = String(
    localized:
      "Audio — during cloud speech recognition in Cloud mode, and during an explicit cloud re-transcription of a recording. The re-transcription is a separate action you start, available whenever a cloud transcription lane is configured."
  )

  static let egressText = String(
    localized: "Text — during AI requests to the providers you configured."
  )

  // MARK: - Privacy details

  static let detailsHeading = String(
    localized: "Privacy details", comment: "Privacy section: diagnostics, keys, refusals")

  /// Bounded to the fields the cloud session actually records.
  static let diagnostics = String(
    localized:
      "Cloud session diagnostics stay content-free: session and utterance identifiers, counts of audio frames and events, seconds of audio, and typed error codes. Never audio, never transcript text."
  )

  /// The no-vendor-key promise is restricted to the gateway architecture; the
  /// keys a user configures themselves are Keychain items.
  static let apiKeys = String(
    localized:
      "API keys you configure are stored in the macOS Keychain. The live cloud lane needs no vendor key of yours: the Libraxis gateway mints a short-lived session bearer outside this Mac and keeps the provider behind it."
  )

  /// What a missing consent record resolves to, and what it does not.
  static let withoutConsent = String(
    localized:
      "Without your consent Cloud never arms. Codescribe refuses the unauthorized cloud use and keeps dictating with Apple on-device plus your dictionary; no local model is loaded in its place."
  )

  /// Choosing a mode is not an install.
  static let localPowerInstall = String(
    localized:
      "Choosing Local power does not download anything. Installing the on-device model is a separate action on the Whisper tab."
  )

  static let providersPointer = String(
    localized: "Endpoints and keys live on Providers › Cloud transcription."
  )

  /// Render order for the two prose sections. The status rows are live values
  /// and stay in the view.
  static let blocks: [Block] = [
    Block(id: "egress", heading: egressHeading, lines: [egressAudio, egressText]),
    Block(
      id: "details", heading: detailsHeading,
      lines: [diagnostics, apiKeys, withoutConsent, localPowerInstall, providersPointer]),
  ]
}
