import SwiftUI

// Tray Menu (glass dropdown panel).
//
// Reusable content view: App.swift hosts this inside an `NSPopover`
// (`NSHostingController(rootView: TrayMenuView(viewModel:))`) anchored to a
// manual `NSStatusItem` — deliberately not `MenuBarExtra`, to sidestep the
// WindowServer status-item session-state issue seen on this bundle id.
// 300pt wide, glass panel, status header bound to runtime, terracotta marking
// ONLY the primary action ("Show Agent"), Notes / Diagnostics as nested
// disclosure groups. Dictation toggle + quick config toggles are wired through
// the composite TrayEngine.
struct TrayMenuView: View {
    @ObservedObject var viewModel: TrayViewModel
    @ObservedObject var trayStatus: TrayStatusStore
    // macOS 14+ action to open the app's Settings scene — replaces the fragile
    // private `showSettingsWindow:` selector that stopped working on newer macOS.
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        GlassPanel(cornerRadius: CSRadius.tray) {
            VStack(spacing: 0) {
                statusHeader
                trayStatusRow
                TrayDivider(top: 3, bottom: 5)

                primaryActions

                TrayDivider()
                quickToggles

                notesGroup
                diagnosticsGroup

                TrayDivider()
                TrayRow(icon: .settings, title: "Settings…", shortcut: "⌘,") {
                    openSettings()
                }
                TrayRow(icon: .setupWizard, title: "Setup Wizard…") { viewModel.onOpenSetupWizard() }
                TrayRow(icon: .help, title: "Help") { viewModel.onHelp() }
                TrayRow(icon: .info, title: "About") { viewModel.onAbout() }

                TrayDivider()
                TrayRow(
                    icon: .power,
                    iconColor: CSColor.terracottaDeep,
                    title: "Quit codescribe",
                    titleColor: TrayRow.subnoteColor,
                    shortcut: "⌘Q"
                ) { viewModel.onQuit() }
            }
            .padding(7)
        }
        .frame(width: 300)
        .onAppear { viewModel.refreshStatus() }
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
        .padding(.top, 11)
        .padding(.bottom, 10)
    }

    private var trayStatusRow: some View {
        HStack(spacing: 7) {
            CSIconView(icon: trayStatus.icon, size: 11, weight: .bold, color: trayStatus.color)
            Text(trayStatus.status.menuLabel)
                .font(CSFont.ui(12, .medium))
                .foregroundStyle(trayStatus.color)
                .lineLimit(1)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 11)
        .padding(.vertical, 7)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
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
                title: "Show Agent",
                titleColor: viewModel.agentAvailable ? CSColor.textBody : CSColor.textFaint,
                titleWeight: .semibold,
                shortcut: "⌥⌥",
                shortcutColor: TrayRow.primaryShortcutColor,
                style: .primary
            ) { viewModel.onShowAgent() }

            TrayRow(
                icon: viewModel.isRecording && !viewModel.isStartingDictation ? .stop : .record,
                iconColor: recordingActionColor,
                title: recordingActionTitle
            ) { viewModel.toggleDictation() }

            historyGroup
            TrayRow(icon: .copy, title: "Copy last transcript") {
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
                title: "Open history",
                disclosureExpanded: viewModel.historyExpanded,
                style: viewModel.historyExpanded ? .raised : .plain
            ) {
                withAnimation(TrayDisclosureChevron.animation) { viewModel.toggleHistory() }
            }

            if viewModel.historyExpanded {
                TrayDisclosureChildren {
                    if viewModel.historyItems.isEmpty {
                        TrayChildRow(title: "No transcripts yet")
                    } else {
                        ForEach(viewModel.historyItems) { item in
                            TrayChildRow(title: item.title) {
                                viewModel.copyTranscript(path: item.path)
                            }
                        }
                    }
                    TrayChildRow(title: "Open history folder") {
                        viewModel.openHistoryFolder()
                    }
                }
            }
        }
    }

    // MARK: - Quick config toggles

    private var quickToggles: some View {
        VStack(spacing: 0) {
            toggleRow(icon: .dock, title: "Show Dock Icon", isOn: viewModel.showDockIcon) {
                viewModel.setShowDockIcon($0)
            }
            toggleRow(
                icon: .overlay,
                title: "Transcription Overlay",
                isOn: viewModel.overlayEnabled
            ) { viewModel.setOverlayEnabled($0) }
            autoPasteToggle
            autoFormatMenu
            holdBadgeMenu
            toggleRow(icon: .notesMode, title: "Notes Mode", isOn: viewModel.notesModeEnabled) {
                viewModel.setNotesMode($0)
            }
            toggleRow(
                icon: .agent,
                title: "Start in Assistive",
                isOn: viewModel.startInAssistive,
                onColor: CSColor.assistive
            ) { viewModel.setStartInAssistive($0) }
        }
    }

    /// Auto Paste shares the exact baseline row (icon + trailing On/Off keycap)
    /// with Show Dock Icon and Transcription Overlay — one visual grammar for
    /// every quick toggle. TrayRow keeps the locked palette and geometry.
    private var autoPasteToggle: some View {
        toggleRow(icon: .send, title: "Auto Paste", isOn: viewModel.autoPasteEnabled) {
            viewModel.setAutoPasteEnabled($0)
        }
    }

    /// Auto Format is a cycling row in the same baseline grammar: each click
    /// advances Off → Correction → Smart → Max → Off. The current level sits in
    /// the trailing keycap slot, so nothing opens over the 300pt popover.
    private var autoFormatMenu: some View {
        TrayRow(
            icon: .edit,
            title: "Auto Format",
            shortcut: viewModel.autoFormatLevel.visibleName,
            shortcutColor: viewModel.autoFormatLevel == .off
                ? CSColor.textFaintAlt : CSColor.oliveLight
        ) { viewModel.setAutoFormatLevel(viewModel.autoFormatLevel.next) }
            .accessibilityLabel("Auto Format")
            .accessibilityValue(viewModel.autoFormatLevel.visibleName)
            .accessibilityHint("Cycle automatic formatting level")
    }

    /// Pointer Indicator follows the same rolling-row grammar as Auto Format:
    /// Off → 4px → 8px → 12px → Off, with the current value in the keycap.
    private var holdBadgeMenu: some View {
        TrayRow(
            icon: .record,
            title: "Pointer Indicator",
            shortcut: viewModel.holdBadgeOption.visibleName,
            shortcutColor: viewModel.holdBadgeOption == .off
                ? CSColor.textFaintAlt : CSColor.oliveLight
        ) { viewModel.setHoldBadgeOption(viewModel.holdBadgeOption.next) }
            .accessibilityLabel("Pointer Indicator")
            .accessibilityValue(viewModel.holdBadgeOption.visibleName)
            .accessibilityHint("Cycle pointer recording indicator size")
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
            shortcut: isOn ? "On" : "Off",
            shortcutColor: isOn ? onColor : CSColor.textFaintAlt
        ) { set(!isOn) }
    }

    private var recordingActionTitle: String {
        if viewModel.isStartingDictation { return "Starting…" }
        if viewModel.isRecording {
            return trayStatus.status.assistive ? "Stop Assistive" : "Stop Dictation"
        }
        return viewModel.startInAssistive ? "Start Assistive" : "Start Dictation"
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
                title: "Notes",
                disclosureExpanded: viewModel.notesExpanded,
                style: viewModel.notesExpanded ? .raised : .plain
            ) {
                withAnimation(TrayDisclosureChevron.animation) { viewModel.notesExpanded.toggle() }
            }

            if viewModel.notesExpanded {
                TrayDisclosureChildren {
                    TrayChildRow(title: "Save last transcript") {
                        viewModel.onSaveLastTranscript()
                    }
                    TrayChildRow(title: "Save selection") {
                        viewModel.onSaveSelection()
                    }
                    TrayChildRow(title: "Open notes folder") {
                        viewModel.onOpenNotesFolder()
                    }
                    TrayChildRow(title: "Open today's note") {
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
                title: "Diagnostics",
                disclosureExpanded: viewModel.diagnosticsExpanded,
                style: viewModel.diagnosticsExpanded ? .raised : .plain
            ) {
                withAnimation(TrayDisclosureChevron.animation) {
                    viewModel.diagnosticsExpanded.toggle()
                }
            }

            if viewModel.diagnosticsExpanded {
                TrayDisclosureChildren {
                    TrayChildRow(title: "Open log folder") { viewModel.onOpenLogFolder() }
                    TrayChildRow(title: "Copy debug info") { viewModel.onCopyDebugInfo() }
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
            RoundedRectangle(cornerRadius: 8, style: .continuous)
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
        .padding(40)
        .background(LinearGradient(
            colors: [Color(hex: 0x15110E), Color(hex: 0x0B0C10), Color(hex: 0x0D1012)],
            startPoint: .topLeading, endPoint: .bottomTrailing
        ))
        .onAppear { FontLoader.register() }
}

#Preview("Tray · Recording") {
    let vm = TrayViewModel(engine: MockTrayEngine(recording: true), isRecording: true)
    TrayMenuView(
        viewModel: vm,
        trayStatus: .preview(kind: .listening, tone: .active, label: "Status: Recording...")
    )
        .padding(40)
        .background(LinearGradient(
            colors: [Color(hex: 0x15110E), Color(hex: 0x0B0C10), Color(hex: 0x0D1012)],
            startPoint: .topLeading, endPoint: .bottomTrailing
        ))
        .onAppear { FontLoader.register() }
}
#endif
