import AppKit
import SwiftUI

/// Thread list content hosted in the native split sidebar column.
///
/// Uses native macOS sidebar List selection and NSSearchField idioms coherent
/// with Settings, delegating row cards, selection fills, and focus outlines
/// to the system list.
struct ThreadRail: View {
  @ObservedObject var store: AgentChatStore
  var onContentWidthChanged: (CGFloat) -> Void = { _ in }
  @State private var search: String = ""
  @State private var selectedThreadID: UUID?
  @State private var deleteCandidate: ChatThread?
  @State private var editingThreadID: UUID?
  @State private var renameDraft: String = ""
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.colorSchemeContrast) private var contrast

  var body: some View {
    VStack(spacing: 0) {
      searchHeader

      threadList
    }
    .safeAreaInset(edge: .bottom, spacing: 0) {
      newThreadFooter
    }
    .background {
      AgentSidebarMaterial(
        reduceTransparency: reduceTransparency,
        increaseContrast: contrast == .increased
      )
    }
    .onPreferenceChange(ThreadRailWidthPreference.self, perform: onContentWidthChanged)
    .onAppear {
      selectedThreadID = store.selectedThreadID
    }
    .onChange(of: store.selectedThreadID) { _, newID in
      if selectedThreadID != newID {
        selectedThreadID = newID
      }
    }
    .onChange(of: selectedThreadID) { _, newID in
      guard let newID, newID != store.selectedThreadID else { return }
      select(newID)
    }
    .onChange(of: search) { _, newValue in
      store.searchThreads(newValue)
    }
    .onChange(of: store.threadSearchQuery) { _, newValue in
      if search.trimmingCharacters(in: .whitespacesAndNewlines) != newValue {
        search = newValue
      }
    }
    .threadDeleteConfirmation(candidate: $deleteCandidate) { store.delete($0) }
  }

  private var searchHeader: some View {
    VStack(alignment: .leading, spacing: 4) {
      NativeThreadSearchField(
        text: $search,
        prompt: String(localized: "Search threads"),
        onCommit: {
          handleSearchCommit()
        },
        onCancel: {
          handleSearchCancel()
        },
        onMoveDown: {
          handleSearchMoveDown()
        }
      )

      if let error = store.threadSearchError {
        Text(error)
          .font(.caption)
          .foregroundStyle(Color.secondary)
          .padding(.horizontal, 4)
          .accessibilityLabel(error)
      }
    }
    .padding(.horizontal, 10)
    .padding(.top, 10)
    .padding(.bottom, 6)
  }

  private var threadList: some View {
    List(selection: $selectedThreadID) {
      ForEach(sectionedThreads, id: \.section) { group in
        Section(group.section.title) {
          ForEach(group.threads) { thread in
            ThreadRow(
              thread: thread,
              isActive: thread.id == selectedThreadID,
              isEditing: editingThreadID == thread.id,
              renameDraft: $renameDraft,
              onSelect: { select(thread.id) },
              onToggleFavorite: { store.toggleFavorite(thread) },
              onRequestDelete: { deleteCandidate = thread },
              onBeginRename: { beginRename(thread) },
              onCommitRename: { commitRename(thread) },
              onCancelRename: { cancelRename(thread) },
              onCopyTitle: { copyTitle(thread) },
              onExportMarkdown: { exportMarkdown(thread) }
            )
            .tag(thread.id)
          }
        }
      }
    }
    .listStyle(.sidebar)
    .scrollContentBackground(.hidden)
    .accessibilityIdentifier("thread-rail-list")
  }

  private var newThreadFooter: some View {
    VStack(spacing: 0) {
      Divider()
      Button(action: { store.newThread() }) {
        Label("New thread", systemImage: "plus")
          .frame(maxWidth: .infinity)
      }
      .buttonStyle(.bordered)
      .controlSize(.small)
      .padding(8)
      .accessibilityLabel("New thread")
    }
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

  // MARK: - Selection (pointer, keyboard, Accessibility)

  private func select(_ id: UUID) {
    guard editingThreadID != id else { return }
    store.select(id)
    if selectedThreadID != id {
      selectedThreadID = id
    }
  }

  private func handleSearchCommit() {
    if selectedThreadID == nil {
      if let first = sectionedThreads.flatMap(\.threads).first {
        select(first.id)
      }
    }
    store.requestComposerFocus()
  }

  private func handleSearchCancel() {
    store.requestComposerFocus()
  }

  private func handleSearchMoveDown() {
    let ordered = sectionedThreads.flatMap(\.threads)
    guard !ordered.isEmpty else { return }
    if let current = selectedThreadID, ordered.contains(where: { $0.id == current }) {
      if let next = ThreadRailNavigation.adjacentThread(from: current, in: ordered, step: 1) {
        select(next.id)
      }
    } else if let first = ordered.first {
      select(first.id)
    }
  }

  // MARK: - Rename (inline edit)

  private func beginRename(_ thread: ChatThread) {
    guard editingThreadID != thread.id else { return }
    renameDraft = ThreadRowTitle.displayTitle(for: thread)
    editingThreadID = thread.id
  }

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

  // MARK: - Copy and Export

  private func copyTitle(_ thread: ChatThread) {
    let title = ThreadRowTitle.displayTitle(for: thread)
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(title, forType: .string)
  }

  private func exportMarkdown(_ thread: ChatThread) {
    _ = store.exportMarkdown(thread, assistantOnly: false)
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

/// One history row presented in the native sidebar List.
///
/// System selection, hover, and focus are owned by the enclosing List.
/// Two-line readable row with normal system UI typography and secondary metadata.
private struct ThreadRow: View {
  let thread: ChatThread
  let isActive: Bool
  let isEditing: Bool
  @Binding var renameDraft: String
  let onSelect: () -> Void
  let onToggleFavorite: () -> Void
  let onRequestDelete: () -> Void
  let onBeginRename: () -> Void
  let onCommitRename: () -> Void
  let onCancelRename: () -> Void
  let onCopyTitle: () -> Void
  let onExportMarkdown: () -> Void

  @FocusState private var renameFieldFocused: Bool

  private var title: String { ThreadRowTitle.displayTitle(for: thread) }

  var body: some View {
    rowContent(measuring: false)
      .background {
        rowContent(measuring: true)
          .fixedSize(horizontal: true, vertical: true)
          .hidden()
          .allowsHitTesting(false)
          .accessibilityHidden(true)
          .background {
            GeometryReader { probe in
              Color.clear.preference(
                key: ThreadRailWidthPreference.self,
                value: probe.size.width + 44)
            }
          }
      }
      .padding(.vertical, 3)
      .contextMenu {
        Button("Rename") {
          onBeginRename()
        }
        Button(favoriteActionTitle) {
          onToggleFavorite()
        }
        Button("Copy") {
          onCopyTitle()
        }
        if thread.backendId != nil {
          Button("Export to Markdown") {
            onExportMarkdown()
          }
        }
        Divider()
        Button("Delete Thread", role: .destructive) {
          onRequestDelete()
        }
      }
      .accessibilityElement(children: isEditing ? .contain : .combine)
      .accessibilityLabel(title)
      .accessibilityValue(thread.meta)
      .accessibilityAddTraits(.isButton)
      .accessibilityAddTraits(isActive ? .isSelected : [])
      .accessibilityHint(
        String(localized: "Opens this thread", comment: "Accessibility hint on a history row")
      )
      .accessibilityAction { onSelect() }
      .accessibilityAction(named: Text("Rename")) { onBeginRename() }
      .accessibilityAction(named: Text(favoriteActionTitle)) { onToggleFavorite() }
      .accessibilityAction(named: Text("Copy")) { onCopyTitle() }
      .accessibilityAction(named: Text("Delete Thread")) { onRequestDelete() }
      .accessibilityIdentifier("thread-row-\(thread.id.uuidString)")
  }

  private var favoriteActionTitle: String {
    thread.isFavorite
      ? String(localized: "Remove from favorites")
      : String(localized: "Add to favorites")
  }

  private var favoriteHelp: String {
    thread.isFavorite
      ? String(localized: "Unfavorite thread")
      : String(localized: "Favorite thread")
  }

  private func rowContent(measuring: Bool) -> some View {
    VStack(alignment: .leading, spacing: 3) {
      HStack(spacing: 6) {
        if thread.isMaxConsultation {
          Image(systemName: "sparkles")
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(Color.secondary)
            .accessibilityLabel("Max consultation")
        }
        if isEditing && !measuring {
          TextField("", text: $renameDraft)
            .textFieldStyle(.plain)
            .font(.system(size: 13, weight: .semibold))
            .focused($renameFieldFocused)
            .onSubmit { onCommitRename() }
            .onExitCommand { onCancelRename() }
            .onAppear { DispatchQueue.main.async { renameFieldFocused = true } }
            .onChange(of: renameFieldFocused) { _, focused in
              if !focused, isEditing { onCommitRename() }
            }
        } else {
          if measuring {
            titleLabel
          } else {
            titleLabel
              .onTapGesture(count: 2) { onBeginRename() }
          }
        }
        Spacer(minLength: 4)
        if measuring {
          favoriteLabel
        } else {
          Button(action: onToggleFavorite) {
            favoriteLabel
          }
          .buttonStyle(.plain)
          .opacity(thread.isFavorite ? 1 : 0.35)
          .help(favoriteHelp)
          .accessibilityLabel(favoriteHelp)
        }
      }

      HStack(spacing: 5) {
        Text(ThreadRailMeta.timeOnly(from: thread.meta))
          .lineLimit(1)
          .font(.caption)
          .foregroundStyle(Color.secondary)

        if let tag = ModelTag.display(for: thread.model) {
          Text(verbatim: "·")
            .font(.caption)
            .foregroundStyle(Color.secondary.opacity(0.6))
          Text(tag)
            .lineLimit(1)
            .truncationMode(.tail)
            .font(.caption)
            .foregroundStyle(Color.secondary)
        }
      }
    }
  }

  private var titleLabel: some View {
    Text(ThreadRowTitle.displayTitle(for: thread))
      .font(.system(size: 13, weight: isActive ? .semibold : .regular))
      .foregroundStyle(isActive ? Color.primary : Color.primary.opacity(0.85))
      .lineLimit(1)
      .truncationMode(.tail)
  }

  private var favoriteLabel: some View {
    Image(systemName: thread.isFavorite ? "star.fill" : "star")
      .font(.system(size: 11, weight: .medium))
      .foregroundStyle(Color.secondary)
      .frame(width: 16, height: 16)
      .contentShape(Rectangle())
  }
}

/// Native macOS search field control wrapping NSSearchField.
///
/// Manages live search without modifying bindings during updateNSView.
/// Releasing Escape clears the search field or dismisses to composer.
struct NativeThreadSearchField: NSViewRepresentable {
  @Binding var text: String
  var prompt: String = String(localized: "Search threads")
  var onCommit: (() -> Void)? = nil
  var onCancel: (() -> Void)? = nil
  var onMoveDown: (() -> Void)? = nil

  func makeCoordinator() -> Coordinator {
    Coordinator(self)
  }

  func makeNSView(context: Context) -> NSSearchField {
    let field = NSSearchField()
    field.placeholderString = prompt
    field.controlSize = .small
    field.font = .systemFont(ofSize: NSFont.systemFontSize(for: .small))
    field.focusRingType = .default
    field.sendsSearchStringImmediately = true
    field.sendsWholeSearchString = false
    field.target = context.coordinator
    field.action = #selector(Coordinator.searchAction(_:))
    field.delegate = context.coordinator
    field.setAccessibilityLabel(prompt)
    field.setAccessibilityIdentifier("thread-rail-search-field")
    field.setContentHuggingPriority(.defaultHigh, for: .vertical)
    field.setContentCompressionResistancePriority(.defaultHigh, for: .vertical)
    return field
  }

  func updateNSView(_ nsView: NSSearchField, context: Context) {
    context.coordinator.parent = self
    if nsView.stringValue != text {
      nsView.stringValue = text
    }
  }

  func sizeThatFits(
    _ proposal: ProposedViewSize, nsView: NSSearchField, context: Context
  ) -> CGSize? {
    let size = nsView.fittingSize
    let width = proposal.width ?? size.width
    return CGSize(width: width, height: max(22, size.height))
  }

  @MainActor
  final class Coordinator: NSObject, NSSearchFieldDelegate {
    var parent: NativeThreadSearchField

    init(_ parent: NativeThreadSearchField) {
      self.parent = parent
    }

    @objc func searchAction(_ sender: NSSearchField) {
      let string = sender.stringValue
      if parent.text != string {
        parent.text = string
      }
    }

    func controlTextDidChange(_ obj: Notification) {
      guard let field = obj.object as? NSSearchField else { return }
      let string = field.stringValue
      if parent.text != string {
        parent.text = string
      }
    }

    func control(
      _ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector
    ) -> Bool {
      if commandSelector == #selector(NSResponder.cancelOperation(_:)) {
        if !parent.text.isEmpty {
          parent.text = ""
          if let field = control as? NSSearchField {
            field.stringValue = ""
          }
          return true
        } else {
          guard let onCancel = parent.onCancel else { return false }
          onCancel()
          return true
        }
      }
      if commandSelector == #selector(NSResponder.insertNewline(_:)) {
        parent.onCommit?()
        return true
      }
      if commandSelector == #selector(NSResponder.moveDown(_:)) {
        if let onMoveDown = parent.onMoveDown {
          onMoveDown()
          return true
        }
      }
      return false
    }
  }
}

// MARK: - Keyboard navigation (pure, unit-testable)

enum ThreadRailNavigation {
  /// The thread `step` rows away from `id` in the rail's visible order
  /// (`+1` down, `-1` up). The ends do not wrap; an unknown `id` or an
  /// out-of-range step yields `nil`.
  static func adjacentThread(
    from id: UUID, in ordered: [ChatThread], step: Int
  ) -> ChatThread? {
    guard let index = ordered.firstIndex(where: { $0.id == id }) else { return nil }
    let target = index + step
    guard ordered.indices.contains(target) else { return nil }
    return ordered[target]
  }
}

// MARK: - Delete confirmation (one contract for every entry point)

/// Every way to delete a thread — the rail's context menu, the Accessibility
/// action and the detail toolbar menu — presents this same confirmation.
/// Cancel keeps the thread; only the destructive button deletes.
struct ThreadDeleteConfirmation: ViewModifier {
  @Binding var candidate: ChatThread?
  let onConfirm: (ChatThread) -> Void

  func body(content: Content) -> some View {
    content.confirmationDialog(
      "Delete this thread?",
      isPresented: Binding(
        get: { candidate != nil },
        set: { if !$0 { candidate = nil } }
      ),
      titleVisibility: .visible,
      presenting: candidate
    ) { thread in
      Button("Delete Thread", role: .destructive) {
        onConfirm(thread)
        candidate = nil
      }
      Button("Cancel", role: .cancel) {
        candidate = nil
      }
    } message: { thread in
      Text(
        "This removes “\(ThreadRowTitle.displayTitle(for: thread))” and its saved conversation. There is no undo.",
        comment: "The placeholder is the thread title")
    }
  }
}

extension View {
  /// Attach the shared thread-deletion confirmation; `candidate` non-nil shows it.
  func threadDeleteConfirmation(
    candidate: Binding<ChatThread?>, onConfirm: @escaping (ChatThread) -> Void
  ) -> some View {
    modifier(ThreadDeleteConfirmation(candidate: candidate, onConfirm: onConfirm))
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
    case .today: String(localized: "Today", comment: "Thread rail section")
    case .yesterday: String(localized: "Yesterday", comment: "Thread rail section")
    case .thisWeek: String(localized: "This week", comment: "Thread rail section")
    case .older: String(localized: "Older", comment: "Thread rail section")
    case .maxConsultations:
      String(localized: "Max consultations", comment: "Thread rail section")
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
    guard let updatedAt else {
      return String(localized: "Untitled thread", comment: "Thread with no title")
    }
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

  /// "today HH:mm" / "yesterday" / month and day — same shape the rail always
  /// used. The words come from the catalog; the month name and the order of
  /// month and day follow the calendar's locale. The time of day stays
  /// `HH:mm`, like the other timestamps in the app.
  ///
  /// Formatters are cached: `DateFormatter()` construction is a full ICU
  /// engine init, and this runs once per rail row per refresh — a fresh
  /// instance here pinned the main thread for whole refresh storms (sample
  /// 2026-08-07 10:43, 42/93 samples under NSDateFormatter init). Main
  /// thread only, like every rail meta path.
  private static let timeFormatter: DateFormatter = {
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.dateFormat = "HH:mm"
    return formatter
  }()
  private static let monthDayFormatter = DateFormatter()

  private static func relativeTime(_ date: Date, now: Date, calendar: Calendar) -> String {
    switch ThreadSection.section(for: date, now: now, calendar: calendar) {
    case .yesterday:
      return String(localized: "yesterday", comment: "Thread rail row: updated yesterday")
    case .today:
      let time = string(from: date, via: timeFormatter, calendar: calendar)
      return String(
        localized: "today \(time)",
        comment: "Thread rail row: updated today; the placeholder is the time of day, e.g. 14:05")
    case .thisWeek, .older, .maxConsultations:
      // Same economy as the calendar below: a new locale regenerates the ICU
      // pattern, so it is applied only when it differs.
      let locale = calendar.locale ?? .current
      if monthDayFormatter.dateFormat.isEmpty
        || monthDayFormatter.locale.identifier != locale.identifier
      {
        monthDayFormatter.locale = locale
        monthDayFormatter.setLocalizedDateFormatFromTemplate("MMMd")
      }
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

/// Selected-row emphasis. Increase Contrast is strictly stronger than the
/// default so a row stays readable on the system selection color.
enum AgentSidebarSelection {
  static func fillOpacity(increaseContrast: Bool) -> CGFloat {
    increaseContrast ? 1 : 0.85
  }

  static func strokeWidth(increaseContrast: Bool) -> CGFloat {
    increaseContrast ? 2 : 1
  }
}

/// System sidebar glass. Reduce Transparency falls back to an opaque
/// content background that still sits inside the window.
struct AgentSidebarMaterial: NSViewRepresentable {
  var reduceTransparency: Bool
  var increaseContrast: Bool

  func makeNSView(context: Context) -> NSVisualEffectView {
    let view = NSVisualEffectView()
    apply(to: view)
    return view
  }

  func updateNSView(_ view: NSVisualEffectView, context: Context) {
    apply(to: view)
  }

  private func apply(to view: NSVisualEffectView) {
    if reduceTransparency {
      view.material = .contentBackground
      view.blendingMode = .withinWindow
    } else {
      view.material = .sidebar
      view.blendingMode = .behindWindow
    }
    view.state = .followsWindowActiveState
    view.isEmphasized = increaseContrast
  }
}
