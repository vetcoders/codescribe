import Foundation

/// The one wording for the start/stop transcription action, shared by the tray
/// row and the Dock menu so both surfaces read the same state the same way.
enum DictationActionTitle {
  static func title(
    isStarting: Bool,
    isRecording: Bool,
    assistiveRecording: Bool,
    startInAssistive: Bool
  ) -> String {
    if isStarting {
      return String(localized: "Starting…", comment: "Tray row: a recording start is in flight")
    }
    if isRecording {
      return assistiveRecording
        ? String(
          localized: "Stop transcription in Agent mode",
          comment: "Tray row: stop the current transcription in Agent mode")
        : String(
          localized: "Stop transcription", comment: "Tray row: stop the current transcription")
    }
    return startInAssistive
      ? String(
        localized: "Start transcription in Agent mode",
        comment: "Tray row: start a new transcription in Agent mode")
      : String(localized: "Start transcription", comment: "Tray row: start a new transcription")
  }
}
