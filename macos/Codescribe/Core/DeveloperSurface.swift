import Foundation
import OSLog

/// Lab extras baked only by keyed `make install-app`.
enum DeveloperSurface {
  /// Corner caption only while Voice Lab is enabled on a developer build.
  static let powerModeCaption = "You use dev power mode"

  static func parse(_ raw: Any?) -> Bool {
    if let flag = raw as? Bool { return flag }
    if let number = raw as? NSNumber { return number.boolValue }
    if let text = raw as? String {
      let normalized = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
      return normalized == "1" || normalized == "true" || normalized == "yes"
    }
    return false
  }

  static func isEnabled(in bundle: Bundle = .main) -> Bool {
    parse(bundle.object(forInfoDictionaryKey: "CSDeveloperSurface"))
  }

  static func isPowerModeEnabled(labMode: Bool, surfaceEnabled: Bool? = nil) -> Bool {
    (surfaceEnabled ?? isEnabled()) && labMode
  }
}

/// Daily overlay visibility. The tray "Transcription Overlay" toggle is the
/// product switch. Lab mode never writes that toggle and is unavailable on
/// a production bundle, even if UserDefaults still holds
/// `codescribe.lab_mode` from a previous install-app.
enum DictationOverlayGate {
  static let labModeDefaultsKey = "codescribe.lab_mode"
  static let logger = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "overlay-gate"
  )

  static func isLabModeOn(
    defaults: UserDefaults = .standard,
    surfaceEnabled: Bool? = nil
  ) -> Bool {
    DeveloperSurface.isPowerModeEnabled(
      labMode: defaults.bool(forKey: labModeDefaultsKey), surfaceEnabled: surfaceEnabled)
  }

  static func shouldShowOverlay(
    trayEnabled: Bool,
    defaults _: UserDefaults = .standard,
    surfaceEnabled _: Bool? = nil
  ) -> Bool {
    trayEnabled
  }
}
