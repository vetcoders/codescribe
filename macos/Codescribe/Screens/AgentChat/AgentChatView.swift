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
    /// Collapse state survives window close/reopen and app relaunch. The split
    /// view's live visibility is derived from this on appear and written back on
    /// every change (button, ⌃⌘S, or a native drag-collapse), so no path can
    /// leave the sidebar unrecoverable.
    @AppStorage("AgentChat.sidebarVisible.v1") private var sidebarVisible = true
    @State private var columnVisibility: NavigationSplitViewVisibility = .all

    init(store: AgentChatStore) {
        _store = StateObject(wrappedValue: store)
    }

    var body: some View {
        NavigationSplitView(columnVisibility: $columnVisibility) {
            ThreadRail(store: store)
                // Resizable range (a single fixed width disables drag-resize);
                // native collapse goes through `columnVisibility`, never through
                // forcing the width to zero.
                .navigationSplitViewColumnWidth(min: 200, ideal: 236, max: 360)
                .toolbar(removing: .sidebarToggle)
        } detail: {
            ThreadDetail(store: store, isSidebarVisible: sidebarVisible, toggleSidebar: toggleSidebar)
        }
        .navigationSplitViewStyle(.balanced)
        .csFocusPolicy()
        .background(CSColor.glassBase)
        .frame(minWidth: 760, idealWidth: 960, minHeight: 560, idealHeight: 600)
        .task {
            // Point-in-time marker: correlate with the adjacent "thread index
            // load" / "selected thread load" durations in the same log stream.
            AgentPerf.logger.info("agent window shell rendered")
            store.startDemoStreamIfNeeded()
        }
        .onAppear { columnVisibility = sidebarVisible ? .all : .detailOnly }
        .onChange(of: columnVisibility) { _, visibility in
            sidebarVisible = visibility != .detailOnly
        }
    }

    private func toggleSidebar() {
        withAnimation {
            columnVisibility = columnVisibility == .detailOnly ? .all : .detailOnly
        }
    }
}

// MARK: - Detail (header · title bar · messages · composer)

private struct ThreadDetail: View {
    @ObservedObject var store: AgentChatStore
    /// Sidebar controls live in the DETAIL header so the toggle stays reachable
    /// while the sidebar itself is collapsed.
    let isSidebarVisible: Bool
    let toggleSidebar: () -> Void
    @Environment(\.openSettings) private var openSettings
    @State private var isRenaming = false
    @State private var renameText = ""
    /// Shared with `MessageList` via `ChatLayoutPolicy.defaultsKey`.
    @AppStorage(ChatLayoutPolicy.defaultsKey) private var widthModeRaw = ChatLayoutPolicy.defaultMode.rawValue

    private var widthMode: ChatWidthMode { ChatWidthMode.resolve(widthModeRaw) }

    var body: some View {
        VStack(spacing: 0) {
            header
            titleBar
            if let thread = store.currentThread {
                MessageList(threadID: thread.id, messages: thread.messages) { messageID in
                    store.toggleRenderMode(messageID: messageID, in: thread.id)
                }
            } else {
                Spacer()
            }
            if let thread = store.currentThread {
                ForEach(store.queuedTurns(in: thread.id)) { queued in
                    QueuedTurnRow(turn: queued) { store.cancelQueuedTurn(queued.id) }
                        .padding(.horizontal, 20)
                        .padding(.bottom, 6)
                }
            }
            ForEach(store.currentToolApprovals) { request in
                ToolApprovalCard(
                    request: request,
                    reject: { store.resolveToolApproval(request, approved: false) },
                    allowOnce: { store.resolveToolApproval(request, approved: true) },
                    allowAlways: {
                        store.resolveToolApproval(request, approved: true, remember: true)
                    }
                )
                .padding(.horizontal, 20)
                .padding(.bottom, 10)
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

    // Header: sidebar toggle · live status pill · width density · Settings · thread menu
    private var header: some View {
        HStack(spacing: 12) {
            Button(action: toggleSidebar) {
                Image(systemName: "sidebar.leading")
                    .font(.system(size: 14, weight: .medium))
            }
            .buttonStyle(.plain)
            .foregroundStyle(isSidebarVisible ? CSColor.textBody : CSColor.textFaint)
            .keyboardShortcut("s", modifiers: [.command, .control])
            .help(isSidebarVisible ? "Hide sidebar (⌃⌘S)" : "Show sidebar (⌃⌘S)")
            .accessibilityLabel("Toggle Sidebar")

            StaticStatusPill(text: status.label, color: status.color)
            Spacer()
            HStack(spacing: 14) {
                widthModeMenu

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

    /// Comfortable / Wide / Full — persists via `ChatLayoutPolicy.defaultsKey`.
    private var widthModeMenu: some View {
        Menu {
            ForEach(ChatWidthMode.allCases) { mode in
                Button {
                    widthModeRaw = mode.rawValue
                } label: {
                    if mode == widthMode {
                        Label(mode.label, systemImage: "checkmark")
                    } else {
                        Text(mode.label)
                    }
                }
            }
        } label: {
            HStack(spacing: 4) {
                CSIconView(icon: .setupWizard, size: 12)
                Text(widthMode.label)
                    .font(CSFont.mono(10, .medium))
            }
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help("Chat column width: Comfortable, Wide, or Full")
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

    // Live status: Idle → Thinking → Streaming → Stopping.
    private var status: (label: String, color: Color) {
        if store.isCancelling { return ("Stopping", CSColor.textFaintAlt) }
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

/// One accepted-but-not-yet-running message. Visible until the queue's single
/// dispatch owner promotes it to the active turn; the ✕ cancels it before it
/// is ever sent.
private struct QueuedTurnRow: View {
    let turn: AgentChatStore.QueuedTurn
    let cancel: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Text("Queued")
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(CSColor.amber)
            Text(turn.text.isEmpty ? "\(turn.attachments.count) attachment(s)" : turn.text)
                .font(CSFont.ui(12, .regular))
                .foregroundStyle(CSColor.textBody)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer()
            Button(action: cancel) {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 13))
                    .foregroundStyle(CSColor.textFaintAlt)
            }
            .buttonStyle(.plain)
            .help("Cancel queued message")
            .accessibilityLabel("Cancel queued message")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .background(CSColor.surfaceRaised(0.04))
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
                .strokeBorder(CSColor.hairline(0.09), lineWidth: 1)
        )
        .clipShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))
    }
}

private struct ToolApprovalCard: View {
    let request: PendingToolApproval
    let reject: () -> Void
    let allowOnce: () -> Void
    let allowAlways: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack {
                Text("Permission required")
                    .font(CSFont.ui(13, .semibold))
                    .foregroundStyle(CSColor.amber)
                Spacer()
                Text(request.risk.replacingOccurrences(of: "_", with: " "))
                    .font(CSFont.mono(10, .medium))
                    .foregroundStyle(CSColor.textFaintAlt)
            }
            Text("\(request.server) · \(request.tool)")
                .font(CSFont.mono(11.5, .semibold))
                .foregroundStyle(CSColor.textHigh)
                .textSelection(.enabled)
            if !request.summary.isEmpty {
                Text(request.summary)
                    .font(CSFont.ui(12, .regular))
                    .foregroundStyle(CSColor.textBody)
            }
            if let command = request.command {
                Text("$ \(command)")
                    .font(CSFont.mono(11, .medium))
                    .foregroundStyle(CSColor.terracottaLight)
                    .textSelection(.enabled)
            }
            if let cwd = request.cwd {
                Text("cwd: \(cwd)")
                    .font(CSFont.mono(10.5, .medium))
                    .foregroundStyle(CSColor.textFaintAlt)
                    .textSelection(.enabled)
            }
            ForEach(request.paths, id: \.self) { path in
                Text(path)
                    .font(CSFont.mono(10.5, .medium))
                    .foregroundStyle(CSColor.textFaintAlt)
                    .textSelection(.enabled)
            }
            HStack {
                Spacer()
                Button("Deny", role: .cancel, action: reject)
                Button("Always allow", action: allowAlways)
                Button("Allow once", action: allowOnce)
                    .buttonStyle(.borderedProminent)
            }
        }
        .padding(14)
        .background(CSColor.surfaceRaised(0.04))
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
                .strokeBorder(CSColor.amber.opacity(0.35), lineWidth: 1)
        )
        .clipShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))
    }
}

// MARK: - Preview (standalone — mock engine + seeded threads)

#if DEBUG
#Preview("Agent Chat") {
    AgentChatView(store: AgentChatStore(engine: MockChatEngine()))
        .frame(width: 960, height: 600)
        .preferredColorScheme(.dark)
}
#endif
