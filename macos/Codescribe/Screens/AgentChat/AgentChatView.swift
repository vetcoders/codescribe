import AppKit
import SwiftUI

/// Agent Chat shell: the same native `NavigationSplitView` composition as
/// Settings — thread rail in the sidebar column, conversation in the detail.
/// Turns render You / Tool-activity / Assistant; `send` routes a streamed
/// `streamReply` turn through the injected `AgentChatEngine`.
struct AgentChatView: View {
  @StateObject var store: AgentChatStore
  private let maxPermissions: SettingsViewModel?
  /// Persist only the user's expanded/collapsed choice; the split view owns
  /// column geometry inside the bounds `CSSidebarColumn` carries.
  @AppStorage("AgentChat.sidebarExpanded.v1") private var sidebarExpanded = true
  @AppStorage("AgentChat.alwaysOnTop.v1") private var isPinned = false
  /// View state only: the context card never touches the thread, draft,
  /// model, recording or any agent call, and is never persisted. Held, not
  /// observed: only the Context button and the card layer redraw on toggle.
  @State private var contextCard = AgentContextCardPresenter()

  /// `contextCard` lets a test drive the card; the app always starts with a
  /// fresh, closed one.
  init(
    store: AgentChatStore, maxPermissions: SettingsViewModel? = nil,
    contextCard: AgentContextCardPresenter = AgentContextCardPresenter()
  ) {
    _store = StateObject(wrappedValue: store)
    self.maxPermissions = maxPermissions
    _contextCard = State(initialValue: contextCard)
  }

  var body: some View {
    NavigationSplitView(columnVisibility: sidebarVisibility) {
      ThreadRail(store: store)
    } detail: {
      ThreadDetail(store: store, maxPermissions: maxPermissions, contextCard: contextCard)
        // Sized so the expanded columns fit the window floor exactly; the
        // floor itself belongs to the window. A minimum on the whole split
        // view makes an opening sidebar slide to half its width and jump.
        .frame(minWidth: AgentWindowMetrics.detailMinWidth)
    }
    .csSidebarToggle(expanded: $sidebarExpanded)
    .modifier(
      AgentIntegratedChrome(store: store, isPinned: $isPinned, contextCard: contextCard)
    )
    .csFocusPolicy()
    .background(
      AgentWindowCapabilities(isPinned: isPinned)
    )
    .frame(
      minWidth: AgentWindowMetrics.minWidth,
      idealWidth: AgentWindowMetrics.idealWidth,
      minHeight: AgentWindowMetrics.minHeight,
      idealHeight: AgentWindowMetrics.idealHeight
    )
    .task {
      // Point-in-time marker: correlate with the adjacent "thread index
      // load" / "selected thread load" durations in the same log stream.
      AgentPerf.logger.info("agent window shell rendered")
      store.startDemoStreamIfNeeded()
    }
  }

  /// The toolbar toggle, a divider drag to collapse and ⌃⌘S all write the
  /// one persisted flag, so every path keeps the same choice.
  private var sidebarVisibility: Binding<NavigationSplitViewVisibility> {
    CSSidebarVisibility.binding(expanded: $sidebarExpanded)
  }
}

/// Separate Max permission projection; it never changes the selected chat thread.
private struct MaxPermissionPresentation: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    if !model.maxToolApprovals.isEmpty || model.maxApprovalError != nil {
      ScrollView {
        MaxApprovalCards(model: model)
          .padding(CSSpace.card)
          .csAutohidingScrollers()
      }
      .frame(maxHeight: 260)
    }
  }
}

/// Desktop-utility window floor for Agent. Named so the shared sidebar
/// column (`CSSidebarWidth.navigation`, opening at its ideal 216) plus a
/// usable detail column stay a single invariant: the root layout and the
/// AppKit window agree on this floor, including when SwiftUI recomputes its
/// window constraints after the sidebar collapses. The floor itself did not
/// move when the rail narrowed; the conversation took the points the rail
/// gave up.
///
/// The context card floats over the conversation, so the floor is the same
/// whether it is open or closed: opening it never resizes the window.
enum AgentWindowMetrics {
  static let minWidth: CGFloat = 640
  static let minHeight: CGFloat = 440
  static let idealWidth: CGFloat = 840
  static let idealHeight: CGFloat = 520
  /// Narrowest conversation column. The split view opens the rail at its
  /// ideal width, not its minimum, so the window lands exactly on `minWidth`
  /// with the rail as it opens — the rule Settings already follows
  /// (880 − 216). Sized from the 196 pt minimum, the columns overran the
  /// 640 pt floor by 20 pt on both sides.
  static let detailMinWidth: CGFloat = minWidth - CSSidebarWidth.navigation.ideal
}

/// One native titlebar for the Agent window. The title string stays "Agent"
/// for Mission Control, Exposé, and the Window menu. It is not painted, so the
/// replacement top accessory is the detail header's only owner.
enum AgentWindowChrome {
  static func title() -> String {
    String(localized: "Agent", comment: "Agent window title")
  }

  /// The one Agent window, built the same way for the app and its tests.
  /// The window carries its final style mask before SwiftUI sees the content:
  /// created from the controller first and given `.fullSizeContentView`
  /// afterwards, it never gets the split view's sidebar toggle.
  @MainActor
  static func makeWindow(content: NSViewController) -> NSWindow {
    // `HidingWindow`: the yellow button and ⌘M order this window out instead of
    // miniaturising it, and `DockPresence.adopt` below puts the minimise bit
    // back into the style mask (Founder decision, 2026-10-10).
    let window = HidingWindow(
      contentRect: NSRect(x: 0, y: 0, width: 1120, height: 720),
      styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
      backing: .buffered,
      defer: false
    )
    window.contentViewController = content
    window.setContentSize(NSSize(width: 1120, height: 720))
    window.isReleasedWhenClosed = false
    // LSUIElement accessory: join the active Space so a passive
    // `orderFrontRegardless` is actually visible during voice delivery.
    window.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
    DockPresence.adopt(window)
    // After adopt: style-mask repair must not leave a second painted "Agent" row.
    apply(window)
    return window
  }

  @MainActor
  static func apply(_ window: NSWindow) {
    window.title = title()
    window.titleVisibility = .hidden
    window.titlebarAppearsTransparent = true
    window.styleMask.insert(.fullSizeContentView)
    window.isOpaque = false
    window.backgroundColor = .clear
    window.toolbarStyle = .unified
    // The full-width glass already supplies its optical edge. A second
    // system separator becomes a bright seam when this window is key.
    window.titlebarSeparatorStyle = .none
    // AppKit ignores contentMinSize with Auto Layout. The production host
    // disables automatic SwiftUI sizing to prevent a display-cycle crash, so
    // this owner must provide the native minimum constraints itself. Width is
    // the whole split; height is readable content below the native titlebar.
    // The hosting view's safe-area guide includes that native allowance in
    // its fitting size without hard-coding the unified bar's current height.
    // Constraining the full-size host to 440 pt admitted a 440 pt user resize
    // that clipped both columns, even though setFrame repaired it in tests.
    guard let owner = window.contentView else { return }
    let dimensions: [(String, NSLayoutDimension, CGFloat)] = [
      ("codescribe-agent-floor-width", owner.widthAnchor, AgentWindowMetrics.minWidth),
      (
        "codescribe-agent-floor-height", owner.safeAreaLayoutGuide.heightAnchor,
        AgentWindowMetrics.minHeight
      ),
    ]
    for (identifier, dimension, minimum) in dimensions {
      if let constraint = owner.constraints.first(where: { $0.identifier == identifier }) {
        constraint.constant = minimum
      } else {
        let constraint = dimension.constraint(greaterThanOrEqualToConstant: minimum)
        constraint.identifier = identifier
        constraint.isActive = true
      }
    }
  }
}

/// Applies the persisted pin to the one AppDelegate-owned Agent NSWindow.
/// Updating level never orders or activates the window.
enum AgentWindowLevelPolicy {
  static func level(isPinned: Bool) -> NSWindow.Level {
    isPinned ? .floating : .normal
  }
}

private struct AgentWindowCapabilities: NSViewRepresentable {
  let isPinned: Bool

  func makeNSView(context: Context) -> NSView {
    let view = NSView(frame: .zero)
    DispatchQueue.main.async { configure(view.window) }
    return view
  }

  func updateNSView(_ nsView: NSView, context: Context) {
    DispatchQueue.main.async { configure(nsView.window) }
  }

  private func configure(_ window: NSWindow?) {
    guard let window else { return }
    window.level = AgentWindowLevelPolicy.level(isPinned: isPinned)
    AgentWindowChrome.apply(window)
  }
}

// MARK: - Integrated titlebar

/// Owns the actual detail header in a top titlebar accessory. The native bar
/// retains the split view's sidebar toggle and standard window controls. The
/// rectangular glass contains wordmark, conversation identity and all actions.
/// Alerts stay on the root modifier so the separate header hosting view does
/// not become a competing owner of rename, delete or export presentation.
private struct AgentIntegratedChrome: ViewModifier {
  @ObservedObject var store: AgentChatStore
  @Binding var isPinned: Bool
  let contextCard: AgentContextCardPresenter
  @Environment(\.openWindow) private var openWindow
  @State private var isRenaming = false
  @State private var renameText = ""
  @State private var deleteCandidate: ChatThread?
  @State private var exportOutcome: ThreadExportOutcome?
  @AppStorage(ChatLayoutPolicy.defaultsKey) private var widthModeRaw = ChatLayoutPolicy.defaultMode
    .rawValue

  private var widthMode: ChatWidthMode { ChatWidthMode.resolve(widthModeRaw) }

  func body(content: Content) -> some View {
    content
      .toolbarVisibility(.visible, for: .windowToolbar)
      .modifier(CSWindowHeaderChrome())
      .background {
        CSWindowTopHeader {
          headerContent
        }
        .ignoresSafeArea(.container, edges: .top)
      }
      .alert("Rename conversation", isPresented: $isRenaming) {
        TextField("Conversation title", text: $renameText)
        Button("Rename") {
          if let thread = store.currentThread { store.rename(thread, to: renameText) }
        }
        Button("Cancel", role: .cancel) {}
      }
      .threadDeleteConfirmation(candidate: $deleteCandidate) { store.delete($0) }
      .alert(
        exportOutcome?.title ?? Text(verbatim: ""),
        isPresented: Binding(
          get: { exportOutcome != nil },
          set: { if !$0 { exportOutcome = nil } }
        ),
        presenting: exportOutcome
      ) { outcome in
        if case .saved(let path, _) = outcome {
          let url = URL(fileURLWithPath: path)
          Button("Reveal in Finder") {
            NSWorkspace.shared.activateFileViewerSelecting([url])
          }
          Button("Open") {
            NSWorkspace.shared.open(url)
          }
        }
        Button("OK", role: .cancel) {}
      } message: { outcome in
        outcome.message
      }
  }

  /// Actual information and controls inside the rectangular detail glass.
  /// AppKit's top accessory places this in the existing titlebar band.
  private var headerContent: some View {
    HStack(spacing: 12) {
      wordmark
      identity
      Spacer(minLength: 12)
      trailingCluster
    }
    .padding(.horizontal, 12)
    .frame(height: 52)
  }

  private var wordmark: some View {
    Wordmark(size: 16)
      .fixedSize(horizontal: true, vertical: false)
  }

  /// Title and "Agent · model" beside the wordmark; absent without a thread.
  @ViewBuilder
  private var identity: some View {
    if store.currentThread != nil {
      AgentConversationHeader(store: store)
        .padding(.leading, 6)
    }
  }

  private var trailingCluster: some View {
    HStack(spacing: 12) {
      DeveloperPowerMark()
        .fixedSize()
      AgentWidthModeButton()
      AgentContextToggle(presenter: contextCard)
      pinToggle
      threadMenu
    }
    .fixedSize()
  }

  /// Always on top. This writes only the persisted flag; the window level is
  /// applied by `AgentWindowCapabilities`.
  private var pinToggle: some View {
    Button {
      isPinned.toggle()
    } label: {
      Image(systemName: isPinned ? "pin.fill" : "pin")
        .font(.system(size: 13, weight: .medium))
        .foregroundStyle(isPinned ? CSColor.chromeAccent : CSColor.secondaryInk)
        .frame(width: 24, height: 20)
        .background(
          Capsule(style: .continuous)
            .fill(isPinned ? CSColor.chromeAccent.opacity(0.16) : Color.clear)
        )
        .overlay(
          Capsule(style: .continuous)
            .strokeBorder(
              isPinned ? CSColor.chromeAccent.opacity(0.32) : Color.clear, lineWidth: 1)
        )
        .contentShape(Rectangle())
    }
    .buttonStyle(.borderless)
    .csFocusRing(cornerRadius: CSRadius.window)
    .help(
      isPinned
        ? String(
          localized: "Always on top — on. Click to let other windows cover the Agent.",
          comment: "Tooltip on the pin button while the Agent window floats")
        : String(
          localized: "Keep the Agent above other windows",
          comment: "Tooltip on the pin button while the Agent window behaves like any other")
    )
    .accessibilityLabel(
      String(localized: "Always on top", comment: "Pin toggle in the Agent header")
    )
    .accessibilityValue(
      isPinned
        ? String(localized: "On", comment: "Toggle state")
        : String(localized: "Off", comment: "Toggle state")
    )
    .accessibilityAddTraits(.isToggle)
    .accessibilityAddTraits(isPinned ? .isSelected : [])
  }

  private var threadMenu: some View {
    Menu {
      if let thread = store.currentThread {
        Section {
          Button("Rename") { beginRename(thread) }
          Button(
            thread.isFavorite
              ? String(localized: "Remove from favorites")
              : String(localized: "Add to favorites")
          ) {
            store.toggleFavorite(thread)
          }
        } header: {
          Text(
            "Conversation",
            comment: "Menu section header above the current conversation's actions")
        }
        if thread.backendId != nil {
          Section {
            Button("Export to Markdown") { export(thread, assistantOnly: false) }
            Button("Export Agent replies only") { export(thread, assistantOnly: true) }
          } header: {
            Text(
              "Exports save to the Transcripts folder",
              comment: "Menu section header above the Markdown export actions")
          }
        }
        Section {
          widthModeMenu
        }
        Section {
          Button("Open settings") { openWindow.presentSettings() }
        }
        Section {
          Button("Delete Conversation", role: .destructive) { deleteCandidate = thread }
        }
      }
    } label: {
      CSIconView(icon: .more, size: 14, color: CSColor.textTertiary)
    }
    .menuStyle(.borderlessButton)
    .menuIndicator(.hidden)
    .fixedSize()
    .help("More actions")
    .accessibilityLabel(String(localized: "More actions"))
  }

  /// A `Picker` inside a `Menu` renders as a native submenu whose selected
  /// row carries the system check mark. The previous `Button` list drew the
  /// mark through a `Label` image, which macOS menus drop, so the submenu
  /// never showed which width was active.
  private var widthModeMenu: some View {
    Picker(selection: $widthModeRaw) {
      ForEach(ChatWidthMode.allCases) { mode in
        Text(mode.label).tag(mode.rawValue)
      }
    } label: {
      Text("Conversation width", comment: "Submenu of the Agent header menu")
    }
    .accessibilityLabel(
      String(localized: "Conversation width", comment: "Submenu of the Agent header menu")
    )
    .accessibilityValue(widthMode.label)
  }

  private func beginRename(_ thread: ChatThread) {
    renameText = thread.title
    isRenaming = true
  }

  private func export(_ thread: ChatThread, assistantOnly: Bool) {
    let title = ThreadRowTitle.displayTitle(for: thread)
    if let path = store.exportMarkdown(thread, assistantOnly: assistantOnly) {
      exportOutcome = .saved(path: path, assistantOnly: assistantOnly)
    } else {
      exportOutcome = .failed(threadTitle: title, assistantOnly: assistantOnly)
    }
  }
}

// MARK: - Detail (messages under the composer dock)

/// The conversation fills its column under the window bar; queued turns,
/// approvals and the composer float above the turns in one bottom bar, so
/// turns scroll beneath the bar's glass and the composer glass instead of
/// stopping at a strip.
///
/// The context card floats over the conversation's top trailing corner,
/// under the window header and never over the composer. It takes no width
/// from the conversation; opening or closing it changes only the presenter.
private struct ThreadDetail: View {
  @ObservedObject var store: AgentChatStore
  let maxPermissions: SettingsViewModel?
  let contextCard: AgentContextCardPresenter
  @Environment(\.openWindow) private var openWindow

  var body: some View {
    conversationColumn
      // The reading canvas stops below the titlebar and beside the sidebar.
      // Extending it into the top safe area backs header glass with an opaque
      // sheet, blocking the desktop exactly over the conversation column.
      .background(CSColor.sheet, ignoresSafeAreaEdges: [.bottom])
      .overlay {
        AgentContextCardLayer(
          store: store, presenter: contextCard, openToolSettings: openToolSettings)
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .alert(
        "Speech unavailable",
        isPresented: Binding(
          get: { store.speechError != nil },
          set: { if !$0 { store.speechError = nil } }
        )
      ) {
        Button("OK") { store.speechError = nil }
      } message: {
        Text(store.speechError ?? "")
      }
  }

  /// The header glass covers the detail area above the conversation, while
  /// the turns scroll beneath it.
  private var conversationColumn: some View {
    conversation
      .modifier(AgentScrollBottomBar { composerDock })
      .frame(
        minWidth: AgentWindowMetrics.detailMinWidth,
        maxWidth: .infinity,
        maxHeight: .infinity
      )
  }

  /// The one owner of tool configuration is Settings › Agent › Tools; the
  /// card only opens it.
  private func openToolSettings() {
    SettingsDeepLink.shared.present(tab: .agentTools)
    openWindow.presentSettings()
  }

  @ViewBuilder
  private var conversation: some View {
    if let thread = store.currentThread {
      MessageList(
        threadID: thread.id,
        messages: thread.messages,
        speechUnavailableReason: store.speechUnavailableReason,
        speakingMessageID: store.speakingMessageID,
        onSpeak: { message in Task { await store.speak(message) } },
        onStopSpeaking: { store.stopSpeaking() },
        onToggleRenderMode: { messageID in
          store.toggleRenderMode(messageID: messageID, in: thread.id)
        }
      )
    } else {
      Color.clear
    }
  }

  /// Everything that answers the selected turn sits directly above the field
  /// it belongs to. Max approvals ride here too, so the sidebar keeps the
  /// window's full height.
  private var composerDock: some View {
    VStack(spacing: 0) {
      if let thread = store.currentThread {
        ForEach(store.queuedTurns(in: thread.id)) { queued in
          QueuedTurnRow(
            turn: queued,
            save: { store.editQueuedTurn(queued.id, text: $0) },
            cancel: { store.cancelQueuedTurn(queued.id) }
          )
          .padding(.horizontal, 14)
          .padding(.bottom, 4)
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
        .padding(.horizontal, 14)
        .padding(.bottom, 6)
      }
      if let maxPermissions {
        MaxPermissionPresentation(model: maxPermissions)
      }
      Composer(store: store, overlay: AppModel.shared.overlay.state)
    }
  }
}

/// One bottom bar over a scrolling column, shared by the conversation's
/// composer dock and the rail's New thread footer. On macOS 26 it is a
/// safe-area bar: the scroll view runs beneath it under the system's soft
/// edge effect, so nothing scrolled below reads as a row. Earlier systems
/// inset it on the bar material.
struct AgentScrollBottomBar<Bar: View>: ViewModifier {
  private let barContent: Bar

  init(@ViewBuilder _ barContent: () -> Bar) {
    self.barContent = barContent()
  }

  @ViewBuilder
  func body(content: Content) -> some View {
    if #available(macOS 26.0, *) {
      content
        .safeAreaBar(edge: .bottom, spacing: 0) { barContent }
        .scrollEdgeEffectStyle(.soft, for: .bottom)
    } else {
      content.safeAreaInset(edge: .bottom, spacing: 0) {
        barContent.background(.bar)
      }
    }
  }
}

/// One accepted-but-not-yet-running message. Visible until the queue's single
/// dispatch owner promotes it to the active turn; the ✕ cancels it before it
/// is ever sent.
private struct QueuedTurnRow: View {
  let turn: AgentChatStore.QueuedTurn
  let save: (String) -> Bool
  let cancel: () -> Void
  @State private var isEditing = false
  @State private var editText = ""
  @FocusState private var editFocused: Bool

  var body: some View {
    HStack(spacing: 10) {
      Text("Queued")
        .font(CSFont.mono(10, .semibold))
        .foregroundStyle(CSColor.amberLight)
      if isEditing {
        TextField("Queued message", text: $editText, axis: .vertical)
          .textFieldStyle(.plain)
          .focused($editFocused)
          .font(CSFont.ui(12, .regular))
          .foregroundStyle(Color.primary)
          .lineLimit(1...4)
          .onSubmit { commitEdit() }
          .onExitCommand { isEditing = false }
      } else {
        Text(
          turn.text.isEmpty
            ? String(localized: "\(turn.attachments.count) attachments") : turn.text
        )
        .font(CSFont.ui(12, .regular))
        .foregroundStyle(Color.primary)
        .lineLimit(2)
        .truncationMode(.tail)
        .textSelection(.enabled)
        .onTapGesture(count: 2) { beginEdit() }
      }
      Spacer()
      if isEditing {
        Button("Save") { commitEdit() }
          .csFocusRing()
          .font(CSFont.mono(10, .semibold))
          .foregroundStyle(CSColor.chromeAccent)
        Button("Cancel") { isEditing = false }
          .csFocusRing()
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textTertiary)
      } else {
        Button(action: beginEdit) {
          Image(systemName: "pencil.circle.fill")
            .font(.system(size: 13))
            .foregroundStyle(CSColor.textTertiary)
        }
        .csFocusRing()
        .help("Edit queued message")
        .accessibilityLabel("Edit queued message")
      }
      Button(action: cancel) {
        Image(systemName: "xmark.circle.fill")
          .font(.system(size: 13))
          .foregroundStyle(CSColor.textTertiary)
      }
      .csFocusRing()
      .help("Cancel queued message")
      .accessibilityLabel("Cancel queued message")
    }
    .padding(.horizontal, 12)
    .padding(.vertical, 7)
    .background(Color.primary.opacity(CSAlpha.subtle))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(Color.primary.opacity(CSAlpha.line), lineWidth: 1)
    )
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))
    .overlay {
      CSFocusOutline(isFocused: isEditing && editFocused, cornerRadius: CSRadius.card)
    }
  }

  private func beginEdit() {
    editText = turn.text
    isEditing = true
  }

  private func commitEdit() {
    if save(editText) { isEditing = false }
  }
}

/// The operator's consent surface for one suspended tool call.
///
/// Every field it paints comes from the Rust permission gate's own preview of
/// the exact call that is waiting, so two calls that differ in path, payload,
/// argv or working directory never read as the same request. The argument
/// block is already redacted and bounded in Rust; this view only gives it a
/// height so a long call cannot push the Deny button off the card.
struct ToolApprovalCard: View {
  let request: PendingToolApproval
  let reject: () -> Void
  let allowOnce: () -> Void
  let allowAlways: () -> Void

  /// Tallest the argument snapshot grows before it scrolls inside the card.
  private static let argumentsMaxHeight: CGFloat = 140

  var body: some View {
    VStack(alignment: .leading, spacing: 9) {
      HStack {
        Text("Permission required")
          .font(CSFont.ui(13, .semibold))
          .foregroundStyle(CSColor.amberLight)
        Spacer()
        Text(request.risk.replacingOccurrences(of: "_", with: " "))
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textTertiary)
      }
      Text(verbatim: "\(request.server) · \(request.tool)")
        .font(CSFont.mono(12, .semibold))
        .foregroundStyle(Color.primary)
        .textSelection(.enabled)
      if !request.summary.isEmpty {
        Text(request.summary)
          .font(CSFont.ui(12, .regular))
          .foregroundStyle(Color.primary)
      }
      if let command = request.command {
        Text(verbatim: "$ \(command)")
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.secondaryInk)
          .textSelection(.enabled)
      }
      if let cwd = request.cwd {
        Text(verbatim: "cwd: \(cwd)")
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.textTertiary)
          .textSelection(.enabled)
      }
      ForEach(request.distinctPaths, id: \.self) { path in
        Text(path)
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.textTertiary)
          .textSelection(.enabled)
      }
      if let arguments = request.arguments {
        VStack(alignment: .leading, spacing: 3) {
          Text(
            "Arguments",
            comment: "Heading above the argument snapshot on an agent permission card"
          )
          .font(CSFont.ui(11, .medium))
          .foregroundStyle(CSColor.textTertiary)
          ScrollView {
            Text(verbatim: arguments)
              .font(CSFont.mono(11, .regular))
              .foregroundStyle(Color.primary)
              .textSelection(.enabled)
              .frame(maxWidth: .infinity, alignment: .leading)
              .csAutohidingScrollers()
          }
          .frame(maxHeight: Self.argumentsMaxHeight)
        }
        .help(
          Text(
            "Secret-looking values read [REDACTED] and long values are shortened.",
            comment: "Tooltip for the argument snapshot on an agent permission card"
          )
        )
      }
      HStack {
        Spacer()
        Button("Deny", role: .cancel, action: reject)
        Button("Always allow", action: allowAlways)
        Button("Allow once", action: allowOnce)
          .buttonStyle(.borderedProminent)
      }
    }
    .padding(CSSpace.card)
    .background(Color.primary.opacity(CSAlpha.subtle))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(CSColor.hairline(CSAlpha.line), lineWidth: 1)
    )
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))
  }
}

// MARK: - Markdown export outcome (pure, unit-testable)

/// What one Markdown export did. Success names the file and the folder it
/// landed in; failure names the thread and the one place to look. Nothing
/// about the export is left to a Finder window appearing on its own.
enum ThreadExportOutcome: Equatable {
  case saved(path: String, assistantOnly: Bool)
  case failed(threadTitle: String, assistantOnly: Bool)

  var title: Text {
    switch self {
    case .saved:
      Text("Exported to Markdown", comment: "Alert title after a successful thread export")
    case .failed:
      Text("Export failed", comment: "Alert title when a thread export wrote nothing")
    }
  }

  var message: Text {
    switch self {
    case .saved(let path, let assistantOnly):
      let file = Self.fileName(of: path)
      let folder = Self.folderLabel(of: path)
      return assistantOnly
        ? Text(
          "Saved the Agent replies as \(file) in \(folder).",
          comment: "Placeholders: file name, then folder path")
        : Text(
          "Saved the whole conversation as \(file) in \(folder).",
          comment: "Placeholders: file name, then folder path")
    case .failed(let threadTitle, _):
      return Text(
        "Codescribe couldn't write the Markdown file for “\(threadTitle)”. Check the Transcripts folder shown under Settings › User › Local data, then try again.",
        comment: "The placeholder is the thread title")
    }
  }

  /// `…/2026-10-03/142501_chat.md` → `142501_chat.md`.
  static func fileName(of path: String) -> String {
    (path as NSString).lastPathComponent
  }

  /// `/Users/me/.codescribe/transcriptions/2026-10-03/x.md` →
  /// `~/.codescribe/transcriptions/2026-10-03`.
  static func folderLabel(of path: String) -> String {
    ((path as NSString).deletingLastPathComponent as NSString).abbreviatingWithTildeInPath
  }
}

// MARK: - Preview (standalone — mock engine + seeded threads)

#if DEBUG
  #Preview("Agent Chat") {
    AgentChatView(store: AgentChatStore(engine: MockChatEngine()))
      .frame(width: 840, height: 520)
  }
#endif
