import SwiftUI

// Dictation panel: everything that shapes how speech becomes text. Read-only
// runtime rows show the live STT truth (sourced from the CsSettings snapshot,
// not hardcoded); the permission matrix reflects live status. Editable owners:
// STT/layered engine controls, preview timing, and the hands-free silence
// window — all persisted through the promoted-key config router.

struct EnginePanel: View {
    @ObservedObject var model: SettingsViewModel
    @State private var advancedTimingExpanded = false

    private let matrixOrder: [PermissionKind] = [
        .microphone, .accessibility, .inputMonitoring, .screenRecording
    ]
    private let columns = [
        GridItem(.flexible(), spacing: 8),
        GridItem(.flexible(), spacing: 8)
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                EyebrowLabel(text: "Settings · \(SettingsSection.engine.title)")
                Text("RUNTIME TRUTH · READ-ONLY ROWS")
                    .font(CSFont.mono(9, .medium))
                    .foregroundStyle(CSColor.textMutedAlt)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(
                        RoundedRectangle(cornerRadius: 6, style: .continuous)
                            .fill(CSColor.surfaceRaised(0.04))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: 6, style: .continuous)
                            .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                    )
            }

            Text("What's actually running.")
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
                .padding(.top, 6)

            runtimeRows
                .padding(.top, 20)

            SettingsSectionLabel("Engine controls")
                .padding(.top, 22)
            engineControls
                .padding(.top, 11)

            SettingsSectionLabel("Preview timing")
                .padding(.top, 22)
            previewTimingSection
                .padding(.top, 11)

            SettingsSectionLabel("Hands-free silence")
                .padding(.top, 22)
            silenceSection
                .padding(.top, 11)

            SettingsSectionLabel("Permission matrix")
                .padding(.top, 22)
            LazyVGrid(columns: columns, spacing: 8) {
                ForEach(matrixOrder) { kind in
                    PermissionMatrixCell(kind: kind, state: model.permissions.state(kind))
                }
            }
            .padding(.top, 11)

            HStack(spacing: 8) {
                Text("●").font(CSFont.mono(11, .medium)).foregroundStyle(CSColor.olive)
                Text("runtime rows reflect the live engine — changes apply on the next recording session")
                    .font(CSFont.mono(11, .medium))
                    .foregroundStyle(CSColor.textFaint)
            }
            .padding(.top, 16)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
    }

    // MARK: Runtime key/value rows (STT truth only — LLM truth lives in Providers)

    private var runtimeRows: some View {
        VStack(spacing: 0) {
            RuntimeRow(key: "Active STT", value: model.activeSTT,
                       tint: true, trailing: .dot(model.sttHealthy ? CSColor.oliveLight : CSColor.amber))
            divider
            RuntimeRow(key: "STT model", value: model.sttModelDescription,
                       tint: false, mono: true, trailing: .none)
            divider
            RuntimeRow(key: "Whisper language", value: model.whisperLanguageCode,
                       tint: true, mono: true, trailing: .none)
        }
        .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
        )
    }

    private var divider: some View {
        Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
    }

    // MARK: Engine controls (editable — F1 layered transcription)

    /// Selectable engines. "onnx" is deliberately NOT exposed (experimental,
    /// frozen); "auto" defers to the core policy (Apple live when available).
    private static let sttEngineOptions: [(id: String, label: String)] = [
        ("auto", "Auto"),
        ("apple", "Apple (live)"),
        ("whisper", "Whisper (Candle)"),
    ]

    private var layeredBinding: Binding<Bool> {
        Binding(get: { model.layeredTranscriptionEnabled },
                set: { model.setLayeredTranscription($0) })
    }

    private var engineControls: some View {
        VStack(spacing: 8) {
            SettingsControlRow(title: "STT engine",
                               subtitle: "Auto prefers Apple live speech, else Whisper") {
                Menu {
                    ForEach(Self.sttEngineOptions, id: \.id) { option in
                        Button {
                            model.setSttEngine(option.id)
                        } label: {
                            if option.id == model.sttEngineId {
                                Label(option.label, systemImage: "checkmark")
                            } else {
                                Text(option.label)
                            }
                        }
                    }
                } label: {
                    SettingsMenuLabel(text: model.sttEngineLabel)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
            }
            SettingsControlRow(title: "Layered transcription",
                               subtitle: "Experimental: Apple live layer + Whisper tail patches") {
                Toggle("", isOn: layeredBinding)
                    .toggleStyle(.switch)
                    .labelsHidden()
                    .tint(CSColor.chromeAccent)
            }
        }
    }

    // MARK: Preview timing (overlay pacing — writes the existing promoted keys)

    private var previewTimingSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Picker("Preview timing preset", selection: presetBinding) {
                ForEach(PreviewTimingPreset.allCases) { preset in
                    Text(preset.rawValue).tag(preset)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()

            Text(previewSummary)
                .font(CSFont.mono(10.5, .medium))
                .foregroundStyle(CSColor.textFaint)

            DisclosureGroup(isExpanded: $advancedTimingExpanded) {
                VStack(spacing: 8) {
                    timingSlider(
                        title: "Buffer delay",
                        value: bufferDelayBinding,
                        range: 0 ... 1500,
                        step: 1,
                        valueLabel: "\(model.previewTimingConfiguration.values.bufferDelayMs) ms"
                    )
                    timingSlider(
                        title: "Typing speed",
                        value: typingCpsBinding,
                        range: 5 ... 180,
                        step: 0.1,
                        valueLabel: String(
                            format: "%.1f cps",
                            model.previewTimingConfiguration.values.typingCps
                        )
                    )
                    timingSlider(
                        title: "Words per tick",
                        value: emitWordsBinding,
                        range: 1 ... 10,
                        step: 1,
                        valueLabel: "\(model.previewTimingConfiguration.values.emitWordsMax)"
                    )
                    timingSlider(
                        title: "Interim cadence",
                        value: interimBinding,
                        range: 1 ... 30,
                        step: 0.1,
                        valueLabel: String(
                            format: "%.1f s",
                            model.previewTimingConfiguration.values.interimSeconds
                        )
                    )
                }
                .padding(.top, 10)
            } label: {
                Text("Advanced")
                    .font(CSFont.ui(12.5, .semibold))
                    .foregroundStyle(CSColor.textBody)
            }
            .tint(CSColor.chromeAccent)
        }
        .padding(15)
        .background(card)
        .overlay(cardBorder)
    }

    private var presetBinding: Binding<PreviewTimingPreset> {
        Binding(
            get: { model.previewTimingPreset },
            set: { preset in
                if preset == .custom {
                    advancedTimingExpanded = true
                } else {
                    advancedTimingExpanded = false
                    model.applyPreviewTimingPreset(preset)
                }
            }
        )
    }

    private var bufferDelayBinding: Binding<Double> {
        Binding(
            get: { Double(model.previewTimingConfiguration.values.bufferDelayMs) },
            set: { model.setPreviewBufferDelayMs(UInt64($0.rounded())) }
        )
    }

    private var typingCpsBinding: Binding<Double> {
        Binding(
            get: { Double(model.previewTimingConfiguration.values.typingCps) },
            set: { model.setPreviewTypingCps(Float($0)) }
        )
    }

    private var emitWordsBinding: Binding<Double> {
        Binding(
            get: { Double(model.previewTimingConfiguration.values.emitWordsMax) },
            set: { model.setPreviewEmitWordsMax(UInt64($0.rounded())) }
        )
    }

    private var interimBinding: Binding<Double> {
        Binding(
            get: { Double(model.previewTimingConfiguration.values.interimSeconds) },
            set: { model.setPreviewInterimSeconds(Float($0)) }
        )
    }

    private var previewSummary: String {
        guard model.previewTimingConfiguration.overlayEnabled else {
            return "Preview off · committed transcripts are unchanged"
        }
        let values = model.previewTimingConfiguration.values
        return String(
            format: "%llu ms · %.1f cps · %llu words · %.1f s interim",
            values.bufferDelayMs,
            values.typingCps,
            values.emitWordsMax,
            values.interimSeconds
        )
    }

    private func timingSlider(
        title: String,
        value: Binding<Double>,
        range: ClosedRange<Double>,
        step: Double,
        valueLabel: String
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(title)
                    .font(CSFont.ui(12, .medium))
                    .foregroundStyle(CSColor.textMutedAlt)
                Spacer(minLength: 0)
                Text(valueLabel)
                    .font(CSFont.mono(10.5, .semibold))
                    .foregroundStyle(CSColor.textBody)
            }
            Slider(value: value, in: range, step: step)
                .tint(CSColor.chromeAccent)
        }
    }

    // MARK: Hands-free silence (toggle-mode VAD window — TOGGLE_SILENCE_SEC)

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
                .tint(CSColor.chromeAccent)
                .accessibilityLabel("Hands-free silence duration")
                .accessibilityValue(String(format: "%.1f seconds", model.settings.toggleSilenceSec))
        }
        .padding(15)
        .background(card)
        .overlay(cardBorder)
    }

    private var silenceBinding: Binding<Double> {
        Binding(
            get: { Double(model.settings.toggleSilenceSec) },
            set: { model.setToggleSilenceSeconds(Float($0)) }
        )
    }

    private var card: some ShapeStyle {
        CSColor.surfaceRaised(0.025)
    }

    private var cardBorder: some View {
        RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
            .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
    }
}

// MARK: - Permission matrix cell

private struct PermissionMatrixCell: View {
    let kind: PermissionKind
    let state: PermissionState

    private var granted: Bool { state.isGranted }
    private var accent: Color { granted ? CSColor.olive : CSColor.terracotta }
    private var accentLight: Color { granted ? CSColor.oliveLight : CSColor.terracottaLight }

    var body: some View {
        HStack(spacing: 10) {
            CSIconView(icon: granted ? .success : .warning, size: 11, weight: .semibold, color: accentLight)
            Text(kind.rawValue)
                .font(CSFont.ui(12.5, .medium))
                .foregroundStyle(CSColor.textBodyAlt)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text(granted ? "granted" : state.label)
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(accentLight)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(accent.opacity(0.08))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(accent.opacity(0.2), lineWidth: 1)
        )
        .contentShape(Rectangle())
        .onTapGesture { if !granted { kind.openSystemSettings() } }
    }
}

#if DEBUG
#Preview("Dictation panel") {
    ScrollView { EnginePanel(model: .preview(.engine)) }
        .frame(width: 720, height: 620)
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
#endif
