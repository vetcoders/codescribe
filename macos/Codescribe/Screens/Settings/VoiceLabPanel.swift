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

  /// Where the rule came from, in the interface language. `source` is the
  /// stored provenance code and stays identity (R6); this is the only form
  /// that reaches the screen or VoiceOver, so both say the same thing.
  var localizedOrigin: String {
    LexiconSourceLabel.text(for: source)
  }
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
  String(localized: "+\(minor.count) minor (punctuation, casing)")
}

/// One flowing line: context in secondary, removed words struck through,
/// corrected words emphasized. Word wrapping comes free from Text.
private func diffSpanText(_ span: CsDiffSpan) -> Text {
  var result = Text(verbatim: "")
  if !span.contextBefore.isEmpty {
    result = result + Text(span.contextBefore + " ").foregroundStyle(Color.secondary)
  }
  if !span.raw.isEmpty {
    result =
      result
      + Text(span.raw).strikethrough().foregroundStyle(CSColor.terracotta)
      + Text(verbatim: " ")
  }
  if !span.edited.isEmpty {
    if !span.raw.isEmpty {
      result = result + Text(verbatim: "→ ").foregroundStyle(CSColor.chromeAccent)
    }
    result =
      result
      + Text(span.edited).fontWeight(.semibold).foregroundStyle(Color.primary)
      + Text(verbatim: " ")
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
  return String(localized: "No confidence telemetry in \(missing) of \(rows.count)")
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
/// recorded nearby in time. When more than one archived take carries the same
/// transcript the pairing is ambiguous and nothing is chosen: the record holds
/// no take identity, and the newest match is not evidence (operator QC
/// 2026-10-08).
func archivedAudioLookup(configDir: String, rawText: String) -> ArchivedAudioLookup {
  let root = URL(fileURLWithPath: configDir, isDirectory: true)
    .appendingPathComponent("transcriptions", isDirectory: true)
  guard
    let enumerator = FileManager.default.enumerator(
      at: root,
      includingPropertiesForKeys: [.isRegularFileKey],
      options: [.skipsHiddenFiles]
    )
  else { return .missing }

  let wanted = rawText.trimmingCharacters(in: .whitespacesAndNewlines)
  var audio: [URL] = []
  for case let transcriptURL as URL in enumerator {
    guard transcriptURL.pathExtension == "txt",
      let text = try? String(contentsOf: transcriptURL, encoding: .utf8),
      text.trimmingCharacters(in: .whitespacesAndNewlines) == wanted
    else { continue }
    for candidate in archivedAudioCandidates(from: transcriptURL)
    where FileManager.default.fileExists(atPath: candidate.path) {
      let resolved = candidate.standardizedFileURL
      if !audio.contains(resolved) { audio.append(resolved) }
      break
    }
  }
  switch audio.count {
  case 0: return .missing
  case 1: return .found(audio[0])
  default: return .ambiguous(audio.count)
  }
}

/// The same pairing, run off the main actor. The walk reads every archived
/// transcript, so neither a view body nor a UI task may call the sync walker
/// directly (review, 2026-10-09).
func pairedArchivedAudio(configDir: String, rawText: String) async -> ArchivedAudioLookup {
  await Task.detached(priority: .userInitiated) {
    archivedAudioLookup(configDir: configDir, rawText: rawText)
  }.value
}

/// The archived recording when exactly one take matches; see `archivedAudioLookup`.
func archivedAudioURL(configDir: String, rawText: String) -> URL? {
  archivedAudioLookup(configDir: configDir, rawText: rawText).url
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

/// NSSound plays independently of the view that started it — playback used to
/// survive Previous/Next and even closing the Settings window, with no way to
/// stop it (operator, 2026-08-09). The delegate flips the button back to Play
/// when the file ends on its own.
private final class VoiceLabPlaybackDelegate: NSObject, NSSoundDelegate {
  var onFinish: ((ObjectIdentifier) -> Void)?
  func sound(_ sound: NSSound, didFinishPlaying flag: Bool) {
    let finished = ObjectIdentifier(sound)
    DispatchQueue.main.async { self.onFinish?(finished) }
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
  @State private var playbackLease: CsAudioReadLease?
  @State private var playbackTask: Task<Void, Never>?
  @State private var playbackMessage: String?
  @State private var playingRowID: String?
  @State private var helperCompare: String?
  @State private var helperText: String?
  @State private var helperPending = false
  @State private var playbackDelegate = VoiceLabPlaybackDelegate()
  @State private var confirmingLearn = false
  @State private var showingDiagnostics = false
  /// Archived-audio pairing per correction id, computed once off the main
  /// actor. The walk reads every archived transcript, so it must not run
  /// inside the view body (review, 2026-10-09).
  @State private var audioLookups: [String: ArchivedAudioLookup] = [:]
  /// Bumped when the records reload so a card whose id did not change still
  /// pairs its audio again.
  @State private var audioLookupGeneration = 0

  private var corrections: [VoiceLabCorrectionRow] {
    qualityCorrectionRows(model.qualityRecords)
  }

  /// Every flattened lexicon pair is a rule PostProcessor applies.
  private var activeRulesCount: Int {
    model.customLexiconEntries.count
  }

  private var rulesFromCorrectionsCount: Int {
    model.customLexiconEntries.lazy.filter { $0.source == "correction" }.count
  }

  private var rulesAddedByHandCount: Int {
    model.customLexiconEntries.lazy.filter { $0.source == "manual" }.count
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(alignment: .top, spacing: 12) {
        VStack(alignment: .leading, spacing: 0) {
          SettingsPageHeader(
            String(localized: "Dictionary and corrections"),
            blurb: String(
              localized:
                "Browse corrected transcripts and the rules that help recognize your vocabulary."
            )
          )
          countersRow
            .padding(.top, 10)
          if let teachMsg = model.voiceLabTeachMessage {
            Text(teachMsg)
              .font(CSFont.mono(11, .medium))
              .foregroundStyle(CSColor.oliveLight)
              .padding(.top, 8)
              .accessibilityIdentifier("dictionary-learn-result")
          }
        }
        Spacer(minLength: 0)
        HStack(spacing: 12) {
          Button(String(localized: "Learn from corrections…", comment: "Dictionary: button")) {
            confirmingLearn = true
          }
          .font(CSFont.mono(11, .semibold))
          .foregroundStyle(CSColor.chromeAccent)
          .csFocusRing()
          .disabled(model.voiceLabTeachPending)
          .accessibilityLabel("Learn dictionary rules from all saved corrections")
          .confirmationDialog(
            Text("Learn from corrections?", comment: "Dictionary: confirmation title"),
            isPresented: $confirmingLearn,
            titleVisibility: .visible
          ) {
            Button(String(localized: "Learn", comment: "Dictionary: confirmation button")) {
              model.teachDictionaryFromStore()
            }
            Button("Cancel", role: .cancel) {}
          } message: {
            Text(learnScopeMessage(corrections: Int(clamping: model.totalQualityCorrections)))
          }
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
        SettingsSectionLabel(
          String(localized: "Suggested rules · \(model.ruleCandidates.count)")
        )
        .padding(.top, CSSpace.section)
        ruleCandidatesSection
          .padding(.top, CSSpace.control)
      }

      SettingsSectionLabel(String(localized: "Recent corrections · \(corrections.count)"))
        .padding(.top, CSSpace.section)
      if let telemetryLine = missingTelemetryLine(rows: corrections) {
        // About the records, not the engine: it stays out of the way.
        DisclosureGroup(isExpanded: $showingDiagnostics) {
          Text(telemetryLine)
            .font(CSFont.mono(10.5, .medium))
            .foregroundStyle(Color.secondary)
            .padding(.top, 4)
            .accessibilityIdentifier("dictionary-telemetry-coverage")
        } label: {
          Text("Diagnostic details", comment: "Dictionary: disclosure over telemetry coverage")
            .font(CSFont.mono(10.5, .medium))
            .foregroundStyle(Color.secondary)
        }
        .padding(.top, 4)
        .accessibilityIdentifier("dictionary-diagnostics")
      }
      correctionsSection
        .padding(.top, CSSpace.control)

      SettingsSectionLabel(
        String(localized: "My rules · \(model.customLexiconEntries.count)")
      )
      .padding(.top, CSSpace.section)
      if let provenance = lexiconProvenanceLine(
        fromCorrections: rulesFromCorrectionsCount,
        addedByHand: rulesAddedByHandCount,
        other: activeRulesCount - rulesFromCorrectionsCount - rulesAddedByHandCount
      ) {
        Text(provenance)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(Color.secondary)
          .padding(.top, 4)
          .accessibilityIdentifier("dictionary-rules-provenance")
      }
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

  /// Three separate counts; the catalog inflects each one.
  private var countersRow: some View {
    HStack(spacing: 8) {
      ForEach(
        Array(
          dictionaryCounters(
            corrections: Int(clamping: model.totalQualityCorrections),
            unchangedTakes: Int(clamping: model.unchangedQualityTakes),
            activeRules: activeRulesCount
          ).enumerated()), id: \.offset
      ) { _, counter in
        Text(counter)
          .font(CSFont.mono(11, .semibold))
          .foregroundStyle(Color.primary)
          .padding(.horizontal, 10)
          .padding(.vertical, 5)
          .background(Capsule().fill(Color.primary.opacity(0.08)))
      }
    }
    .accessibilityElement(children: .combine)
    .accessibilityIdentifier("dictionary-counters")
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
        let audioLookup = audioLookups[row.id]
        // Nil while the archive walk is still running: the button stays
        // disabled without claiming that the recording is missing.
        let retranscribeReason: String? = audioLookup.flatMap {
          retranscribeUnavailableReason(
            asrMode: model.asrModeId, lookup: $0, pending: helperPending)
        }
        VStack(alignment: .leading, spacing: 12) {
          HStack(spacing: 8) {
            Text(
              String(
                localized: "dictionary.correction.header", defaultValue: "Correction",
                comment: "Dictionary: header of one correction card")
            )
            .textCase(.uppercase)
            .font(CSFont.mono(10.5, .semibold))
            .foregroundStyle(row.isLowConfidence ? CSColor.terracotta : CSColor.oliveLight)
            .accessibilityAddTraits(.isHeader)
            .accessibilityLabel(
              "Heard \(row.variant). Current correction \(row.editedText). Revision \(row.revision)."
            )
            .accessibilityIdentifier("dictionary-correction-summary")
            if row.isLowConfidence {
              Text("Low confidence", comment: "Dictionary badge: the engine was unsure here")
                .textCase(.uppercase)
                .font(CSFont.mono(9.5, .semibold))
                .foregroundStyle(CSColor.terracotta)
                .padding(.horizontal, 7)
                .padding(.vertical, 3)
                .background(Capsule().fill(CSColor.terracotta.opacity(0.12)))
            }
            // Deferred-correction desk: sessions closed without an
            // overlay edit land here as UNREVIEWED, awaiting Edit.
            if row.action == "close-unreviewed" {
              Text(
                "Unreviewed",
                comment: "Dictionary badge: the session closed before this take was reviewed"
              )
              .textCase(.uppercase)
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
            .disabled(retranscribeReason != nil || audioLookup == nil)
            .accessibilityLabel("Retranscribe this take on the helper engine")
            .help(retranscribeReason ?? "")
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
          if let retranscribeReason, !helperPending {
            Text(retranscribeReason)
              .font(CSFont.ui(10.5))
              .foregroundStyle(Color.secondary)
              .accessibilityIdentifier("dictionary-retranscribe-reason")
          }
          let stages = correctionStageDiffs(
            raw: row.rawText, delivered: row.variant, edited: row.editedText)
          if !stages.isEmpty {
            VStack(alignment: .leading, spacing: 5) {
              Text(
                "Differences between versions",
                comment: "Dictionary: header above the compared versions of the text"
              )
              .textCase(.uppercase)
              .font(CSFont.mono(10, .semibold))
              .foregroundStyle(Color.secondary)
              VStack(alignment: .leading, spacing: 10) {
                ForEach(Array(stages.enumerated()), id: \.offset) { stageIndex, stage in
                  stageDiffBlock(stage, index: stageIndex, showTitle: stages.count > 1)
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
                  ? String(localized: "Hide full comparison")
                  : fullComparisonLabel(
                    rawCount: row.rawText.count, editedCount: row.editedText.count)
              )
              .font(CSFont.mono(10.5, .medium))
            }
            .foregroundStyle(CSColor.chromeAccent)
          }
          .buttonStyle(.plain)
          .accessibilityLabel("Toggle the full comparison for this correction")
          .accessibilityValue(showFullText ? "Expanded" : "Collapsed")
          .accessibilityIdentifier("dictionary-full-transcript-toggle")
          if showFullText {
            VStack(alignment: .leading, spacing: 10) {
              fullTextBlock("Raw STT · \(row.rawText.count) characters", text: row.rawText)
              if normalizedCorrectionText(row.variant) != normalizedCorrectionText(row.rawText) {
                fullTextBlock(
                  "After formatting · \(row.variant.count) characters", text: row.variant)
              }
              if normalizedCorrectionText(row.editedText) != normalizedCorrectionText(row.variant) {
                fullTextBlock(
                  "After your correction · \(row.editedText.count) characters",
                  text: row.editedText)
              }
            }
          }
          if editor.correctionID == row.id {
            VStack(alignment: .leading, spacing: 8) {
              Text(
                "Corrected text",
                comment: "Dictionary: header above the editor holding the corrected text"
              )
              .textCase(.uppercase)
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
          Text(
            correctionFooter(
              action: row.action, revision: row.revision,
              timestamp: timestampLabel(row.timestampMs))
          )
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(Color.secondary)
          .accessibilityIdentifier("dictionary-correction-footer")
        }
        .settingsGroupedInset()
        // Keep controls and selectable text as children; the summary belongs
        // to the static header, not to the mixed AppKit/SwiftUI container.
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("dictionary-correction-card")
        .task(id: [row.id, String(audioLookupGeneration)]) {
          guard audioLookups[row.id] == nil else { return }
          let lookup = await pairedArchivedAudio(configDir: model.configDir, rawText: row.rawText)
          guard !Task.isCancelled else { return }
          audioLookups[row.id] = lookup
        }
        .onChange(of: model.qualityRecords) {
          audioLookups = [:]
          audioLookupGeneration += 1
        }
        if corrections.count > 1 {
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
  }

  /// One stage of the comparison: its spans with what each one did, minor
  /// casing/punctuation adjustments collapsed.
  @ViewBuilder
  private func stageDiffBlock(_ stage: CorrectionStageDiff, index: Int, showTitle: Bool)
    -> some View
  {
    let major = majorDiffSpans(stage.spans)
    let minor = minorDiffSpans(stage.spans)
    VStack(alignment: .leading, spacing: 8) {
      if showTitle {
        Text(stage.stage.title)
          .font(CSFont.mono(10, .semibold))
          .foregroundStyle(Color.secondary)
          .accessibilityAddTraits(.isHeader)
      }
      ForEach(Array(major.enumerated()), id: \.offset) { spanIndex, span in
        HStack(alignment: .firstTextBaseline, spacing: 8) {
          Text(diffSpanKind(span).label)
            .textCase(.uppercase)
            .font(CSFont.mono(9.5, .semibold))
            .foregroundStyle(Color.secondary)
            .frame(minWidth: 64, alignment: .leading)
          diffSpanText(span)
            .font(CSFont.ui(13, .medium))
            .textSelection(.enabled)
        }
        .accessibilityIdentifier("dictionary-major-diff-\(index)-\(spanIndex)")
      }
      if !minor.isEmpty {
        DisclosureGroup {
          VStack(alignment: .leading, spacing: 6) {
            ForEach(Array(minor.enumerated()), id: \.offset) { spanIndex, span in
              diffSpanText(span)
                .font(CSFont.ui(12, .medium))
                .foregroundStyle(Color.secondary)
                .textSelection(.enabled)
                .accessibilityIdentifier("dictionary-minor-diff-\(index)-\(spanIndex)")
            }
          }
          .padding(.top, 4)
        } label: {
          Text(minorAdjustmentsSummary(minor))
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(Color.secondary)
        }
        .accessibilityIdentifier("dictionary-minor-adjustments-\(index)")
      }
    }
  }

  private func saveEdit(_ row: VoiceLabCorrectionRow) {
    if model.finalizeVoiceLabCorrection(id: row.id, canonical: editor.canonical) {
      editor.cancel()
    }
  }

  private func startHelperRetranscribe(_ row: VoiceLabCorrectionRow) {
    helperPending = true
    Task { @MainActor in
      defer { helperPending = false }
      do {
        let lease = try await CodescribeHotkeys().acquireAudioReadLease()
        defer { lease.release() }
        try Task.checkCancellation()
        let archived = await pairedArchivedAudio(
          configDir: lease.rootDirectory(), rawText: row.rawText
        ).url
        try Task.checkCancellation()
        switch HelperFilePass.request(asrMode: model.asrModeId, archivedAudio: archived) {
        case .failure(.noHelper):
          helperText = nil
          helperCompare = String(localized: "No helper in Apple-only — pick Local power or Cloud.")
        case .failure(.noArchivedAudio):
          helperText = nil
          helperCompare = String(
            localized: "No archived audio for this row — will not fall back to last_session.wav.")
        case .success(let (pass, prefixed)):
          helperText = nil
          helperCompare = String(
            localized: "Running \(pass.visibleName) on archived audio…",
            comment: "The placeholder is the name of a speech-to-text pass")
          let engine = AppModel.shared.overlay.state.engine ?? ControllerDictationEngine()
          let result = try await engine.transcribeFile(path: prefixed)
          let next = result.text.trimmingCharacters(in: .whitespacesAndNewlines)
          helperText = next
          helperCompare = HelperFilePass.compare(daily: row.rawText, helper: next, pass: pass)
        }
      } catch {
        helperText = nil
        helperCompare = "Helper file pass failed: \(error.userFacingMessage)"
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
    stopPlayback()
    playingRowID = row.id
    playbackTask = Task { @MainActor in
      do {
        let lease = try await CodescribeHotkeys().acquireAudioReadLease()
        var retainedForPlayback = false
        defer { if !retainedForPlayback { lease.release() } }
        guard !Task.isCancelled, playingRowID == row.id else { return }
        let lookup = await pairedArchivedAudio(
          configDir: lease.rootDirectory(), rawText: row.rawText)
        guard !Task.isCancelled, playingRowID == row.id else { return }
        if case .ambiguous(let count) = lookup {
          stopPlayback()
          playbackMessage = String(
            localized:
              "\(count) archived recordings share this exact transcript, so Codescribe cannot tell which one is this take.",
            comment: "Dictionary: why Retranscribe and playback are disabled; plural")
          return
        }
        guard let url = lookup.url, let sound = NSSound(contentsOf: url, byReference: true)
        else {
          stopPlayback()
          playbackMessage = String(
            localized: "Original audio is not available for this correction.")
          return
        }
        playbackDelegate.onFinish = { finished in
          if playbackSound.map(ObjectIdentifier.init) == finished { stopPlayback() }
        }
        sound.delegate = playbackDelegate
        playbackLease = lease
        retainedForPlayback = true
        playbackSound = sound
        playbackMessage = String(
          localized: "Playing \(url.lastPathComponent)",
          comment: "The placeholder is an audio file name")
        if !sound.play() {
          stopPlayback()
          playbackMessage = String(localized: "Original audio could not be played.")
        }
      } catch {
        guard !Task.isCancelled else { return }
        stopPlayback()
        playbackMessage = error.userFacingMessage
      }
    }
  }

  private func stopPlayback() {
    playbackTask?.cancel()
    playbackTask = nil
    playbackSound?.stop()
    playbackSound = nil
    playbackLease?.release()
    playbackLease = nil
    playingRowID = nil
    playbackMessage = nil
  }

  @ViewBuilder
  private var lexiconSection: some View {
    if let error = model.voiceLabReadError {
      readError(error)
    } else if model.customLexiconEntries.isEmpty {
      emptyState("No rules yet — corrections you save and rules you teach will appear here.")
    } else if model.customLexiconEntries.count <= dictionaryRuleListLimit {
      // A few rules read better as a list than as a pager.
      VStack(spacing: 0) {
        ForEach(Array(model.customLexiconEntries.enumerated()), id: \.offset) { index, row in
          lexiconRow(row)
          if index < model.customLexiconEntries.count - 1 {
            Divider().opacity(0.4)
          }
        }
      }
      .settingsGroupedInset()
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier("dictionary-lexicon-list")
    } else {
      VStack(spacing: 8) {
        let safeIndex = min(lexiconIndex, model.customLexiconEntries.count - 1)
        lexiconRow(model.customLexiconEntries[safeIndex])
          .settingsGroupedInset()
          .accessibilityElement(children: .contain)
          .accessibilityIdentifier("dictionary-lexicon-card")
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

  private func lexiconRow(_ row: CsLexiconEntry) -> some View {
    let origin = LexiconSourceLabel.text(for: row.source)
    return HStack(spacing: 10) {
      Text(row.variant)
        .font(CSFont.mono(11.5, .medium))
        .foregroundStyle(Color.secondary)
        .textSelection(.enabled)
      Text(verbatim: "→")
        .font(CSFont.mono(11, .semibold))
        .foregroundStyle(CSColor.chromeAccent)
      Text(row.canonical)
        .font(CSFont.mono(11.5, .semibold))
        .foregroundStyle(Color.primary)
        .textSelection(.enabled)
      Spacer(minLength: 0)
      Text(origin)
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(Color.secondary)
        .accessibilityLabel("\(row.variant) to \(row.canonical), source \(origin)")
        .accessibilityIdentifier("dictionary-lexicon-summary")
    }
    .padding(.vertical, 6)
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
              .accessibilityAddTraits(.isHeader)
              .accessibilityLabel(
                "Suggested rule for \(candidate.target) from \(candidate.variants.count) variants. \(candidate.occurrences) occurrences."
              )
              .accessibilityIdentifier("dictionary-rule-summary")
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
                .accessibilityLabel("Teach \(variant) as \(candidate.target)")
              }
            }
          }
        }
        .settingsGroupedInset()
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("dictionary-rule-card")
        if model.ruleCandidates.count > 1 {
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
  }

  private func emptyState(_ message: LocalizedStringKey) -> some View {
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

  /// Date and clock follow the interface language, not the system region, so
  /// a Polish interface never shows "10:36 AM" under a Polish date.
  private func timestampLabel(_ timestampMs: UInt64) -> String {
    let locale = InterfaceLanguage.preferred(from: Bundle.main.preferredLocalizations).locale
    return Date(timeIntervalSince1970: Double(timestampMs) / 1000.0)
      .formatted(Date.FormatStyle(date: .abbreviated, time: .shortened, locale: locale))
  }

  private func fullTextBlock(_ title: LocalizedStringKey, text: String) -> some View {
    VStack(alignment: .leading, spacing: 5) {
      Text(title)
        .textCase(.uppercase)
        .font(CSFont.mono(10, .semibold))
        .foregroundStyle(Color.secondary)
        .accessibilityAddTraits(.isHeader)
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
