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

/// Honest Dictionary headline from two independent store counts (LL-F).
/// No causality claim — corrections recorded ≠ rules learned until the loop teaches.
func dictionaryHeadline(correctionsRecorded: Int, rulesLearned: Int) -> String {
    "\(correctionsRecorded) corrections recorded · \(rulesLearned) rules learned"
}

func dictionarySubtitle(correctionsRecorded: Int, rulesLearned: Int, totalEntries: Int) -> String {
    if rulesLearned > 0 {
        return "\(rulesLearned) rules from correction provenance · \(totalEntries) custom dictionary entries total."
    }
    if correctionsRecorded > 0 {
        return "\(correctionsRecorded) corrections on disk · 0 rules taught yet from this store."
    }
    return "Correction history and custom dictionary entries from the local quality store."
}


struct VoiceLabPanel: View {
    @ObservedObject var model: SettingsViewModel
    @State private var editor = VoiceLabEditorState()

    private var corrections: [VoiceLabCorrectionRow] {
        qualityCorrectionRows(model.qualityRecords)
    }

    private var lexicon: [VoiceLabLexiconRow] {
        customLexiconRows(model.customLexiconEntries)
    }

    private var rulesLearnedCount: Int {
        lexicon.filter { $0.source == "correction" }.count
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
                        totalEntries: lexicon.count
                    ))
                        .font(CSFont.ui(12.5))
                        .foregroundStyle(CSColor.textMutedAlt)
                        .padding(.top, 8)
                }
                Spacer(minLength: 0)
                Button("Refresh") {
                    model.refreshVoiceLab()
                }
                .font(CSFont.mono(11, .semibold))
                .foregroundStyle(CSColor.chromeAccent)
                .buttonStyle(.plain)
                .accessibilityLabel("Refresh \(SettingsSection.voiceLab.title) data")
            }

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
                            .foregroundStyle(CSColor.chromeAccent)
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
