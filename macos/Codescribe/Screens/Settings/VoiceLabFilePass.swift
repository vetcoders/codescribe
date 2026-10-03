import Foundation

/// Voice Lab's optional second-pass transcription follows the configured ASR
/// mode. The overlay has no role in file analysis or correction teaching.
func helperRetranscribePass(asrMode: String) -> FileRetranscribePass? {
  switch asrMode.lowercased() {
  case "local_power": return .fullHq
  case "cloud": return .cloud
  default: return nil
  }
}

enum HelperFilePassRefusal: Equatable, Error {
  case noHelper
  case noArchivedAudio
}

enum HelperFilePass {
  static func request(asrMode: String, archivedAudio: URL?) -> Result<
    (FileRetranscribePass, String), HelperFilePassRefusal
  > {
    guard let pass = helperRetranscribePass(asrMode: asrMode) else {
      return .failure(.noHelper)
    }
    guard let archived = archivedAudio else {
      return .failure(.noArchivedAudio)
    }
    return .success((pass, "\(pass.rawValue):\(archived.path)"))
  }

  static func compare(
    daily: String, helper: String, pass: FileRetranscribePass, locale: Locale = .current
  ) -> String {
    let left = daily.trimmingCharacters(in: .whitespacesAndNewlines)
    let right = helper.trimmingCharacters(in: .whitespacesAndNewlines)
    if left == right {
      return String(localized: "Helper \(pass.visibleName) matches daily.", locale: locale)
    }
    return String(
      localized:
        "DAILY\n\(left)\n\nHELPER \(pass.visibleName.uppercased(with: locale))\n\(right)\n\nDaily is unchanged until you save a correction.",
      locale: locale
    )
  }
}

enum FileRetranscribePass: String, CaseIterable, Identifiable {
  case fullHq = "hq"
  case cloud = "cloud"

  var id: String { rawValue }

  var visibleName: String {
    switch self {
    case .fullHq: String(localized: "Full HQ file pass")
    case .cloud: String(localized: "Cloud pass")
    }
  }

  var help: String {
    switch self {
    case .fullHq: String(localized: "Full local Whisper pass over the selected audio file")
    case .cloud: String(localized: "Cloud STT pass over the selected audio file")
    }
  }
}
