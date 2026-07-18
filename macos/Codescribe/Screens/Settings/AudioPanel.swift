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
            detail: "Saved: \(saved). Restart Codescribe to apply it; an explicit AUDIO_INPUT_DEVICE launch override can keep a different runtime input active."
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
                    Text("Device choice, hands-free silence, and feedback use the live recorder config.")
                        .font(CSFont.ui(12.5))
                        .lineSpacing(2)
                        .foregroundStyle(CSColor.textMutedAlt)
                        .padding(.top, 8)
                }
                Spacer(minLength: 0)
                Button("Refresh") {
                    model.refreshAudioInput()
                }
                .buttonStyle(.plain)
                .font(CSFont.mono(11, .semibold))
                .foregroundStyle(CSColor.terracottaLight)
                .accessibilityLabel("Refresh audio input devices")
            }

            SettingsSectionLabel("Input device")
                .padding(.top, 24)
            inputDeviceSection
                .padding(.top, 11)

            SettingsSectionLabel("Voice detection")
                .padding(.top, 24)
            silenceSection
                .padding(.top, 11)

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

            runtimeInputStatus

            HStack {
                Text("Reset removes the preference; it never writes an empty device name.")
                    .font(CSFont.mono(10, .medium))
                    .foregroundStyle(CSColor.textFaint)
                Spacer(minLength: 12)
                Button("Use system default") {
                    model.resetAudioInputDevice()
                }
                .buttonStyle(.plain)
                .font(CSFont.mono(10.5, .semibold))
                .foregroundStyle(CSColor.terracottaLight)
                .disabled(model.settings.audioInputDevice == nil)
                .accessibilityLabel("Reset audio input to system default")
            }
        }
        .padding(15)
        .background(card)
        .overlay(cardBorder)
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

    private var silenceSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Hands-free silence")
                        .font(CSFont.ui(13, .semibold))
                        .foregroundStyle(CSColor.textBody)
                    Text("End a toggle-mode utterance after this much live VAD silence")
                        .font(CSFont.ui(11.5))
                        .foregroundStyle(CSColor.textMutedAlt)
                }
                Spacer(minLength: 12)
                Text(String(format: "%.1f s", model.settings.toggleSilenceSec))
                    .font(CSFont.mono(11, .semibold))
                    .foregroundStyle(CSColor.textBody)
            }
            Slider(value: silenceBinding, in: 0.5 ... 30, step: 0.5)
                .tint(CSColor.terracotta)
                .accessibilityLabel("Hands-free silence duration")
                .accessibilityValue(String(format: "%.1f seconds", model.settings.toggleSilenceSec))
        }
        .padding(15)
        .background(card)
        .overlay(cardBorder)
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
                    .tint(CSColor.terracotta)
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
                Slider(value: soundVolumeBinding, in: 0 ... 1, step: 0.05)
                    .tint(CSColor.terracotta)
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

    private var silenceBinding: Binding<Double> {
        Binding(
            get: { Double(model.settings.toggleSilenceSec) },
            set: { model.setToggleSilenceSeconds(Float($0)) }
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
        model.settings.audioInputDevice ?? "System default"
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
