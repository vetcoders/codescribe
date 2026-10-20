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
  /// View state only: opening the context panel never touches the thread,
  /// draft, model, recording or any agent call.
  @AppStorage(AgentContextPanelPreference.defaultsKey) private var contextPanelOpen =
    AgentContextPanelPreference.defaultIsOpen

  init(store: AgentChatStore, maxPermissions: SettingsViewModel? = nil) {
    _store = StateObject(wrappedValue: store)
    self.maxPermissions = maxPermissions
  }

  var body: some View {
    NavigationSplitView(columnVisibility: sidebarVisibility) {
      ThreadRail(store: store)
    } detail: {
      ThreadDetail(
        store: store, maxPermissions: maxPermissions, isContextOpen: $contextPanelOpen
      )
      // Sized so the expanded columns fit the window floor exactly; the
      // floor itself belongs to the window. A minimum on the whole split
      // view makes an opening sidebar slide to half its width and jump.
      .frame(minWidth: AgentWindowMetrics.detailFloorWidth(contextPanelOpen: contextPanelOpen))
    }
    .csSidebarToggle(expanded: $sidebarExpanded)
    .modifier(AgentIntegratedChrome(store: store, isPinned: $isPinned))
    .csFocusPolicy()
    .background(
      AgentWindowCapabilities(isPinned: isPinned, contextPanelOpen: contextPanelOpen)
    )
    .frame(
      minWidth: AgentWindowMetrics.windowFloorWidth(contextPanelOpen: contextPanelOpen),
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
/// The optional context panel never eats into the conversation: the
/// conversation keeps `detailMinWidth` whether the panel is open or not, and
/// an open panel raises the window floor by its own footprint instead
/// (`windowFloorWidth(contextPanelOpen:)`). A window narrower than that floor
/// widens when the panel opens (`AgentWindowChrome.apply`).
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
  /// The context panel is a fixed column (Founder, 2026-10-20: start at
  /// 260 pt). Long titles wrap inside it; nothing in it widens the panel.
  static let contextPanelWidth: CGFloat = 260
  /// The hairline between the conversation and the panel.
  static let contextPanelDividerWidth: CGFloat = 1
  /// Points an open panel adds to the detail column.
  static let contextPanelFootprint: CGFloat = contextPanelWidth + contextPanelDividerWidth
  /// The conversation header row and the panel header share one height, so
  /// "Agent context" sits on the same line as the conversation title.
  static let conversationHeaderHeight: CGFloat = 48

  /// Window floor: the closed-panel floor plus the panel footprint when open.
  static func windowFloorWidth(contextPanelOpen: Bool) -> CGFloat {
    contextPanelOpen ? minWidth + contextPanelFootprint : minWidth
  }

  /// Detail column floor: the conversation's `detailMinWidth`, plus the panel
  /// when it is open. Still exactly the window floor minus the rail as it opens.
  static func detailFloorWidth(contextPanelOpen: Bool) -> CGFloat {
    windowFloorWidth(contextPanelOpen: contextPanelOpen) - CSSidebarWidth.navigation.ideal
  }
}

/// One native titlebar for the Agent window. The title string stays "Agent"
/// for Mission Control, Exposé, and the Window menu. It is not painted, so the
/// toolbar is the only header plane.
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
  static func apply(_ window: NSWindow, contextPanelOpen: Bool = false) {
    window.title = title()
    window.titleVisibility = .hidden
    window.titlebarAppearsTransparent = true
    window.styleMask.insert(.fullSizeContentView)
    window.isOpaque = false
    window.backgroundColor = .clear
    window.toolbarStyle = .unified
    window.titlebarSeparatorStyle = .automatic
    // The window owns the floor. The columns alone would let a collapsed
    // window shrink to the detail column, and a minimum on the split view
    // makes an opening sidebar slide and jump.
    let floor = NSSize(
      width: AgentWindowMetrics.windowFloorWidth(contextPanelOpen: contextPanelOpen),
      height: AgentWindowMetrics.minHeight
    )
    window.contentMinSize = floor
    growToFloor(window, floor: floor)
  }

  /// A raised floor does not resize a window by itself. When the context
  /// panel opens in a window narrower than the new floor, the window widens by
  /// the shortfall, keeping its leading edge unless that would leave the
  /// screen. Closing the panel lowers the floor and leaves the width alone.
  @MainActor
  static func growToFloor(_ window: NSWindow, floor: NSSize) {
    let content = window.contentRect(forFrameRect: window.frame)
    let shortfall = floor.width - content.width
    guard shortfall > 0.5 else { return }
    var frame = window.frame
    frame.size.width += shortfall
    if let visible = (window.screen ?? NSScreen.main)?.visibleFrame, frame.maxX > visible.maxX {
      frame.origin.x = max(visible.minX, visible.maxX - frame.width)
    }
    window.setFrame(frame, display: true)
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
  let contextPanelOpen: Bool

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
    AgentWindowChrome.apply(window, contextPanelOpen: contextPanelOpen)
  }
}

// MARK: - Integrated titlebar

/// The window's native bar. The split view supplies the sidebar toggle; the
/// wordmark and pin/menu share the toolbar. The conversation's identity
/// (title, "Agent · model", live status, Context) lives in one place, the
/// header row at the top of the conversation column
/// (`AgentConversationHeader`), so the toolbar carries no principal item.
/// Rename, delete, and export alerts stay on this modifier: alerts attached to
/// a toolbar item do not present.
private struct AgentIntegratedChrome: ViewModifier {
  @ObservedObject var store: AgentChatStore
  @Binding var isPinned: Bool
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
      .toolbar { agentToolbar }
      .modifier(CSWindowHeaderChrome())
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

  /// Brand and actions occupy one native bar, without separate glass
  /// capsules. The bar stays visible as the turns scroll beneath it.
  @ToolbarContentBuilder
  private var agentToolbar: some ToolbarContent {
    if #available(macOS 26.0, *) {
      ToolbarItem(placement: .navigation) { CSWindowHeaderItem { wordmark } }
        .sharedBackgroundVisibility(.hidden)
      ToolbarItem(placement: .primaryAction) { CSWindowHeaderItem { trailingCluster } }
        .sharedBackgroundVisibility(.hidden)
    } else {
      ToolbarItem(placement: .navigation) { CSWindowHeaderItem { wordmark } }
      ToolbarItem(placement: .primaryAction) { CSWindowHeaderItem { trailingCluster } }
    }
  }

  private var wordmark: some View {
    Wordmark(size: 16)
      .fixedSize(horizontal: true, vertical: false)
  }

  private var trailingCluster: some View {
    HStack(spacing: 12) {
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
        .foregroundStyle(isPinned ? CSColor.chromeAccent : Color.secondary)
        .frame(width: 24, height: 20)
        .background(
          RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous)
            .fill(isPinned ? CSColor.chromeAccent.opacity(0.16) : Color.clear)
        )
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous)
            .strokeBorder(
              isPinned ? CSColor.chromeAccent.opacity(0.32) : Color.clear, lineWidth: 1)
        )
        .contentShape(Rectangle())
    }
    .buttonStyle(.borderless)
    .csFocusRing(cornerRadius: CSRadius.pill)
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

/// The conversation fills its column. Its header row (title, "Agent ·
/// model", Context) sits under the toolbar; queued turns, approvals and the
/// composer float above the turns in one bottom bar, so turns scroll beneath
/// the header glass and the composer glass instead of stopping at a strip.
///
/// The optional context panel is a fixed trailing column beside the
/// conversation, inside the detail column, never over the composer. Opening
/// or closing it changes only the persisted flag.
private struct ThreadDetail: View {
  @ObservedObject var store: AgentChatStore
  let maxPermissions: SettingsViewModel?
  @Binding var isContextOpen: Bool
  @Environment(\.openWindow) private var openWindow

  var body: some View {
    HStack(spacing: 0) {
      conversationColumn
      if isContextOpen {
        HStack(spacing: 0) {
          // Runs up through the titlebar like the sidebar edge: the header
          // glass covers the conversation, not the panel.
          Divider()
            .frame(width: AgentWindowMetrics.contextPanelDividerWidth)
            .ignoresSafeArea(.container, edges: .top)
          AgentContextPanel(
            thread: store.currentThread,
            close: { isContextOpen = false },
            openToolSettings: openToolSettings
          )
        }
        .transition(.move(edge: .trailing).combined(with: .opacity))
      }
    }
    .animation(.easeOut(duration: 0.18), value: isContextOpen)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background(CSColor.sheet)
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

  /// The header glass is measured on the turns, inside the header row's
  /// inset, so one plane covers the toolbar and the header row above the
  /// conversation, and the turns scroll beneath both.
  private var conversationColumn: some View {
    conversation
      .modifier(CSDetailHeaderGlass())
      .safeAreaInset(edge: .top, spacing: 0) {
        if store.currentThread != nil {
          AgentConversationHeader(store: store, isContextOpen: $isContextOpen)
        }
      }
      .modifier(AgentScrollBottomBar { composerDock })
      .frame(
        minWidth: AgentWindowMetrics.detailMinWidth,
        maxWidth: .infinity,
        maxHeight: .infinity
      )
  }

  /// The one owner of tool configuration is Settings › Agent › Tools; the
  /// panel only opens it.
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
      ForEach(request.distinctPaths, id: \.self) { path in
        Text(path)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.textTertiary)
          .textSelection(.enabled)
      }
      if let arguments = request.arguments {
        VStack(alignment: .leading, spacing: 3) {
          Text(
            "Arguments",
            comment: "Heading above the argument snapshot on an agent permission card"
          )
          .font(CSFont.ui(10.5, .medium))
          .foregroundStyle(CSColor.textTertiary)
          ScrollView {
            Text(verbatim: arguments)
              .font(CSFont.mono(10.5, .regular))
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
