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
    case .assistive: String(localized: "Assistive prompt")
    }
  }

  /// The parenthesised file names are identifiers on disk — keep them verbatim.
  var editorSubtitle: String {
    switch self {
    case .correction:
      String(
        localized: "Correction only AI formatting (formatting.txt)",
        comment: "formatting.txt is a file name — do not translate"
      )
    case .smart:
      String(
        localized: "Balanced transcript editing (formatting-smart.txt)",
        comment: "formatting-smart.txt is a file name — do not translate"
      )
    case .max:
      String(
        localized: "Maximum supported prose polish (formatting-max.txt)",
        comment: "formatting-max.txt is a file name — do not translate"
      )
    case .assistive:
      String(
        localized: "Base system prompt for the voice assistant (assistive.txt)",
        comment: "assistive.txt is a file name — do not translate"
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
