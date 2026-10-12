import Foundation

/// Interface-language headline for an acoustic admission refusal.
///
/// Rust keeps the English headline and the diagnostic message on the
/// presentation event. A known `code` is painted through the String Catalog.
/// An unknown code keeps the fallback headline.
enum AdmissionRefusalCopy {
  static func headline(code: String, fallback: String) -> String {
    headline(
      code: code,
      fallback: fallback,
      locale: InterfaceLanguage.preferred(from: Bundle.main.preferredLocalizations).locale)
  }

  static func headline(code: String, fallback: String, locale: Locale) -> String {
    switch code {
    case "admission_calibration_missing", "admission_calibration_no_profile":
      return String(
        localized: LocalizedStringResource(
          "Microphone calibration required",
          locale: locale,
          comment: "Admission refusal when microphone calibration is missing"))
    case "admission_calibration_refused", "admission_calibration_unusable":
      return String(
        localized: LocalizedStringResource(
          "Stored microphone calibration cannot be used",
          locale: locale,
          comment: "Admission refusal when stored microphone calibration cannot be used"))
    case "admission_microphone_permission_unavailable":
      return String(
        localized: LocalizedStringResource(
          "Microphone access is unavailable",
          locale: locale,
          comment: "Admission refusal when microphone permission blocks recording"))
    case "admission_speech_recognition_unavailable":
      return String(
        localized: LocalizedStringResource(
          "Speech Recognition is unavailable",
          locale: locale,
          comment: "Admission refusal when Speech Recognition is unavailable"))
    case "admission_capture_device_unavailable":
      return String(
        localized: LocalizedStringResource(
          "No microphone is available",
          locale: locale,
          comment: "Admission refusal when no microphone is available"))
    case "admission_seal_lane_disarmed":
      return String(
        localized: LocalizedStringResource(
          "Recording admission is disabled",
          locale: locale,
          comment: "Admission refusal when recording admission is disabled"))
    case "admission_seal_vad_unavailable":
      return String(
        localized: LocalizedStringResource(
          "Speech detection is unavailable",
          locale: locale,
          comment: "Admission refusal when speech detection is unavailable"))
    default:
      return fallback
    }
  }
}
