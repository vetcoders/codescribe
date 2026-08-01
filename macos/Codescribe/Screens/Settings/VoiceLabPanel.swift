import SwiftUI

// Dictionary panel (internal VoiceLab name stays — the Rust/FFI quality
// contracts are unchanged): textual corrections and the custom lexicon coming
// from the live local quality loop. Preview timing lives in Dictation.

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
    let source: String
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
            canonical: entry.canonical,
            source: entry.source
        )
    }
}

/// Honest Dictionary headline.
/// Every custom lexicon variant→canonical is a **live rule** the engine applies.
/// Correction provenance is a subset, not the only “real” count.
func dictionaryHeadline(correctionsRecorded: Int, rulesLearned: Int) -> String {
    "\(correctionsRecorded) corrections recorded · \(rulesLearned) rules in dictionary"
}

func dictionarySubtitle(
    correctionsRecorded: Int,
    rulesLearned: Int,
    taughtFromCorrections: Int,
    totalEntries: Int
) -> String {
    if rulesLearned > 0 {
        return "\(rulesLearned) live rules (variant→canonical) · \(taughtFromCorrections) with correction provenance · \(totalEntries) store rows."
    }
    if correctionsRecorded > 0 {
        return "\(correctionsRecorded) corrections on disk · dictionary empty — press Teach to mine rules from the store."
    }
    return "Correction history and custom dictionary. Teach promotes corrections + proposed → live lexicon."
}


struct VoiceLabPanel: View {
    @ObservedObject var model: SettingsViewModel
    @State private var editor = VoiceLabEditorState()
    @State private var correctionIndex = 0
    @State private var lexiconIndex = 0

    private var corrections: [VoiceLabCorrectionRow] {
        qualityCorrectionRows(model.qualityRecords)
    }

    /// Every flattened lexicon pair is a rule PostProcessor applies.
    private var rulesLearnedCount: Int {
        model.customLexiconEntries.count
    }

    /// Subset taught from correction / proposed provenance (source=correction).
    private var taughtFromCorrectionsCount: Int {
        model.customLexiconEntries.lazy.filter { $0.source == "correction" }.count
    }

    private var correctionsRecordedCount: Int {
        corrections.count
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 0) {
                    EyebrowLabel(text: "Settings · \(SettingsSection.voiceLab.title)")
                    Text(dictionaryHeadline(
                        correctionsRecorded: correctionsRecordedCount,
                        rulesLearned: rulesLearnedCount
                    ))
                        .font(CSFont.ui(26, .bold))
                        .tracking(-0.5)
                        .foregroundStyle(CSColor.textHigh)
                        .padding(.top, 6)
                    Text(dictionarySubtitle(
                        correctionsRecorded: correctionsRecordedCount,
                        rulesLearned: rulesLearnedCount,
                        taughtFromCorrections: taughtFromCorrectionsCount,
                        totalEntries: model.customLexiconEntries.count
                    ))
                        .font(CSFont.ui(12.5))
                        .foregroundStyle(CSColor.textMutedAlt)
                        .padding(.top, 8)
                    if let teachMsg = model.voiceLabTeachMessage {
                        Text(teachMsg)
                            .font(CSFont.mono(11, .medium))
                            .foregroundStyle(CSColor.oliveLight)
                            .padding(.top, 8)
                    }
                }
                Spacer(minLength: 0)
                HStack(spacing: 12) {
                    Button("Teach") {
                        model.teachDictionaryFromStore()
                    }
                    .font(CSFont.mono(11, .semibold))
                    .foregroundStyle(CSColor.chromeAccent)
                    .buttonStyle(.plain)
                    .disabled(model.voiceLabTeachPending)
                    .accessibilityLabel("Teach dictionary from corrections and proposed rules")
                    Button("Refresh") {
                        model.refreshVoiceLab()
                    }
                    .font(CSFont.mono(11, .semibold))
                    .foregroundStyle(CSColor.chromeAccent)
                    .buttonStyle(.plain)
                    .accessibilityLabel("Refresh \(SettingsSection.voiceLab.title) data")
                }
            }

            SettingsSectionLabel("Recent corrections · \(corrections.count)")
                .padding(.top, 24)
            correctionsSection
                .padding(.top, 11)

            SettingsSectionLabel("Custom dictionary · \(model.customLexiconEntries.count)")
                .padding(.top, 24)
            lexiconSection
                .padding(.top, 11)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
    }

    @ViewBuilder
    private var correctionsSection: some View {
        if let error = model.voiceLabReadError {
            readError(error)
        } else if corrections.isEmpty {
            emptyState("No corrections yet — edit a transcript in the overlay so the engine can learn.")
        } else {
            VStack(spacing: 8) {
                let safeIndex = min(correctionIndex, corrections.count - 1)
                let row = corrections[safeIndex]
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
                                    .foregroundStyle(CSColor.chromeAccent)
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
                        if let note = model.voiceLabEditNotes[row.id] {
                            Text(note)
                                .font(CSFont.ui(10.5))
                                .foregroundStyle(CSColor.oliveLight)
                                .accessibilityLabel("Correction saved. \(note)")
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
                HStack {
                    Button("Previous") { correctionIndex = max(0, safeIndex - 1) }
                        .disabled(safeIndex == 0)
                    Spacer()
                    Text("\(safeIndex + 1) of \(corrections.count)")
                        .font(CSFont.mono(10.5, .medium))
                        .foregroundStyle(CSColor.textFaintAlt)
                    Spacer()
                    Button("Next") { correctionIndex = min(corrections.count - 1, safeIndex + 1) }
                        .disabled(safeIndex == corrections.count - 1)
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
        } else if model.customLexiconEntries.isEmpty {
            emptyState("The custom dictionary is empty — accepted overlay corrections will appear here.")
        } else {
            VStack(spacing: 8) {
                let safeIndex = min(lexiconIndex, model.customLexiconEntries.count - 1)
                let row = model.customLexiconEntries[safeIndex]
                HStack(spacing: 10) {
                        Text(row.variant)
                            .font(CSFont.mono(11.5, .medium))
                            .foregroundStyle(CSColor.textMutedAlt)
                            .textSelection(.enabled)
                        Text("→")
                            .font(CSFont.mono(11, .semibold))
                            .foregroundStyle(CSColor.chromeAccent)
                        Text(row.canonical)
                            .font(CSFont.mono(11.5, .semibold))
                            .foregroundStyle(CSColor.textBody)
                            .textSelection(.enabled)
                        Spacer(minLength: 0)
                        Text(row.source)
                            .font(CSFont.mono(10, .medium))
                            .foregroundStyle(CSColor.textFaintAlt)
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 12)
                    .background(card)
                    .overlay(cardBorder)
                    .accessibilityLabel("\(row.variant) to \(row.canonical), source \(row.source)")
                HStack {
                    Button("Previous") { lexiconIndex = max(0, safeIndex - 1) }
                        .disabled(safeIndex == 0)
                    Spacer()
                    Text("\(safeIndex + 1) of \(model.customLexiconEntries.count)")
                        .font(CSFont.mono(10.5, .medium))
                        .foregroundStyle(CSColor.textFaintAlt)
                    Spacer()
                    Button("Next") {
                        lexiconIndex = min(model.customLexiconEntries.count - 1, safeIndex + 1)
                    }
                    .disabled(safeIndex == model.customLexiconEntries.count - 1)
                }
            }
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

#if DEBUG
#Preview("Settings — Dictionary") {
    SettingsView(model: SettingsViewModel.preview(.voiceLab))
        .frame(width: 960, height: 720)
}
#endif
