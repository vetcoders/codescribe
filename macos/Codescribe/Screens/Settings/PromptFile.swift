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
    case .correction: "Correction"
    case .smart: "Smart"
    case .max: "Max"
    case .assistive: "Assistive"
    }
  }

  /// Editor heading, also the subject of the restore confirmation.
  var editorTitle: String { "\(title) prompt" }

  var editorSubtitle: String {
    switch self {
    case .correction: "Correction only AI formatting (formatting.txt)"
    case .smart: "Balanced transcript editing (formatting-smart.txt)"
    case .max: "Maximum supported prose polish (formatting-max.txt)"
    case .assistive: "Base system prompt for the voice assistant (assistive.txt)"
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
