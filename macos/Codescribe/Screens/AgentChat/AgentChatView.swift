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
    .background(AgentWindowCapabilities(isPinned: isPinned, model: store.currentThread?.model))
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
  let model: String?

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
    let name = model?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    window?.title = name.isEmpty ? "Agent" : "Agent — \(name)"
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
      .help(isSidebarExpanded ? "Collapse sidebar (⌃⌘S)" : "Expand sidebar (⌃⌘S)")
      .accessibilityLabel("Toggle Sidebar")
      .accessibilityValue(isSidebarExpanded ? "Expanded" : "Compact")

      Text(store.currentThread?.title ?? "—")
        .font(CSFont.ui(13, .semibold))
        .foregroundStyle(ChatPalette.nameActive)
        .lineLimit(1)
        .truncationMode(.tail)
        .layoutPriority(1)

      if turnCount > 0 {
        Text("· \(turnCount)")
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textTertiary)
          .fixedSize()
      }

      liveStatusPill
        .layoutPriority(2)

      Spacer(minLength: 8)

      HStack(spacing: 10) {
        widthModeMenu

        Button {
          isPinned.toggle()
        } label: {
          Image(systemName: isPinned ? "pin.fill" : "pin")
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(isPinned ? CSColor.chromeAccent : CSColor.textTertiary)
        }
        .csFocusRing()
        .help(isPinned ? "Disable Always on Top" : "Enable Always on Top")
        .accessibilityLabel(
          isPinned ? "Agent pinned, disable Always on Top" : "Agent unpinned, enable Always on Top"
        )
        .accessibilityValue(isPinned ? "Pinned" : "Unpinned")

        Button(action: { openWindow(id: SettingsView.windowID) }) {
          CSIconView(icon: .settings, size: 14)
        }
        .csFocusRing()
        .help("Settings")

        threadMenu
      }
      .foregroundStyle(CSColor.chromeAccent)
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
      CSIconView(icon: .more, size: 14, weight: .bold)
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

  /// Live status only. Idle is silent chrome — the always-on olive pill was
  /// a second title row's worth of empty studio.
  @ViewBuilder
  private var liveStatusPill: some View {
    if store.isCancelling {
      StaticStatusPill(text: "Stopping", color: CSColor.textTertiary)
    } else if store.isStreaming {
      StatusPill(text: "Streaming", color: CSColor.terracotta, rippling: true)
    } else if store.isThinking {
      StatusPill(text: "Thinking", color: CSColor.amber, rippling: true)
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
        Text(turn.text.isEmpty ? "\(turn.attachments.count) attachment(s)" : turn.text)
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
      Text("\(request.server) · \(request.tool)")
        .font(CSFont.mono(11.5, .semibold))
        .foregroundStyle(Color.primary)
        .textSelection(.enabled)
      if !request.summary.isEmpty {
        Text(request.summary)
          .font(CSFont.ui(12, .regular))
          .foregroundStyle(Color.primary)
      }
      if let command = request.command {
        Text("$ \(command)")
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.terracotta)
          .textSelection(.enabled)
      }
      if let cwd = request.cwd {
        Text("cwd: \(cwd)")
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

// MARK: - Preview (standalone — mock engine + seeded threads)

#if DEBUG
  #Preview("Agent Chat") {
    AgentChatView(store: AgentChatStore(engine: MockChatEngine()))
      .frame(width: 840, height: 520)
  }
#endif
