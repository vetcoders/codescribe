import SwiftUI

/// One product mode selector with observed local refinement readiness.
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
        subtitle: String(
          localized:
            "Apple only = live Apple without Layer 1. Local power = Apple-first with mandatory on-device Whisper refinement. Cloud uses its consent-gated provider, not local Whisper."
        )
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
