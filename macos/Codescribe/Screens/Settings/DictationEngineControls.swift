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
        title: String(localized: "ASR mode"),
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
            Text(localWhisperRuntimeLabel)
              .font(CSFont.mono(11, .medium))
              .foregroundStyle(localWhisperRuntimeColor)
            Button("Recheck", action: model.recheckLocalWhisperRuntime)
              .buttonStyle(.bordered)
              .controlSize(.small)
          }
        }
      }
    }
  }

  /// One sentence for the selected mode. Cloud is described without a
  /// "no local Whisper" promise: the tail provider falls back once to the
  /// in-process engine when the remote one fails, and the Engine row then
  /// shows "Whisper (fallback)".
  private var asrModeSubtitle: String {
    switch model.asrModeId {
    case "local_power":
      String(
        localized: "Live Apple recognition, refined by the local Whisper model.",
        comment: "ASR mode description: Local power")
    case "cloud":
      String(
        localized: "The cloud provider you agreed to.",
        comment: "ASR mode description: Cloud; choosing it is the consent")
    default:
      String(
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
