import SwiftUI

/// One product mode selector with observed local refinement readiness. The row
/// describes the selected mode only; the other two are one click away.
struct DictationEngineControls: View {
  @ObservedObject var model: SettingsViewModel

  private static let asrModeOptions = [
    SettingsMenuOption(id: "apple_only", label: String(localized: "Apple only")),
    SettingsMenuOption(id: "local_power", label: String(localized: "Local power")),
    SettingsMenuOption(id: "cloud", label: String(localized: "Cloud")),
  ]

  var body: some View {
    VStack(spacing: 8) {
      SettingsControlRow(
        title: String(
          localized: "Recognition mode",
          comment: "Engine tab row: the one editable choice, how speech becomes text"),
        subtitle: asrModeSubtitle
      ) {
        SettingsOptionMenu(
          options: Self.asrModeOptions,
          selectedId: model.asrModeId,
          currentLabel: model.asrModeLabel,
          onSelect: model.setAsrMode
        )
      }
      if model.asrModeId == "local_power" {
        SettingsControlRow(
          title: String(localized: "Live Whisper refinement"),
          subtitle: localWhisperRuntimeSubtitle
        ) {
          HStack(spacing: 8) {
            // Readiness is an ordinary word, so it reads in the interface
            // font; monospace stays for technical values on this pane
            // (Founder brief, round 13, 2026-10-10).
            Text(localWhisperRuntimeLabel)
              .font(CSFont.ui(11.5, .semibold))
              .foregroundStyle(localWhisperRuntimeColor)
            Button("Recheck", action: model.recheckLocalWhisperRuntime)
              .buttonStyle(.bordered)
              .controlSize(.small)
          }
        }
      }
    }
  }

  /// One sentence for the selected mode, where there is one to say. The two
  /// on-device modes describe what runs; Cloud has no caption, because naming
  /// the consent again told nobody anything they could act on (Founder brief,
  /// round 13, 2026-10-10). That is a caption, not a warning: a consent that
  /// is actually missing never reaches this row — `asrModeId` reports Apple
  /// only until it is granted — and the Privacy tab owns the grant itself.
  private var asrModeSubtitle: String? {
    switch model.asrModeId {
    case "local_power":
      return String(
        localized: "Live Apple recognition, refined by the local Whisper model.",
        comment: "ASR mode description: Local power")
    case "cloud":
      return nil
    default:
      return String(
        localized: "Live recognition by Apple.",
        comment: "ASR mode description: Apple only")
    }
  }

  private var localWhisperRuntimeLabel: String {
    switch model.localWhisperRuntimeState {
    case .livePatchingNotReady: String(localized: "Not ready")
    case .livePatchingConfigured: String(localized: "Ready")
    case .degradedEnvOverride: String(localized: "Degraded (env override)")
    case .notSelected: String(localized: "Not selected")
    }
  }

  private var localWhisperRuntimeSubtitle: String {
    switch model.localWhisperRuntimeState {
    case .livePatchingConfigured:
      String(
        localized:
          "Apple paints immediately; the validated local FP16 model repairs the same audio spans during the take."
      )
    case .livePatchingNotReady:
      String(
        localized:
          "The local FP16 bundle is missing or invalid. Local power is explicitly not ready until validation passes."
      )
    case .degradedEnvOverride:
      String(
        localized:
          "CODESCRIBE_LAYERED_TRANSCRIPTION disables live refinement. Remove the diagnostic override from .env and restart.",
        comment: "CODESCRIBE_LAYERED_TRANSCRIPTION is an environment variable name"
      )
    case .notSelected:
      String(localized: "Local Whisper is not the selected provider.")
    }
  }

  private var localWhisperRuntimeColor: Color {
    switch model.localWhisperRuntimeState {
    case .livePatchingConfigured:
      CSColor.oliveLight
    case .livePatchingNotReady, .degradedEnvOverride:
      CSColor.amber
    case .notSelected:
      Color.secondary
    }
  }
}
