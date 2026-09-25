import SwiftUI

/// One product mode selector with observed local refinement readiness.
struct DictationEngineControls: View {
  @ObservedObject var model: SettingsViewModel

  private static let asrModeOptions = [
    SettingsMenuOption(id: "apple_only", label: "Apple only"),
    SettingsMenuOption(id: "local_power", label: "Local power"),
    SettingsMenuOption(id: "cloud", label: "Cloud"),
  ]

  var body: some View {
    VStack(spacing: 8) {
      SettingsControlRow(
        title: "ASR mode",
        subtitle:
          "Apple only = live Apple without Layer 1. Local power = Apple-first with mandatory on-device Whisper refinement. Cloud uses its consent-gated provider, not local Whisper."
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
          title: "Live Whisper refinement",
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

  private var localWhisperRuntimeLabel: String {
    switch model.localWhisperRuntimeState {
    case .livePatchingNotReady: "Not ready"
    case .livePatchingConfigured: "Ready"
    case .degradedEnvOverride: "Degraded (env override)"
    case .notSelected: "Not selected"
    }
  }

  private var localWhisperRuntimeSubtitle: String {
    switch model.localWhisperRuntimeState {
    case .livePatchingConfigured:
      "Apple paints immediately; the validated local FP16 model repairs the same audio spans during the take."
    case .livePatchingNotReady:
      "The local FP16 bundle is missing or invalid. Local power is explicitly not ready until validation passes."
    case .degradedEnvOverride:
      "CODESCRIBE_LAYERED_TRANSCRIPTION disables live refinement. Remove the diagnostic override from .env and restart."
    case .notSelected:
      "Local Whisper is not the selected provider."
    }
  }

  private var localWhisperRuntimeColor: Color {
    switch model.localWhisperRuntimeState {
    case .livePatchingConfigured:
      CSColor.oliveLight
    case .livePatchingNotReady, .degradedEnvOverride:
      CSColor.amber
    case .notSelected:
      CSColor.textMutedAlt
    }
  }
}
