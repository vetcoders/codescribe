import Foundation

// Whether the selected dictation path can take its first recording, projected
// from verdicts that already exist. Nothing here owns state or decides on its
// own: the controller's admission (`CodescribeHotkeys.admissionReadiness`, the
// same check that runs before a microphone opens), the shared Whisper download
// store and the native permission probe are the inputs. Setup's Finish and the
// Settings health footer read this one result, so setup cannot report success
// while Settings reports a blocker for the same path, and the reverse.

/// Transcription lane the runtime runs. Raw values are `AsrProductMode::as_str()`.
enum DictationLane: String, Equatable, Sendable {
  case appleOnly = "apple_only"
  case localPower = "local_power"
  case cloud

  /// The core's own resolution (`resolve_asr_product_mode`: persisted mode,
  /// consent record and the legacy `use_local_stt` choice) projected by the
  /// bridge from the same snapshot as `CsSettings.resolvedAsrMode`. Swift reads
  /// it and never re-derives it from `asrMode` and `cloudConsent`. The bridge
  /// writes only these three values; anything else is a binding mismatch and
  /// lands where the core lands every refusal, Apple only.
  init(resolvedAsrMode: String) {
    self = DictationLane(rawValue: resolvedAsrMode) ?? .appleOnly
  }

  /// Only Local power transcribes with the downloaded Whisper weights. Apple
  /// only and Cloud never wait for them.
  var requiresLocalModel: Bool { self == .localPower }

  /// The names Settings › Dictation shows for the same lanes.
  func displayName(locale: Locale) -> String {
    switch self {
    case .appleOnly:
      return String(
        localized: LocalizedStringResource(
          "Apple only", locale: locale, comment: "Dictation lane: on-device Apple Speech"))
    case .localPower:
      return String(
        localized: LocalizedStringResource(
          "Local power", locale: locale, comment: "Dictation lane: local model weights"))
    case .cloud:
      return String(
        localized: LocalizedStringResource(
          "Cloud", locale: locale, comment: "Dictation lane: cloud refinement"))
    }
  }
}

/// The local Whisper weights as the shared download store observes them.
enum LocalModelReadiness: Equatable, Sendable {
  case installed
  case downloading
  case notInstalled
  /// The user stopped the transfer; the partial file was removed.
  case cancelled
  /// The transfer failed, or finished without usable weights.
  case failed
}

/// Admission blocker codes Swift acts on. The controller owns the full set;
/// these are the ones with a remedy setup can offer in place.
enum AdmissionCode {
  static let microphonePermission = "admission_microphone_permission_unavailable"
  static let calibration: Set<String> = [
    "admission_calibration_missing", "admission_calibration_no_profile",
    "admission_calibration_refused", "admission_calibration_unusable",
  ]
}

struct DictationReadiness: Equatable, Sendable {
  enum Blocker: Equatable, Sendable {
    /// A grant setup requires is not observed for this process.
    case permission(PermissionKind, PermissionState)
    /// Local power is selected and its weights are not installed.
    case localModel(LocalModelReadiness)
    /// The controller refuses the next recording. `code` is the admission code.
    case admission(code: String, message: String)
    /// No admission verdict yet (`error == nil`), or the read failed.
    case admissionUnknown(error: String?)

    /// The refusal is about the stored microphone measurement.
    var isCalibration: Bool {
      if case .admission(let code, _) = self { return AdmissionCode.calibration.contains(code) }
      return false
    }
  }

  let lane: DictationLane
  let blockers: [Blocker]

  var isReady: Bool { blockers.isEmpty }

  var missingPermissions: [PermissionKind] {
    blockers.compactMap {
      if case .permission(let kind, _) = $0 { return kind }
      return nil
    }
  }

  var localModelBlocker: LocalModelReadiness? {
    for blocker in blockers {
      if case .localModel(let state) = blocker { return state }
    }
    return nil
  }

  var admissionBlocker: Blocker? {
    blockers.first {
      switch $0 {
      case .admission, .admissionUnknown: return true
      case .permission, .localModel: return false
      }
    }
  }
}

/// One verdict for the selected path. Order is the order of action: grants,
/// then the selected lane's model, then the controller's admission. A
/// microphone refusal already named by the native probe is not repeated.
func evaluateDictationReadiness(
  lane: DictationLane,
  permissions: PermissionSnapshot,
  localModel: LocalModelReadiness,
  admission: CsAdmissionReadiness?,
  admissionError: String?
) -> DictationReadiness {
  var blockers: [DictationReadiness.Blocker] = []
  for kind in PermissionKind.allCases where kind.isRequiredForSetup {
    let state = permissions.state(kind)
    if !state.isGranted { blockers.append(.permission(kind, state)) }
  }
  if lane.requiresLocalModel, localModel != .installed {
    blockers.append(.localModel(localModel))
  }
  if let admissionError {
    blockers.append(.admissionUnknown(error: admissionError))
  } else if let admission {
    let probeNamesMicrophone =
      !permissions.microphone.isGranted && admission.code == AdmissionCode.microphonePermission
    if !admission.ready, !probeNamesMicrophone {
      blockers.append(.admission(code: admission.code, message: admission.message))
    }
  } else {
    blockers.append(.admissionUnknown(error: nil))
  }
  return DictationReadiness(lane: lane, blockers: blockers)
}
