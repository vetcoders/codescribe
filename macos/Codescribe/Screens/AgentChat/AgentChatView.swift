import AppKit
import SwiftUI

/// Agent Chat shell: native split between the thread rail and
/// thread view. Turns render You / Tool-activity / Assistant; `send` routes a
/// streamed `streamReply` turn through the injected `AgentChatEngine`.
struct AgentChatView: View {
  @StateObject var store: AgentChatStore
  @State private var sidebarContentWidth = AgentSidebarMetrics.minimumWidth
  private let maxPermissions: SettingsViewModel?
  /// Persist only the user's expanded/collapsed choice; AppKit owns column geometry.
  @AppStorage("AgentChat.sidebarExpanded.v1") private var sidebarExpanded = true
  @AppStorage("AgentChat.alwaysOnTop.v1") private var isPinned = false

  init(store: AgentChatStore, maxPermissions: SettingsViewModel? = nil) {
    _store = StateObject(wrappedValue: store)
    self.maxPermissions = maxPermissions
  }

  var body: some View {
    AgentColumns(
      sidebarExpanded: sidebarExpanded,
      sidebarMaximumWidth: AgentSidebarMetrics.boundedMaximum(sidebarContentWidth),
      sidebar: ThreadRail(store: store) { width in
        if width > 0 { sidebarContentWidth = width }
      },
      detail: ThreadDetail(
        store: store,
        isSidebarExpanded: sidebarExpanded,
        isPinned: $isPinned,
        toggleSidebar: toggleSidebar
      )
    )
    .safeAreaInset(edge: .bottom) {
      if let maxPermissions {
        MaxPermissionPresentation(model: maxPermissions)
      }
    }
    .csFocusPolicy()
    .developerPowerCorner(padding: 8)
    .background(AgentWindowCapabilities(isPinned: isPinned))
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

  private func toggleSidebar() {
    withAnimation {
      sidebarExpanded.toggle()
    }
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
      }
      .frame(maxHeight: 260)
    }
  }
}

/// Desktop-utility window floor for Agent. Named so the split-view rail
/// (expanded min 267) plus a usable detail column stay a single invariant.
enum AgentWindowMetrics {
  static let minWidth: CGFloat = 640
  static let minHeight: CGFloat = 440
  static let idealWidth: CGFloat = 840
  static let idealHeight: CGFloat = 520
  /// Traffic-light cluster when the rail is `.detailOnly` under
  /// `fullSizeContentView` — the sidebar toggle lives in the detail chrome
  /// and must stay clickable after native collapse.
  static let collapsedTrafficLightClearance: CGFloat = 70
}

/// Bounds enforced by the native sidebar item. Collapsing removes the whole column.
enum AgentSidebarMetrics {
  static let minimumWidth: CGFloat = 267
  static let maximumWidth: CGFloat = 360
  static func boundedMaximum(_ contentWidth: CGFloat) -> CGFloat {
    min(maximumWidth, max(minimumWidth, ceil(contentWidth)))
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
    window?.level = AgentWindowLevelPolicy.level(isPinned: isPinned)
    // Only the app identity. The model belongs to the conversation, so it
    // reads next to the thread title in the chrome — a second copy in the
    // native titlebar stacked two headers over one window (Founder brief
    // 2026-10-10). The titlebar itself stays native: it owns dragging and
    // the window controls.
    window?.title = String(localized: "Agent", comment: "Agent window title")
  }
}

/// One native owner for the divider's hard bounds and collapse state.
private struct AgentColumns<Sidebar: View, Detail: View>: NSViewControllerRepresentable {
  let sidebarExpanded: Bool
  let sidebarMaximumWidth: CGFloat
  let sidebar: Sidebar
  let detail: Detail

  func makeNSViewController(context: Context) -> NSSplitViewController {
    let controller = NSSplitViewController()
    controller.splitView.isVertical = true
    controller.splitView.dividerStyle = .thin
    let rail = NSSplitViewItem(
      sidebarWithViewController: NSHostingController(
        rootView: AnyView(sidebar.environment(\.self, context.environment))))
    rail.minimumThickness = AgentSidebarMetrics.minimumWidth
    rail.maximumThickness = sidebarMaximumWidth
    rail.canCollapse = false
    controller.addSplitViewItem(rail)
    let conversation = NSSplitViewItem(
      viewController: NSHostingController(
        rootView: AnyView(detail.environment(\.self, context.environment))))
    conversation.minimumThickness = 320
    controller.addSplitViewItem(conversation)
    return controller
  }

  func updateNSViewController(_ controller: NSSplitViewController, context: Context) {
    let rail = controller.splitViewItems[0]
    (rail.viewController as? NSHostingController<AnyView>)?.rootView =
      AnyView(sidebar.environment(\.self, context.environment))
    (controller.splitViewItems[1].viewController as? NSHostingController<AnyView>)?.rootView =
      AnyView(detail.environment(\.self, context.environment))
    if rail.maximumThickness != sidebarMaximumWidth {
      rail.maximumThickness = sidebarMaximumWidth
      if !rail.isCollapsed, rail.viewController.view.frame.width > sidebarMaximumWidth {
        controller.splitView.setPosition(sidebarMaximumWidth, ofDividerAt: 0)
      }
    }
    rail.isCollapsed = !sidebarExpanded
  }
}

// MARK: - Detail (chrome · messages · composer)

private struct ThreadDetail: View {
  @ObservedObject var store: AgentChatStore
  /// Sidebar controls live in the DETAIL header so the toggle stays reachable
  /// while the native sidebar is collapsed.
  let isSidebarExpanded: Bool
  @Binding var isPinned: Bool
  let toggleSidebar: () -> Void
  @Environment(\.openWindow) private var openWindow
  @State private var isRenaming = false
  @State private var renameText = ""
  /// Toolbar deletion goes through the same confirmation as the rail.
  @State private var deleteCandidate: ChatThread?
  /// The last Markdown export, shown until the user dismisses the alert.
  @State private var exportOutcome: ThreadExportOutcome?
  /// Shared with `MessageList` via `ChatLayoutPolicy.defaultsKey`.
  @AppStorage(ChatLayoutPolicy.defaultsKey) private var widthModeRaw = ChatLayoutPolicy.defaultMode
    .rawValue

  private var widthMode: ChatWidthMode { ChatWidthMode.resolve(widthModeRaw) }

  var body: some View {
    VStack(spacing: 0) {
      chrome
      if let thread = store.currentThread {
        MessageList(
          threadID: thread.id,
          messages: thread.messages,
          speechUnavailableReason: store.speechUnavailableReason,
          speakingMessageID: store.speakingMessageID,
          onSpeak: { message in Task { await store.speak(message) } },
          onStopSpeaking: { store.stopSpeaking() }
        ) { messageID in
          store.toggleRenderMode(messageID: messageID, in: thread.id)
        }
      } else {
        Spacer()
      }
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
      Composer(store: store, overlay: AppModel.shared.overlay.state)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background(CSColor.windowCanvas)
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
    .alert("Rename thread", isPresented: $isRenaming) {
      TextField("Thread title", text: $renameText)
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

  // One compact chrome row: sidebar · title · live pill · pin / settings / thread.
  private var chrome: some View {
    HStack(spacing: 8) {
      Button(action: toggleSidebar) {
        Image(systemName: "sidebar.leading")
          .font(.system(size: 13, weight: .medium))
      }
      .csFocusRing()
      .foregroundStyle(isSidebarExpanded ? CSColor.chromeAccent : CSColor.textTertiary)
      .keyboardShortcut("s", modifiers: [.command, .control])
      .help(
        isSidebarExpanded
          ? String(localized: "Collapse sidebar (⌃⌘S)")
          : String(localized: "Expand sidebar (⌃⌘S)")
      )
      .accessibilityLabel("Toggle Sidebar")
      .accessibilityValue(
        isSidebarExpanded
          ? String(localized: "Expanded", comment: "Sidebar state")
          : String(localized: "Compact", comment: "Sidebar state"))

      Text(store.currentThread?.title ?? "—")
        .font(CSFont.ui(13, .semibold))
        .foregroundStyle(ChatPalette.nameActive)
        .lineLimit(1)
        .truncationMode(.tail)
        .layoutPriority(1)

      if turnCount > 0 {
        Text(verbatim: "· \(turnCount)")
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textTertiary)
          .fixedSize()
      }

      // The model is conversation information, so it reads here beside the
      // thread title — not in the native titlebar. First to give way when
      // the window narrows.
      if let model = store.currentThread?.model?.trimmingCharacters(in: .whitespacesAndNewlines),
        !model.isEmpty
      {
        Text(verbatim: model)
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textTertiary)
          .lineLimit(1)
          .truncationMode(.tail)
      }

      liveStatusPill
        .layoutPriority(2)

      Spacer(minLength: 8)

      // One visual weight for the whole trailing cluster: 14 pt glyphs,
      // tertiary at rest, accent only for an active state (the pin). Even
      // 12 pt gaps — no control is louder than its neighbours.
      HStack(spacing: 12) {
        widthModeMenu

        Button {
          isPinned.toggle()
        } label: {
          Image(systemName: isPinned ? "pin.fill" : "pin")
            .font(.system(size: 14, weight: .medium))
            .foregroundStyle(isPinned ? CSColor.chromeAccent : CSColor.textTertiary)
        }
        .csFocusRing()
        .help(
          isPinned
            ? String(localized: "Disable Always on Top")
            : String(localized: "Enable Always on Top")
        )
        .accessibilityLabel(
          isPinned
            ? String(localized: "Agent pinned, disable Always on Top")
            : String(localized: "Agent unpinned, enable Always on Top")
        )
        .accessibilityValue(
          isPinned
            ? String(localized: "Pinned", comment: "Always-on-top state")
            : String(localized: "Unpinned", comment: "Always-on-top state"))

        Button(action: { openWindow.presentSettings() }) {
          CSIconView(icon: .settings, size: 14, color: CSColor.textTertiary)
        }
        .csFocusRing()
        .help("Settings")

        threadMenu
      }
    }
    .padding(
      .leading,
      isSidebarExpanded ? 12 : AgentWindowMetrics.collapsedTrafficLightClearance
    )
    .padding(.trailing, 12)
    .padding(.vertical, 6)
    .overlay(alignment: .bottom) {
      Rectangle().fill(Color.primary.opacity(0.06)).frame(height: 1)
    }
  }

  /// Comfortable / Wide / Full — persists via `ChatLayoutPolicy.defaultsKey`.
  /// A small selector with a disclosure chevron: it is a layout setting, not
  /// a headline Agent feature, and the sparkle glyph read as an AI function
  /// rather than a width choice (Founder brief 2026-10-10).
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
      HStack(spacing: 3) {
        Text(widthMode.label)
          .font(CSFont.mono(10, .medium))
        Image(systemName: "chevron.down")
          .font(.system(size: 8, weight: .semibold))
      }
      .foregroundStyle(CSColor.textTertiary)
    }
    .menuStyle(.borderlessButton)
    .menuIndicator(.hidden)
    .fixedSize()
    .help("Chat column width: Comfortable, Wide, or Full")
  }

  // Current-thread actions. Export entries appear only for persisted threads
  // (a not-yet-saved local thread has no backend id to export from). The
  // export section names its fixed destination up front: there is no file
  // chooser, the file always lands in the Transcripts folder.
  private var threadMenu: some View {
    Menu {
      if let thread = store.currentThread {
        Button("Rename") { beginRename(thread) }
        Button(
          thread.isFavorite
            ? String(localized: "Unfavorite") : String(localized: "Favorite")
        ) {
          store.toggleFavorite(thread)
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
        Divider()
        Button("Delete Thread", role: .destructive) { deleteCandidate = thread }
      }
    } label: {
      CSIconView(icon: .more, size: 14, color: CSColor.textTertiary)
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

  /// Export the thread, then report where the file went — or that nothing was
  /// written. Finder is opened only from the alert's own button, never as a
  /// side effect of the menu action. The path lives under the app's own data
  /// directory, so no permission prompt is involved.
  private func export(_ thread: ChatThread, assistantOnly: Bool) {
    let title = ThreadRowTitle.displayTitle(for: thread)
    if let path = store.exportMarkdown(thread, assistantOnly: assistantOnly) {
      exportOutcome = .saved(path: path, assistantOnly: assistantOnly)
    } else {
      exportOutcome = .failed(threadTitle: title, assistantOnly: assistantOnly)
    }
  }

  /// Live status only. Idle is silent chrome — the always-on olive pill was
  /// a second title row's worth of empty studio.
  @ViewBuilder
  private var liveStatusPill: some View {
    if store.isCancelling {
      StaticStatusPill(
        text: String(localized: "Stopping", comment: "Turn status"),
        color: CSColor.textTertiary)
    } else if store.isStreaming {
      StatusPill(
        text: String(localized: "Streaming", comment: "Turn status"),
        color: CSColor.terracotta, rippling: true)
    } else if store.isThinking {
      StatusPill(
        text: String(localized: "Thinking", comment: "Turn status"),
        color: CSColor.amber, rippling: true)
    }
  }

  private var turnCount: Int { store.currentThread?.messages.count ?? 0 }
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
        .foregroundStyle(CSColor.amber)
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
          .foregroundStyle(CSColor.oliveLight)
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
    .background(Color.primary.opacity(0.04))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.09), lineWidth: 1)
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

struct ToolApprovalCard: View {
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
          .foregroundStyle(CSColor.textTertiary)
      }
      Text(verbatim: "\(request.server) · \(request.tool)")
        .font(CSFont.mono(11.5, .semibold))
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
          .foregroundStyle(CSColor.terracotta)
          .textSelection(.enabled)
      }
      if let cwd = request.cwd {
        Text(verbatim: "cwd: \(cwd)")
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.textTertiary)
          .textSelection(.enabled)
      }
      ForEach(request.paths, id: \.self) { path in
        Text(path)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.textTertiary)
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
    .padding(CSSpace.card)
    .background(Color.primary.opacity(0.04))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(CSColor.amber.opacity(0.35), lineWidth: 1)
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
          "Saved the whole thread as \(file) in \(folder).",
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
