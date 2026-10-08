import Foundation

/// Semantic chapters of the first-run wizard. Rust app/os/onboarding.rs owns
/// the versioned marker migration; these nine indices are the v3 layout.
enum OnboardingStep: Equatable {
  case interfaceLanguage
  case mode
  case permissions
  case language
  /// Opt-in local model download; never a navigation or completion gate.
  case localModel
  case apiKey
  case hotkeyMode
  case agenticReadiness
  case done

  static let flow: [OnboardingStep] = [
    .interfaceLanguage, .mode, .permissions, .language, .localModel,
    .apiKey, .hotkeyMode, .agenticReadiness, .done,
  ]

  static var count: Int { flow.count }

  static func step(at index: Int) -> OnboardingStep {
    flow[min(max(0, index), count - 1)]
  }
}
