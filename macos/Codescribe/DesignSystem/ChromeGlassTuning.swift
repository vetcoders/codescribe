import SwiftUI

/// Experimental chrome only; the accepted Agent/Settings window materials
/// and quiet reading canvases do not read these developer preferences.
enum ChromeGlassSurface: String, CaseIterable, Identifiable {
  case onboarding
  case compactOverlay
  case busHeader

  var id: Self { self }

  var title: String {
    switch self {
    case .onboarding: String(localized: "Onboarding")
    case .compactOverlay: String(localized: "Mini and MIDI")
    case .busHeader: String(localized: "Bus header")
    }
  }
}

enum ChromeGlassTuning {
  static let defaultTint = 0.0
  static let onboardingKey = "codescribe.dev_glass_tint.onboarding.v1"
  static let compactKey = "codescribe.dev_glass_tint.compact.v1"
  static let busHeaderKey = "codescribe.dev_glass_tint.bus_header.v1"

  static func effectiveTint(_ raw: Double, developerEnabled: Bool) -> Double {
    guard developerEnabled, raw.isFinite else { return defaultTint }
    return min(1, max(0, raw))
  }
}

/// AppStorage updates already-open windows during a Lab drag. Production
/// builds ignore retained values from a previous developer build.
struct ChromeGlassPreferences: DynamicProperty {
  @AppStorage(ChromeGlassTuning.onboardingKey) private var onboarding = ChromeGlassTuning
    .defaultTint
  @AppStorage(ChromeGlassTuning.compactKey) private var compact = ChromeGlassTuning.defaultTint
  @AppStorage(ChromeGlassTuning.busHeaderKey) private var busHeader = ChromeGlassTuning
    .defaultTint

  init() {}

  func binding(for surface: ChromeGlassSurface) -> Binding<Double> {
    switch surface {
    case .onboarding: $onboarding
    case .compactOverlay: $compact
    case .busHeader: $busHeader
    }
  }

  func tint(for surface: ChromeGlassSurface) -> Double {
    ChromeGlassTuning.effectiveTint(
      binding(for: surface).wrappedValue, developerEnabled: DeveloperSurface.isEnabled())
  }
}
