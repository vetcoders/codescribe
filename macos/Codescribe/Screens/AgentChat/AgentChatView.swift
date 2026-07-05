import AppKit
import SwiftUI

/// Agent Chat MVP shell. `NavigationSplitView`: local in-memory thread rail ↔
/// thread view. Turns render You / Tool-activity / Assistant; `send` routes a
/// single-shot `formatText(_:assistive:)` round-trip through the injected
/// `AgentChatEngine`, then simulates a word-reveal stream. See AgentChatStore
/// for the full FFI-gap note (no streaming / threads / tools backend yet —
/// real streaming chat is a tracked core-change follow-up).
struct AgentChatView: View {
    @StateObject var store: AgentChatStore

    init(store: AgentChatStore) {
        _store = StateObject(wrappedValue: store)
    }

    var body: some View {
        NavigationSplitView {
            ThreadRail(store: store)
                .navigationSplitViewColumnWidth(236)
                .toolbar(removing: .sidebarToggle)
        } detail: {
            ThreadDetail(store: store)
        }
        .navigationSplitViewStyle(.balanced)
        .background(CSColor.glassBase)
        .frame(minWidth: 760, idealWidth: 960, minHeight: 560, idealHeight: 600)
        .task { store.startDemoStreamIfNeeded() }
    }
}

// MARK: - Detail (header · title bar · messages · composer)

private struct ThreadDetail: View {
    @ObservedObject var store: AgentChatStore
    @Environment(\.openSettings) private var openSettings
    @State private var isRenaming = false
    @State private var renameText = ""

    var body: some View {
        VStack(spacing: 0) {
            header
            titleBar
            if let thread = store.currentThread {
                MessageList(messages: thread.messages)
            } else {
                Spacer()
            }
            Composer(store: store)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(CSColor.glassBase)
        .alert("Rename thread", isPresented: $isRenaming) {
            TextField("Thread title", text: $renameText)
            Button("Rename") {
                if let thread = store.currentThread { store.rename(thread, to: renameText) }
            }
            Button("Cancel", role: .cancel) {}
        }
    }

    // Header: live status pill · Settings · thread menu (⋯ wired in the menu section)
    private var header: some View {
        HStack(spacing: 12) {
            StaticStatusPill(text: status.label, color: status.color)
            Spacer()
            HStack(spacing: 14) {
                Button(action: { openSettings() }) {
                    CSIconView(icon: .settings, size: 16)
                }
                .buttonStyle(.plain)
                .help("Settings")

                threadMenu
            }
            .foregroundStyle(CSColor.textFaint)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 14)
        .overlay(alignment: .bottom) {
            Rectangle().fill(CSColor.hairline(0.06)).frame(height: 1)
        }
    }

    // Current-thread actions. Export entries appear only for persisted threads
    // (a not-yet-saved local thread has no backend id to export from).
    private var threadMenu: some View {
        Menu {
            if let thread = store.currentThread {
                Button("Rename") { beginRename(thread) }
                Button(thread.isFavorite ? "Unfavorite" : "Favorite") {
                    store.toggleFavorite(thread)
                }
                if thread.backendId != nil {
                    Button("Export to Markdown") { export(thread, assistantOnly: false) }
                    Button("Export assistant replies only") { export(thread, assistantOnly: true) }
                }
                Divider()
                Button("Delete Thread", role: .destructive) { store.delete(thread) }
            }
        } label: {
            CSIconView(icon: .more, size: 16, weight: .bold)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help("Thread actions")
    }

    private func beginRename(_ thread: ChatThread) {
        renameText = thread.title
        isRenaming = true
    }

    /// Export the thread and reveal the written file in Finder (no permission
    /// prompt — the path lives under the app's own `~/.codescribe` data dir).
    private func export(_ thread: ChatThread, assistantOnly: Bool) {
        guard let path = store.exportMarkdown(thread, assistantOnly: assistantOnly) else { return }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }

    // Live status: Idle (olive) → Thinking (amber) → Streaming (terracotta).
    private var status: (label: String, color: Color) {
        if store.isStreaming { return ("Streaming", CSColor.terracottaLight) }
        if store.isThinking { return ("Thinking", CSColor.amber) }
        return ("Idle", CSColor.oliveLight)
    }

    // Title bar: thread title · turn count
    private var titleBar: some View {
        HStack(spacing: 10) {
            Text(store.currentThread?.title ?? "—")
                .font(CSFont.ui(14, .semibold))
                .foregroundStyle(ChatPalette.nameActive)
            Text("· \(turnCount) turns")
                .font(CSFont.mono(11, .medium))
                .foregroundStyle(CSColor.textFaintAlt)
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
        .overlay(alignment: .bottom) {
            Rectangle().fill(CSColor.hairline(0.04)).frame(height: 1)
        }
    }

    private var turnCount: Int { store.currentThread?.messages.count ?? 0 }
}

// MARK: - Preview (standalone — mock engine + seeded threads)

#if DEBUG
#Preview("Agent Chat") {
    AgentChatView(store: AgentChatStore(engine: MockChatEngine()))
        .frame(width: 960, height: 600)
        .preferredColorScheme(.dark)
}
#endif
