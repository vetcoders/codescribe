import AppKit
import SwiftUI

/// Agent Chat MVP shell. `NavigationSplitView`: local in-memory thread rail ↔
/// thread view. Turns render You / Tool-activity / Assistant; `send` routes a
/// streamed `streamReply` turn through the injected `AgentChatEngine`.
struct AgentChatView: View {
  @StateObject var store: AgentChatStore
  /// Rail state survives window close/reopen and app relaunch. Collapse is
  /// the NATIVE split-view collapse (`columnVisibility = .detailOnly`) — the
  /// same mechanism the Settings window uses, so both windows speak one
  /// design language. The previous shape faked collapse by clamping the
  /// column to a 56pt icon strip, but `navigationSplitViewColumnWidth` is
  /// read only when the column is built and the split view's width autosave
  /// outlives an `.id()` content rebuild, so "Collapse sidebar" left the
  /// monogram strip floating in a 200-500pt band (operator screenshots,
  /// 2026-08-09). Recovery stays guaranteed: the toggle lives in the DETAIL
  /// header, which never collapses.
  @AppStorage("AgentChat.sidebarExpanded.v1") private var sidebarExpanded = true
  @AppStorage("AgentChat.alwaysOnTop.v1") private var isPinned = false
  @State private var columnVisibility: NavigationSplitViewVisibility = .all

  init(store: AgentChatStore) {
    _store = StateObject(wrappedValue: store)
  }

  var body: some View {
    NavigationSplitView(columnVisibility: $columnVisibility) {
      ThreadRail(store: store, mode: .expanded)
        // Drag-resizable within the expanded bounds. The rail view
        // itself carries no fixed width — a hardcoded 236 inside a
        // resizable column left a dead band between rail and detail.
        .navigationSplitViewColumnWidth(
          min: AgentSidebarMode.expanded.minimumWidth,
          ideal: AgentSidebarMode.expanded.idealWidth,
          max: AgentSidebarMode.expanded.maximumWidth
        )
        .toolbar(removing: .sidebarToggle)
    } detail: {
      ThreadDetail(
        store: store,
        isSidebarExpanded: sidebarExpanded,
        isPinned: $isPinned,
        toggleSidebar: toggleSidebar
      )
    }
    .navigationSplitViewStyle(.balanced)
    .csFocusPolicy()
    .developerPowerCorner(padding: 12)
    .background(CSColor.glassBase)
    .background(AgentWindowCapabilities(isPinned: isPinned))
    .frame(minWidth: 760, idealWidth: 960, minHeight: 560, idealHeight: 600)
    .task {
      // Point-in-time marker: correlate with the adjacent "thread index
      // load" / "selected thread load" durations in the same log stream.
      AgentPerf.logger.info("agent window shell rendered")
      store.startDemoStreamIfNeeded()
    }
    // Restore the persisted rail state through the native mechanism.
    .onAppear { columnVisibility = sidebarExpanded ? .all : .detailOnly }
  }

  private func toggleSidebar() {
    withAnimation {
      sidebarExpanded.toggle()
      columnVisibility = sidebarExpanded ? .all : .detailOnly
    }
  }
}

/// The rail's two presentation states and the column geometry each owns. Pure,
/// so the widths are unit-testable without rendering a split view.
enum AgentSidebarMode: Equatable {
  case expanded
  case compact

  /// Icon strip width: one hit target plus symmetric padding.
  static let compactWidth: CGFloat = 56

  var isExpanded: Bool { self == .expanded }
  var minimumWidth: CGFloat { isExpanded ? 200 : Self.compactWidth }
  var idealWidth: CGFloat { isExpanded ? 236 : Self.compactWidth }
  var maximumWidth: CGFloat { isExpanded ? 360 : Self.compactWidth }

  static func toggled(_ mode: AgentSidebarMode) -> AgentSidebarMode {
    mode == .expanded ? .compact : .expanded
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
  }
}

// MARK: - Detail (header · title bar · messages · composer)

private struct ThreadDetail: View {
  @ObservedObject var store: AgentChatStore
  /// Sidebar controls live in the DETAIL header so the toggle stays reachable
  /// while the rail is in its compact icon state.
  let isSidebarExpanded: Bool
  @Binding var isPinned: Bool
  let toggleSidebar: () -> Void
  @Environment(\.openSettings) private var openSettings
  @State private var isRenaming = false
  @State private var renameText = ""
  /// Shared with `MessageList` via `ChatLayoutPolicy.defaultsKey`.
  @AppStorage(ChatLayoutPolicy.defaultsKey) private var widthModeRaw = ChatLayoutPolicy.defaultMode
    .rawValue

  private var widthMode: ChatWidthMode { ChatWidthMode.resolve(widthModeRaw) }

  var body: some View {
    VStack(spacing: 0) {
      header
      titleBar
      if let thread = store.currentThread {
        MessageList(threadID: thread.id, messages: thread.messages) { messageID in
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
          .padding(.horizontal, 20)
          .padding(.bottom, 6)
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
        .padding(.horizontal, 20)
        .padding(.bottom, 10)
      }
      Composer(store: store, overlay: AppModel.shared.overlay.state)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background(CSColor.glassBase)
    .alert("Rename thread", isPresented: $isRenaming) {
      TextField("Thread title", text: $renameText)
      Button("Rename") {
        if let thread = store.currentThread { store.rename(thread, to: renameText) }
      }
      Button("Cancel", role: .cancel) {}
    }
  }

  // Header: sidebar toggle · live status pill · width density · Settings · thread menu
  private var header: some View {
    HStack(spacing: 12) {
      Button(action: toggleSidebar) {
        Image(systemName: "sidebar.leading")
          .font(.system(size: 14, weight: .medium))
      }
      .csFocusRing(cornerRadius: 8)
      .foregroundStyle(isSidebarExpanded ? CSColor.textBody : CSColor.textFaint)
      .keyboardShortcut("s", modifiers: [.command, .control])
      .help(isSidebarExpanded ? "Collapse sidebar (⌃⌘S)" : "Expand sidebar (⌃⌘S)")
      .accessibilityLabel("Toggle Sidebar")
      .accessibilityValue(isSidebarExpanded ? "Expanded" : "Compact")

      StaticStatusPill(text: status.label, color: status.color)
      Spacer()
      HStack(spacing: 14) {
        widthModeMenu

        Button {
          isPinned.toggle()
        } label: {
          Image(systemName: isPinned ? "pin.fill" : "pin")
            .font(.system(size: 14, weight: .medium))
            .foregroundStyle(isPinned ? CSColor.chromeAccent : CSColor.textFaint)
        }
        .csFocusRing(cornerRadius: 8)
        .help(isPinned ? "Disable Always on Top" : "Enable Always on Top")
        .accessibilityLabel(
          isPinned ? "Agent pinned, disable Always on Top" : "Agent unpinned, enable Always on Top"
        )
        .accessibilityValue(isPinned ? "Pinned" : "Unpinned")

        Button(action: { openSettings() }) {
          CSIconView(icon: .settings, size: 16)
        }
        .csFocusRing(cornerRadius: 8)
        .help("Settings")

        threadMenu
      }
      .foregroundStyle(CSColor.textFaint)
    }
    .padding(.horizontal, 18)
    .padding(.vertical, 14)
    .overlay(alignment: .bottom) {
      Rectangle().fill(CSColor.hairline(0.06)).frame(height: 1)
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
      CSIconView(icon: .more, size: 16, weight: .bold)
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

  // Live status: Idle → Thinking → Streaming → Stopping.
  private var status: (label: String, color: Color) {
    if store.isCancelling { return ("Stopping", CSColor.textFaintAlt) }
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

/// One accepted-but-not-yet-running message. Visible until the queue's single
/// dispatch owner promotes it to the active turn; the ✕ cancels it before it
/// is ever sent.
private struct QueuedTurnRow: View {
  let turn: AgentChatStore.QueuedTurn
  let save: (String) -> Bool
  let cancel: () -> Void
  @State private var isEditing = false
  @State private var editText = ""

  var body: some View {
    HStack(spacing: 10) {
      Text("Queued")
        .font(CSFont.mono(10, .semibold))
        .foregroundStyle(CSColor.amber)
      if isEditing {
        TextField("Queued message", text: $editText, axis: .vertical)
          .textFieldStyle(.plain)
          .font(CSFont.ui(12, .regular))
          .foregroundStyle(CSColor.textHigh)
          .lineLimit(1...4)
          .onSubmit { commitEdit() }
          .onExitCommand { isEditing = false }
      } else {
        Text(turn.text.isEmpty ? "\(turn.attachments.count) attachment(s)" : turn.text)
          .font(CSFont.ui(12, .regular))
          .foregroundStyle(CSColor.textBody)
          .lineLimit(2)
          .truncationMode(.tail)
          .textSelection(.enabled)
          .onTapGesture(count: 2) { beginEdit() }
      }
      Spacer()
      if isEditing {
        Button("Save") { commitEdit() }
          .csFocusRing(cornerRadius: 8)
          .font(CSFont.mono(10, .semibold))
          .foregroundStyle(CSColor.oliveLight)
        Button("Cancel") { isEditing = false }
          .csFocusRing(cornerRadius: 8)
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textFaintAlt)
      } else {
        Button(action: beginEdit) {
          Image(systemName: "pencil.circle.fill")
            .font(.system(size: 13))
            .foregroundStyle(CSColor.textFaintAlt)
        }
        .csFocusRing(cornerRadius: 8)
        .help("Edit queued message")
        .accessibilityLabel("Edit queued message")
      }
      Button(action: cancel) {
        Image(systemName: "xmark.circle.fill")
          .font(.system(size: 13))
          .foregroundStyle(CSColor.textFaintAlt)
      }
      .csFocusRing(cornerRadius: 8)
      .help("Cancel queued message")
      .accessibilityLabel("Cancel queued message")
    }
    .padding(.horizontal, 12)
    .padding(.vertical, 7)
    .background(CSColor.surfaceRaised(0.04))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(CSColor.hairline(0.09), lineWidth: 1)
    )
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))
  }

  private func beginEdit() {
    editText = turn.text
    isEditing = true
  }

  private func commitEdit() {
    if save(editText) { isEditing = false }
  }
}

private struct ToolApprovalCard: View {
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
          .foregroundStyle(CSColor.textFaintAlt)
      }
      Text("\(request.server) · \(request.tool)")
        .font(CSFont.mono(11.5, .semibold))
        .foregroundStyle(CSColor.textHigh)
        .textSelection(.enabled)
      if !request.summary.isEmpty {
        Text(request.summary)
          .font(CSFont.ui(12, .regular))
          .foregroundStyle(CSColor.textBody)
      }
      if let command = request.command {
        Text("$ \(command)")
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.terracottaLight)
          .textSelection(.enabled)
      }
      if let cwd = request.cwd {
        Text("cwd: \(cwd)")
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.textFaintAlt)
          .textSelection(.enabled)
      }
      ForEach(request.paths, id: \.self) { path in
        Text(path)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.textFaintAlt)
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
    .padding(14)
    .background(CSColor.surfaceRaised(0.04))
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
      .frame(width: 960, height: 600)
      .preferredColorScheme(.dark)
  }
#endif
