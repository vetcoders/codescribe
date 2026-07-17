import SwiftUI

enum PreviewTimingPreset: String, CaseIterable, Identifiable, Equatable {
    case smooth = "Smooth"
    case snappy = "Snappy"
    case relaxed = "Relaxed"
    case off = "Off"
    case custom = "Custom"

    var id: String { rawValue }
}

struct PreviewTimingValues: Equatable {
    let bufferDelayMs: UInt64
    let typingCps: Float
    let emitWordsMax: UInt64
    let interimSeconds: Float

    // Source: operator-tested C5b values (2026-06-11). Smooth is the
    // recommended default; Snappy/Relaxed retain the original values without
    // the optional +/-20% retuning because all are inside current clamps.
    static let smooth = PreviewTimingValues(
        bufferDelayMs: 1038,
        typingCps: 10.6,
        emitWordsMax: 5,
        interimSeconds: 8.0
    )
    static let snappy = PreviewTimingValues(
        bufferDelayMs: 350,
        typingCps: 28.0,
        emitWordsMax: 3,
        interimSeconds: 4.0
    )
    static let relaxed = PreviewTimingValues(
        bufferDelayMs: 1500,
        typingCps: 8.0,
        emitWordsMax: 8,
        interimSeconds: 8.0
    )
}

struct PreviewTimingConfiguration: Equatable {
    let overlayEnabled: Bool
    let values: PreviewTimingValues
}

func presetValues(_ preset: PreviewTimingPreset) -> PreviewTimingValues? {
    switch preset {
    case .smooth: return .smooth
    case .snappy: return .snappy
    case .relaxed: return .relaxed
    case .off, .custom: return nil
    }
}

func detectPreset(_ configuration: PreviewTimingConfiguration) -> PreviewTimingPreset {
    guard configuration.overlayEnabled else { return .off }
    for preset in [PreviewTimingPreset.smooth, .snappy, .relaxed] {
        guard let values = presetValues(preset) else { continue }
        let current = configuration.values
        let bufferClose = current.bufferDelayMs.absDiff(values.bufferDelayMs) <= 10
        let cpsClose = abs(current.typingCps - values.typingCps) <= 0.15
        let wordsMatch = current.emitWordsMax == values.emitWordsMax
        let interimClose = abs(current.interimSeconds - values.interimSeconds) <= 0.15
        if bufferClose, cpsClose, wordsMatch, interimClose {
            return preset
        }
    }
    return .custom
}

struct VoiceLabCorrectionRow: Identifiable, Equatable {
    let id: String
    let revision: UInt64
    let rawText: String
    let variant: String
    let editedText: String
    let action: String
    let timestampMs: UInt64
}

struct VoiceLabEditorState: Equatable {
    var correctionID: String?
    var canonical = ""

    mutating func begin(_ row: VoiceLabCorrectionRow) {
        correctionID = row.id
        canonical = row.editedText
    }

    mutating func cancel() {
        correctionID = nil
        canonical = ""
    }
}

struct VoiceLabLexiconRow: Identifiable, Equatable {
    let id: Int
    let variant: String
    let canonical: String
}

func qualityCorrectionRows(_ records: [CsQualityRecord]) -> [VoiceLabCorrectionRow] {
    records.map { record in
        VoiceLabCorrectionRow(
            id: record.id,
            revision: record.revision,
            rawText: record.rawText,
            variant: record.variant,
            editedText: record.editedText,
            action: record.action,
            timestampMs: record.timestampMs
        )
    }
}

func customLexiconRows(_ entries: [CsLexiconEntry]) -> [VoiceLabLexiconRow] {
    entries.enumerated().map { index, entry in
        VoiceLabLexiconRow(
            id: index,
            variant: entry.variant,
            canonical: entry.canonical
        )
    }
}

struct VoiceLabPanel: View {
    @ObservedObject var model: SettingsViewModel
    @State private var advancedExpanded = false
    @State private var editor = VoiceLabEditorState()

    private var corrections: [VoiceLabCorrectionRow] {
        qualityCorrectionRows(model.qualityRecords)
    }

    private var lexicon: [VoiceLabLexiconRow] {
        customLexiconRows(model.customLexiconEntries)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 0) {
                    EyebrowLabel(text: "Settings · Voice Lab")
                    Text("See what your voice taught.")
                        .font(CSFont.ui(26, .bold))
                        .tracking(-0.5)
                        .foregroundStyle(CSColor.textHigh)
                        .padding(.top, 6)
                    Text("Corrections and custom words come from the live local quality loop.")
                        .font(CSFont.ui(12.5))
                        .foregroundStyle(CSColor.textMutedAlt)
                        .padding(.top, 8)
                }
                Spacer(minLength: 0)
                Button("Refresh") {
                    model.refreshVoiceLab()
                }
                .font(CSFont.mono(11, .semibold))
                .foregroundStyle(CSColor.terracottaLight)
                .buttonStyle(.plain)
                .accessibilityLabel("Refresh Voice Lab data")
            }

            SettingsSectionLabel("Preview timing")
                .padding(.top, 24)
            previewTimingSection
                .padding(.top, 11)

            SettingsSectionLabel("Recent corrections · \(corrections.count)")
                .padding(.top, 24)
            correctionsSection
                .padding(.top, 11)

            SettingsSectionLabel("Custom dictionary · \(lexicon.count)")
                .padding(.top, 24)
            lexiconSection
                .padding(.top, 11)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
    }

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

            DisclosureGroup(isExpanded: $advancedExpanded) {
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
            .tint(CSColor.terracottaLight)
        }
        .padding(15)
        .background(card)
        .overlay(cardBorder)
    }

    @ViewBuilder
    private var correctionsSection: some View {
        if let error = model.voiceLabReadError {
            readError(error)
        } else if corrections.isEmpty {
            emptyState("No corrections yet — edit a transcript in the overlay so the engine can learn.")
        } else {
            VStack(spacing: 8) {
                ForEach(corrections) { row in
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Heard: \(row.variant)")
                            .font(CSFont.ui(12.5, .medium))
                            .foregroundStyle(CSColor.textMutedAlt)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        if editor.correctionID == row.id {
                            HStack(spacing: 8) {
                                TextField("Canonical correction", text: $editor.canonical)
                                    .textFieldStyle(.roundedBorder)
                                    .onSubmit { saveEdit(row) }
                                    .onExitCommand { editor.cancel() }
                                    .accessibilityLabel("Canonical correction for \(row.variant)")
                                Button("Save") { saveEdit(row) }
                                    .disabled(
                                        editor.canonical.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                            || model.voiceLabEditPending.contains(row.id)
                                    )
                                Button("Cancel") { editor.cancel() }
                            }
                        } else {
                            HStack(spacing: 7) {
                                Text("→")
                                    .font(CSFont.mono(11, .semibold))
                                    .foregroundStyle(CSColor.terracottaLight)
                                Text(row.editedText)
                                    .font(CSFont.ui(13, .semibold))
                                    .foregroundStyle(CSColor.textBody)
                                    .textSelection(.enabled)
                                Spacer(minLength: 0)
                                Button("Edit") { editor.begin(row) }
                                    .disabled(model.voiceLabEditPending.contains(row.id))
                                    .accessibilityLabel("Edit correction for \(row.variant)")
                            }
                        }
                        if model.voiceLabEditPending.contains(row.id) {
                            ProgressView()
                                .controlSize(.small)
                                .accessibilityLabel("Saving correction")
                        }
                        if let error = model.voiceLabEditErrors[row.id] {
                            Text("Save failed: \(error)")
                                .font(CSFont.ui(10.5))
                                .foregroundStyle(CSColor.terracottaLight)
                        }
                        HStack(spacing: 7) {
                            Text(row.action)
                                .foregroundStyle(CSColor.oliveLight)
                            Text("·")
                            Text("revision \(row.revision)")
                            Text("·")
                            Text(timestampLabel(row.timestampMs))
                        }
                        .font(CSFont.mono(10, .medium))
                        .foregroundStyle(CSColor.textFaintAlt)
                    }
                    .padding(14)
                    .background(card)
                    .overlay(cardBorder)
                    .accessibilityElement(children: .contain)
                    .accessibilityLabel(
                        "Heard \(row.variant). Current correction \(row.editedText). Revision \(row.revision)."
                    )
                }
            }
        }
    }

    private func saveEdit(_ row: VoiceLabCorrectionRow) {
        if model.finalizeVoiceLabCorrection(id: row.id, canonical: editor.canonical) {
            editor.cancel()
        }
    }

    @ViewBuilder
    private var lexiconSection: some View {
        if let error = model.voiceLabReadError {
            readError(error)
        } else if lexicon.isEmpty {
            emptyState("The custom dictionary is empty — accepted overlay corrections will appear here.")
        } else {
            VStack(spacing: 8) {
                ForEach(lexicon) { row in
                    HStack(spacing: 10) {
                        Text(row.variant)
                            .font(CSFont.mono(11.5, .medium))
                            .foregroundStyle(CSColor.textMutedAlt)
                            .textSelection(.enabled)
                        Text("→")
                            .font(CSFont.mono(11, .semibold))
                            .foregroundStyle(CSColor.terracottaLight)
                        Text(row.canonical)
                            .font(CSFont.mono(11.5, .semibold))
                            .foregroundStyle(CSColor.textBody)
                            .textSelection(.enabled)
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 12)
                    .background(card)
                    .overlay(cardBorder)
                }
            }
        }
    }

    private var presetBinding: Binding<PreviewTimingPreset> {
        Binding(
            get: { model.previewTimingPreset },
            set: { preset in
                if preset == .custom {
                    advancedExpanded = true
                } else {
                    advancedExpanded = false
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
                .tint(CSColor.terracotta)
        }
    }

    private func emptyState(_ message: String) -> some View {
        Text(message)
            .font(CSFont.ui(12.5))
            .lineSpacing(2)
            .foregroundStyle(CSColor.textMutedAlt)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(15)
            .background(card)
            .overlay(cardBorder)
    }

    private func readError(_ error: String) -> some View {
        Text("Live quality data is unavailable: \(error)")
            .font(CSFont.ui(12.5))
            .foregroundStyle(CSColor.terracottaLight)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(15)
            .background(card)
            .overlay(cardBorder)
    }

    private func timestampLabel(_ timestampMs: UInt64) -> String {
        Date(timeIntervalSince1970: Double(timestampMs) / 1000.0)
            .formatted(date: .abbreviated, time: .shortened)
    }

    private var card: some ShapeStyle {
        CSColor.surfaceRaised(0.025)
    }

    private var cardBorder: some View {
        RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
            .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
    }
}

private extension UInt64 {
    func absDiff(_ other: UInt64) -> UInt64 {
        self >= other ? self - other : other - self
    }
}

#if DEBUG
#Preview("Settings — Voice Lab") {
    SettingsView(model: SettingsViewModel.preview(.voiceLab))
        .frame(width: 960, height: 720)
}
#endif
