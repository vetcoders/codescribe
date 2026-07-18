import SwiftUI

/// Left rail: wordmark, search field, THREADS list (active = terracotta tint),
/// and a dashed "+ New thread" footer.
struct ThreadRail: View {
    @ObservedObject var store: AgentChatStore
    @State private var search: String = ""
    @State private var deleteCandidate: ChatThread?
    @State private var editingThreadID: UUID?
    @State private var renameDraft: String = ""

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
                CSIconView(icon: .search, size: 12, color: CSColor.textFaintAlt)
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

            // Thread list — search-filtered first, then grouped by recency
            ScrollView {
                LazyVStack(spacing: 4) {
                    ForEach(sectionedThreads, id: \.section) { group in
                        HStack {
                            Text(group.section.title)
                                .font(CSFont.mono(9, .semibold))
                                .tracking(0.8)
                                .foregroundStyle(CSColor.textFaintAlt)
                            Spacer()
                        }
                        .padding(.horizontal, 2)
                        .padding(.top, 8)
                        .padding(.bottom, 2)
                        ForEach(group.threads) { thread in
                            ThreadRow(
                                thread: thread,
                                isActive: thread.id == store.selectedThreadID,
                                isEditing: editingThreadID == thread.id,
                                renameDraft: $renameDraft,
                                onToggleFavorite: { store.toggleFavorite(thread) },
                                onRequestDelete: { deleteCandidate = thread },
                                onBeginRename: { beginRename(thread) },
                                onCommitRename: { commitRename(thread) },
                                onCancelRename: { cancelRename(thread) }
                            )
                            .contentShape(Rectangle())
                            .onTapGesture {
                                if editingThreadID != thread.id { store.select(thread.id) }
                            }
                        }
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
        .onChange(of: search) { _, newValue in
            store.searchThreads(newValue)
        }
        .confirmationDialog(
            "Delete this thread?",
            isPresented: Binding(
                get: { deleteCandidate != nil },
                set: { if !$0 { deleteCandidate = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Delete Thread", role: .destructive) {
                if let deleteCandidate {
                    store.delete(deleteCandidate)
                    self.deleteCandidate = nil
                }
            }
            Button("Cancel", role: .cancel) {
                deleteCandidate = nil
            }
        } message: {
            Text("This removes the persisted conversation from the thread store.")
        }
    }

    private var filteredThreads: [ChatThread] {
        let q = search.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return store.threads }
        if store.usesRealThreadSearch { return store.threads }
        return store.threads.filter { $0.title.lowercased().contains(q) }
    }

    /// Groups the (already search-filtered) threads into recency sections,
    /// preserving the store's updated-desc order inside each group. Local-only
    /// drafts carry no `updatedAt` and group under Today.
    private var sectionedThreads: [(section: ThreadSection, threads: [ChatThread])] {
        let now = Date()
        var groups: [ThreadSection: [ChatThread]] = [:]
        for thread in filteredThreads {
            groups[ThreadSection.section(for: thread.updatedAt ?? now, now: now), default: []]
                .append(thread)
        }
        return ThreadSection.allCases.compactMap { section in
            groups[section].map { (section, $0) }
        }
    }

    // MARK: Rename (inline edit)

    private func beginRename(_ thread: ChatThread) {
        guard editingThreadID != thread.id else { return }
        renameDraft = thread.title
        editingThreadID = thread.id
    }

    /// Persist the typed title. Clearing `editingThreadID` first makes any
    /// trailing focus-loss commit a no-op (see ThreadRow's blur handling).
    private func commitRename(_ thread: ChatThread) {
        guard editingThreadID == thread.id else { return }
        let value = renameDraft
        editingThreadID = nil
        store.rename(thread, to: value)
    }

    private func cancelRename(_ thread: ChatThread) {
        guard editingThreadID == thread.id else { return }
        editingThreadID = nil
    }
}

private struct ThreadRow: View {
    let thread: ChatThread
    let isActive: Bool
    let isEditing: Bool
    @Binding var renameDraft: String
    let onToggleFavorite: () -> Void
    let onRequestDelete: () -> Void
    let onBeginRename: () -> Void
    let onCommitRename: () -> Void
    let onCancelRename: () -> Void

    @FocusState private var renameFieldFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 7) {
                if isActive {
                    Circle().fill(CSColor.terracotta).frame(width: 6, height: 6)
                }
                if isEditing {
                    TextField("", text: $renameDraft)
                        .textFieldStyle(.plain)
                        .font(CSFont.ui(13, .semibold))
                        .foregroundStyle(ChatPalette.nameActive)
                        .focused($renameFieldFocused)
                        .onSubmit { onCommitRename() }
                        .onExitCommand { onCancelRename() }
                        .onAppear { DispatchQueue.main.async { renameFieldFocused = true } }
                        .onChange(of: renameFieldFocused) { _, focused in
                            // Click-away commits the typed value; Enter/Esc already
                            // cleared editing, so those paths make this a no-op.
                            if !focused, isEditing { onCommitRename() }
                        }
                } else {
                    Text(thread.title)
                        .font(CSFont.ui(13, isActive ? .semibold : .medium))
                        .foregroundStyle(isActive ? ChatPalette.nameActive : ChatPalette.nameInactive)
                        .lineLimit(1)
                        .onTapGesture(count: 2) { onBeginRename() }
                }
                Spacer(minLength: 4)
                Button(action: onToggleFavorite) {
                    CSIconView(
                        icon: thread.isFavorite ? .starFill : .star,
                        size: 11,
                        weight: .semibold,
                        color: thread.isFavorite ? CSColor.oliveLight : CSColor.textFaintAlt
                    )
                    .frame(width: 18, height: 18)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .opacity(thread.isFavorite || isActive ? 1 : 0.38)
                .help(thread.isFavorite ? "Unfavorite thread" : "Favorite thread")
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
        .contextMenu {
            Button("Rename") {
                onBeginRename()
            }
            Button(thread.isFavorite ? "Unfavorite" : "Favorite") {
                onToggleFavorite()
            }
            Divider()
            Button("Delete Thread", role: .destructive) {
                onRequestDelete()
            }
        }
    }
}

// MARK: - Recency sections (pure, unit-tested)

/// Time buckets for the rail's section headers, ordered newest-first.
enum ThreadSection: CaseIterable, Hashable {
    case today, yesterday, thisWeek, older

    var title: String {
        switch self {
        case .today: "Today"
        case .yesterday: "Yesterday"
        case .thisWeek: "This week"
        case .older: "Older"
        }
    }

    /// Buckets by whole calendar days between `updatedAt` and `now`:
    /// 0 → today, 1 → yesterday, 2–6 → this week, 7+ → older. Future dates
    /// (clock skew) clamp to today.
    static func section(
        for updatedAt: Date, now: Date, calendar: Calendar = .current
    ) -> ThreadSection {
        let days = calendar.dateComponents(
            [.day],
            from: calendar.startOfDay(for: updatedAt),
            to: calendar.startOfDay(for: now)
        ).day ?? 0
        switch days {
        case ..<1: return .today
        case 1: return .yesterday
        case 2...6: return .thisWeek
        default: return .older
        }
    }
}

// MARK: - Row metadata formatter (pure, unit-tested)

enum ThreadRailMeta {
    /// "relative time · model · tokens", skipping whatever is missing — nils
    /// never leave dangling separators. All inputs absent → empty string.
    static func drawerSubtitle(
        model: String?,
        tokens: UInt64?,
        updatedAt: Date?,
        now: Date = Date(),
        calendar: Calendar = .current
    ) -> String {
        var parts: [String] = []
        if let updatedAt {
            parts.append(relativeTime(updatedAt, now: now, calendar: calendar))
        }
        if let model, !model.isEmpty {
            // "openai/gpt-5" → "gpt-5"; plain names pass through.
            parts.append(String(model.split(separator: "/").last ?? Substring(model)))
        }
        if let tokens, tokens > 0 {
            parts.append(tokenLabel(tokens))
        }
        return parts.joined(separator: " · ")
    }

    /// "today HH:mm" / "yesterday" / "MMM d" — same shape the rail always used.
    private static func relativeTime(_ date: Date, now: Date, calendar: Calendar) -> String {
        let formatter = DateFormatter()
        formatter.calendar = calendar
        formatter.timeZone = calendar.timeZone
        formatter.locale = Locale(identifier: "en_US_POSIX")
        switch ThreadSection.section(for: date, now: now, calendar: calendar) {
        case .today:
            formatter.dateFormat = "'today' HH:mm"
        case .yesterday:
            return "yesterday"
        case .thisWeek, .older:
            formatter.dateFormat = "MMM d"
        }
        return formatter.string(from: date)
    }

    private static func tokenLabel(_ tokens: UInt64) -> String {
        switch tokens {
        case ..<1_000: "\(tokens) tok"
        case ..<1_000_000: String(format: "%.1fk tok", Double(tokens) / 1_000)
        default: String(format: "%.1fM tok", Double(tokens) / 1_000_000)
        }
    }
}
