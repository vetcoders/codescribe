import Foundation

/// The bridge's `CsError` conforms to `LocalizedError` with
/// `errorDescription = String(reflecting: self)` (generated binding), so
/// `localizedDescription` renders as `Codescribe.CsError.Agent(msg: "…")` —
/// an enum dump in the chat bubble (Founder 2026-09-09 16:15). Surfaces that
/// show an error to a person read the payload through this seam instead.
extension CsError {
  /// The message the core composed, without the enum wrapper. A typed case
  /// carries a code and arguments; its sentence comes from the String Catalog.
  var userFacingMessage: String {
    switch self {
    case .Agent(let msg), .Config(let msg), .Recording(let msg), .License(let msg),
      .Quality(let msg), .Runtime(let msg):
      return msg
    case .CalibrationRefused(let refusal):
      return CalibrationRefusalCopy.sentence(refusal)
    }
  }
}

/// Interface-language sentence for a guided calibration that kept no profile.
/// Founder, 2026-10-15: the AirPods refusal reached Settings › Audio in English,
/// inside the enum dump `Recording(msg: …)`. Rust now sends the refusal as a
/// code with its arguments (docs/LOCALIZATION.md §5); the sentence lives here.
enum CalibrationRefusalCopy {
  static func sentence(_ refusal: CsCalibrationRefusal) -> String {
    sentence(
      refusal, locale: InterfaceLanguage.preferred(from: Bundle.main.preferredLocalizations).locale)
  }

  static func sentence(_ refusal: CsCalibrationRefusal, locale: Locale) -> String {
    switch refusal {
    case .digitalSilence(let device, _):
      return String(
        localized: LocalizedStringResource(
          "No audio signal arrives from “\(device)”. Check that this device sends audio to this Mac and select the microphone in Control Center.",
          locale: locale,
          comment: "Calibration refusal. The placeholder is an input device name"))
    case .noActiveSpeech:
      return String(
        localized: LocalizedStringResource(
          "No speech was heard. Speak normally for the whole measurement.", locale: locale,
          comment: "Calibration refusal when the measurement contained no speech"))
    case .insufficientSpeech(let measured, let required):
      let heard = seconds(measured, locale: locale)
      let needed = seconds(required, locale: locale)
      return String(
        localized: LocalizedStringResource(
          "Only \(heard) s of speech was heard; at least \(needed) s is needed. Speak normally for the whole measurement.",
          locale: locale,
          comment:
            "Calibration refusal. Both placeholders are pre-formatted numbers of seconds: measured, then required"
        ))
    case .insufficientSeparation:
      return String(
        localized: LocalizedStringResource(
          "Speech was barely louder than the background noise. Move closer to the microphone or to a quieter place.",
          locale: locale,
          comment: "Calibration refusal when speech is too close to the room noise"))
    case .unusable:
      return String(
        localized: LocalizedStringResource(
          "The measurement could not be used. Try again.", locale: locale,
          comment: "Calibration refusal for a measurement that can only be repeated"))
    }
  }

  private static func seconds(_ value: Float, locale: Locale) -> String {
    Double(value).formatted(.number.precision(.fractionLength(1)).locale(locale))
  }
}

extension Error {
  /// `CsError` payload when the error crossed the bridge; the system
  /// description otherwise.
  var userFacingMessage: String {
    (self as? CsError)?.userFacingMessage ?? localizedDescription
  }
}
