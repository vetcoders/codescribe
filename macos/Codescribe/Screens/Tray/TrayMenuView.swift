import SwiftUI

// Tray Menu (MenuBarExtra `.window` glass dropdown).
//
// Reusable content view: App.swift hosts this inside
// `MenuBarExtra(...) { TrayMenuView(viewModel:) }.menuBarExtraStyle(.window)`.
// 300pt wide, glass panel, status header bound to runtime, terracotta marking
// ONLY the primary action ("Show Agent"), Notes / Diagnostics as nested
// disclosure groups. Dictation toggle + quick config toggles are wired through
// the composite TrayEngine.
struct TrayMenuView: View {
    @ObservedObject var viewModel: TrayViewModel
    // macOS 14+ action to open the app's Settings scene — replaces the fragile
    // private `showSettingsWindow:` selector that stopped working on newer macOS.
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        GlassPanel(cornerRadius: CSRadius.tray) {
            VStack(spacing: 0) {
                statusHeader
                TrayDivider(top: 3, bottom: 5)

                primaryActions

                TrayDivider()
                quickToggles

                notesGroup
                diagnosticsGroup

                TrayDivider()
                TrayRow(icon: "⚙", title: "Settings…", shortcut: "⌘,") {
                    openSettings()
                }
                TrayRow(icon: "?", title: "Help") { viewModel.onHelp() }
                TrayRow(icon: "ⓘ", title: "About") { viewModel.onAbout() }

                TrayDivider()
                TrayRow(
                    icon: "⏻",
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
            // Separate view type on live vs idle (same rule as the overlay header):
            // the animated pill exists only while recording; idle uses the static
            // type so no @State/onAppear animation can survive into idle.
            if viewModel.isRecording && !viewModel.isStartingDictation {
                StatusPill(
                    text: viewModel.statusText,
                    color: viewModel.statusColor,
                    rippling: true
                )
            } else {
                StaticStatusPill(text: viewModel.statusText, color: viewModel.statusColor)
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 11)
        .padding(.bottom, 10)
    }

    // MARK: - Primary actions

    private var primaryActions: some View {
        VStack(spacing: 0) {
            TrayRow(
                icon: "▦",
                title: "Show Agent",
                titleColor: viewModel.agentAvailable ? CSColor.textBody : CSColor.textFaint,
                titleWeight: .semibold,
                shortcut: "⌥⌥",
                shortcutColor: TrayRow.primaryShortcutColor,
                style: .primary
            ) { viewModel.onShowAgent() }

            TrayRow(
                icon: viewModel.isRecording && !viewModel.isStartingDictation ? "■" : "●",
                iconColor: viewModel.isRecording ? CSColor.terracotta : CSColor.oliveLight,
                title: viewModel.isStartingDictation
                    ? "Starting…"
                    : (viewModel.isRecording ? "Stop Dictation" : "Start Dictation")
            ) { viewModel.toggleDictation() }

            TrayRow(icon: "🕑", title: "Open history…") { viewModel.openHistory() }
            TrayRow(icon: "⧉", title: "Copy last transcript") {
                viewModel.copyLastTranscript()
            }
        }
    }

    // MARK: - Quick config toggles

    private var quickToggles: some View {
        VStack(spacing: 0) {
            toggleRow(icon: "◱", title: "Show Dock Icon", isOn: viewModel.showDockIcon) {
                viewModel.setShowDockIcon($0)
            }
            toggleRow(
                icon: "◰",
                title: "Transcription Overlay",
                isOn: viewModel.overlayEnabled
            ) { viewModel.setOverlayEnabled($0) }
        }
    }

    /// A checkbox-style row reusing `TrayRow`, with the on/off state shown as the
    /// trailing keycap so it shares the locked palette and geometry.
    private func toggleRow(
        icon: String,
        title: String,
        isOn: Bool,
        set: @escaping (Bool) -> Void
    ) -> some View {
        TrayRow(
            icon: icon,
            title: title,
            shortcut: isOn ? "On" : "Off",
            shortcutColor: isOn ? CSColor.oliveLight : CSColor.textFaintAlt
        ) { set(!isOn) }
    }

    // MARK: - Notes (nested disclosure)

    private var notesGroup: some View {
        VStack(spacing: 0) {
            TrayRow(
                icon: "✎",
                title: "Notes",
                showChevron: true,
                style: viewModel.notesExpanded ? .raised : .plain
            ) {
                withAnimation(.easeOut(duration: 0.18)) { viewModel.notesExpanded.toggle() }
            }

            if viewModel.notesExpanded {
                TrayDisclosureChildren {
                    TrayChildRow(title: "Quick Notes", suffix: "(save)") {
                        viewModel.onQuickNotes()
                    }
                    TrayChildRow(title: "Save-only", suffix: "(no paste)") {
                        viewModel.onSaveOnlyNotes()
                    }
                    TrayChildRow(title: "Open notes folder") {
                        viewModel.onOpenNotesFolder()
                    }
                    TrayChildRow(title: "Open today's note") {
                        viewModel.onOpenTodayNote()
                    }
                }
            }
        }
    }

    // MARK: - Diagnostics (nested disclosure)

    private var diagnosticsGroup: some View {
        VStack(spacing: 0) {
            TrayRow(
                icon: "🩺",
                title: "Diagnostics",
                showChevron: true,
                style: viewModel.diagnosticsExpanded ? .raised : .plain
            ) {
                withAnimation(.easeOut(duration: 0.18)) { viewModel.diagnosticsExpanded.toggle() }
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

// MARK: - Previews (standalone, mock-seeded)

#Preview("Tray · Idle") {
    FontLoader.register()
    let vm = TrayViewModel(engine: MockTrayEngine(recording: false), isRecording: false)
    return TrayMenuView(viewModel: vm)
        .padding(40)
        .background(LinearGradient(
            colors: [Color(hex: 0x15110E), Color(hex: 0x0B0C10), Color(hex: 0x0D1012)],
            startPoint: .topLeading, endPoint: .bottomTrailing
        ))
}

#Preview("Tray · Recording") {
    FontLoader.register()
    let vm = TrayViewModel(engine: MockTrayEngine(recording: true), isRecording: true)
    return TrayMenuView(viewModel: vm)
        .padding(40)
        .background(LinearGradient(
            colors: [Color(hex: 0x15110E), Color(hex: 0x0B0C10), Color(hex: 0x0D1012)],
            startPoint: .topLeading, endPoint: .bottomTrailing
        ))
}
