import SwiftUI

/// Left rail, two states. `.expanded`: wordmark, search field, THREADS list,
/// and a dashed "+ New thread" footer. `.compact`: a narrow icon strip that
/// keeps thread switching and "+ New thread" one click away — the rail is never
/// removed from the split view, so the window can't show an empty band.
struct ThreadRail: View {
  @ObservedObject var store: AgentChatStore
  var mode: AgentSidebarMode = .expanded
  @State private var search: String = ""
  @State private var deleteCandidate: ChatThread?
  @State private var editingThreadID: UUID?
  @State private var renameDraft: String = ""

  var body: some View {
    Group {
      if mode.isExpanded {
        expandedRail
      } else {
        compactRail
      }
    }
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

  /// Narrow icon strip: brand dot, one dot per recent thread (active tinted),
  /// and a "+" footer. No fixed frame — the column width owns the geometry.
  private var compactRail: some View {
    VStack(spacing: 0) {
      ModeDot(color: CSColor.terracotta, size: 9)
        .padding(.top, 18)
        .padding(.bottom, 14)

      ScrollView {
        LazyVStack(spacing: 6) {
          ForEach(filteredThreads) { thread in
            let title = ThreadRowTitle.displayTitle(for: thread)
            let isActive = thread.id == store.selectedThreadID
            Button {
              store.select(thread.id)
            } label: {
              Text(ThreadRowTitle.compactMonogram(for: thread))
                .font(CSFont.ui(11, .semibold))
                .foregroundStyle(
                  isActive ? CSColor.chromeAccent : CSColor.textMuted
                )
                .lineLimit(1)
                .minimumScaleFactor(0.7)
                .frame(width: 28, height: 28)
                .background(
                  isActive
                    ? CSColor.chromeAccent.opacity(0.12)
                    : CSColor.surfaceRaised(0.03)
                )
                .clipShape(
                  RoundedRectangle(cornerRadius: 8, style: .continuous)
                )
                .overlay(
                  RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(
                      isActive
                        ? CSColor.chromeAccent.opacity(0.45)
                        : CSColor.hairline(0.08),
                      lineWidth: 1
                    )
                )
                .contentShape(Rectangle())
            }
            .csFocusRing(cornerRadius: 8)
            .help(title)
            .accessibilityLabel(title)
            .accessibilityAddTraits(isActive ? [.isSelected] : [])
          }
        }
        .padding(.vertical, 4)
      }
      .scrollContentBackground(.hidden)

      Button(action: { store.newThread() }) {
        Text("+")
          .font(CSFont.ui(15, .semibold))
          .foregroundStyle(CSColor.textMuted)
          .frame(width: 30, height: 30)
          .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
              .strokeBorder(
                CSColor.hairline(0.14),
                style: StrokeStyle(lineWidth: 1, dash: [4, 3])
              )
          )
          .contentShape(Rectangle())
      }
      .csFocusRing(cornerRadius: 8)
      .help("New thread")
      .accessibilityLabel("New thread")
      .padding(.vertical, 12)
      .overlay(alignment: .top) {
        Rectangle().fill(CSColor.hairline(0.06)).frame(height: 1)
      }
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
  }

  private var expandedRail: some View {
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
        TextField(
          "", text: $search,
          prompt:
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
        .csFocusRing(cornerRadius: 8)
      }
      .padding(12)
      .overlay(alignment: .top) {
        Rectangle().fill(CSColor.hairline(0.06)).frame(height: 1)
      }
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

  /// Identity for the collapsed rail. The strip used to draw one anonymous
  /// 7pt dot per thread — a vertical row of identical dots that told the user
  /// nothing and forced a hover-and-wait tooltip to pick a conversation
  /// (UI_DIVERGENCE_AUDIT pkt 2). Two initials from the display title carry
  /// enough identity at 28pt; the digit/letter scan keeps titles that open
  /// with punctuation or an emoji from yielding a blank tile.
  static func compactMonogram(
    for thread: ChatThread,
    now: Date = Date(),
    calendar: Calendar = .current
  ) -> String {
    let title = displayTitle(for: thread, now: now, calendar: calendar)
    let words =
      title
      .split(whereSeparator: { !$0.isLetter && !$0.isNumber })
      .prefix(2)
    let initials = words.compactMap { $0.first }.map(String.init).joined()
    // `displayTitle` always resolves to a string carrying a letter or digit
    // (ThreadTitlePolicy rejects the rest and the date fallback never is),
    // so the dot is a guard against a future title source, not a live case.
    return initials.isEmpty ? "•" : initials.uppercased()
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
          Circle().fill(CSColor.chromeAccent).frame(width: 6, height: 6)
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
          Text(ThreadRowTitle.displayTitle(for: thread))
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
        .csFocusRing(cornerRadius: 8)
        .opacity(thread.isFavorite || isActive ? 1 : 0.38)
        .help(thread.isFavorite ? "Unfavorite thread" : "Favorite thread")
      }
      HStack(spacing: 6) {
        if let tag = ModelTag.display(for: thread.model) {
          Text(tag)
            .font(CSFont.mono(9, .semibold))
            .foregroundStyle(isActive ? CSColor.modeAgent : CSColor.textFaintAlt)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(
              (isActive ? CSColor.modeAgent : CSColor.textFaintAlt).opacity(0.14)
            )
            .clipShape(Capsule())
            .accessibilityLabel("model \(tag)")
        }
        Text(ThreadRailMeta.timeOnly(from: thread.meta))
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(isActive ? ChatPalette.activeThreadSub : CSColor.textFaintAlt)
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .padding(.horizontal, 12)
    .padding(.vertical, 11)
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
    case .thisWeek, .older:
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
