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

    // Header: Drawer/Agent toggle · Idle pill · inert glyphs
    private var header: some View {
        HStack(spacing: 12) {
            ModeToggle()
            StatusPill(text: "Idle", color: CSColor.oliveLight)
            Spacer()
            HStack(spacing: 14) {
                Text("🎙").font(.system(size: 15))
                Text("⚙").font(.system(size: 16))
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

    // Title bar: thread title · memory/turn meta · restore
    private var titleBar: some View {
        HStack(spacing: 10) {
            Text(store.currentThread?.title ?? "—")
                .font(CSFont.ui(14, .semibold))
                .foregroundStyle(ChatPalette.nameActive)
            Text("· thread memory on · \(turnCount) turns")
                .font(CSFont.mono(11, .medium))
                .foregroundStyle(CSColor.textFaintAlt)
            Spacer()
            Text("restore ↺")
                .font(CSFont.mono(11, .medium))
                .foregroundStyle(CSColor.textFaint)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
        .overlay(alignment: .bottom) {
            Rectangle().fill(CSColor.hairline(0.04)).frame(height: 1)
        }
    }

    private var turnCount: Int { store.currentThread?.messages.count ?? 0 }
}

/// Segmented Drawer / Agent control. Agent is active (terracotta); Drawer inert.
private struct ModeToggle: View {
    var body: some View {
        HStack(spacing: 2) {
            segment("Drawer", active: false)
            segment("Agent", active: true)
        }
        .padding(3)
        .background(CSColor.surfaceRaised(0.04))
        .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
    }

    private func segment(_ title: String, active: Bool) -> some View {
        Text(title)
            .font(CSFont.ui(12, .semibold))
            .foregroundStyle(active ? CSColor.terracottaLight : CSColor.textMuted)
            .padding(.horizontal, 13)
            .padding(.vertical, 6)
            .background(active ? CSColor.terracotta.opacity(0.16) : .clear)
            .clipShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
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
