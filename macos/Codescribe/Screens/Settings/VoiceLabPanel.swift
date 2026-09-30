import AppKit
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
  let avgLogprob: Float?
  let speechPct: Float?
  let confidenceFlags: [String]

  var isLowConfidence: Bool {
    if let avgLogprob, avgLogprob <= -1.20 { return true }
    return confidenceFlags.contains { flag in
      let normalized = flag.lowercased()
      return normalized.contains("low_logprob")
        || normalized.contains("hallucination")
        || normalized.contains("quality_gate")
    }
  }

  /// Whitespace-normalized text delta against the raw STT (mirror of the Rust
  /// read-side rule in `QualityRecord::has_text_change`).
  var hasTextChange: Bool {
    let raw = normalizedCorrectionText(rawText)
    return normalizedCorrectionText(variant) != raw || normalizedCorrectionText(editedText) != raw
  }

  var hasTelemetry: Bool {
    avgLogprob != nil || speechPct != nil || !confidenceFlags.isEmpty
  }

  /// Telemetry chips for the card header; empty when nothing was recorded
  /// (the list header carries that aggregate, not every card).
  var telemetryChips: [String] {
    var chips: [String] = []
    if let avgLogprob {
      chips.append(String(format: "logprob %.2f", avgLogprob))
    }
    if let speechPct {
      let percent = speechPct <= 1 ? speechPct * 100 : speechPct
      chips.append(String(format: "speech %.0f%%", percent))
    }
    chips.append(contentsOf: confidenceFlags)
    return chips
  }
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

/// Whitespace runs collapse to single spaces so a rewrap is not a change.
/// One definition, two enforcers: the Rust `recent_quality_listing` read is
/// the storage-level throne; this mirror keeps the panel consistent with the
/// headline count for previews and mocks.
func normalizedCorrectionText(_ text: String) -> String {
  text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
}

/// A record is a correction only when the text changed after whitespace
/// normalization (delivered/edited vs raw STT) or confidence telemetry was
/// recorded. Takes with neither never enter the corrections list.
func isQualityCorrection(
  rawText: String,
  deliveredText: String,
  editedText: String,
  avgLogprob: Float?,
  speechPct: Float?,
  confidenceFlags: [String]
) -> Bool {
  let raw = normalizedCorrectionText(rawText)
  let changed =
    normalizedCorrectionText(deliveredText) != raw
    || normalizedCorrectionText(editedText) != raw
  return changed || avgLogprob != nil || speechPct != nil || !confidenceFlags.isEmpty
}

func qualityCorrectionRows(_ records: [CsQualityRecord]) -> [VoiceLabCorrectionRow] {
  records.compactMap { record in
    guard
      isQualityCorrection(
        rawText: record.rawText,
        deliveredText: record.variant,
        editedText: record.editedText,
        avgLogprob: record.avgLogprob,
        speechPct: record.speechPct,
        confidenceFlags: record.confidenceFlags
      )
    else { return nil }
    return VoiceLabCorrectionRow(
      id: record.id,
      revision: record.revision,
      rawText: record.rawText,
      variant: record.variant,
      editedText: record.editedText,
      action: record.action,
      timestampMs: record.timestampMs,
      avgLogprob: record.avgLogprob,
      speechPct: record.speechPct,
      confidenceFlags: record.confidenceFlags
    )
  }
}

/// Tiered word-level diff for one correction. Delegates to the Rust engine so
/// Swift only renders; Polish diacritics compare as words, never bytes.
func voiceLabDiffSpans(raw: String, edited: String) -> [CsDiffSpan] {
  qualityDiffSpans(raw: raw, edited: edited)
}

extension CsDiffTier {
  var isMajor: Bool {
    switch self {
    case .vocabulary, .insert, .delete: return true
    case .casing, .punctuation: return false
    }
  }
}

/// Major tiers are real content changes; minor tiers (casing/punctuation) are
/// collapsed in the correction card summary.
func majorDiffSpans(_ spans: [CsDiffSpan]) -> [CsDiffSpan] {
  spans.filter { $0.tier.isMajor }
}

func minorDiffSpans(_ spans: [CsDiffSpan]) -> [CsDiffSpan] {
  spans.filter { !$0.tier.isMajor }
}

/// The Suggested rules section is shown only when the engine mined at least
/// one candidate; at zero it disappears entirely instead of showing an empty box.
func ruleCandidatesSectionVisible(_ candidates: [CsRuleCandidate]) -> Bool {
  !candidates.isEmpty
}

/// One-line summary of collapsed minor (casing/punctuation) adjustments.
func minorAdjustmentsSummary(_ minor: [CsDiffSpan]) -> String {
  "+\(minor.count) minor (punctuation, casing)"
}

/// One flowing line: context in secondary, removed words struck through,
/// corrected words emphasized. Word wrapping comes free from Text.
private func diffSpanText(_ span: CsDiffSpan) -> Text {
  var result = Text("")
  if !span.contextBefore.isEmpty {
    result = result + Text(span.contextBefore + " ").foregroundStyle(Color.secondary)
  }
  if !span.raw.isEmpty {
    result =
      result
      + Text(span.raw).strikethrough().foregroundStyle(CSColor.terracotta)
      + Text(" ")
  }
  if !span.edited.isEmpty {
    if !span.raw.isEmpty {
      result = result + Text("→ ").foregroundStyle(CSColor.chromeAccent)
    }
    result =
      result
      + Text(span.edited).fontWeight(.semibold).foregroundStyle(Color.primary)
      + Text(" ")
  } else if !span.raw.isEmpty {
    result = result + Text("(removed) ").foregroundStyle(Color.secondary)
  }
  if !span.contextAfter.isEmpty {
    result = result + Text(span.contextAfter).foregroundStyle(Color.secondary)
  }
  return result
}

/// One aggregate line replaces the old per-card "No confidence telemetry was
/// recorded" (Founder report 2026-09-30): it belongs to the list, not a card.
func missingTelemetryLine(rows: [VoiceLabCorrectionRow]) -> String? {
  guard !rows.isEmpty else { return nil }
  let missing = rows.filter { !$0.hasTelemetry }.count
  guard missing > 0 else { return nil }
  return "No confidence telemetry in \(missing) of \(rows.count)"
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

/// Resolve the archived recording paired with an exact raw transcript. History
/// stores `<stem>_raw.txt` beside `<stem>_raw.m4a`; exact text matching prevents
/// a correction from ever playing a different dictation merely because it was
/// recorded nearby in time.
func archivedAudioURL(configDir: String, rawText: String) -> URL? {
  let root = URL(fileURLWithPath: configDir, isDirectory: true)
    .appendingPathComponent("transcriptions", isDirectory: true)
  guard
    let enumerator = FileManager.default.enumerator(
      at: root,
      includingPropertiesForKeys: [.contentModificationDateKey, .isRegularFileKey],
      options: [.skipsHiddenFiles]
    )
  else { return nil }

  var matches: [(URL, Date)] = []
  for case let transcriptURL as URL in enumerator {
    guard transcriptURL.pathExtension == "txt",
      let text = try? String(contentsOf: transcriptURL, encoding: .utf8),
      text.trimmingCharacters(in: .whitespacesAndNewlines)
        == rawText.trimmingCharacters(in: .whitespacesAndNewlines)
    else { continue }
    let date =
      (try? transcriptURL.resourceValues(forKeys: [.contentModificationDateKey]))?
      .contentModificationDate ?? .distantPast
    matches.append((transcriptURL, date))
  }

  for (transcriptURL, _) in matches.sorted(by: { $0.1 > $1.1 }) {
    for candidate in archivedAudioCandidates(from: transcriptURL) {
      if FileManager.default.fileExists(atPath: candidate.path) { return candidate }
    }
  }
  return nil
}

/// `<stem>_raw.txt` sits beside `<stem>_raw.m4a`. A colliding second text file
/// (`_raw_1.txt`) must still find the take's audio, not disable Retranscribe.
func archivedAudioCandidates(from transcriptURL: URL) -> [URL] {
  let stem = transcriptURL.deletingPathExtension()
  var stems = [stem]
  let name = stem.lastPathComponent
  if let digits = name.range(of: "_\\d+$", options: .regularExpression) {
    let base = String(name[..<digits.lowerBound])
    stems.append(stem.deletingLastPathComponent().appendingPathComponent(base))
  }
  var candidates: [URL] = []
  for next in stems {
    for ext in ["m4a", "wav", "flac"] {
      candidates.append(next.appendingPathExtension(ext))
    }
  }
  return candidates
}

/// Honest Dictionary headline.
/// Every custom lexicon variant→canonical is a **live rule** the engine applies.
/// Corrections are real diffs; takes that changed nothing are counted apart.
func dictionaryHeadline(
  corrections: Int, vocabularyCorrections: Int, unchangedTakes: Int, rulesLearned: Int
) -> String {
  "\(corrections) corrections (\(vocabularyCorrections) vocabulary) · \(unchangedTakes) unchanged takes · \(rulesLearned) rules in dictionary"
}

func dictionarySubtitle(
  correctionsRecorded: Int,
  rulesLearned: Int,
  taughtFromCorrections: Int,
  totalEntries: Int
) -> String {
  if rulesLearned > 0 {
    return
      "\(rulesLearned) live rules (variant→canonical) · \(taughtFromCorrections) with correction provenance · \(totalEntries) store rows."
  }
  if correctionsRecorded > 0 {
    return
      "\(correctionsRecorded) corrections on disk · dictionary empty — Teach explicitly promotes eligible store pairs now."
  }
  return
    "Correction history and custom dictionary. Teach is explicit bulk promotion; automatic learning still needs 3 matching human corrections."
}

/// NSSound plays independently of the view that started it — playback used to
/// survive Previous/Next and even closing the Settings window, with no way to
/// stop it (operator, 2026-08-09). The delegate flips the button back to Play
/// when the file ends on its own.
private final class VoiceLabPlaybackDelegate: NSObject, NSSoundDelegate {
  var onFinish: (() -> Void)?
  func sound(_ sound: NSSound, didFinishPlaying flag: Bool) {
    DispatchQueue.main.async { self.onFinish?() }
  }
}

struct VoiceLabPanel: View {
  @ObservedObject var model: SettingsViewModel
  @State private var editor = VoiceLabEditorState()
  @FocusState private var focusedCorrectionID: String?
  @State private var correctionIndex = 0
  @State private var lexiconIndex = 0
  @State private var ruleCandidateIndex = 0
  @State private var showFullText = false
  @State private var playbackSound: NSSound?
  @State private var playbackMessage: String?
  @State private var playingRowID: String?
  @State private var helperCompare: String?
  @State private var helperText: String?
  @State private var helperPending = false
  @State private var playbackDelegate = VoiceLabPlaybackDelegate()

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

  /// Corrections that contain at least one vocabulary-tier span.
  private var vocabularyCorrectionsCount: Int {
    corrections.lazy.filter { row in
      voiceLabDiffSpans(raw: row.rawText, edited: row.editedText)
        .contains { $0.tier == .vocabulary }
    }.count
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(alignment: .top, spacing: 12) {
        VStack(alignment: .leading, spacing: 0) {
          SettingsPageHeader(
            dictionaryHeadline(
              corrections: correctionsRecordedCount,
              vocabularyCorrections: vocabularyCorrectionsCount,
              unchangedTakes: Int(clamping: model.unchangedQualityTakes),
              rulesLearned: rulesLearnedCount
            ),
            blurb: dictionarySubtitle(
              correctionsRecorded: correctionsRecordedCount,
              rulesLearned: rulesLearnedCount,
              taughtFromCorrections: taughtFromCorrectionsCount,
              totalEntries: model.customLexiconEntries.count
            )
          )
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
          .csFocusRing()
          .disabled(model.voiceLabTeachPending)
          .accessibilityLabel("Teach dictionary from corrections and proposed rules")
          Button("Refresh") {
            model.refreshVoiceLab()
          }
          .font(CSFont.mono(11, .semibold))
          .foregroundStyle(CSColor.chromeAccent)
          .csFocusRing()
          .accessibilityLabel("Refresh \(SettingsSection.voiceLab.title) data")
        }
      }

      if ruleCandidatesSectionVisible(model.ruleCandidates) {
        SettingsSectionLabel("Suggested rules · \(model.ruleCandidates.count)")
          .padding(.top, CSSpace.section)
        ruleCandidatesSection
          .padding(.top, CSSpace.control)
      }

      SettingsSectionLabel("Recent corrections · \(corrections.count)")
        .padding(.top, CSSpace.section)
      if let telemetryLine = missingTelemetryLine(rows: corrections) {
        Text(telemetryLine)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(Color.secondary)
          .padding(.top, 4)
      }
      correctionsSection
        .padding(.top, CSSpace.control)

      SettingsSectionLabel("Custom dictionary · \(model.customLexiconEntries.count)")
        .padding(.top, CSSpace.section)
      lexiconSection
        .padding(.top, CSSpace.control)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
    // The sound belongs to the visible row: navigating away from the row
    // or from the panel ends it. Without these, NSSound kept playing after
    // the Settings window was closed.
    .onChange(of: correctionIndex) {
      stopPlayback()
      showFullText = false
    }
    .onDisappear { stopPlayback() }
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
        VStack(alignment: .leading, spacing: 12) {
          HStack(spacing: 8) {
            Text("CORRECTION")
              .font(CSFont.mono(10.5, .semibold))
              .foregroundStyle(row.isLowConfidence ? CSColor.terracotta : CSColor.oliveLight)
            if row.isLowConfidence {
              Text("LOW CONFIDENCE")
                .font(CSFont.mono(9.5, .semibold))
                .foregroundStyle(CSColor.terracotta)
                .padding(.horizontal, 7)
                .padding(.vertical, 3)
                .background(Capsule().fill(CSColor.terracotta.opacity(0.12)))
            }
            // Deferred-correction desk: sessions closed without an
            // overlay edit land here as UNREVIEWED, awaiting Edit.
            if row.action == "close-unreviewed" {
              Text("UNREVIEWED")
                .font(CSFont.mono(9.5, .semibold))
                .foregroundStyle(CSColor.chromeAccent)
                .padding(.horizontal, 7)
                .padding(.vertical, 3)
                .background(Capsule().fill(CSColor.chromeAccent.opacity(0.12)))
            }
            Spacer(minLength: 0)
            Button {
              togglePlayback(row)
            } label: {
              Label(
                playingRowID == row.id ? "Stop" : "Play original",
                systemImage: playingRowID == row.id ? "stop.fill" : "play.fill"
              )
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .accessibilityLabel(
              playingRowID == row.id
                ? "Stop playing the original recording"
                : "Play the original recording for this correction"
            )
            Button("Retranscribe") {
              startHelperRetranscribe(row)
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(
              helperPending
                || helperRetranscribePass(asrMode: model.asrModeId) == nil
                || archivedAudioURL(configDir: model.configDir, rawText: row.rawText) == nil
            )
            .accessibilityLabel("Retranscribe this take on the helper engine")
            if let helperText, !helperText.isEmpty {
              Button("Use helper as correction") {
                editor.begin(row)
                editor.canonical = helperText
              }
              .buttonStyle(.bordered)
              .controlSize(.small)
              .disabled(helperPending)
              .accessibilityLabel("Load helper text into the correction editor without saving")
            }
          }
          if row.hasTextChange {
            let spans = voiceLabDiffSpans(raw: row.rawText, edited: row.editedText)
            let major = majorDiffSpans(spans)
            let minor = minorDiffSpans(spans)
            VStack(alignment: .leading, spacing: 5) {
              Text("CHANGED")
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(Color.secondary)
              VStack(alignment: .leading, spacing: 8) {
                ForEach(Array(major.enumerated()), id: \.offset) { _, span in
                  diffSpanText(span)
                    .font(CSFont.ui(13, .medium))
                    .textSelection(.enabled)
                }
                if !minor.isEmpty {
                  DisclosureGroup {
                    VStack(alignment: .leading, spacing: 6) {
                      ForEach(Array(minor.enumerated()), id: \.offset) { _, span in
                        diffSpanText(span)
                          .font(CSFont.ui(12, .medium))
                          .foregroundStyle(Color.secondary)
                          .textSelection(.enabled)
                      }
                    }
                    .padding(.top, 4)
                  } label: {
                    Text(minorAdjustmentsSummary(minor))
                      .font(CSFont.mono(10, .medium))
                      .foregroundStyle(Color.secondary)
                  }
                }
              }
              .frame(maxWidth: .infinity, alignment: .leading)
              .padding(11)
              .background(
                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                  .fill(
                    (row.isLowConfidence ? CSColor.terracotta : Color.primary.opacity(0.08))
                      .opacity(row.isLowConfidence ? 0.12 : 1)
                  )
              )
            }
          } else {
            Text("No text change — kept for its confidence telemetry.")
              .font(CSFont.ui(12, .medium))
              .foregroundStyle(Color.secondary)
          }
          if !row.telemetryChips.isEmpty {
            HStack(spacing: 6) {
              ForEach(Array(row.telemetryChips.enumerated()), id: \.offset) { _, chip in
                Text(chip)
                  .font(CSFont.mono(9.5, .semibold))
                  .foregroundStyle(row.isLowConfidence ? CSColor.terracotta : CSColor.oliveLight)
                  .padding(.horizontal, 7)
                  .padding(.vertical, 3)
                  .background(
                    Capsule().fill(
                      (row.isLowConfidence ? CSColor.terracotta : CSColor.oliveLight)
                        .opacity(0.12)
                    )
                  )
              }
            }
          }
          if let playbackMessage {
            Text(playbackMessage)
              .font(CSFont.ui(10.5))
              .foregroundStyle(Color.secondary)
          }
          if let helperCompare {
            Text(helperCompare)
              .font(CSFont.mono(11, .medium))
              .foregroundStyle(CSColor.oliveLight)
              .textSelection(.enabled)
          }
          Button {
            showFullText.toggle()
          } label: {
            HStack(spacing: 6) {
              Image(systemName: showFullText ? "chevron.down" : "chevron.right")
                .font(.system(size: 9, weight: .semibold))
              Text(
                showFullText
                  ? "Hide full transcript"
                  : "Full transcript · raw \(row.rawText.count) chars · edited \(row.editedText.count) chars"
              )
              .font(CSFont.mono(10.5, .medium))
            }
            .foregroundStyle(CSColor.chromeAccent)
          }
          .buttonStyle(.plain)
          .accessibilityLabel("Toggle the full transcript for this correction")
          if showFullText {
            VStack(alignment: .leading, spacing: 10) {
              fullTextBlock("RAW STT · \(row.rawText.count) CHARS", text: row.rawText)
              fullTextBlock(
                "DELIVERED AFTER FORMATTING · \(row.variant.count) CHARS", text: row.variant)
              if normalizedCorrectionText(row.editedText) != normalizedCorrectionText(row.variant)
              {
                fullTextBlock("EDITED · \(row.editedText.count) CHARS", text: row.editedText)
              }
            }
          }
          if editor.correctionID == row.id {
            VStack(alignment: .leading, spacing: 8) {
              Text("CORRECTED ORIGINAL")
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(CSColor.chromeAccent)
              TextEditor(text: $editor.canonical)
                .focused($focusedCorrectionID, equals: row.id)
                .font(CSFont.ui(13))
                .scrollContentBackground(.hidden)
                .frame(minHeight: 120, idealHeight: 180, maxHeight: 320)
                .padding(8)
                .background(
                  RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .fill(Color.primary.opacity(0.08))
                )
                .overlay(
                  RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .strokeBorder(CSColor.chromeAccent.opacity(0.32), lineWidth: 1)
                )
                .onExitCommand { editor.cancel() }
                .overlay {
                  CSFocusOutline(
                    isFocused: focusedCorrectionID == row.id, cornerRadius: CSRadius.input)
                }
                .accessibilityLabel("Correct the original transcript")
              HStack {
                Spacer()
                Button("Cancel") { editor.cancel() }
                Button("Save correction") { saveEdit(row) }
                  .disabled(
                    editor.canonical.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                      || model.voiceLabEditPending.contains(row.id)
                  )
              }
            }
          } else {
            HStack {
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
              .foregroundStyle(CSColor.terracotta)
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
          .foregroundStyle(Color.secondary)
        }
        .settingsGroupedInset()
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
            .foregroundStyle(Color.secondary)
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

  private func startHelperRetranscribe(_ row: VoiceLabCorrectionRow) {
    let archived = archivedAudioURL(configDir: model.configDir, rawText: row.rawText)
    switch HelperFilePass.request(asrMode: model.asrModeId, archivedAudio: archived) {
    case .failure(.noHelper):
      helperText = nil
      helperCompare = "No helper in Apple-only — pick Local power or Cloud."
    case .failure(.noArchivedAudio):
      helperText = nil
      helperCompare = "No archived audio for this row — will not fall back to last_session.wav."
    case .success(let (pass, prefixed)):
      helperPending = true
      helperText = nil
      helperCompare = "Running \(pass.visibleName) on archived audio…"
      Task { @MainActor in
        defer { helperPending = false }
        do {
          let engine = AppModel.shared.overlay.state.engine ?? ControllerDictationEngine()
          let result = try await engine.transcribeFile(path: prefixed)
          let next = result.text.trimmingCharacters(in: .whitespacesAndNewlines)
          helperText = next
          helperCompare = HelperFilePass.compare(daily: row.rawText, helper: next, pass: pass)
        } catch {
          helperText = nil
          helperCompare =
            "Helper \(pass.visibleName) failed: \(error.userFacingMessage)"
        }
      }
    }
  }

  /// Play/stop toggle for one row. Playback stops on navigation and when the
  /// panel disappears — the sound must never outlive the row it belongs to.
  private func togglePlayback(_ row: VoiceLabCorrectionRow) {
    if playingRowID == row.id {
      stopPlayback()
      return
    }
    playbackSound?.stop()
    guard let url = archivedAudioURL(configDir: model.configDir, rawText: row.rawText),
      let sound = NSSound(contentsOf: url, byReference: true)
    else {
      playingRowID = nil
      playbackMessage = "Original audio is unavailable for this legacy correction."
      return
    }
    playbackDelegate.onFinish = { stopPlayback() }
    sound.delegate = playbackDelegate
    playbackSound = sound
    playingRowID = row.id
    playbackMessage = "Playing \(url.lastPathComponent)"
    sound.play()
  }

  private func stopPlayback() {
    playbackSound?.stop()
    playbackSound = nil
    playingRowID = nil
    playbackMessage = nil
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
            .foregroundStyle(Color.secondary)
            .textSelection(.enabled)
          Text("→")
            .font(CSFont.mono(11, .semibold))
            .foregroundStyle(CSColor.chromeAccent)
          Text(row.canonical)
            .font(CSFont.mono(11.5, .semibold))
            .foregroundStyle(Color.primary)
            .textSelection(.enabled)
          Spacer(minLength: 0)
          Text(row.source)
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(Color.secondary)
        }
        .settingsGroupedInset()
        .accessibilityLabel("\(row.variant) to \(row.canonical), source \(row.source)")
        HStack {
          Button("Previous") { lexiconIndex = max(0, safeIndex - 1) }
            .disabled(safeIndex == 0)
          Spacer()
          Text("\(safeIndex + 1) of \(model.customLexiconEntries.count)")
            .font(CSFont.mono(10.5, .medium))
            .foregroundStyle(Color.secondary)
          Spacer()
          Button("Next") {
            lexiconIndex = min(model.customLexiconEntries.count - 1, safeIndex + 1)
          }
          .disabled(safeIndex == model.customLexiconEntries.count - 1)
        }
      }
    }
  }

  @ViewBuilder
  private var ruleCandidatesSection: some View {
    if let error = model.voiceLabReadError {
      readError(error)
    } else if !model.ruleCandidates.isEmpty {
      VStack(spacing: 8) {
        let safeIndex = min(ruleCandidateIndex, model.ruleCandidates.count - 1)
        let candidate = model.ruleCandidates[safeIndex]
        VStack(alignment: .leading, spacing: 10) {
          HStack(spacing: 10) {
            Text(candidate.target)
              .font(CSFont.mono(11.5, .semibold))
              .foregroundStyle(Color.primary)
              .textSelection(.enabled)
            Spacer(minLength: 0)
            Text("\(candidate.occurrences) occurrences")
              .font(CSFont.mono(10, .medium))
              .foregroundStyle(Color.secondary)
          }
          VStack(alignment: .leading, spacing: 4) {
            ForEach(Array(candidate.variants.enumerated()), id: \.offset) { _, variant in
              HStack(spacing: 8) {
                Text(variant)
                  .font(CSFont.mono(11, .medium))
                  .foregroundStyle(Color.secondary)
                  .textSelection(.enabled)
                Spacer(minLength: 0)
                Button("Teach") {
                  model.teachRuleCandidate(target: candidate.target, variant: variant)
                }
                .font(CSFont.mono(10.5, .semibold))
                .controlSize(.small)
                .disabled(model.voiceLabTeachPending)
              }
            }
          }
        }
        .settingsGroupedInset()
        .accessibilityLabel("Suggested rule for \(candidate.target) from \(candidate.variants.count) variants")
        HStack {
          Button("Previous") { ruleCandidateIndex = max(0, safeIndex - 1) }
            .disabled(safeIndex == 0)
          Spacer()
          Text("\(safeIndex + 1) of \(model.ruleCandidates.count)")
            .font(CSFont.mono(10.5, .medium))
            .foregroundStyle(Color.secondary)
          Spacer()
          Button("Next") {
            ruleCandidateIndex = min(model.ruleCandidates.count - 1, safeIndex + 1)
          }
          .disabled(safeIndex == model.ruleCandidates.count - 1)
        }
      }
    }
  }

  private func emptyState(_ message: String) -> some View {
    Text(message)
      .font(CSFont.ui(12.5))
      .lineSpacing(2)
      .foregroundStyle(Color.secondary)
      .frame(maxWidth: .infinity, alignment: .leading)
      .settingsGroupedInset()
  }

  private func readError(_ error: String) -> some View {
    Text("Live quality data is unavailable: \(error)")
      .font(CSFont.ui(12.5))
      .foregroundStyle(CSColor.terracotta)
      .frame(maxWidth: .infinity, alignment: .leading)
      .settingsGroupedInset()
  }

  private func timestampLabel(_ timestampMs: UInt64) -> String {
    Date(timeIntervalSince1970: Double(timestampMs) / 1000.0)
      .formatted(date: .abbreviated, time: .shortened)
  }

  private func fullTextBlock(_ title: String, text: String) -> some View {
    VStack(alignment: .leading, spacing: 5) {
      Text(title)
        .font(CSFont.mono(10, .semibold))
        .foregroundStyle(Color.secondary)
      Text(text)
        .font(CSFont.ui(12.5, .medium))
        .foregroundStyle(Color.secondary)
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
  }

}

#if DEBUG
  #Preview("Settings — Dictionary") {
    SettingsView(model: SettingsViewModel.preview(.voiceLab))
      .frame(width: 960, height: 720)
  }
#endif
