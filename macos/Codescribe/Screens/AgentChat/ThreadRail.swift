import SwiftUI

/// Thread list content; the native split controller owns sidebar collapse and width.
struct ThreadRail: View {
  @ObservedObject var store: AgentChatStore
  var onContentWidthChanged: (CGFloat) -> Void = { _ in }
  @State private var search: String = ""
  @State private var deleteCandidate: ChatThread?
  @State private var editingThreadID: UUID?
  @State private var renameDraft: String = ""

  var body: some View {
    expandedRail
      .onPreferenceChange(ThreadRailWidthPreference.self, perform: onContentWidthChanged)
      .onChange(of: search) { _, newValue in
        store.searchThreads(newValue)
      }
      .onChange(of: store.threadSearchQuery) { _, newValue in
        if search.trimmingCharacters(in: .whitespacesAndNewlines) != newValue {
          search = newValue
        }
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

  private var expandedRail: some View {
    VStack(spacing: 0) {
      HStack(spacing: 9) {
        Wordmark(size: 13)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(.horizontal, 12)
      .padding(.top, 10)
      .padding(.bottom, 8)

      HStack(spacing: 6) {
        Image(systemName: "magnifyingglass")
          .foregroundStyle(CSColor.textTertiary)
          .imageScale(.small)
          .accessibilityHidden(true)
        TextField("Search threads", text: $search)
          .textFieldStyle(.plain)
          .font(CSFont.ui(13, .regular))
          .foregroundStyle(Color.primary)
      }
      .padding(.horizontal, 8)
      .padding(.vertical, 5)
      .background(CSColor.controlFill)
      .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
      .padding(.horizontal, 12)
      .padding(.bottom, 8)

      if let error = store.threadSearchError {
        Text(error)
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(Color.primary)
          .padding(.horizontal, 12)
          .padding(.bottom, 8)
          .accessibilityLabel(error)
      }

      // Section eyebrow
      HStack {
        Text("THREADS")
          .font(CSFont.mono(10, .semibold))
          .tracking(1.0)
          .foregroundStyle(CSColor.textTertiary)
        Spacer()
      }
      .padding(.horizontal, 12)
      .padding(.top, 4)
      .padding(.bottom, 2)

      // Thread list — search-filtered first, then grouped by recency
      ScrollView {
        LazyVStack(spacing: 4) {
          ForEach(sectionedThreads, id: \.section) { group in
            HStack {
              Text(group.section.title)
                .font(CSFont.mono(9, .semibold))
                .tracking(0.8)
                .foregroundStyle(CSColor.textTertiary)
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

      VStack {
        Button(action: { store.newThread() }) {
          Label("New thread", systemImage: "plus")
            .frame(maxWidth: .infinity)
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .accessibilityLabel("New thread")
      }
      .padding(8)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
  }

  private var filteredThreads: [ChatThread] {
    let q = search.trimmingCharacters(in: .whitespaces).lowercased()
    guard !q.isEmpty else { return store.threads }
    if store.usesRealThreadSearch { return store.threads }
    return store.threads.filter {
      ThreadRowTitle.displayTitle(for: $0).lowercased().contains(q)
    }
  }

  /// Agent recency sections precede the separate Max consultation section.
  private var sectionedThreads: [(section: ThreadSection, threads: [ChatThread])] {
    ThreadSection.railGroups(filteredThreads)
  }

  // MARK: Rename (inline edit)

  private func beginRename(_ thread: ChatThread) {
    guard editingThreadID != thread.id else { return }
    renameDraft = ThreadRowTitle.displayTitle(for: thread)
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

/// Pure row-view model: a transport placeholder can exist in a corrupt/stale
/// input object, but it can never become visible text. Prefer the first user
/// excerpt and fall back to a relative date label when messages are still lazy.
enum ThreadRowTitle {
  static func displayTitle(
    for thread: ChatThread,
    now: Date = Date(),
    calendar: Calendar = .current
  ) -> String {
    if let title = ThreadTitlePolicy.normalized(thread.title) {
      return title
    }
    if let excerpt = ThreadTitlePolicy.firstUserExcerpt(in: thread.messages) {
      return excerpt
    }
    return ThreadRailMeta.fallbackTitle(
      updatedAt: thread.updatedAt,
      now: now,
      calendar: calendar
    )
  }

}

private struct ThreadRailWidthPreference: PreferenceKey {
  static let defaultValue: CGFloat = 0
  static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
    value = max(value, nextValue())
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
    rowContent(measuring: false)
      .background {
        // Measure the same fonts, symbols, metadata and spacing before truncation.
        // The probe never contains a rename field or participates in interaction.
        rowContent(measuring: true)
          .fixedSize(horizontal: true, vertical: true)
          .hidden()
          .allowsHitTesting(false)
          .accessibilityHidden(true)
          .background {
            GeometryReader { geometry in
              Color.clear.preference(
                key: ThreadRailWidthPreference.self,
                // Row padding: 12pt per side; rail list padding: 10pt per side.
                value: geometry.size.width + 2 * 12 + 2 * 10)
            }
          }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(.horizontal, 12)
      // Two-line rail rows stay list-dense. 11pt of vertical padding plus the
      // title and meta was reading as a stack of cards.
      .padding(.vertical, 7)
      .background(isActive ? CSColor.chromeAccent.opacity(0.12) : .clear)
      .overlay(
        RoundedRectangle(cornerRadius: 10, style: .continuous)
          .strokeBorder(isActive ? CSColor.chromeAccent.opacity(0.28) : .clear, lineWidth: 1)
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

  private func rowContent(measuring: Bool) -> some View {
    VStack(alignment: .leading, spacing: 4) {
      HStack(spacing: 7) {
        if thread.isMaxConsultation {
          Image(systemName: "sparkles")
            .font(CSFont.ui(11, .semibold))
            .foregroundStyle(Color.secondary)
            .accessibilityLabel("Max consultation")
        }
        if isActive {
          Circle().fill(CSColor.chromeAccent).frame(width: 6, height: 6)
        }
        if isEditing && !measuring {
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
          if measuring {
            titleLabel
          } else {
            titleLabel.onTapGesture(count: 2) { onBeginRename() }
          }
        }
        Spacer(minLength: 4)
        if measuring {
          favoriteLabel
        } else {
          Button(action: onToggleFavorite) { favoriteLabel }
            .csFocusRing()
            .opacity(thread.isFavorite || isActive ? 1 : 0.38)
            .help(thread.isFavorite ? "Unfavorite thread" : "Favorite thread")
        }
      }
      HStack(spacing: 6) {
        if let tag = ModelTag.display(for: thread.model) {
          Text(tag)
            .lineLimit(1)
            .truncationMode(.tail)
            .font(CSFont.mono(9, .semibold))
            .foregroundStyle(isActive ? CSColor.modeAgent : CSColor.textTertiary)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(
              (isActive ? CSColor.modeAgent : CSColor.textTertiary).opacity(0.14)
            )
            .clipShape(Capsule())
            .accessibilityLabel("model \(tag)")
        }
        Text(ThreadRailMeta.timeOnly(from: thread.meta))
          .lineLimit(1)
          .fixedSize(horizontal: true, vertical: false)
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(isActive ? ChatPalette.activeThreadSub : CSColor.textTertiary)
      }
    }
  }

  private var titleLabel: some View {
    Text(ThreadRowTitle.displayTitle(for: thread))
      .font(CSFont.ui(13, isActive ? .semibold : .medium))
      .foregroundStyle(isActive ? ChatPalette.nameActive : ChatPalette.nameInactive)
      .lineLimit(1)
  }

  private var favoriteLabel: some View {
    CSIconView(
      icon: thread.isFavorite ? .starFill : .star, size: 11, weight: .semibold,
      color: thread.isFavorite ? CSColor.oliveLight : CSColor.textTertiary
    )
    .frame(width: 18, height: 18)
    .contentShape(Rectangle())
  }

}

/// Short model chip on a thread row. Path prefixes are stripped; the three
/// operator tags (`claude-fable-5`, `grok-4.5`, `gpt-5.6-terra`) pass through
/// unchanged so the rail matches the palette the user actually runs.
enum ModelTag {
  static func display(for model: String?) -> String? {
    guard let model else { return nil }
    let id = String(model.split(separator: "/").last ?? Substring(model))
      .trimmingCharacters(in: .whitespacesAndNewlines)
    guard !id.isEmpty else { return nil }
    return id
  }
}

// MARK: - Recency sections (pure, unit-tested)

/// Agent recency buckets followed by the Max consultation section.
enum ThreadSection: CaseIterable, Hashable {
  case today, yesterday, thisWeek, older, maxConsultations

  var title: String {
    switch self {
    case .today: "Today"
    case .yesterday: "Yesterday"
    case .thisWeek: "This week"
    case .older: "Older"
    case .maxConsultations: "Max consultations"
    }
  }

  static func railGroups(
    _ threads: [ChatThread], now: Date = Date(), calendar: Calendar = .current
  ) -> [(section: ThreadSection, threads: [ChatThread])] {
    var groups: [ThreadSection: [ChatThread]] = [:]
    for thread in threads {
      let section: ThreadSection =
        thread.isMaxConsultation
        ? .maxConsultations
        : Self.section(for: thread.updatedAt ?? now, now: now, calendar: calendar)
      groups[section, default: []].append(thread)
    }
    return allCases.compactMap { section in
      groups[section].map { (section, $0) }
    }
  }

  /// Buckets by whole calendar days between `updatedAt` and `now`:
  /// 0 → today, 1 → yesterday, 2–6 → this week, 7+ → older. Future dates
  /// (clock skew) clamp to today.
  static func section(
    for updatedAt: Date, now: Date, calendar: Calendar = .current
  ) -> ThreadSection {
    let days =
      calendar.dateComponents(
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
  static func fallbackTitle(
    updatedAt: Date?,
    now: Date = Date(),
    calendar: Calendar = .current
  ) -> String {
    guard let updatedAt else { return "Untitled thread" }
    let relative = relativeTime(updatedAt, now: now, calendar: calendar)
    return relative.prefix(1).uppercased() + relative.dropFirst()
  }

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

  /// First segment of a rail meta line — the time, not the model/token tail.
  static func timeOnly(from meta: String) -> String {
    let head = meta.split(separator: "·").first.map { $0.trimmingCharacters(in: .whitespaces) }
    return (head?.isEmpty == false) ? head! : meta
  }

  /// "today HH:mm" / "yesterday" / "MMM d" — same shape the rail always used.
  ///
  /// Formatters are cached: `DateFormatter()` construction is a full ICU
  /// engine init, and this runs once per rail row per refresh — a fresh
  /// instance here pinned the main thread for whole refresh storms (sample
  /// 2026-08-07 10:43, 42/93 samples under NSDateFormatter init). Main
  /// thread only, like every rail meta path.
  private static let todayFormatter = makeFormatter("'today' HH:mm")
  private static let monthDayFormatter = makeFormatter("MMM d")

  private static func makeFormatter(_ format: String) -> DateFormatter {
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.dateFormat = format
    return formatter
  }

  private static func relativeTime(_ date: Date, now: Date, calendar: Calendar) -> String {
    switch ThreadSection.section(for: date, now: now, calendar: calendar) {
    case .yesterday:
      return "yesterday"
    case .today:
      return string(from: date, via: todayFormatter, calendar: calendar)
    case .thisWeek, .older, .maxConsultations:
      return string(from: date, via: monthDayFormatter, calendar: calendar)
    }
  }

  private static func string(from date: Date, via formatter: DateFormatter, calendar: Calendar)
    -> String
  {
    // Reassigning calendar forces an ICU regenerate on next use — only
    // touch it when a caller (tests inject fixed calendars) differs.
    if formatter.calendar != calendar {
      formatter.calendar = calendar
      formatter.timeZone = calendar.timeZone
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
