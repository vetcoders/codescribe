import Foundation

// Single source of truth for the cloud-transcription privacy copy (C2).
// The Rust core enforces the contract these strings describe
// (core/config/cloud_asr.rs + core/asr_session/consent.rs): cloud requires an
// explicit audio-egress consent record, every refusal resolves to Apple +
// dictionary, and no consent fallback may load local weights. Tests pin the
// copy to that contract so UI text cannot drift from the enforced behavior.
enum CloudPrivacyCopy {
  /// Section title in the Dictation settings panel.
  static let title = "Cloud & privacy"

  /// Where audio lives by default, and the one condition under which it moves.
  static let intro =
    "Codescribe transcribes on this Mac. Audio leaves this Mac only in Cloud mode, "
    + "and only after you explicitly allow it."

  /// The safe floor every failure resolves to.
  static let modeAppleOnly =
    "Apple only — on-device Apple Speech plus your dictionary. "
    + "The default, and the mode every failure falls back to."

  /// The consent-gated cloud lane. Provider-neutral by contract: the app
  /// talks to the Libraxis gateway and never stores a vendor key.
  static let modeCloud =
    "Cloud — live refinement through the Libraxis gateway. "
    + "Requires your explicit consent to send audio off this Mac; "
    + "the app stores no vendor keys."

  /// The power-user local lane: separate killable process, opt-in weights.
  static let modeLocalPower =
    "Local power — optional on-device model weights in a separate helper process, "
    + "downloaded only when you choose them."

  /// What happens when consent is missing or declined.
  static let consentFallback =
    "Without your consent, Cloud never arms: dictation continues with Apple "
    + "plus your dictionary, and no local model is loaded in its place."

  /// The telemetry bound: counters and identifiers, never content.
  static let telemetry =
    "Cloud diagnostics are counters only — latency, bytes, error and model identifiers. "
    + "Never audio, never transcript text."

  /// Render order for the settings section.
  static let lines: [String] = [
    intro, modeAppleOnly, modeCloud, modeLocalPower, consentFallback, telemetry,
  ]
}
