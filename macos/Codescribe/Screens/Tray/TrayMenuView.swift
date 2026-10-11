import SwiftUI

// TrayPanel supplies the native glass and anchors this content to NSStatusItem.
// 300pt wide, glass panel, status header bound to runtime, terracotta marking
// ONLY the primary action ("Agent"), Notes / Diagnostics as nested
// disclosure groups. Dictation toggle + quick config toggles are wired through
// the composite TrayEngine.
struct TrayMenuView: View {
  @ObservedObject var viewModel: TrayViewModel
  @ObservedObject var trayStatus: TrayStatusStore
  // macOS 14+ action to open the app's Settings scene — replaces the fragile
  // private `showSettingsWindow:` selector that stopped working on newer macOS.
  // The row goes through `presentSettings()` so the window arrives with
  // Codescribe active: this menu is a non-activating panel.
  @Environment(\.openWindow) private var openWindow
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  var body: some View {
    VStack(spacing: 0) {
      statusHeader
      if trayStatus.showsDetailStatusRow {
        trayStatusRow
      }
      TrayDivider(top: 3, bottom: 5)

      primaryActions

      TrayDivider()
      quickSettingsGroup

      notesGroup
      diagnosticsGroup

      TrayDivider()
      if DeveloperSurface.isEnabled() {
        TrayRow(
          icon: .diagnostics,
          title: String(localized: "Voice Lab")
        ) {
          Task { await VoiceLabRuntime.shared.openConsole() }
        }
      }
      TrayRow(icon: .setupWizard, title: String(localized: "Setup Wizard")) {
        viewModel.onOpenSetupWizard()
      }
      TrayRow(icon: .refresh, title: String(localized: "Check for Updates")) {
        viewModel.onCheckForUpdates()
      }
      TrayRow(icon: .help, title: String(localized: "Help")) { viewModel.onHelp() }
      TrayRow(icon: .info, title: String(localized: "About codescribe")) { viewModel.onAbout() }

      TrayDivider()
      TrayRow(
        icon: .power,
        iconColor: CSColor.terracottaDeep,
        title: String(localized: "Quit"),
        shortcut: "⌘Q"
      ) { viewModel.onQuit() }
    }
    .padding(7)
    .frame(width: 300)
    .transaction { if reduceMotion { $0.disablesAnimations = true } }
    .onAppear { viewModel.refreshStatus() }
    .onDisappear { viewModel.collapseDisclosures() }
  }

  // MARK: - Header (wordmark + runtime-bound status pill)

  private var statusHeader: some View {
    HStack(spacing: 9) {
      Wordmark(size: 14)
      Spacer(minLength: 8)
      // Separate view type on active vs idle/error (same rule as the overlay
      // header): the animated pill exists only for live status phases.
      if trayStatus.shouldRipple {
        StatusPill(
          text: trayStatus.compactLabel,
          color: trayStatus.color,
          rippling: true
        )
      } else {
        StaticStatusPill(text: trayStatus.compactLabel, color: trayStatus.color)
      }
    }
    .padding(.horizontal, 12)
    .padding(.top, CSSpace.control)
    .padding(.bottom, 10)
  }

  private var trayStatusRow: some View {
    HStack(spacing: 7) {
      CSIconView(icon: trayStatus.icon, size: 11, weight: .bold, color: trayStatus.color)
      Text(trayStatus.detailLabel)
        .font(CSFont.ui(12, .medium))
        .foregroundStyle(trayStatus.color)
        .lineLimit(1)
      Spacer(minLength: 0)
    }
    .padding(.horizontal, 11)
    .padding(.vertical, 7)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.chip, style: .continuous)
        .fill(trayStatus.color.opacity(0.10))
    )
    .padding(.horizontal, 5)
    .padding(.bottom, 3)
  }

  // MARK: - Primary actions

  private var primaryActions: some View {
    VStack(spacing: 0) {
      TrayRow(
        icon: .agent,
        title: String(localized: "Agent"),
        titleColor: viewModel.agentAvailable ? CSColor.textBody : CSColor.textFaint,
        titleWeight: .semibold,
        shortcut: "⌥⌥",
        shortcutColor: TrayRow.primaryShortcutColor,
        style: .primary
      ) { viewModel.onShowAgent() }

      TrayRow(icon: .dock, title: String(localized: "Open widget")) {
        viewModel.onOpenWidget()
      }
      .accessibilityIdentifier("tray-open-widget")

      TrayRow(
        icon: viewModel.isRecording && !viewModel.isStartingDictation ? .stop : .record,
        iconColor: recordingActionColor,
        title: recordingActionTitle
      ) { viewModel.toggleDictation() }

      historyGroup
      TrayRow(icon: .copy, title: String(localized: "Copy last transcript")) {
        viewModel.copyLastTranscript()
      }

      // Permission-free "✓ Copied" confirmation after a history / last-transcript
      // copy — reuses the Notes result banner row.
      if let copyStatus = viewModel.copyStatus {
        TrayNoteStatusRow(status: copyStatus)
          .padding(.top, 2)
      }
    }
    .animation(.easeOut(duration: 0.18), value: viewModel.copyStatus)
  }

  // MARK: - History (nested disclosure → copy a recent transcript)

  private var historyGroup: some View {
    VStack(spacing: 0) {
      TrayRow(
        icon: .history,
        title: String(localized: "Transcription history"),
        disclosureExpanded: viewModel.historyExpanded,
        style: viewModel.historyExpanded ? .raised : .plain
      ) {
        withAnimation(TrayDisclosureChevron.animation) { viewModel.toggleHistory() }
      }

      if viewModel.historyExpanded {
        TrayDisclosureChildren {
          if viewModel.historyItems.isEmpty {
            TrayChildRow(title: String(localized: "No transcripts yet"))
          } else {
            ForEach(viewModel.historyItems) { item in
              TrayHistoryRow(time: item.time, snippet: item.snippet) {
                viewModel.copyTranscript(path: item.path)
              }
            }
          }
          TrayChildRow(title: String(localized: "Open history folder")) {
            viewModel.openHistoryFolder()
          }
        }
      }
    }
  }

  // MARK: - Quick config toggles (collapsed by default)

  /// Seven day-to-day toggles live under one disclosure so the cold-open tray
  /// stays short: primary actions first, preferences on demand.
  private var quickSettingsGroup: some View {
    VStack(spacing: 0) {
      TrayRow(
        icon: .settings,
        title: String(localized: "Settings"),
        disclosureExpanded: viewModel.quickSettingsExpanded,
        style: viewModel.quickSettingsExpanded ? .raised : .plain
      ) {
        withAnimation(TrayDisclosureChevron.animation) {
          viewModel.quickSettingsExpanded.toggle()
        }
      }

      if viewModel.quickSettingsExpanded {
        TrayDisclosureChildren {
          quickToggles
        }
      }
    }
  }

  private var quickToggles: some View {
    VStack(spacing: 0) {
      toggleRow(
        icon: .dock,
        title: String(
          localized: "Show icon", comment: "Tray toggle: show the application's Dock icon"),
        isOn: viewModel.showDockIcon
      ) { viewModel.setShowDockIcon($0) }
      toggleRow(
        icon: .overlay,
        title: String(
          localized: "Show overlay", comment: "Tray toggle: enable the transcription overlay"),
        isOn: viewModel.overlayEnabled
      ) { viewModel.setOverlayEnabled($0) }
      autoPasteToggle
      autoFormatMenu
      holdBadgeMenu
      toggleRow(
        icon: .notesMode,
        title: String(localized: "Notes Mode"),
        isOn: viewModel.notesModeEnabled
      ) { viewModel.setNotesMode($0) }
      toggleRow(
        icon: .agent,
        title: String(
          localized: "Start in Agent mode",
          comment: "Tray toggle: new transcriptions started here use Agent mode"),
        isOn: viewModel.startInAssistive,
        onColor: CSColor.assistive
      ) { viewModel.setStartInAssistive($0) }

      TrayRow(
        icon: .settings,
        title: String(localized: "Open Settings"),
        shortcut: "⌘,",
        scale: .child
      ) {
        openWindow.presentSettings()
      }
    }
  }

  /// Auto Paste is a cycling row in the Auto Format grammar: each click
  /// advances Safe → Comfort → Off → Safe (one stored `PASTE_MODE`, the same
  /// choice as Settings › Shortcuts). The current mode sits in the keycap.
  private var autoPasteToggle: some View {
    TrayRow(
      icon: .send,
      title: String(localized: "Auto Paste"),
      shortcut: viewModel.pasteMode.visibleName,
      shortcutColor: viewModel.pasteMode == .off ? CSColor.textFaintAlt : CSColor.oliveLight,
      scale: .child
    ) { viewModel.setPasteMode(viewModel.pasteMode.next) }
    .accessibilityLabel("Auto Paste")
    .accessibilityValue(viewModel.pasteMode.visibleName)
    .accessibilityHint("Cycle automatic paste mode")
  }

  /// Auto Format is a cycling row in the same baseline grammar: each click
  /// advances Off → Correction → Smart → Max → Off. The current level sits in
  /// the trailing keycap slot, so nothing opens over the 300pt popover.
  private var autoFormatMenu: some View {
    TrayRow(
      icon: .edit,
      title: String(localized: "Auto Format"),
      shortcut: viewModel.autoFormatLevel.visibleName,
      shortcutColor: viewModel.autoFormatLevel == .off
        ? CSColor.textFaintAlt : CSColor.oliveLight,
      scale: .child
    ) { viewModel.setAutoFormatLevel(viewModel.autoFormatLevel.next) }
    .accessibilityLabel("Auto Format")
    .accessibilityValue(viewModel.autoFormatLevel.visibleName)
    .accessibilityHint("Cycle automatic formatting level")
  }

  /// Cursor indicator follows the same rolling-row grammar as Auto Format:
  /// Off → 4px → 8px → 12px → Off, with the current value in the keycap.
  /// Founder brief, round 17, 2026-10-10: the label names the place, not the
  /// mechanism — Settings › Hotkeys keeps the long "Recording indicator" copy.
  private var holdBadgeMenu: some View {
    TrayRow(
      icon: .record,
      title: String(
        localized: "Cursor indicator",
        comment: "Tray row: size of the recording dot drawn next to the mouse cursor"),
      shortcut: viewModel.holdBadgeOption.visibleName,
      shortcutColor: viewModel.holdBadgeOption == .off
        ? CSColor.textFaintAlt : CSColor.oliveLight,
      scale: .child
    ) { viewModel.setHoldBadgeOption(viewModel.holdBadgeOption.next) }
    .accessibilityLabel("Cursor indicator")
    .accessibilityValue(viewModel.holdBadgeOption.visibleName)
    .accessibilityHint("Cycle the cursor indicator size")
  }

  /// A checkbox-style row reusing `TrayRow`, with the on/off state shown as the
  /// trailing keycap so it shares the locked palette and geometry.
  private func toggleRow(
    icon: CSIcon,
    title: String,
    isOn: Bool,
    onColor: Color = CSColor.oliveLight,
    set: @escaping (Bool) -> Void
  ) -> some View {
    TrayRow(
      icon: icon,
      title: title,
      shortcut: isOn
        ? String(
          localized: "tray.keycap.on", defaultValue: "On",
          comment: "Tray keycap, a few letters wide: this toggle is enabled")
        : String(
          localized: "tray.keycap.off", defaultValue: "Off",
          comment: "Tray keycap, a few letters wide: this toggle is disabled"),
      shortcutColor: isOn ? onColor : CSColor.textFaintAlt,
      scale: .child
    ) { set(!isOn) }
  }

  private var recordingActionTitle: String {
    if viewModel.isStartingDictation {
      return String(localized: "Starting…", comment: "Tray row: a recording start is in flight")
    }
    if viewModel.isRecording {
      return trayStatus.status.assistive
        ? String(
          localized: "Stop transcription in Agent mode",
          comment: "Tray row: stop the current transcription in Agent mode")
        : String(
          localized: "Stop transcription", comment: "Tray row: stop the current transcription")
    }
    return viewModel.startInAssistive
      ? String(
        localized: "Start transcription in Agent mode",
        comment: "Tray row: start a new transcription in Agent mode")
      : String(localized: "Start transcription", comment: "Tray row: start a new transcription")
  }

  private var recordingActionColor: Color {
    if viewModel.isStartingDictation {
      return viewModel.startInAssistive ? CSColor.assistive : CSColor.terracotta
    }
    if viewModel.isRecording {
      return trayStatus.status.assistive ? CSColor.assistive : CSColor.terracotta
    }
    return viewModel.startInAssistive ? CSColor.assistive : CSColor.oliveLight
  }

  // MARK: - Notes (nested disclosure)

  private var notesGroup: some View {
    VStack(spacing: 0) {
      TrayRow(
        icon: .notes,
        title: String(localized: "Notes", comment: "Tray group: the daily-note actions"),
        disclosureExpanded: viewModel.notesExpanded,
        style: viewModel.notesExpanded ? .raised : .plain
      ) {
        withAnimation(TrayDisclosureChevron.animation) { viewModel.notesExpanded.toggle() }
      }

      if viewModel.notesExpanded {
        TrayDisclosureChildren {
          TrayChildRow(title: String(localized: "Save last transcript")) {
            viewModel.onSaveLastTranscript()
          }
          TrayChildRow(
            title: String(
              localized: "Save selection",
              comment: "Tray row: save the text selected in another app to the daily note")
          ) {
            viewModel.onSaveSelection()
          }
          TrayChildRow(title: String(localized: "Open notes folder")) {
            viewModel.onOpenNotesFolder()
          }
          TrayChildRow(title: String(localized: "Open today's note")) {
            viewModel.onOpenTodayNote()
          }
          if let status = viewModel.noteStatus {
            TrayNoteStatusRow(status: status)
          }
        }
      }
    }
  }

  // MARK: - Diagnostics (nested disclosure)

  private var diagnosticsGroup: some View {
    VStack(spacing: 0) {
      TrayRow(
        icon: .diagnostics,
        title: String(localized: "Diagnostics"),
        disclosureExpanded: viewModel.diagnosticsExpanded,
        style: viewModel.diagnosticsExpanded ? .raised : .plain
      ) {
        withAnimation(TrayDisclosureChevron.animation) {
          viewModel.diagnosticsExpanded.toggle()
        }
      }

      if viewModel.diagnosticsExpanded {
        TrayDisclosureChildren {
          TrayChildRow(title: String(localized: "Open log folder")) {
            viewModel.onOpenLogFolder()
          }
          TrayChildRow(title: String(localized: "Copy debug info")) {
            viewModel.onCopyDebugInfo()
          }
        }
      }
    }
  }
}

// MARK: - Notes action result banner

/// Transient confirmation row for the Notes actions. Olive check on success,
/// terracotta cross on failure — a permission-free, always-visible replacement
/// for the OS notification that an accessory app can't guarantee.
private struct TrayNoteStatusRow: View {
  let status: TrayActionStatus

  private var isSuccess: Bool { status.kind == .success }
  private var tint: Color { isSuccess ? CSColor.oliveLight : CSColor.terracotta }

  var body: some View {
    HStack(spacing: 6) {
      CSIconView(icon: isSuccess ? .success : .failure, size: 11, weight: .bold, color: tint)
      Text(status.message)
        .font(CSFont.ui(12, .medium))
        .foregroundStyle(tint)
        .lineLimit(1)
      Spacer(minLength: 0)
    }
    .padding(.horizontal, 11)
    .padding(.vertical, 7)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.chip, style: .continuous)
        .fill(tint.opacity(0.10))
    )
    .transition(.opacity)
  }
}

// MARK: - Previews (standalone, mock-seeded)

#if DEBUG
  #Preview("Tray · Idle") {
    let vm = TrayViewModel(engine: MockTrayEngine(recording: false), isRecording: false)
    TrayMenuView(viewModel: vm, trayStatus: .preview())
      .padding(CSSpace.previewInset)
      .background(CSColor.windowWash)
      .onAppear { FontLoader.register() }
  }

  #Preview("Tray · Recording") {
    let vm = TrayViewModel(engine: MockTrayEngine(recording: true), isRecording: true)
    TrayMenuView(
      viewModel: vm,
      trayStatus: .preview(kind: .listening, tone: .active)
    )
    .padding(CSSpace.previewInset)
    .background(CSColor.windowWash)
    .onAppear { FontLoader.register() }
  }
#endif
