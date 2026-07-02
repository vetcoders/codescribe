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
    }

    // Header: live status pill · Settings · thread menu (⋯ wired in the menu section)
    private var header: some View {
        HStack(spacing: 12) {
            StaticStatusPill(text: status.label, color: status.color)
            Spacer()
            HStack(spacing: 14) {
                Button(action: { openSettings() }) {
                    Text("⚙").font(.system(size: 16))
                }
                .buttonStyle(.plain)
                .help("Settings")

                Text("⋯").font(.system(size: 16, weight: .bold)).tracking(1)
            }
            .foregroundStyle(CSColor.textFaint)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 14)
        .overlay(alignment: .bottom) {
            Rectangle().fill(CSColor.hairline(0.06)).frame(height: 1)
        }
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
