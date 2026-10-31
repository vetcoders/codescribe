import Foundation

// Backs the tray with the REAL codescribe core via the UniFFI bridge. It is a
// composite over four thin handles:
//   • CodescribeAgent    — assistive-provider readiness gate.
//   • CodescribeHotkeys  — shared legacy controller hotkey/recording spine.
//   • CodescribeConfig    — quick config toggles (settings.json / .env router).
//   • CodescribeThreads   — most-recent transcript path + text.
//
// Dictation routes through CodescribeHotkeys so tray, Agent, and keyboard
// shortcuts share one RecordingController and cannot open parallel recorders.
final class RealTrayEngine: TrayEngine {
  private let agent: CodescribeAgent
  private let hotkeys: CodescribeHotkeys
  private let config: CodescribeConfig
  private let threads: CodescribeThreads

  init(
    agent: CodescribeAgent = CodescribeAgent(),
    hotkeys: CodescribeHotkeys = CodescribeHotkeys(),
    config: CodescribeConfig = CodescribeConfig(),
    threads: CodescribeThreads = CodescribeThreads()
  ) {
    self.agent = agent
    self.hotkeys = hotkeys
    self.config = config
    self.threads = threads
  }

  func isAgentAvailable() -> Bool { agent.isAvailable() }

  func isRecording() async -> Bool { await hotkeys.isRecording() }

  func startRecording(assistive: Bool) async throws {
    let action: StaticString = assistive ? "recording.startAssistive" : "recording.startDictation"
    let interaction = DiagnosticLog.beginInteraction(action: action, surface: "tray")
    do {
      if assistive {
        try await hotkeys.startAssistiveRecording()
      } else {
        try await hotkeys.startRecording()
      }
      interaction.finish(.succeeded, effect: .callbackCompleted, cleanup: .notRequired)
    } catch {
      interaction.finish(
        error is CancellationError ? .cancelled : .failed,
        effect: .unverified, cleanup: .unverified, reason: .operationFailed)
      throw error
    }
  }

  func stopRecording() async throws {
    let interaction = DiagnosticLog.beginInteraction(action: "recording.stop", surface: "tray")
    do {
      try await hotkeys.stopRecording()
      // The bridge callback returned. This does not independently prove device
      // release, delivery or UI cleanup; those require their owner receipts.
      interaction.finish(.succeeded, effect: .callbackCompleted, cleanup: .unverified)
    } catch {
      interaction.finish(
        error is CancellationError ? .cancelled : .failed,
        effect: .unverified, cleanup: .unverified, reason: .operationFailed)
      throw error
    }
  }

  func currentToggles() -> (
    showDockIcon: Bool,
    overlayEnabled: Bool,
    pasteMode: CsPasteMode,
    autoFormatLevel: FormattingPolicyOption,
    notesMode: Bool,
    startInAssistive: Bool,
    holdBadgeOption: HoldBadgeOption
  )? {
    let toggles = config.trayToggles()
    // Automatic level: a stored Max runs Smart at the core's automatic
    // boundary, so the tray shows Smart and cycles from it.
    guard
      let formatLevel = FormattingPolicyOption(rawValue: toggles.formattingLevel)?.automaticLevel
    else {
      return nil
    }
    return (
      toggles.showDockIcon,
      toggles.transcriptionOverlayEnabled,
      toggles.pasteMode,
      formatLevel,
      toggles.notesModeEnabled,
      toggles.startAssistive,
      HoldBadgeOption(
        indicatorEnabled: toggles.holdIndicator,
        size: toggles.holdBadgeSize
      )
    )
  }

  func setQuickToggle(_ toggle: TrayQuickToggle, enabled: Bool) {
    let action: StaticString
    switch toggle {
    case .showDockIcon: action = "settings.showDockIcon"
    case .transcriptionOverlay: action = "settings.transcriptionOverlay"
    }
    _ = recordWrite(action: action) {
      try config.updateConfig(key: toggle.configKey, value: enabled ? "1" : "0")
    }
  }

  func setPasteMode(_ mode: CsPasteMode) {
    _ = recordWrite(action: "settings.pasteMode") { _ = try config.setPasteMode(mode: mode) }
  }

  func setAutoFormatLevel(_ level: FormattingPolicyOption) {
    guard FormattingPolicyOption.automaticCases.contains(level) else { return }
    _ = recordWrite(action: "settings.formatLevel") {
      _ = try config.setAutoFormatLevel(level: level.rawValue)
    }
  }

  func setHoldBadgeOption(_ option: HoldBadgeOption) -> Bool {
    recordWrite(action: "settings.holdBadge") {
      guard let size = option.size else {
        try config.updateConfig(key: "HOLD_INDICATOR", value: "0")
        return
      }
      try config.updateConfigMany(entries: [
        CsConfigEntry(key: "HOLD_INDICATOR", value: "1"),
        CsConfigEntry(key: "HOLD_BADGE_SIZE", value: String(size)),
      ])
    }
  }

  func setNotesMode(_ enabled: Bool) -> Bool {
    // Preserve the owner's atomic two-key write and its failure result.
    recordWrite(action: "settings.notesMode") { _ = try config.setNotesMode(enabled: enabled) }
  }

  func setStartInAssistive(_ enabled: Bool) -> Bool {
    recordWrite(action: "settings.startAssistive") {
      try config.updateConfig(key: "TRAY_START_ASSISTIVE", value: enabled ? "1" : "0")
    }
  }

  private func recordWrite(action: StaticString, _ write: () throws -> Void) -> Bool {
    let interaction = DiagnosticLog.beginInteraction(action: action, surface: "tray")
    do {
      try write()
      interaction.finish(.succeeded, effect: .callbackCompleted, cleanup: .notRequired)
      return true
    } catch {
      interaction.finish(
        .failed, effect: .unverified, cleanup: .notRequired, reason: .operationFailed)
      return false
    }
  }

  func latestHistoryPath() -> String? {
    // Skip failure / no-speech markers: "copy / save last transcript" must land
    // on the newest entry that actually carries copyable text, not a "failed"
    // placeholder (mirrors Rust's `TranscriptKind::is_copyable_transcript`).
    threads.recentHistory(limit: 32).first { $0.kind.isCopyableTranscript }?.path
  }

  func latestTranscriptText() -> String? {
    guard let path = latestHistoryPath() else { return nil }
    return try? threads.readHistoryText(path: path)
  }

  func recentTranscripts(limit: Int) -> [TrayTranscript] {
    threads.recentHistory(limit: UInt32(limit)).map { entry in
      TrayTranscript(
        path: entry.path,
        time: Self.historyTime(entry),
        snippet: Self.historySnippet(entry)
      )
    }
  }

  func transcriptText(forPath path: String) -> String? {
    try? threads.readHistoryText(path: path)
  }

  /// "HH:mm" capture time for the tray's fixed-width time column.
  private static func historyTime(_ entry: CsHistoryEntry) -> String {
    let date = Date(timeIntervalSince1970: TimeInterval(entry.timestampMs) / 1000)
    return timeFormatter.string(from: date)
  }

  /// First words of a history entry; falls back to the file name when the
  /// preview is empty. No character cut here: the core already caps the preview
  /// at one line of 60 characters, and the tray row owns the visible truncation
  /// so every entry ends in the same single ellipsis (Founder brief, round 17,
  /// 2026-10-10).
  private static func historySnippet(_ entry: CsHistoryEntry) -> String {
    let preview = entry.preview.trimmingCharacters(in: .whitespacesAndNewlines)
    return preview.isEmpty ? (entry.path as NSString).lastPathComponent : preview
  }

  private static let timeFormatter: DateFormatter = {
    let formatter = DateFormatter()
    formatter.dateFormat = "HH:mm"
    return formatter
  }()
}

extension CsTranscriptKind {
  /// Mirrors `TranscriptKind::is_copyable_transcript` (core/state/history.rs):
  /// true for entries that carry copyable transcript text, false for the
  /// assistant interpretation and the failure / no-speech marker.
  var isCopyableTranscript: Bool {
    switch self {
    case .raw, .cloud, .formattedTranscript, .formattingFailed:
      return true
    case .assistantInterpretation, .failed:
      return false
    }
  }
}
