import Foundation

/// The four editable BASE prompt files shown on Agent › Prompts. Presentation
/// identity only: loading, saving and restoring stay on `SettingsViewModel`.
enum PromptFile: String, CaseIterable, Identifiable {
  case correction
  case smart
  case max
  case assistive

  var id: String { rawValue }

  /// Segment label.
  var title: String {
    switch self {
    case .correction:
      String(localized: "Correction", comment: "Prompt file: correction-only formatting")
    case .smart: String(localized: "Smart", comment: "Prompt file: balanced transcript editing")
    case .max: String(localized: "Max", comment: "Prompt file: maximum prose polish")
    case .assistive:
      String(localized: "Assistive", comment: "Prompt file: voice-assistant base prompt")
    }
  }

  /// Editor heading, also the subject of the restore confirmation.
  var editorTitle: String {
    switch self {
    case .correction: String(localized: "Correction prompt")
    case .smart: String(localized: "Smart prompt")
    case .max: String(localized: "Max prompt")
    case .assistive: String(localized: "Agent prompt")
    }
  }

  /// One plain sentence per prompt. File names stay under File details on the
  /// panel; the Agent sentence names the one lane that reads assistive.txt
  /// (`compose_agent_system_prompt`), since voice chat carries its own persona.
  var editorSubtitle: String {
    switch self {
    case .correction:
      String(localized: "Formatting limited to corrections.")
    case .smart:
      String(localized: "Balanced editing of the transcript.")
    case .max:
      String(localized: "The fullest polish of the text.")
    case .assistive:
      String(
        localized:
          "Base instructions for the Agent acting on a dictated request. Voice chat uses its own instructions."
      )
    }
  }

  /// Formatting level whose prompt file this is; nil for the assistive prompt.
  var formattingLevel: FormattingPolicyOption? {
    switch self {
    case .correction: .correction
    case .smart: .smart
    case .max: .max
    case .assistive: nil
    }
  }
}
