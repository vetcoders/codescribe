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

/// One technical fact behind the readiness rows, shown only under the
/// disclosure. The rows above say whether recording works; these say with which
/// stored measurement, for the rare case where that matters.
struct AudioCalibrationDetail: Identifiable, Equatable {
  let id: String
  let label: String
  let value: String
}

/// Reads the measured profile out of the controller verdict. Nothing here is
/// computed: a missing field is an omitted row, never an invented value.
func audioCalibrationDetails(_ readiness: CsAdmissionReadiness?) -> [AudioCalibrationDetail] {
  guard let readiness else { return [] }
  var details: [AudioCalibrationDetail] = []
  if let device = readiness.deviceName, !device.isEmpty {
    details.append(
      AudioCalibrationDetail(
        id: "device", label: String(localized: "Measured microphone"), value: device))
  }
  if let rate = readiness.sampleRate, rate > 0 {
    details.append(
      AudioCalibrationDetail(
        id: "sampleRate",
        label: String(localized: "Sample rate"),
        value: String(
          localized: "\(rate.formatted()) Hz",
          comment: "The placeholder is an audio sample rate in hertz")
      ))
  }
  if let version = readiness.calibrationVersion, !version.isEmpty {
    details.append(
      AudioCalibrationDetail(
        id: "profile", label: String(localized: "Calibration profile"), value: version))
  }
  if !readiness.calibrationStatus.isEmpty {
    details.append(
      AudioCalibrationDetail(
        id: "status",
        label: String(localized: "Calibration state"),
        value: readiness.calibrationStatus))
  }
  if !readiness.calibrationPath.isEmpty {
    details.append(
      AudioCalibrationDetail(
        id: "path",
        label: String(localized: "Calibration file"),
        value: readiness.calibrationPath))
  }
  return details
}

/// The retention sentence states what the SELECTED choice does. The choices are
/// not variants of one rule: a finite age expires completed takes once they are
/// past it, while `off` discards only takes captured while it is selected, so
/// they cannot share a sentence. Unknown values resolve to `forever`, exactly
/// as the config loader does.
func audioRetentionDetail(_ choice: String) -> String {
  switch choice {
  case "off":
    return String(
      localized:
        "Audio from a new recording is discarded as soon as processing finishes. Recordings already saved are kept, and text history stays available."
    )
  case "24h":
    return String(
      localized:
        "Audio from a completed recording is deleted 24 hours after it finishes. Text history stays available."
    )
  case "7_days":
    return String(
      localized:
        "Audio from a completed recording is deleted 7 days after it finishes. Text history stays available."
    )
  case "30_days":
    return String(
      localized:
        "Audio from a completed recording is deleted 30 days after it finishes. Text history stays available."
    )
  default:
    return String(
      localized:
        "Audio from completed recordings stays on this Mac until you delete it. Nothing expires automatically."
    )
  }
}

/// Stable four-step projection of recording readiness. The bridge verdict is
/// still authoritative; this only makes its prerequisites visible together so
/// users do not discover them one failed take at a time.
func audioReadinessSteps(
  input: CsAudioInputSnapshot,
  microphonePermission: PermissionState,
  admission: CsAdmissionReadiness?,
  dictationShortcut: String?,
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
    if admission.calibrationVersion != nil, admission.calibrationStatus == "sealed" {
      // The stored profile identifier stays available under Calibration
      // details; the row itself answers whether recording can rely on it.
      calibration = AudioReadinessStep(
        id: .calibration,
        tone: .healthy,
        title: String(localized: "Microphone calibrated"),
        detail: String(
          localized: "Codescribe has measured how loudly you speak into this microphone.")
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

  // One name across every state of this row: the user should not have to learn
  // that "seal check" and "committing fragments" are the same prerequisite.
  let sealLaneTitle = String(localized: "Committing transcript fragments")
  let sealLane: AudioReadinessStep
  if microphonePermission != .granted {
    sealLane = AudioReadinessStep(
      id: .sealLane,
      tone: .fallback,
      title: sealLaneTitle,
      detail: String(localized: "Complete step 1 before checking this.")
    )
  } else if let admission {
    if admission.code == "admission_seal_vad_unavailable" {
      sealLane = AudioReadinessStep(
        id: .sealLane,
        tone: .unavailable,
        title: String(localized: "Silero VAD did not load"),
        detail: admission.message
      )
    } else {
      // One title plus the readable switch state; the sentence also says who
      // owns the switch, because an override keeps Settings read-only.
      let detail: String
      switch (admission.sealLaneSource == "env_override", admission.sealLaneArmed) {
      case (true, true):
        detail = String(
          localized: "On, set by the \(admission.sealLaneEnv) override; the switch is read-only.",
          comment: "The placeholder is an environment variable name")
      case (true, false):
        detail = String(
          localized:
            "Off, set by the \(admission.sealLaneEnv) override. Recording stays blocked until the override is removed.",
          comment: "The placeholder is an environment variable name")
      case (false, true):
        detail = String(localized: "On. Change it with the switch in this row.")
      case (false, false):
        detail = String(localized: "Off. Recording cannot start until you turn this on.")
      }
      sealLane = AudioReadinessStep(
        id: .sealLane,
        tone: admission.sealLaneArmed ? .healthy : .unavailable,
        title: sealLaneTitle,
        detail: detail
      )
    }
  } else {
    sealLane = AudioReadinessStep(
      id: .sealLane,
      tone: .fallback,
      title: sealLaneTitle,
      detail: String(localized: "Reading the current setting…")
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
        ? recordingStartHint(dictationShortcut)
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

/// Names the two real ways to start a take. Without a configured gesture the
/// sentence must not send the user after a shortcut that does not exist, so the
/// dynamic name appears only when Hotkeys actually binds one.
func recordingStartHint(_ dictationShortcut: String?) -> String {
  guard let dictationShortcut, !dictationShortcut.isEmpty else {
    return String(
      localized:
        "Click Start recording. You can also set a Dictation shortcut under Settings › Hotkeys.")
  }
  return String(
    localized: "Press \(dictationShortcut) or click Start recording.",
    comment: "The placeholder is the configured dictation gesture")
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
      detail: String(
        localized: "Codescribe records on whichever microphone macOS currently uses.")
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
  @State private var showingCalibrationDetails = false

  private static let systemDefaultChoice = "__codescribe_system_default__"

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(alignment: .top, spacing: 12) {
        VStack(alignment: .leading, spacing: 0) {
          SettingsPageHeader(
            String(localized: "Microphone and recording"),
            blurb: String(
              localized:
                "Choose a microphone, check that recording is ready and adjust the sound settings."
            )
          )
        }
        Spacer(minLength: 0)
        Button("Refresh microphones") {
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
        // The sentence follows the selected choice: "Forever" must not carry
        // the warning that belongs to "Off".
        Text(audioRetentionDetail(model.audioRetention))
          .font(CSFont.ui(12))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
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
          localized:
            "Codescribe records on this microphone. If it is unplugged, recording continues on the system microphone."
        )
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

      // The button names what it does; the sentence that repeated it is gone.
      HStack {
        Spacer(minLength: 0)
        Button("Use the system microphone") {
          model.resetAudioInputDevice()
        }
        .csFocusRing()
        .font(CSFont.mono(10.5, .semibold))
        .foregroundStyle(CSColor.chromeAccent)
        .disabled(model.settings.audioInputDevice == nil)
        .accessibilityHint("Clears the saved microphone and follows the macOS input device")
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

      calibrationDetails
    }
    .settingsGroupedInset()
  }

  /// The stored measurement, kept out of the four rows above. Collapsed by
  /// default: the rows answer whether recording works, this answers with which
  /// profile, which only matters when a measurement is in question.
  @ViewBuilder
  private var calibrationDetails: some View {
    let details = audioCalibrationDetails(model.admission)
    if !details.isEmpty {
      DisclosureGroup(isExpanded: $showingCalibrationDetails) {
        VStack(alignment: .leading, spacing: 6) {
          ForEach(details) { detail in
            // Same shape as the Whisper model details: label above a verbatim,
            // selectable value. A stored path is never a localization key.
            VStack(alignment: .leading, spacing: 1) {
              Text(verbatim: detail.label)
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(Color.secondary)
              Text(verbatim: detail.value)
                .font(CSFont.mono(10, .medium))
                .foregroundStyle(Color.secondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(detail.label)
            .accessibilityValue(detail.value)
          }
        }
        .padding(.top, CSSpace.xs)
      } label: {
        SettingsSectionLabel(String(localized: "Calibration details"))
      }
      .padding(.top, CSSpace.xs)
      .accessibilityIdentifier("audio-calibration-details")
    }
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
      Toggle("Committing transcript fragments", isOn: sealLaneBinding)
        .toggleStyle(.switch)
        .labelsHidden()
        .tint(CSColor.chromeAccent)
        .disabled(!sealLane.isEnabled)
        .accessibilityLabel("Committing transcript fragments")
        .accessibilityValue(sealLaneAccessibilityValue(sealLane))
        .accessibilityHint(
          sealLane.isEnabled
            ? String(
              localized:
                "Codescribe must be able to commit finished fragments before a recording can start."
            )
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

  /// Nil when Hotkeys binds no Dictation gesture: the readiness row then names
  /// the button instead of a shortcut the user does not have.
  private var dictationShortcutLabel: String? {
    model.modeBindings.first { $0.mode == .dictation }?.binding.visibleName
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
        title: String(localized: "Recording start signal"),
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
