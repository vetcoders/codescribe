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

/// The one remedy the readiness line offers for the prerequisite it names.
/// `none` is the normal case: a neutral line carries no button.
enum AudioReadinessRemedy: Equatable {
  case none
  case microphonePermission
  case calibrate
  case openLab
}

/// The single status line of Recording readiness. `detail` and `remedy` are
/// present ONLY while something blocks recording — a neutral line never
/// explains a working state, and it must never mask a blocked one.
struct AudioReadinessSummary: Equatable {
  let tone: AudioInputDisplayTone
  let title: String
  let detail: String?
  let remedy: AudioReadinessRemedy
}

/// One short prerequisite line under the status: a name and its current value.
/// Everything that is not the value belongs to the status line above it.
struct AudioReadinessFact: Identifiable, Equatable {
  let id: AudioReadinessStepID
  let label: String
  let value: String
  let tone: AudioInputDisplayTone
}

/// The microphone card: which input actually records, plus the one notice that
/// applies — a missing device or a saved device the running recorder is not
/// using. A healthy input carries no sentence at all.
struct AudioInputCardState: Equatable {
  let current: String
  let notice: String?
  let noticeTone: AudioInputDisplayTone?
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

/// The retention sentence states what the SELECTED choice does, and only when
/// the choice actually expires something. `Forever` is self-explanatory, so it
/// carries no sentence; unknown values resolve to `forever`, exactly as the
/// config loader does. The finite ages and `off` are not variants of one rule —
/// an age expires completed takes, `off` discards takes captured while it is
/// selected — so they cannot share a sentence. Every sentence keeps the
/// distinction between recorded audio and text history.
func audioRetentionDetail(_ choice: String) -> String? {
  switch choice {
  case "off":
    return String(
      localized: "Audio is discarded as soon as processing finishes. Text history stays.")
  case "24h":
    return String(
      localized: "Audio is deleted 24 hours after a recording finishes. Text history stays.")
  case "7_days":
    return String(
      localized: "Audio is deleted 7 days after a recording finishes. Text history stays.")
  case "30_days":
    return String(
      localized: "Audio is deleted 30 days after a recording finishes. Text history stays.")
  default:
    return nil
  }
}

/// Stable four-step projection of recording readiness. The bridge verdict is
/// still authoritative; this only makes its prerequisites visible together so
/// users do not discover them one failed take at a time. The panel reads this
/// through `audioReadinessSummary` and the two fact rows — it stays the one
/// place that words a prerequisite.
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
      // owns the switch, because an override keeps the Lab switch read-only.
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
        detail = String(localized: "On. Change it with the switch in Settings › Lab.")
      case (false, false):
        detail = String(
          localized: "Off. Recording cannot start until it is turned on in Settings › Lab.")
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

/// Collapses the four prerequisites into the one line the panel shows. A
/// healthy or in-flight state is a bare headline; a blocked one becomes the
/// blocking prerequisite, its explanation and the single action that clears it.
/// A `.fallback` prerequisite is only promoted when recording really cannot
/// start — a system-fallback microphone is a notice on the microphone card.
func audioReadinessSummary(
  input: CsAudioInputSnapshot,
  microphonePermission: PermissionState,
  admission: CsAdmissionReadiness?,
  recording isRecording: Bool? = false,
  preparing: Bool = false,
  processing: Bool = false
) -> AudioReadinessSummary {
  let steps = audioReadinessSteps(
    input: input,
    microphonePermission: microphonePermission,
    admission: admission,
    dictationShortcut: nil,
    recording: isRecording,
    preparing: preparing,
    processing: processing
  )
  guard let recordingStep = steps.first(where: { $0.id == .recording }) else {
    return AudioReadinessSummary(
      tone: .fallback,
      title: String(localized: "Checking recording readiness…"),
      detail: nil,
      remedy: .none
    )
  }

  // A take in flight, warming up or finishing is lifecycle, not a blocker.
  if processing || preparing || isRecording != false {
    return AudioReadinessSummary(
      tone: recordingStep.tone, title: recordingStep.title, detail: nil, remedy: .none)
  }

  let prerequisites = steps.filter { $0.id != .recording }
  let blocker =
    prerequisites.first { $0.tone == .unavailable }
    ?? (recordingStep.tone == .healthy ? nil : prerequisites.first { $0.tone == .fallback })

  if let blocker {
    return AudioReadinessSummary(
      tone: blocker.tone,
      title: audioReadinessProblemTitle(blocker, admission: admission),
      detail: blocker.detail,
      remedy: audioReadinessRemedy(
        for: blocker.id, admission: admission, microphonePermission: microphonePermission)
    )
  }

  return AudioReadinessSummary(
    tone: recordingStep.tone,
    title: recordingStep.title,
    detail: recordingStep.tone == .healthy ? nil : recordingStep.detail,
    remedy: .none
  )
}

/// The committing row is named for what it is, not for what went wrong, so as
/// a headline it has to state the consequence instead. Every other
/// prerequisite already titles its own failure.
func audioReadinessProblemTitle(
  _ step: AudioReadinessStep, admission: CsAdmissionReadiness?
) -> String {
  guard step.id == .sealLane, admission?.code != "admission_seal_vad_unavailable" else {
    return step.title
  }
  return String(localized: "Recording cannot start")
}

/// The one action that clears a blocked prerequisite. A verdict still loading
/// offers nothing: there is no established problem to act on yet.
func audioReadinessRemedy(
  for step: AudioReadinessStepID,
  admission: CsAdmissionReadiness?,
  microphonePermission: PermissionState
) -> AudioReadinessRemedy {
  switch step {
  case .microphone:
    return microphonePermission == .granted ? .none : .microphonePermission
  case .calibration:
    guard microphonePermission == .granted, let admission else { return .none }
    let sealed = admission.calibrationVersion != nil && admission.calibrationStatus == "sealed"
    return sealed ? .none : .calibrate
  case .sealLane:
    // An override is removed outside the app; Lab cannot edit it.
    guard let admission, admission.code != "admission_seal_vad_unavailable",
      admission.sealLaneSource != "env_override", !admission.sealLaneArmed
    else { return .none }
    return .openLab
  case .recording:
    return .none
  }
}

/// Calibration as one short fact. The measurement itself stays under
/// Calibration details; this row only answers whether one exists.
func audioCalibrationFact(
  _ admission: CsAdmissionReadiness?, microphonePermission: PermissionState
) -> AudioReadinessFact {
  let label = String(localized: "Microphone calibration")
  guard microphonePermission == .granted else {
    return AudioReadinessFact(
      id: .calibration, label: label, value: audioReadinessWaitingValue(), tone: .fallback)
  }
  guard let admission else {
    return AudioReadinessFact(
      id: .calibration, label: label, value: audioReadinessCheckingValue(), tone: .fallback)
  }
  if admission.calibrationVersion != nil, admission.calibrationStatus == "sealed" {
    return AudioReadinessFact(
      id: .calibration,
      label: label,
      value: String(
        localized: "audio.readiness.calibration.ready", defaultValue: "Ready",
        comment: "Value of the microphone calibration row: a usable measurement exists"),
      tone: .healthy
    )
  }
  return AudioReadinessFact(
    id: .calibration,
    label: label,
    value: String(
      localized: "audio.readiness.calibration.required", defaultValue: "Required",
      comment: "Value of the microphone calibration row: no usable measurement yet"),
    tone: .unavailable
  )
}

/// The committing lane as read-only status. The switch itself lives on the Lab
/// desk; Audio states the effective value and nothing else.
func audioSealLaneFact(_ admission: CsAdmissionReadiness?) -> AudioReadinessFact {
  let label = String(localized: "Committing fragments")
  guard let admission else {
    return AudioReadinessFact(
      id: .sealLane, label: label, value: audioReadinessCheckingValue(), tone: .fallback)
  }
  if admission.code == "admission_seal_vad_unavailable" {
    return AudioReadinessFact(
      id: .sealLane,
      label: label,
      value: String(
        localized: "audio.readiness.sealLane.unavailable", defaultValue: "Unavailable",
        comment: "Value of the committing fragments row: the detector did not load"),
      tone: .unavailable
    )
  }
  return admission.sealLaneArmed
    ? AudioReadinessFact(
      id: .sealLane,
      label: label,
      value: String(
        localized: "audio.readiness.sealLane.on", defaultValue: "On",
        comment: "Value of the committing fragments row; keep it as short as the UI allows"),
      tone: .healthy)
    : AudioReadinessFact(
      id: .sealLane,
      label: label,
      value: String(
        localized: "audio.readiness.sealLane.off", defaultValue: "Off",
        comment: "Value of the committing fragments row; keep it as short as the UI allows"),
      tone: .unavailable)
}

private func audioReadinessCheckingValue() -> String {
  String(
    localized: "audio.readiness.value.checking", defaultValue: "Checking…",
    comment: "Value of a readiness row while the controller verdict is still being read")
}

private func audioReadinessWaitingValue() -> String {
  String(
    localized: "audio.readiness.value.waiting", defaultValue: "Waiting",
    comment: "Value of a readiness row blocked by an earlier prerequisite")
}

/// Which microphone records right now, plus the one notice that applies. The
/// notice reuses the wording of the microphone readiness state, so a missing
/// device or an unapplied saved device reads the same wherever it appears.
func audioInputCardState(_ snapshot: CsAudioInputSnapshot) -> AudioInputCardState {
  let display = audioInputDisplayState(snapshot)
  let current: String
  if let runtimeDevice = snapshot.runtimeDevice, !runtimeDevice.isEmpty {
    current = String(
      localized: "Currently: \(runtimeDevice)",
      comment: "The placeholder is the input device the recorder actually uses")
  } else {
    current = String(localized: "Currently: no microphone")
  }
  guard display.tone != .healthy else {
    return AudioInputCardState(current: current, notice: nil, noticeTone: nil)
  }
  return AudioInputCardState(current: current, notice: display.detail, noticeTone: display.tone)
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
/// Lab toggle lie about which authority currently wins.
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
      : String(localized: "Seal lane is off in Settings › Lab")
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
      SettingsPageHeader(
        String(localized: "Microphone and recording"),
        blurb: String(localized: "Choose a microphone and adjust recording.")
      )

      SettingsSectionLabel(String(localized: "Microphone"))
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
      retentionSection
        .padding(.top, CSSpace.control)

      SettingsSectionLabel(String(localized: "Recording start sound"))
        .padding(.top, CSSpace.section)
      feedbackSection
        .padding(.top, CSSpace.control)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
  }

  /// One card: the choice, the input that is actually recording, and a notice
  /// only when the two disagree or no microphone is reachable at all.
  private var inputDeviceSection: some View {
    let card = audioInputCardState(model.audioInput)
    return VStack(alignment: .leading, spacing: 10) {
      HStack(spacing: 12) {
        Text("Input device")
          .font(.body.weight(.semibold))
          .foregroundStyle(.primary)
        Spacer(minLength: 0)
        Picker("Input device", selection: inputDeviceBinding) {
          Text(
            String(
              localized: "audio.input.systemDefault", defaultValue: "System default",
              comment: "Input device option: follow whichever microphone macOS uses")
          )
          .tag(Self.systemDefaultChoice)
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

      HStack(spacing: 12) {
        Text(card.current)
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
        Spacer(minLength: 0)
        Button {
          model.refreshAudioInput()
        } label: {
          HStack(spacing: 4) {
            CSIconView(icon: .refresh, size: 10, weight: .semibold)
            Text("Refresh")
          }
        }
        .csFocusRing()
        .font(CSFont.mono(10.5, .semibold))
        .foregroundStyle(CSColor.chromeAccent)
        .accessibilityLabel("Refresh audio input devices")
        .accessibilityHint("Re-reads the microphone list; readiness is checked below")
      }

      if let notice = card.notice {
        Text(notice)
          .font(CSFont.ui(11.5))
          .lineSpacing(2)
          .foregroundStyle(statusColor(card.noticeTone ?? .fallback))
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityIdentifier("audio-input-notice")
      }
    }
    .settingsGroupedInset()
  }

  /// The controller's precondition for any take, and the one operator step
  /// that can satisfy it locally: a ~10 s guided measurement through the real
  /// recorder path. No value is ever invented here.
  private var admissionSection: some View {
    VStack(alignment: .leading, spacing: 14) {
      if let error = model.admissionReadError {
        statusRow(
          color: CSColor.terracotta,
          title: "Readiness check unavailable",
          detail: error
        )
      }

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

  /// The stored measurement, kept out of the status above. Collapsed by
  /// default and laid out as label/value pairs: the status answers whether
  /// recording works, this answers with which profile, which only matters when
  /// a measurement is in question.
  @ViewBuilder
  private var calibrationDetails: some View {
    let details = audioCalibrationDetails(model.admission)
    if !details.isEmpty {
      DisclosureGroup(isExpanded: $showingCalibrationDetails) {
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 6) {
          ForEach(details) { detail in
            // A stored path is never a localization key, so both columns are
            // verbatim and the value stays selectable.
            GridRow {
              Text(verbatim: detail.label)
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(Color.secondary)
                .gridColumnAlignment(.leading)
              Text(verbatim: detail.value)
                .font(CSFont.mono(10, .medium))
                .foregroundStyle(Color.secondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
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
    let summary = audioReadinessSummary(
      input: model.audioInput,
      microphonePermission: model.permissions.microphone,
      admission: model.admission,
      recording: recordingState?.recording,
      preparing: recordingState?.warmingUp == true,
      processing: recordingProcessing(recordingState)
    )
    let calibration = audioCalibrationFact(
      model.admission, microphonePermission: model.permissions.microphone)
    let sealLane = audioSealLaneFact(model.admission)

    return VStack(alignment: .leading, spacing: 0) {
      summaryRow(
        summary, recordingState: recordingState, tray: tray, stopRequested: stopRequested)

      Divider()
        .overlay(Color.primary.opacity(0.12))
        .padding(.vertical, 8)

      HStack(alignment: .firstTextBaseline, spacing: 8) {
        Text(calibration.label)
          .font(CSFont.ui(12.5))
          .foregroundStyle(Color.primary)
        Text(calibration.value)
          .font(CSFont.ui(11.5, .medium))
          .foregroundStyle(statusColor(calibration.tone))
        Spacer(minLength: 0)
        calibrationControl(recordingState: recordingState, tray: tray)
      }
      .padding(.vertical, 5)
      .accessibilityElement(children: .contain)

      HStack(alignment: .firstTextBaseline, spacing: 8) {
        Text(sealLane.label)
          .font(CSFont.ui(12.5))
          .foregroundStyle(Color.primary)
        Spacer(minLength: 0)
        Text(sealLane.value)
          .font(CSFont.ui(11.5, .medium))
          .foregroundStyle(statusColor(sealLane.tone))
      }
      .padding(.vertical, 5)
      .help(Text("Required before a recording can start."))
      .accessibilityElement(children: .ignore)
      .accessibilityLabel(sealLane.label)
      .accessibilityValue(sealLane.value)
      .accessibilityHint("Required before a recording can start. The switch is on the Lab desk.")
      .accessibilityIdentifier("audio-readiness-committing-status")
    }
    .padding(CSSpace.md)
    .background(Color.primary.opacity(0.06))
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Recording readiness")
  }

  /// The one status line. A neutral state is a headline and the recorder
  /// button; a blocked one grows the problem and the action that clears it,
  /// so the neutral wording can never hide a blocker.
  private func summaryRow(
    _ summary: AudioReadinessSummary,
    recordingState: OverlayState?, tray: TrayViewModel?, stopRequested: Binding<Bool>
  ) -> some View {
    HStack(alignment: .top, spacing: 10) {
      CSIconView(
        icon: summary.tone == .healthy ? .checkCircleFill : .warning,
        size: 13,
        weight: .semibold,
        color: statusColor(summary.tone)
      )
      .padding(.top, 2)
      .accessibilityHidden(true)

      VStack(alignment: .leading, spacing: 5) {
        Text(summary.title)
          .font(CSFont.ui(13.5, .semibold))
          .foregroundStyle(Color.primary)
          .fixedSize(horizontal: false, vertical: true)
        if let detail = summary.detail {
          Text(detail)
            .font(CSFont.ui(11.5))
            .lineSpacing(2)
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
        remedyControl(summary.remedy, recordingState: recordingState, tray: tray)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      .accessibilityElement(children: .contain)
      .accessibilityLabel(summary.title)
      .accessibilityValue(summary.detail ?? "")

      recordingControl(
        recordingState: recordingState, tray: tray, stopRequested: stopRequested)
    }
    .padding(.vertical, 2)
  }

  @ViewBuilder
  private func remedyControl(
    _ remedy: AudioReadinessRemedy, recordingState: OverlayState?, tray: TrayViewModel?
  ) -> some View {
    switch remedy {
    case .none:
      EmptyView()
    case .microphonePermission:
      Button(microphonePermissionActionTitle) {
        resolveMicrophonePermission()
      }
      .buttonStyle(.bordered)
      .controlSize(.small)
      .accessibilityHint("Grants Codescribe access to the microphone selected above")
    case .calibrate:
      Button(model.calibrationPending ? "Measuring…" : "Calibrate") {
        Task { await calibrateMicrophone(recordingState, tray: tray) }
      }
      .buttonStyle(.bordered)
      .controlSize(.small)
      .disabled(!canCalibrateMicrophone(recordingState, tray: tray))
      .accessibilityLabel("Calibrate microphone")
      .accessibilityHint("Measures about ten seconds of normal speech; audio is not kept")
    case .openLab:
      // Production builds carry no Lab desk; the problem is still stated, but
      // the panel does not offer a destination that is not there.
      if SettingsSection.lab.availability == .available {
        Button("Open Lab") {
          model.select(.lab)
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .accessibilityHint("Opens the Lab desk, which owns the committing switch")
        .accessibilityIdentifier("audio-readiness-open-lab")
      }
    }
  }

  @ViewBuilder
  private func recordingControl(
    recordingState: OverlayState?, tray: TrayViewModel?, stopRequested: Binding<Bool>
  ) -> some View {
    if recordingProcessing(recordingState) {
      ProgressView()
        .controlSize(.small)
        .accessibilityLabel("Finishing recording")
    } else if recordingState?.recording == true {
      Button(
        String(
          localized: "audio.readiness.stop", defaultValue: "Stop",
          comment: "Button on the readiness status line: stop the running take")
      ) {
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
      .accessibilityLabel("Stop recording")
      .accessibilityHint("Stops the active take in the shared recorder")
      .accessibilityIdentifier("audio-readiness-stop-recording")
    } else {
      Button(
        String(
          localized: "audio.readiness.start", defaultValue: "Start",
          comment: "Button on the readiness status line: start a take")
      ) {
        startRecording(recordingState, tray: tray)
      }
      .buttonStyle(.borderedProminent)
      .controlSize(.small)
      .tint(CSColor.chromeAccent)
      .disabled(!canStartRecording(recordingState, tray: tray))
      .accessibilityLabel("Start recording")
      .accessibilityHint("Starts a real dictation session in the shared recorder")
      .accessibilityIdentifier("audio-readiness-start-recording")
    }
  }

  private func calibrationControl(
    recordingState: OverlayState?, tray: TrayViewModel?
  ) -> some View {
    let enabled = canCalibrateMicrophone(recordingState, tray: tray)
    return VStack(alignment: .trailing, spacing: 5) {
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
      Button(model.calibrationPending ? "Measuring…" : "Recalibrate") {
        Task { await calibrateMicrophone(recordingState, tray: tray) }
      }
      .csFocusRing()
      .font(CSFont.mono(10.5, .semibold))
      .foregroundStyle(enabled ? CSColor.chromeAccent : Color.secondary)
      .disabled(!enabled)
      .accessibilityLabel("Calibrate microphone")
      .accessibilityHint("Measures about ten seconds of normal speech; audio is not kept")
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

  /// The choice, and a sentence only where one is needed: `Forever` says
  /// everything in the value itself.
  private var retentionSection: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack(spacing: 12) {
        Text("Keep completed recordings")
          .font(.body.weight(.semibold))
          .foregroundStyle(.primary)
        Spacer(minLength: 0)
        Picker(
          "Keep completed recordings",
          selection: Binding(
            get: { model.audioRetention },
            set: { model.setAudioRetention($0) }
          )
        ) {
          Text("Forever").tag("forever")
          Text("30 days").tag("30_days")
          Text("7 days").tag("7_days")
          Text("24h").tag("24h")
          Text("Off").tag("off")
        }
        .pickerStyle(.menu)
        .labelsHidden()
        .fixedSize()
      }
      // The sentence follows the selected choice, and only the choices that
      // actually expire something carry one.
      if let detail = audioRetentionDetail(model.audioRetention) {
        Text(detail)
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
    }
    .settingsGroupedInset()
  }

  private var feedbackSection: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack(spacing: 12) {
        Text("Play a signal")
          .font(.body.weight(.semibold))
          .foregroundStyle(.primary)
        Spacer(minLength: 0)
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

  private var soundVolumeBinding: Binding<Double> {
    Binding(
      get: { Double(model.settings.soundVolume) },
      set: { model.setSoundVolume(Float($0)) }
    )
  }

  private var inputDeviceAccessibilityValue: String {
    model.settings.audioInputDevice ?? String(localized: "System default")
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
