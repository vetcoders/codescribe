import Foundation
import OSLog

/// Lab extras baked only by keyed `make install-app`.
enum DeveloperSurface {
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
}

/// Daily overlay visibility. The tray "Transcription Overlay" toggle is the
/// product switch. Lab mode is a developer veto that never writes that toggle
/// and never fires on a production bundle, even if UserDefaults still holds
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
    let surface = surfaceEnabled ?? DeveloperSurface.isEnabled()
    return surface && defaults.bool(forKey: labModeDefaultsKey)
  }

  static func shouldShowOverlay(
    trayEnabled: Bool,
    defaults: UserDefaults = .standard,
    surfaceEnabled: Bool? = nil
  ) -> Bool {
    trayEnabled && !isLabModeOn(defaults: defaults, surfaceEnabled: surfaceEnabled)
  }
}
