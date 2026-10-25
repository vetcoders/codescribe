import Foundation
import OSLog

/// Lab extras baked only by keyed `make install-app`.
enum DeveloperSurface {
  /// Agent header mark, only while Voice Lab is enabled on a developer build.
  static var powerModeCaption: String {
    String(
      localized: "Voice Lab power mode",
      comment: "Agent header mark on a developer build while Voice Lab power mode is on"
    )
  }

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

/// The build's release channel, baked as `CSBuildChannel` by
/// scripts/build-app.sh: "nightly" for every non-release profile.
enum BuildChannel {
  static func isNightly(in bundle: Bundle = .main) -> Bool {
    (bundle.object(forInfoDictionaryKey: "CSBuildChannel") as? String)?
      .trimmingCharacters(in: .whitespacesAndNewlines).lowercased() == "nightly"
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
