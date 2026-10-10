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

  /// One rendered subsection of the privacy details: a heading and the
  /// sentences under it.
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
    localized: "Recognition mode", comment: "Privacy row: the selected recognition mode")

  /// Row label for the stored consent record, deliberately a separate row:
  /// a granted record is not the same thing as audio leaving right now.
  static let savedConsentLabel = String(
    localized: "Cloud consent", comment: "Privacy row: the stored audio-egress consent record")

  static let consentGranted = String(
    localized: "Granted", comment: "Stored audio-egress consent state")
  static let consentNotGranted = String(
    localized: "Not granted", comment: "Stored audio-egress consent state")

  /// The point of splitting the two rows, said out loud. The condition that
  /// arms audio egress reads on the Audio row below.
  static let consentIsNotLiveEgress = String(
    localized: "A saved consent does not mean audio is being sent now."
  )

  // MARK: - What can leave this computer

  static let egressHeading = String(
    localized: "What can leave this computer?", comment: "Privacy section: outbound data")

  static let egressAudioTitle = String(
    localized: "Audio", comment: "Privacy egress row: recorded sound")

  /// Both audio egresses in one scannable condition: the live Cloud mode
  /// session and the explicit re-transcription, which does not run through
  /// the mode picker.
  static let egressAudioDetail = String(
    localized:
      "In Cloud mode, and when you start a cloud re-transcription of a recording yourself."
  )

  static let egressTextTitle = String(
    localized: "Text", comment: "Privacy egress row: transcript and prompt text")

  static let egressTextDetail = String(
    localized: "During AI requests to the providers you configured."
  )

  // MARK: - Privacy details (collapsed by default; nothing is removed)

  static let detailsHeading = String(
    localized: "Privacy details", comment: "Privacy section: diagnostics, keys, refusals")

  /// What the collapsed disclosure holds, so nobody has to open it blind.
  static let detailsCaption = String(
    localized: "Diagnostics, API keys, safeguards, and how the local modes behave."
  )

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

  /// Expanded details, split into short headed subsections instead of one
  /// continuous wall of prose. Every safeguard stays; only the default
  /// visibility changed.
  static let detailBlocks: [Block] = [
    Block(
      id: "diagnostics",
      heading: String(
        localized: "Diagnostics", comment: "Privacy details subsection: cloud session telemetry"),
      lines: [diagnostics]),
    Block(
      id: "keys",
      heading: String(
        localized: "API keys", comment: "Privacy details subsection: key storage and the gateway"),
      lines: [apiKeys]),
    Block(
      id: "consent",
      heading: String(
        localized: "Consent", comment: "Privacy details subsection: refusal without a grant"),
      lines: [withoutConsent]),
    Block(
      id: "local",
      heading: String(
        localized: "Local modes", comment: "Privacy details subsection: local lanes and installs"),
      lines: [localPowerInstall]),
  ]

  /// One discreet action instead of a prose pointer. It deep-links to
  /// Providers › Cloud transcription through `SettingsDeepLink`, which the
  /// open Settings window consumes — the link is live, not decorative.
  static let configureCloudServices = String(
    localized: "Configure cloud services",
    comment: "Privacy action: opens Providers > Cloud transcription")
}
