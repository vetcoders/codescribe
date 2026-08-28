import SwiftUI

enum AudioInputDisplayTone: Equatable {
  case healthy
  case fallback
  case unavailable
}

struct AudioInputDisplayState: Equatable {
  let tone: AudioInputDisplayTone
  let title: String
  let detail: String
}

struct SealLaneControlState: Equatable {
  let isOn: Bool
  let isEnabled: Bool
  let detail: String
}

enum AudioReadinessStepID: Int, CaseIterable, Identifiable {
  case microphone
  case calibration
  case sealLane

  var id: Int { rawValue }
}

struct AudioReadinessStep: Identifiable, Equatable {
  let id: AudioReadinessStepID
  let tone: AudioInputDisplayTone
  let title: String
  let detail: String
}

/// Stable three-step projection of recording readiness. The bridge verdict is
/// still authoritative; this only makes its prerequisites visible together so
/// users do not discover them one failed take at a time.
func audioReadinessSteps(
  input: CsAudioInputSnapshot,
  admission: CsAdmissionReadiness?
) -> [AudioReadinessStep] {
  let microphone = audioInputDisplayState(input)

  let calibration: AudioReadinessStep
  if let admission {
    if let version = admission.calibrationVersion, admission.calibrationStatus == "sealed" {
      calibration = AudioReadinessStep(
        id: .calibration,
        tone: .healthy,
        title: "Microphone calibrated",
        detail: version
      )
    } else {
      calibration = AudioReadinessStep(
        id: .calibration,
        tone: .unavailable,
        title: "Calibration required",
        detail: "Measure about 10 seconds of normal speech on the current microphone."
      )
    }
  } else {
    calibration = AudioReadinessStep(
      id: .calibration,
      tone: .fallback,
      title: "Checking calibration…",
      detail: "Reading the controller's measured profile."
    )
  }

  let sealLane: AudioReadinessStep
  if let admission {
    let source =
      admission.sealLaneSource == "env_override"
      ? "Controlled by \(admission.sealLaneEnv) override."
      : "Controlled by the product setting below."
    sealLane = AudioReadinessStep(
      id: .sealLane,
      tone: admission.sealLaneArmed ? .healthy : .unavailable,
      title: admission.sealLaneArmed ? "Seal lane armed" : "Seal lane must be enabled",
      detail: source
    )
  } else {
    sealLane = AudioReadinessStep(
      id: .sealLane,
      tone: .fallback,
      title: "Checking seal lane…",
      detail: "Reading the effective product setting and override."
    )
  }

  return [
    AudioReadinessStep(
      id: .microphone,
      tone: microphone.tone,
      title: microphone.title,
      detail: microphone.detail
    ),
    calibration,
    sealLane,
  ]
}

/// Present the persisted product choice independently from its effective
/// value. An env override stays visible and read-only instead of making the
/// Settings toggle lie about which authority currently wins.
func sealLaneControlState(_ readiness: CsAdmissionReadiness?) -> SealLaneControlState {
  guard let readiness else {
    return SealLaneControlState(
      isOn: true,
      isEnabled: false,
      detail: "Reading the product setting and any power-user override."
    )
  }
  guard readiness.sealLaneSource == "env_override" else {
    return SealLaneControlState(
      isOn: readiness.sealLaneSettingArmed,
      isEnabled: true,
      detail: "Required for committed utterances; stored in Settings."
    )
  }
  let state = readiness.sealLaneArmed ? "armed" : "disarmed"
  return SealLaneControlState(
    isOn: readiness.sealLaneSettingArmed,
    isEnabled: false,
    detail:
      "Product setting is read-only while \(readiness.sealLaneEnv) keeps the lane \(state). Remove the power-user override to edit here."
  )
}

/// Pure UI projection of the controller's admission verdict for XCTest. The
/// bridge record already carries the one blocker the controller would apply;
/// this function only words it — it never decides readiness itself.
func admissionDisplayState(_ readiness: CsAdmissionReadiness?) -> AudioInputDisplayState {
  guard let readiness else {
    return AudioInputDisplayState(
      tone: .fallback,
      title: "Checking acoustic admission…",
      detail: "Reading the controller's calibration and seal-lane verdict."
    )
  }
  if readiness.ready {
    let device = readiness.deviceName ?? "input device"
    let version = readiness.calibrationVersion ?? "measured profile"
    return AudioInputDisplayState(
      tone: .healthy,
      title: "Ready to record on \(device)",
      detail: "Calibration \(version); seal lane armed."
    )
  }
  let title: String
  switch readiness.code {
  case "admission_calibration_missing":
    title = "Microphone not calibrated yet"
  case "admission_calibration_no_profile":
    title = "No calibration for the current microphone"
  case "admission_calibration_refused", "admission_calibration_unusable":
    title = "Stored calibration cannot be used"
  case "admission_seal_lane_disarmed":
    title =
      readiness.sealLaneSource == "env_override"
      ? "Seal lane is disarmed by \(readiness.sealLaneEnv) override"
      : "Seal lane is off in Settings › Audio"
  case "admission_seal_vad_unavailable":
    title = "Silero VAD did not load"
  case "admission_capture_device_unavailable":
    title = "No input device available"
  default:
    title = "Recording cannot start"
  }
  return AudioInputDisplayState(tone: .unavailable, title: title, detail: readiness.message)
}

/// Pure UI projection for XCTest. The bridge snapshot already contains the
/// live cpal resolution; this function never re-resolves a configured wish.
func audioInputDisplayState(_ snapshot: CsAudioInputSnapshot) -> AudioInputDisplayState {
  guard let runtimeDevice = snapshot.runtimeDevice, !runtimeDevice.isEmpty else {
    return AudioInputDisplayState(
      tone: .unavailable,
      title: "No input device available",
      detail: "Connect a microphone and refresh Audio settings."
    )
  }

  if !snapshot.runtimeConfigurationMatches {
    let saved = snapshot.configuredDevice ?? "System default"
    return AudioInputDisplayState(
      tone: .fallback,
      title: "Currently using: \(runtimeDevice)",
      detail:
        "Saved: \(saved). Restart Codescribe to apply it; an explicit AUDIO_INPUT_DEVICE launch override can keep a different runtime input active."
    )
  }

  if snapshot.fallbackToDefault {
    let missing = snapshot.configuredDevice ?? "The configured input"
    return AudioInputDisplayState(
      tone: .fallback,
      title: "Using system fallback: \(runtimeDevice)",
      detail: "\(missing) is unavailable. Recording continues on the live default input."
    )
  }

  if snapshot.configuredDevice == nil {
    return AudioInputDisplayState(
      tone: .healthy,
      title: "System default: \(runtimeDevice)",
      detail: "The recorder resolves this device from Core Audio at runtime."
    )
  }

  return AudioInputDisplayState(
    tone: .healthy,
    title: "Runtime input: \(runtimeDevice)",
    detail: "The configured device is present and selected by the recorder."
  )
}

struct AudioPanel: View {
  @ObservedObject var model: SettingsViewModel

  private static let systemDefaultChoice = "__codescribe_system_default__"

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(alignment: .top, spacing: 12) {
        VStack(alignment: .leading, spacing: 0) {
          EyebrowLabel(text: "Settings · Audio")
          Text("Hear the real input.")
            .font(CSFont.ui(26, .bold))
            .tracking(-0.5)
            .foregroundStyle(CSColor.textHigh)
            .padding(.top, 6)
          Text("Device choice and sound feedback use the live recorder config.")
            .font(CSFont.ui(12.5))
            .lineSpacing(2)
            .foregroundStyle(CSColor.textMutedAlt)
            .padding(.top, 8)
        }
        Spacer(minLength: 0)
        Button("Refresh") {
          model.refreshAudioInput()
        }
        .csFocusRing(cornerRadius: 8)
        .font(CSFont.mono(11, .semibold))
        .foregroundStyle(CSColor.chromeAccent)
        .accessibilityLabel("Refresh audio input devices")
      }

      SettingsSectionLabel("Input device")
        .padding(.top, 24)
      inputDeviceSection
        .padding(.top, 11)

      SettingsSectionLabel("Recording readiness")
        .padding(.top, 24)
      admissionSection
        .padding(.top, 11)
        .task { await model.refreshAdmission() }

      SettingsSectionLabel("Sound feedback")
        .padding(.top, 24)
      feedbackSection
        .padding(.top, 11)
    }
    .padding(.horizontal, 28)
    .padding(.vertical, 24)
  }

  private var inputDeviceSection: some View {
    VStack(alignment: .leading, spacing: 14) {
      SettingsControlRow(
        title: "Microphone",
        subtitle: "Saved in settings.json; runtime falls back safely if it disappears"
      ) {
        Picker("Input device", selection: inputDeviceBinding) {
          Text("System default").tag(Self.systemDefaultChoice)
          ForEach(deviceOptions, id: \.self) { device in
            if device == model.audioInput.configuredDevice,
              !model.audioInput.configuredDeviceAvailable
            {
              Text("\(device) — unavailable").tag(device)
            } else {
              Text(device).tag(device)
            }
          }
        }
        .labelsHidden()
        .frame(width: 260)
        .accessibilityLabel("Audio input device")
        .accessibilityValue(inputDeviceAccessibilityValue)
      }

      HStack {
        Text("Reset removes the preference; it never writes an empty device name.")
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textFaint)
        Spacer(minLength: 12)
        Button("Use system default") {
          model.resetAudioInputDevice()
        }
        .csFocusRing(cornerRadius: 8)
        .font(CSFont.mono(10.5, .semibold))
        .foregroundStyle(CSColor.chromeAccent)
        .disabled(model.settings.audioInputDevice == nil)
        .accessibilityLabel("Reset audio input to system default")
      }
    }
    .padding(15)
    .background(card)
    .overlay(cardBorder)
  }

  /// The controller's precondition for any take, and the one operator step
  /// that can satisfy it locally: a ~10 s guided measurement through the real
  /// recorder path. No value is ever invented here.
  private var admissionSection: some View {
    VStack(alignment: .leading, spacing: 14) {
      readinessCockpit

      let sealLane = sealLaneControlState(model.admission)
      SettingsControlRow(
        title: "Seal lane",
        subtitle: sealLane.detail
      ) {
        Toggle("", isOn: sealLaneBinding)
          .toggleStyle(.switch)
          .labelsHidden()
          .tint(CSColor.chromeAccent)
          .disabled(!sealLane.isEnabled)
          .accessibilityLabel("Seal lane")
          .accessibilityValue(sealLaneAccessibilityValue(sealLane))
          .accessibilityHint(
            sealLane.isEnabled
              ? "Controls whether committed utterances can be sealed."
              : sealLane.detail
          )
      }

      HStack(alignment: .top) {
        Text(
          "Calibration measures your normal speech level on this microphone and derives the existence floor (ITU-T P.56 margin). Audio is not kept."
        )
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(CSColor.textFaint)
        Spacer(minLength: 12)
        Button(model.calibrationPending ? "Listening…" : "Calibrate microphone") {
          Task { await model.runCalibration() }
        }
        .csFocusRing(cornerRadius: 8)
        .font(CSFont.mono(10.5, .semibold))
        .foregroundStyle(CSColor.chromeAccent)
        .disabled(model.calibrationPending)
        .accessibilityLabel("Calibrate microphone")
      }

      if let notice = model.calibrationNotice {
        Text(notice)
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.textMutedAlt)
          .accessibilityLabel("Calibration result")
      }
    }
    .padding(15)
    .background(card)
    .overlay(cardBorder)
  }

  private var readinessCockpit: some View {
    VStack(alignment: .leading, spacing: 0) {
      admissionStatus
        .padding(.bottom, 4)

      ForEach(audioReadinessSteps(input: model.audioInput, admission: model.admission)) { step in
        readinessStep(step)
        if step.id != .sealLane {
          Divider().overlay(CSColor.hairline(0.06))
        }
      }
    }
    .padding(12)
    .background(CSColor.surfaceRaised(0.03))
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Recording readiness")
  }

  private func readinessStep(_ step: AudioReadinessStep) -> some View {
    HStack(alignment: .top, spacing: 10) {
      ZStack {
        Circle()
          .fill(statusColor(step.tone).opacity(0.14))
          .frame(width: 24, height: 24)
        if step.tone == .healthy {
          Image(systemName: "checkmark")
            .font(.system(size: 10, weight: .bold))
            .foregroundStyle(statusColor(step.tone))
        } else {
          Text("\(step.id.rawValue + 1)")
            .font(CSFont.mono(10, .semibold))
            .foregroundStyle(statusColor(step.tone))
        }
      }
      VStack(alignment: .leading, spacing: 3) {
        Text(step.title)
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(CSColor.textBody)
        Text(step.detail)
          .font(CSFont.ui(11.5))
          .lineSpacing(2)
          .foregroundStyle(CSColor.textMutedAlt)
      }
      Spacer(minLength: 0)
    }
    .padding(.vertical, 9)
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("Step \(step.id.rawValue + 1), \(step.title)")
    .accessibilityValue(step.detail)
  }

  @ViewBuilder
  private var admissionStatus: some View {
    if let error = model.admissionReadError {
      statusRow(
        color: CSColor.terracottaLight,
        title: "Admission check unavailable",
        detail: error
      )
    } else {
      let state = admissionDisplayState(model.admission)
      statusRow(
        color: statusColor(state.tone),
        title: state.title,
        detail: state.detail
      )
      .accessibilityElement(children: .ignore)
      .accessibilityLabel("Acoustic admission")
      .accessibilityValue("\(state.title). \(state.detail)")
    }
  }

  @ViewBuilder
  private var runtimeInputStatus: some View {
    if let error = model.audioInputReadError {
      statusRow(
        color: CSColor.terracottaLight,
        title: "Audio hardware unavailable",
        detail: error
      )
    } else {
      let state = audioInputDisplayState(model.audioInput)
      statusRow(
        color: statusColor(state.tone),
        title: state.title,
        detail: state.detail
      )
      .accessibilityElement(children: .ignore)
      .accessibilityLabel("Runtime audio input")
      .accessibilityValue("\(state.title). \(state.detail)")
    }
  }

  private var feedbackSection: some View {
    VStack(alignment: .leading, spacing: 14) {
      SettingsControlRow(
        title: "Start sound",
        subtitle: "Play the recorder's live start confirmation"
      ) {
        Toggle("", isOn: soundFeedbackBinding)
          .toggleStyle(.switch)
          .labelsHidden()
          .tint(CSColor.chromeAccent)
          .accessibilityLabel("Recording start sound")
          .accessibilityValue(model.settings.beepOnStart ? "On" : "Off")
      }

      VStack(alignment: .leading, spacing: 7) {
        HStack {
          Text("Volume")
            .font(CSFont.ui(12.5, .medium))
            .foregroundStyle(CSColor.textMutedAlt)
          Spacer(minLength: 0)
          Text("\(Int((model.settings.soundVolume * 100).rounded()))%")
            .font(CSFont.mono(10.5, .semibold))
            .foregroundStyle(CSColor.textBody)
        }
        Slider(value: soundVolumeBinding, in: 0...1, step: 0.05)
          .tint(CSColor.chromeAccent)
          .disabled(!model.settings.beepOnStart)
          .accessibilityLabel("Recording start sound volume")
          .accessibilityValue("\(Int((model.settings.soundVolume * 100).rounded())) percent")
      }
    }
    .padding(15)
    .background(card)
    .overlay(cardBorder)
  }

  private var deviceOptions: [String] {
    var devices = model.audioInput.devices
    if let configured = model.audioInput.configuredDevice,
      !devices.contains(configured)
    {
      devices.insert(configured, at: 0)
    }
    return devices
  }

  private var inputDeviceBinding: Binding<String> {
    Binding(
      get: { model.settings.audioInputDevice ?? Self.systemDefaultChoice },
      set: { choice in
        if choice == Self.systemDefaultChoice {
          model.resetAudioInputDevice()
        } else {
          model.setAudioInputDevice(choice)
        }
      }
    )
  }

  private var soundFeedbackBinding: Binding<Bool> {
    Binding(
      get: { model.settings.beepOnStart },
      set: { model.setSoundFeedbackEnabled($0) }
    )
  }

  private var sealLaneBinding: Binding<Bool> {
    Binding(
      get: { sealLaneControlState(model.admission).isOn },
      set: { armed in
        model.setSealLaneArmed(armed)
        Task { await model.refreshAdmission() }
      }
    )
  }

  private var soundVolumeBinding: Binding<Double> {
    Binding(
      get: { Double(model.settings.soundVolume) },
      set: { model.setSoundVolume(Float($0)) }
    )
  }

  private var inputDeviceAccessibilityValue: String {
    model.settings.audioInputDevice ?? "System default"
  }

  private func sealLaneAccessibilityValue(_ state: SealLaneControlState) -> String {
    let value = state.isOn ? "On" : "Off"
    return state.isEnabled ? value : "\(value), overridden"
  }

  private func statusRow(color: Color, title: String, detail: String) -> some View {
    HStack(alignment: .top, spacing: 9) {
      Circle().fill(color).frame(width: 7, height: 7).padding(.top, 4)
      VStack(alignment: .leading, spacing: 3) {
        Text(title)
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(CSColor.textBody)
        Text(detail)
          .font(CSFont.ui(11.5))
          .lineSpacing(2)
          .foregroundStyle(CSColor.textMutedAlt)
      }
      Spacer(minLength: 0)
    }
    .padding(12)
    .background(CSColor.surfaceRaised(0.03))
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
  }

  private func statusColor(_ tone: AudioInputDisplayTone) -> Color {
    switch tone {
    case .healthy: return CSColor.oliveLight
    case .fallback: return CSColor.amber
    case .unavailable: return CSColor.terracottaLight
    }
  }

  private var card: some ShapeStyle {
    CSColor.surfaceRaised(0.025)
  }

  private var cardBorder: some View {
    RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
      .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
  }
}

#if DEBUG
  #Preview("Settings — Audio") {
    SettingsView(model: SettingsViewModel.preview(.audio))
      .frame(width: 960, height: 720)
  }
#endif
