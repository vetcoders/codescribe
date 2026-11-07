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
  static let defaultAmount = 1.0
  static let onboardingKey = "codescribe.dev_glass.onboarding.v1"
  static let compactKey = "codescribe.dev_glass.compact.v1"
  static let busHeaderKey = "codescribe.dev_glass.bus_header.v1"

  static func effectiveAmount(_ raw: Double, developerEnabled: Bool) -> Double {
    guard developerEnabled, raw.isFinite else { return defaultAmount }
    return min(1, max(0, raw))
  }
}

/// AppStorage updates already-open windows during a Lab drag. Production
/// builds ignore retained values from a previous developer build.
struct ChromeGlassPreferences: DynamicProperty {
  @AppStorage(ChromeGlassTuning.onboardingKey) private var onboarding = ChromeGlassTuning
    .defaultAmount
  @AppStorage(ChromeGlassTuning.compactKey) private var compact = ChromeGlassTuning.defaultAmount
  @AppStorage(ChromeGlassTuning.busHeaderKey) private var busHeader = ChromeGlassTuning
    .defaultAmount

  init() {}

  func binding(for surface: ChromeGlassSurface) -> Binding<Double> {
    switch surface {
    case .onboarding: $onboarding
    case .compactOverlay: $compact
    case .busHeader: $busHeader
    }
  }

  func amount(for surface: ChromeGlassSurface) -> Double {
    ChromeGlassTuning.effectiveAmount(
      binding(for: surface).wrappedValue, developerEnabled: DeveloperSurface.isEnabled())
  }
}
