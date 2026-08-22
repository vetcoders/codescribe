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

  var confidenceSummary: String {
    var parts: [String] = []
    if let avgLogprob {
      parts.append(String(format: "Whisper logprob %.2f", avgLogprob))
    }
    if let speechPct {
      let percent = speechPct <= 1 ? speechPct * 100 : speechPct
      parts.append(String(format: "Silero/VAD speech %.0f%%", percent))
    }
    if !confidenceFlags.isEmpty {
      parts.append(confidenceFlags.joined(separator: ", "))
    }
    return parts.isEmpty ? "No confidence telemetry was recorded" : parts.joined(separator: " · ")
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

func qualityCorrectionRows(_ records: [CsQualityRecord]) -> [VoiceLabCorrectionRow] {
  records.map { record in
    VoiceLabCorrectionRow(
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
  @State private var correctionIndex = 0
  @State private var lexiconIndex = 0
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

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(alignment: .top, spacing: 12) {
        VStack(alignment: .leading, spacing: 0) {
          EyebrowLabel(text: "Settings · \(SettingsSection.voiceLab.title)")
          Text(
            dictionaryHeadline(
              correctionsRecorded: correctionsRecordedCount,
              rulesLearned: rulesLearnedCount
            )
          )
          .font(CSFont.ui(26, .bold))
          .tracking(-0.5)
          .foregroundStyle(CSColor.textHigh)
          .padding(.top, 6)
          Text(
            dictionarySubtitle(
              correctionsRecorded: correctionsRecordedCount,
              rulesLearned: rulesLearnedCount,
              taughtFromCorrections: taughtFromCorrectionsCount,
              totalEntries: model.customLexiconEntries.count
            )
          )
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
          .csFocusRing(cornerRadius: 8)
          .disabled(model.voiceLabTeachPending)
          .accessibilityLabel("Teach dictionary from corrections and proposed rules")
          Button("Refresh") {
            model.refreshVoiceLab()
          }
          .font(CSFont.mono(11, .semibold))
          .foregroundStyle(CSColor.chromeAccent)
          .csFocusRing(cornerRadius: 8)
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
    // The sound belongs to the visible row: navigating away from the row
    // or from the panel ends it. Without these, NSSound kept playing after
    // the Settings window was closed.
    .onChange(of: correctionIndex) { stopPlayback() }
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
            Text("ORIGINAL STT")
              .font(CSFont.mono(10.5, .semibold))
              .foregroundStyle(row.isLowConfidence ? CSColor.terracottaLight : CSColor.oliveLight)
            Text(row.isLowConfidence ? "LOW CONFIDENCE" : "CONFIDENCE DATA")
              .font(CSFont.mono(9.5, .semibold))
              .foregroundStyle(row.isLowConfidence ? CSColor.terracottaLight : CSColor.textFaintAlt)
              .padding(.horizontal, 7)
              .padding(.vertical, 3)
              .background(
                Capsule().fill(
                  (row.isLowConfidence ? CSColor.terracottaLight : CSColor.oliveLight)
                    .opacity(0.12)
                )
              )
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
          Text(row.rawText)
            .font(CSFont.ui(13, .medium))
            .foregroundStyle(CSColor.textHigh)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(11)
            .background(
              RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .fill(
                  (row.isLowConfidence ? CSColor.terracottaLight : CSColor.surfaceRaised(0.04))
                    .opacity(row.isLowConfidence ? 0.12 : 1)
                )
            )
          Text(row.confidenceSummary)
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(row.isLowConfidence ? CSColor.terracottaLight : CSColor.textFaintAlt)
            .textSelection(.enabled)
          if let playbackMessage {
            Text(playbackMessage)
              .font(CSFont.ui(10.5))
              .foregroundStyle(CSColor.textMutedAlt)
          }
          if let helperCompare {
            Text(helperCompare)
              .font(CSFont.mono(11, .medium))
              .foregroundStyle(CSColor.oliveLight)
              .textSelection(.enabled)
          }
          VStack(alignment: .leading, spacing: 5) {
            Text("DELIVERED AFTER FORMATTING")
              .font(CSFont.mono(10, .semibold))
              .foregroundStyle(CSColor.textFaintAlt)
            Text(row.variant)
              .font(CSFont.ui(12.5, .medium))
              .foregroundStyle(CSColor.textMutedAlt)
              .textSelection(.enabled)
              .frame(maxWidth: .infinity, alignment: .leading)
          }
          if editor.correctionID == row.id {
            VStack(alignment: .leading, spacing: 8) {
              Text("CORRECTED ORIGINAL")
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(CSColor.chromeAccent)
              TextEditor(text: $editor.canonical)
                .font(CSFont.ui(13))
                .scrollContentBackground(.hidden)
                .frame(minHeight: 120, idealHeight: 180, maxHeight: 320)
                .padding(8)
                .background(
                  RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .fill(CSColor.surfaceRaised(0.04))
                )
                .overlay(
                  RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .strokeBorder(CSColor.chromeAccent.opacity(0.32), lineWidth: 1)
                )
                .onExitCommand { editor.cancel() }
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
            "Helper \(pass.visibleName) failed: \(error.localizedDescription)"
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
