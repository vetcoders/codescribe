import SwiftUI

/// Left rail: wordmark, search field, THREADS list (active = terracotta tint),
/// and a dashed "+ New thread" footer. Threads are local in-memory state.
struct ThreadRail: View {
    @ObservedObject var store: AgentChatStore
    @State private var search: String = ""

    var body: some View {
        VStack(spacing: 0) {
            // Wordmark header
            HStack(spacing: 9) {
                Wordmark(size: 15)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.top, 16)
            .padding(.bottom, 12)

            // Search field
            HStack(spacing: 8) {
                Text("⌕")
                    .font(.system(size: 12))
                    .foregroundStyle(CSColor.textFaintAlt)
                TextField("", text: $search, prompt:
                    Text("search threads")
                        .font(CSFont.mono(12, .medium))
                        .foregroundColor(CSColor.textFaint)
                )
                .textFieldStyle(.plain)
                .font(CSFont.mono(12, .medium))
                .foregroundStyle(CSColor.textBody)
            }
            .padding(.horizontal, 11)
            .padding(.vertical, 8)
            .background(CSColor.surfaceRaised(0.04))
            .overlay(
                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .strokeBorder(CSColor.hairline(0.06), lineWidth: 1)
            )
            .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
            .padding(.horizontal, 12)
            .padding(.bottom, 10)

            // Section eyebrow
            HStack {
                Text("THREADS")
                    .font(CSFont.mono(10, .semibold))
                    .tracking(1.0)
                    .foregroundStyle(CSColor.textFaintAlt)
                Spacer()
            }
            .padding(.horizontal, 12)
            .padding(.top, 6)
            .padding(.bottom, 4)

            // Thread list
            ScrollView {
                LazyVStack(spacing: 4) {
                    ForEach(filteredThreads) { thread in
                        ThreadRow(
                            thread: thread,
                            isActive: thread.id == store.selectedThreadID
                        )
                        .contentShape(Rectangle())
                        .onTapGesture { store.select(thread.id) }
                    }
                }
                .padding(.horizontal, 10)
            }
            .scrollContentBackground(.hidden)

            // New thread footer
            VStack {
                Button(action: { store.newThread() }) {
                    HStack(spacing: 7) {
                        Text("+ New thread")
                            .font(CSFont.ui(12, .semibold))
                            .foregroundStyle(CSColor.textMuted)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(10)
                    .overlay(
                        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                            .strokeBorder(
                                CSColor.hairline(0.14),
                                style: StrokeStyle(lineWidth: 1, dash: [4, 3])
                            )
                    )
                }
                .buttonStyle(.plain)
            }
            .padding(12)
            .overlay(alignment: .top) {
                Rectangle().fill(CSColor.hairline(0.06)).frame(height: 1)
            }
        }
        .frame(width: 236)
        .background(Color.white.opacity(0.015))
        .overlay(alignment: .trailing) {
            Rectangle().fill(CSColor.hairline(0.06)).frame(width: 1)
        }
    }

    private var filteredThreads: [ChatThread] {
        let q = search.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return store.threads }
        return store.threads.filter { $0.title.lowercased().contains(q) }
    }
}

private struct ThreadRow: View {
    let thread: ChatThread
    let isActive: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 7) {
                if isActive {
                    Circle().fill(CSColor.terracotta).frame(width: 6, height: 6)
                }
                Text(thread.title)
                    .font(CSFont.ui(13, isActive ? .semibold : .medium))
                    .foregroundStyle(isActive ? ChatPalette.nameActive : ChatPalette.nameInactive)
                    .lineLimit(1)
            }
            Text(thread.meta)
                .font(CSFont.mono(10, .medium))
                .foregroundStyle(isActive ? ChatPalette.activeThreadSub : CSColor.textFaintAlt)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 12)
        .padding(.vertical, 11)
        .background(isActive ? CSColor.terracotta.opacity(0.12) : .clear)
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(isActive ? CSColor.terracotta.opacity(0.28) : .clear, lineWidth: 1)
        )
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
    }
}
