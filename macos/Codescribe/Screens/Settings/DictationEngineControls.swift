import SwiftUI

/// Editable STT/layered engine controls, persisted through the promoted-key
/// config router.
struct DictationEngineControls: View {
  @ObservedObject var model: SettingsViewModel

  /// Selectable engines. "auto" defers to the core policy (Apple live when available).
  private static let sttEngineOptions = [
    SettingsMenuOption(id: "auto", label: "Auto"),
    SettingsMenuOption(id: "apple", label: "Apple (live)"),
    SettingsMenuOption(id: "whisper", label: "Whisper (Candle)"),
  ]

  private static let asrModeOptions = [
    SettingsMenuOption(id: "apple_only", label: "Apple only"),
    SettingsMenuOption(id: "local_power", label: "Local power"),
    SettingsMenuOption(id: "cloud", label: "Cloud"),
  ]

  var body: some View {
    VStack(spacing: 8) {
      if let note = model.sttEngineTruthNote {
        Text(note)
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.amber)
          .frame(maxWidth: .infinity, alignment: .leading)
          .padding(.bottom, 2)
      }
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
      SettingsControlRow(
        title: "STT engine",
        subtitle:
          "Auto/Apple = immediate Apple text; Local power continuously repairs it with Whisper. Whisper = direct local engine."
      ) {
        SettingsOptionMenu(
          options: Self.sttEngineOptions,
          selectedId: model.sttEngineId,
          currentLabel: model.sttEngineLabel,
          onSelect: model.setSttEngine
        )
      }
      if model.asrModeId == "local_power" {
        SettingsControlRow(
          title: localWhisperRuntimeTitle,
          subtitle: localWhisperRuntimeSubtitle
        ) {
          HStack(spacing: 8) {
            Text(localWhisperRuntimeLabel)
              .font(CSFont.mono(11, .medium))
              .foregroundStyle(localWhisperRuntimeColor)
            if model.localWhisperRuntimeState == .livePatchingConfigurationMismatch {
              Button("Repair", action: model.repairLocalWhisperLivePatching)
                .buttonStyle(.bordered)
                .controlSize(.small)
            } else {
              Button("Recheck", action: model.recheckLocalWhisperRuntime)
                .buttonStyle(.bordered)
                .controlSize(.small)
            }
          }
        }
      }
      SettingsControlRow(
        title: "Whole-session final pass",
        subtitle: wholeSessionFinalPassSubtitle(asrModeId: model.asrModeId)
      ) {
        Text("Off")
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.textMutedAlt)
      }
    }
  }

  private var localWhisperRuntimeTitle: String {
    switch model.localWhisperRuntimeState {
    case .directEngineReady, .directEngineNotReady:
      "Local Whisper engine"
    default:
      "Live Whisper refinement"
    }
  }

  private var localWhisperRuntimeLabel: String {
    switch model.localWhisperRuntimeState {
    case .directEngineReady: "Ready"
    case .directEngineNotReady, .livePatchingNotReady: "Not ready"
    case .livePatchingConfigured: "Required · configured"
    case .livePatchingConfigurationMismatch: "Degraded"
    case .notSelected: "Not selected"
    }
  }

  private var localWhisperRuntimeSubtitle: String {
    switch model.localWhisperRuntimeState {
    case .directEngineReady:
      "The validated local FP16 bundle serves transcription directly; no parallel Apple patcher runs."
    case .directEngineNotReady:
      "The local FP16 bundle is missing or invalid. Direct Whisper cannot serve the next take."
    case .livePatchingConfigured:
      "Apple paints immediately; the validated local FP16 model repairs the same audio spans during the take."
    case .livePatchingNotReady:
      "The local FP16 bundle is missing or invalid. Local power is explicitly not ready until validation passes."
    case .livePatchingConfigurationMismatch:
      "Local power requires the runtime phase1 arming token, but persisted readback is disarmed. Repair before the next take."
    case .notSelected:
      "Local Whisper is not the selected provider."
    }
  }

  private var localWhisperRuntimeColor: Color {
    switch model.localWhisperRuntimeState {
    case .directEngineReady, .livePatchingConfigured:
      CSColor.oliveLight
    case .directEngineNotReady, .livePatchingNotReady, .livePatchingConfigurationMismatch:
      CSColor.amber
    case .notSelected:
      CSColor.textMutedAlt
    }
  }
}
