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
  case recording

  var id: Int { rawValue }
}

struct AudioReadinessStep: Identifiable, Equatable {
  let id: AudioReadinessStepID
  let tone: AudioInputDisplayTone
  let title: String
  let detail: String
}

/// Stable four-step projection of recording readiness. The bridge verdict is
/// still authoritative; this only makes its prerequisites visible together so
/// users do not discover them one failed take at a time.
func audioReadinessSteps(
  input: CsAudioInputSnapshot,
  microphonePermission: PermissionState,
  admission: CsAdmissionReadiness?,
  dictationShortcut: String,
  recording isRecording: Bool? = false,
  preparing: Bool = false,
  processing: Bool = false
) -> [AudioReadinessStep] {
  let microphone: AudioInputDisplayState
  switch microphonePermission {
  case .granted:
    microphone = audioInputDisplayState(input)
  case .notDetermined:
    microphone = AudioInputDisplayState(
      tone: .fallback,
      title: String(localized: "Allow microphone access"),
      detail: String(
        localized: "macOS has not granted Codescribe access to the selected input yet.")
    )
  case .denied:
    microphone = AudioInputDisplayState(
      tone: .unavailable,
      title: String(localized: "Microphone access is off"),
      detail: String(
        localized: "Enable Codescribe in System Settings › Privacy & Security › Microphone.")
    )
  }

  let calibration: AudioReadinessStep
  if microphonePermission != .granted {
    calibration = AudioReadinessStep(
      id: .calibration,
      tone: .fallback,
      title: String(localized: "Calibration waits for microphone access"),
      detail: String(localized: "Complete step 1 before measuring this input.")
    )
  } else if let admission {
    if let version = admission.calibrationVersion, admission.calibrationStatus == "sealed" {
      calibration = AudioReadinessStep(
        id: .calibration,
        tone: .healthy,
        title: String(localized: "Microphone calibrated"),
        detail: version
      )
    } else {
      calibration = AudioReadinessStep(
        id: .calibration,
        tone: .unavailable,
        title: String(localized: "Calibration required"),
        detail: String(
          localized: "Measure about 10 seconds of normal speech on the current microphone.")
      )
    }
  } else {
    calibration = AudioReadinessStep(
      id: .calibration,
      tone: .fallback,
      title: String(localized: "Checking calibration…"),
      detail: String(localized: "Reading the controller's measured profile.")
    )
  }

  let sealLane: AudioReadinessStep
  if microphonePermission != .granted {
    sealLane = AudioReadinessStep(
      id: .sealLane,
      tone: .fallback,
      title: String(localized: "Seal check waits for microphone access"),
      detail: String(localized: "Complete step 1 before validating the acoustic lane.")
    )
  } else if let admission {
    let source =
      admission.sealLaneSource == "env_override"
      ? String(
        localized: "Controlled by \(admission.sealLaneEnv) override.",
        comment: "The placeholder is an environment variable name")
      : String(localized: "Controlled by the product setting below.")
    if admission.code == "admission_seal_vad_unavailable" {
      sealLane = AudioReadinessStep(
        id: .sealLane,
        tone: .unavailable,
        title: String(localized: "Silero VAD did not load"),
        detail: admission.message
      )
    } else {
      sealLane = AudioReadinessStep(
        id: .sealLane,
        tone: admission.sealLaneArmed ? .healthy : .unavailable,
        title: admission.sealLaneArmed
          ? String(localized: "Seal lane armed")
          : String(localized: "Seal lane must be enabled"),
        detail: source
      )
    }
  } else {
    sealLane = AudioReadinessStep(
      id: .sealLane,
      tone: .fallback,
      title: String(localized: "Checking seal lane…"),
      detail: String(localized: "Reading the effective product setting and override.")
    )
  }

  let recording: AudioReadinessStep
  if processing {
    recording = AudioReadinessStep(
      id: .recording,
      tone: .fallback,
      title: String(localized: "Finishing recording…"),
      detail: String(
        localized: "The shared recorder is processing this take. Wait before starting another.")
    )
  } else if preparing {
    recording = AudioReadinessStep(
      id: .recording,
      tone: .fallback,
      title: String(localized: "Starting recording…"),
      detail: String(localized: "The shared recorder is preparing this take.")
    )
  } else if isRecording == true {
    recording = AudioReadinessStep(
      id: .recording,
      tone: .healthy,
      title: String(localized: "Recording in progress"),
      detail: String(
        localized: "Choose Stop recording or use your recording shortcut to finish this take.")
    )
  } else if isRecording == nil {
    recording = AudioReadinessStep(
      id: .recording,
      tone: .fallback,
      title: String(localized: "Checking recording activity…"),
      detail: String(localized: "Waiting for the shared recorder's lifecycle.")
    )
  } else if microphonePermission != .granted {
    recording = AudioReadinessStep(
      id: .recording,
      tone: .unavailable,
      title: String(localized: "Grant microphone access first"),
      detail: String(localized: "Recording stays disabled until step 1 is complete.")
    )
  } else if let admission {
    recording = AudioReadinessStep(
      id: .recording,
      tone: admission.ready ? .healthy : .unavailable,
      title: admission.ready
        ? String(localized: "Ready to record")
        : String(localized: "Finish setup above"),
      detail: admission.ready
        ? String(
          localized: "Use \(dictationShortcut) or choose Start recording.",
          comment: "The placeholder is the configured dictation gesture")
        : admission.message
    )
  } else {
    recording = AudioReadinessStep(
      id: .recording,
      tone: .fallback,
      title: String(localized: "Checking recording readiness…"),
      detail: String(localized: "Waiting for the controller's admission verdict.")
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
    recording,
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
      detail: String(localized: "Reading the product setting and any power-user override.")
    )
  }
  guard readiness.sealLaneSource == "env_override" else {
    return SealLaneControlState(
      isOn: readiness.sealLaneSettingArmed,
      isEnabled: true,
      detail: String(localized: "Required for committed utterances; stored in Settings.")
    )
  }
  let detail =
    readiness.sealLaneArmed
    ? String(
      localized:
        "Product setting is read-only while \(readiness.sealLaneEnv) keeps the lane armed. Remove the power-user override to edit here.",
      comment: "The placeholder is an environment variable name")
    : String(
      localized:
        "Product setting is read-only while \(readiness.sealLaneEnv) keeps the lane disarmed. Remove the power-user override to edit here.",
      comment: "The placeholder is an environment variable name")
  return SealLaneControlState(
    isOn: readiness.sealLaneSettingArmed,
    isEnabled: false,
    detail: detail
  )
}

/// Pure UI projection of the controller's admission verdict for XCTest. The
/// bridge record already carries the one blocker the controller would apply;
/// this function only words it — it never decides readiness itself.
func admissionDisplayState(_ readiness: CsAdmissionReadiness?) -> AudioInputDisplayState {
  guard let readiness else {
    return AudioInputDisplayState(
      tone: .fallback,
      title: String(localized: "Checking acoustic admission…"),
      detail: String(localized: "Reading the controller's calibration and seal-lane verdict.")
    )
  }
  if readiness.ready {
    let device = readiness.deviceName ?? String(localized: "input device")
    let version = readiness.calibrationVersion ?? String(localized: "measured profile")
    return AudioInputDisplayState(
      tone: .healthy,
      title: String(
        localized: "Ready to record on \(device)",
        comment: "The placeholder is an input device name"),
      detail: String(
        localized: "Calibration \(version); seal lane armed.",
        comment: "The placeholder is a calibration profile version")
    )
  }
  let title: String
  switch readiness.code {
  case "admission_calibration_missing":
    title = String(localized: "Microphone not calibrated yet")
  case "admission_calibration_no_profile":
    title = String(localized: "No calibration for the current microphone")
  case "admission_calibration_refused", "admission_calibration_unusable":
    title = String(localized: "Stored calibration cannot be used")
  case "admission_seal_lane_disarmed":
    title =
      readiness.sealLaneSource == "env_override"
      ? String(
        localized: "Seal lane is disarmed by \(readiness.sealLaneEnv) override",
        comment: "The placeholder is an environment variable name")
      : String(localized: "Seal lane is off in Settings › Audio")
  case "admission_seal_vad_unavailable":
    title = String(localized: "Silero VAD did not load")
  case "admission_capture_device_unavailable":
    title = String(localized: "No input device available")
  default:
    title = String(localized: "Recording cannot start")
  }
  return AudioInputDisplayState(tone: .unavailable, title: title, detail: readiness.message)
}

/// Pure UI projection for XCTest. The bridge snapshot already contains the
/// live cpal resolution; this function never re-resolves a configured wish.
func audioInputDisplayState(_ snapshot: CsAudioInputSnapshot) -> AudioInputDisplayState {
  guard let runtimeDevice = snapshot.runtimeDevice, !runtimeDevice.isEmpty else {
    return AudioInputDisplayState(
      tone: .unavailable,
      title: String(localized: "No input device available"),
      detail: String(localized: "Connect a microphone and refresh Audio settings.")
    )
  }

  if !snapshot.runtimeConfigurationMatches {
    let saved = snapshot.configuredDevice ?? String(localized: "System default")
    return AudioInputDisplayState(
      tone: .fallback,
      title: String(
        localized: "Currently using: \(runtimeDevice)",
        comment: "The placeholder is an input device name"),
      detail: String(
        localized:
          "Saved: \(saved). Restart Codescribe to apply it; an explicit AUDIO_INPUT_DEVICE launch override can keep a different runtime input active.",
        comment: "The placeholder is the saved input device name")
    )
  }

  if snapshot.fallbackToDefault {
    let missing = snapshot.configuredDevice ?? String(localized: "The configured input")
    return AudioInputDisplayState(
      tone: .fallback,
      title: String(
        localized: "Using system fallback: \(runtimeDevice)",
        comment: "The placeholder is an input device name"),
      detail: String(
        localized: "\(missing) is unavailable. Recording continues on the live default input.",
        comment: "The placeholder is the configured input device name")
    )
  }

  if snapshot.configuredDevice == nil {
    return AudioInputDisplayState(
      tone: .healthy,
      title: String(
        localized: "System default: \(runtimeDevice)",
        comment: "The placeholder is an input device name"),
      detail: String(localized: "The recorder resolves this device from Core Audio at runtime.")
    )
  }

  return AudioInputDisplayState(
    tone: .healthy,
    title: String(
      localized: "Runtime input: \(runtimeDevice)",
      comment: "The placeholder is an input device name"),
    detail: String(localized: "The configured device is present and selected by the recorder.")
  )
}

struct AudioPanel: View {
  @ObservedObject var model: SettingsViewModel
  @State private var recordingControls: (state: OverlayState, tray: TrayViewModel)?

  private static let systemDefaultChoice = "__codescribe_system_default__"

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(alignment: .top, spacing: 12) {
        VStack(alignment: .leading, spacing: 0) {
          SettingsPageHeader(
            String(localized: "Hear the real input."),
            blurb: String(
              localized: "Device choice and sound feedback use the live recorder config.")
          )
        }
        Spacer(minLength: 0)
        Button("Refresh") {
          model.refreshAudioInput()
        }
        .csFocusRing()
        .font(CSFont.mono(11, .semibold))
        .foregroundStyle(CSColor.chromeAccent)
        .accessibilityLabel("Refresh audio input devices")
      }

      SettingsSectionLabel(String(localized: "Input device"))
        .padding(.top, CSSpace.section)
        .id(SettingsAnchor.audioInput)
      inputDeviceSection
        .padding(.top, CSSpace.control)

      SettingsSectionLabel(String(localized: "Recording readiness"))
        .padding(.top, CSSpace.section)
        .id(SettingsAnchor.audioReadiness)
      admissionSection
        .padding(.top, CSSpace.control)
        .task {
          recordingControls = model.audioRecordingControls()
          await model.refreshAdmission()
        }

      SettingsSectionLabel(String(localized: "Audio retention"))
        .padding(.top, CSSpace.section)
      VStack(alignment: .leading, spacing: CSSpace.control) {
        Picker("Keep completed recordings", selection: Binding(
          get: { model.audioRetention },
          set: { model.setAudioRetention($0) }
        )) {
          Text("Forever").tag("forever")
          Text("30 days").tag("30_days")
          Text("7 days").tag("7_days")
          Text("24h").tag("24h")
          Text("Off").tag("off")
        }
        .pickerStyle(.menu)
        Text("Off discards the audio of new takes once processing finishes. Text history stays available. A take already in progress keeps the choice it started with.")
          .font(CSFont.ui(12))
          .foregroundStyle(Color.secondary)
      }
      .padding(.top, CSSpace.control)

      SettingsSectionLabel(String(localized: "Sound feedback"))
        .padding(.top, CSSpace.section)
      feedbackSection
        .padding(.top, CSSpace.control)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
  }

  private var inputDeviceSection: some View {
    VStack(alignment: .leading, spacing: 14) {
      SettingsControlRow(
        title: String(localized: "Microphone"),
        subtitle: String(
          localized: "Saved in settings.json; runtime falls back safely if it disappears")
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
        Text("Restores the system default microphone.")
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(Color.secondary)
        Spacer(minLength: 12)
        Button("Use system default") {
          model.resetAudioInputDevice()
        }
        .csFocusRing()
        .font(CSFont.mono(10.5, .semibold))
        .foregroundStyle(CSColor.chromeAccent)
        .disabled(model.settings.audioInputDevice == nil)
        .accessibilityLabel("Reset audio input to system default")
      }
    }
    .settingsGroupedInset()
  }

  /// The controller's precondition for any take, and the one operator step
  /// that can satisfy it locally: a ~10 s guided measurement through the real
  /// recorder path. No value is ever invented here.
  private var admissionSection: some View {
    VStack(alignment: .leading, spacing: 14) {
      if let recordingControls {
        AudioReadinessObserver(
          recordingState: recordingControls.state, tray: recordingControls.tray
        ) { state, tray, stopRequested in
          readinessCockpit(recordingState: state, tray: tray, stopRequested: stopRequested)
        }
      } else {
        readinessCockpit(recordingState: nil, tray: nil, stopRequested: .constant(false))
      }

      if let notice = model.calibrationNotice {
        Text(notice)
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
          .accessibilityLabel("Calibration result")
      }
    }
    .settingsGroupedInset()
  }

  private func readinessCockpit(
    recordingState: OverlayState?, tray: TrayViewModel?, stopRequested: Binding<Bool>
  ) -> some View {
    VStack(alignment: .leading, spacing: 0) {
      if let error = model.admissionReadError {
        statusRow(
          color: CSColor.terracotta,
          title: "Readiness check unavailable",
          detail: error
        )
        .padding(.bottom, 6)
      }

      ForEach(
        audioReadinessSteps(
          input: model.audioInput,
          microphonePermission: model.permissions.microphone,
          admission: model.admission,
          dictationShortcut: dictationShortcutLabel,
          recording: recordingState?.recording,
          preparing: recordingState?.warmingUp == true,
          processing: recordingProcessing(recordingState)
        )
      ) { step in
        readinessStep(
          step, recordingState: recordingState, tray: tray, stopRequested: stopRequested
        )
        if step.id != .recording {
          Divider().overlay(Color.primary.opacity(0.12))
        }
      }
    }
    .padding(CSSpace.md)
    .background(Color.primary.opacity(0.06))
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Recording readiness")
  }

  private func readinessStep(
    _ step: AudioReadinessStep,
    recordingState: OverlayState?, tray: TrayViewModel?, stopRequested: Binding<Bool>
  ) -> some View {
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
          Text(verbatim: "\(step.id.rawValue + 1)")
            .font(CSFont.mono(10, .semibold))
            .foregroundStyle(statusColor(step.tone))
        }
      }
      .accessibilityHidden(true)
      VStack(alignment: .leading, spacing: 3) {
        Text(step.title)
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(Color.primary)
        Text(step.detail)
          .font(CSFont.ui(11.5))
          .lineSpacing(2)
          .foregroundStyle(Color.secondary)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      .accessibilityElement(children: .ignore)
      .accessibilityLabel("Step \(step.id.rawValue + 1), \(step.title)")
      .accessibilityValue(step.detail)
      readinessControl(
        for: step, recordingState: recordingState, tray: tray, stopRequested: stopRequested
      )
    }
    .padding(.vertical, 9)
    .accessibilityElement(children: .contain)
  }

  @ViewBuilder
  private func readinessControl(
    for step: AudioReadinessStep,
    recordingState: OverlayState?, tray: TrayViewModel?, stopRequested: Binding<Bool>
  ) -> some View {
    switch step.id {
    case .microphone:
      if model.permissions.microphone != .granted {
        Button(microphonePermissionActionTitle) {
          resolveMicrophonePermission()
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .accessibilityHint("Grants Codescribe access to the microphone selected above")
      }
    case .calibration:
      VStack(alignment: .trailing, spacing: 5) {
        if model.calibrationPending, let startedAt = model.calibrationStartedAt {
          TimelineView(.periodic(from: .now, by: 0.2)) { context in
            let elapsed = context.date.timeIntervalSince(startedAt)
            VStack(alignment: .trailing, spacing: 3) {
              ProgressView(
                value: SettingsViewModel.calibrationProgress(elapsedSeconds: elapsed)
              )
              .progressViewStyle(.linear)
              .frame(width: 92)
              Text(
                "\(SettingsViewModel.calibrationRemainingSeconds(elapsedSeconds: elapsed)) s left"
              )
              .font(CSFont.mono(9.5, .medium))
              .foregroundStyle(Color.secondary)
            }
          }
          .accessibilityLabel("Calibration capture progress")
        }
        Button(model.calibrationPending ? "Measuring…" : "Calibrate") {
          Task { await calibrateMicrophone(recordingState, tray: tray) }
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .disabled(!canCalibrateMicrophone(recordingState, tray: tray))
        .accessibilityLabel("Calibrate microphone")
        .accessibilityHint("Measures about ten seconds of normal speech; audio is not kept")
      }
    case .sealLane:
      let sealLane = sealLaneControlState(model.admission)
      Toggle("Seal lane", isOn: sealLaneBinding)
        .toggleStyle(.switch)
        .labelsHidden()
        .tint(CSColor.chromeAccent)
        .disabled(!sealLane.isEnabled)
        .accessibilityLabel("Seal lane")
        .accessibilityValue(sealLaneAccessibilityValue(sealLane))
        .accessibilityHint(
          sealLane.isEnabled
            ? String(localized: "Controls whether committed utterances can be sealed.")
            : sealLane.detail
        )
    case .recording:
      if recordingProcessing(recordingState) {
        ProgressView()
          .controlSize(.small)
          .accessibilityLabel("Finishing recording")
      } else if recordingState?.recording == true {
        Button("Stop recording") {
          guard !stopRequested.wrappedValue else { return }
          stopRequested.wrappedValue = true
          recordingState?.stop()
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.small)
        .tint(CSColor.chromeAccent)
        .disabled(
          stopRequested.wrappedValue || recordingState?.warmingUp == true
            || recordingState?.transcribing == true || model.calibrationPending
        )
        .accessibilityHint("Stops the active take in the shared recorder")
        .accessibilityIdentifier("audio-readiness-stop-recording")
      } else {
        Button("Start recording") {
          startRecording(recordingState, tray: tray)
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.small)
        .tint(CSColor.chromeAccent)
        .disabled(!canStartRecording(recordingState, tray: tray))
        .accessibilityHint("Starts a real dictation session in the shared recorder")
        .accessibilityIdentifier("audio-readiness-start-recording")
      }
    }
  }

  /// Same admission for the real calibration button and its action.
  func canCalibrateMicrophone(
    _ recordingState: OverlayState?, tray: TrayViewModel?
  ) -> Bool {
    !model.calibrationPending && recordingState?.recording == false
      && tray?.isRecording == false && tray?.isStartingDictation == false
      && !recordingProcessing(recordingState)
      && model.permissions.microphone == .granted && model.audioInput.runtimeDevice != nil
  }

  func calibrateMicrophone(_ recordingState: OverlayState?, tray: TrayViewModel?) async {
    guard canCalibrateMicrophone(recordingState, tray: tray) else { return }
    await model.runCalibration()
  }

  /// Same action for the real button and integrator interaction witnesses.
  func startRecording(_ recordingState: OverlayState?, tray: TrayViewModel?) {
    guard canStartRecording(recordingState, tray: tray) else { return }
    // The tray admits the next capture before starting the shared controller.
    tray?.toggleDictation()
  }

  func canStartRecording(
    _ recordingState: OverlayState?, tray: TrayViewModel?
  ) -> Bool {
    model.permissions.microphone == .granted && model.admission?.ready == true
      && model.admissionReadError == nil && !model.calibrationPending
      && recordingState?.recording == false && recordingState?.warmingUp == false
      && tray?.isRecording == false && tray?.isStartingDictation == false
      && !recordingProcessing(recordingState)
  }

  func recordingProcessing(_ recordingState: OverlayState?) -> Bool {
    recordingState?.transcribing == true
      || recordingState?.isFinalPass == true
      || (recordingState?.mode == .finalizing && recordingState?.terminal == false)
  }

  private var dictationShortcutLabel: String {
    model.modeBindings.first { $0.mode == .dictation }?.binding.visibleName
      ?? String(localized: "your Dictation shortcut")
  }

  private var microphonePermissionActionTitle: String {
    model.permissions.microphone == .notDetermined
      ? String(
        localized: "audio.microphone.permission.allow", defaultValue: "Allow",
        comment: "Button: grant the microphone permission now")
      : String(localized: "System Settings", comment: "Button: open the System Settings pane")
  }

  private func resolveMicrophonePermission() {
    let kind = PermissionKind.microphone
    if model.permissions.microphone == .notDetermined {
      Task { @MainActor in
        _ = await kind.requestInApp()
        model.refresh()
      }
    } else {
      kind.openSystemSettings()
    }
  }

  private var feedbackSection: some View {
    VStack(alignment: .leading, spacing: 14) {
      SettingsControlRow(
        title: String(localized: "Start sound"),
        subtitle: String(localized: "Play the recorder's live start confirmation")
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
            .foregroundStyle(Color.secondary)
          Spacer(minLength: 0)
          Text(verbatim: "\(Int((model.settings.soundVolume * 100).rounded()))%")
            .font(CSFont.mono(10.5, .semibold))
            .foregroundStyle(Color.primary)
        }
        Slider(value: soundVolumeBinding, in: 0...1, step: 0.05)
          .tint(CSColor.chromeAccent)
          .disabled(!model.settings.beepOnStart)
          .accessibilityLabel("Recording start sound volume")
          .accessibilityValue("\(Int((model.settings.soundVolume * 100).rounded())) percent")
      }
    }
    .settingsGroupedInset()
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
    model.settings.audioInputDevice ?? String(localized: "System default")
  }

  private func sealLaneAccessibilityValue(_ state: SealLaneControlState) -> String {
    let value =
      state.isOn
      ? String(localized: "On", comment: "Toggle state")
      : String(localized: "Off", comment: "Toggle state")
    return state.isEnabled
      ? value
      : String(
        localized: "\(value), overridden",
        comment: "VoiceOver value: a toggle state frozen by an override")
  }

  private func statusRow(color: Color, title: LocalizedStringKey, detail: String) -> some View {
    HStack(alignment: .top, spacing: 9) {
      Circle().fill(color).frame(width: 7, height: 7).padding(.top, 4)
      VStack(alignment: .leading, spacing: 3) {
        Text(title)
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(Color.primary)
        Text(detail)
          .font(CSFont.ui(11.5))
          .lineSpacing(2)
          .foregroundStyle(Color.secondary)
      }
      Spacer(minLength: 0)
    }
    .padding(CSSpace.md)
    .background(Color.primary.opacity(0.06))
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
  }

  private func statusColor(_ tone: AudioInputDisplayTone) -> Color {
    switch tone {
    case .healthy: return CSColor.oliveLight
    case .fallback: return CSColor.amber
    case .unavailable: return CSColor.terracotta
    }
  }

}

/// Observe lifecycle and command admission in the same readiness consumer.
/// The latch only suppresses repeated Stop clicks; both capture owners are borrowed.
struct AudioReadinessObserver<Content: View>: View {
  @Bindable var recordingState: OverlayState
  @ObservedObject var tray: TrayViewModel
  @State private var stopRequested = false
  let content: (OverlayState, TrayViewModel, Binding<Bool>) -> Content

  var body: some View {
    content(recordingState, tray, $stopRequested)
      .onChange(of: recordingState.recording) { _, recording in
        if !recording { stopRequested = false }
      }
      .onChange(of: recordingState.transcribing) { _, processing in
        if !processing { stopRequested = false }
      }
  }
}

#if DEBUG
  #Preview("Settings — Audio") {
    SettingsView(model: SettingsViewModel.preview(.audio))
      .frame(width: 960, height: 720)
  }
#endif
