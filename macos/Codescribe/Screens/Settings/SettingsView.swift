import AppKit
import Combine
import SwiftUI

// Shared Settings content, hosted by the app’s single resizable Settings window.
struct SettingsView: View {
  static let windowID = "codescribe-settings"
  /// Narrowest detail column: an 880 pt window with the 216 pt sidebar open.
  /// The window minimum is carried by the columns. A minimum width on the
  /// whole `NavigationSplitView` makes the sidebar slide to half its width and
  /// then jump whenever the window is narrower than that minimum plus the
  /// sidebar, because the split view lays the opening sidebar out beside a
  /// detail that may not shrink yet.
  static let detailMinWidth: CGFloat = 664
  @StateObject private var model: SettingsViewModel
  // Native selection reconciliation writes only view state. Navigation and
  // its refresh effects are committed by onChange, outside the List setter.
  @State private var sidebarSelection: SettingsSection?
  /// Persist only the user's expanded/collapsed choice; the split view owns
  /// column geometry inside the bounds `CSSidebarColumn` carries.
  @AppStorage("Settings.sidebarExpanded.v1") private var sidebarExpanded = true
  @State private var search: String = ""
  @State private var pendingScrollAnchor: SettingsAnchor?
  @State private var hostWindow: NSWindow?

  init(model: SettingsViewModel? = nil) {
    _model = StateObject(wrappedValue: model ?? SettingsViewModel())
    _sidebarSelection = State(initialValue: model?.section ?? .creator)
  }

  var body: some View {
    NavigationSplitView(columnVisibility: sidebarVisibility) {
      CSSidebarColumn(
        width: .navigation,
        search: $search,
        prompt: Text("Search settings")
      ) {
        sidebar
      } footer: {
        SettingsHealthFooter(model: model)
      }
    } detail: {
      detail
        .frame(minWidth: Self.detailMinWidth)
        .modifier(CSDetailHeaderGlass())
        .background(CSColor.sheet)
    }
    .csSidebarToggle(expanded: $sidebarExpanded)
    .toolbar {
      if #available(macOS 26.0, *) {
        brandToolbar.sharedBackgroundVisibility(.hidden)
      } else {
        brandToolbar
      }
    }
    .modifier(CSWindowHeaderChrome())
    .csFocusPolicy()
    .controlSize(.regular)
    .frame(maxWidth: .infinity, minHeight: 620, maxHeight: .infinity)
    .onAppear {
      sidebarSelection = model.section
      model.refresh()
      consumePendingDeepLink()
      model.refreshForCurrentSection()
      model.beginWhisperResidencyObservation()
      model.beginAudioDeviceObservation()
    }
    .onDisappear {
      model.endWhisperResidencyObservation()
      model.endAudioDeviceObservation()
    }
    .task {
      // The health footer must include the controller's real recording
      // admission verdict even when Audio is not the selected section.
      await model.refreshAdmission()
    }
    .background(HostingWindowReader(onWindow: adoptHostWindow))
    .onReceive(
      NotificationCenter.default.publisher(for: NSWindow.didBecomeKeyNotification)
    ) { notification in
      guard let window = notification.object as? NSWindow,
        window === hostWindow, window.isVisible
      else { return }
      model.refreshForCurrentSection()
    }
    .onReceive(
      NotificationCenter.default.publisher(
        for: SettingsDeepLink.pendingSectionDidChange, object: SettingsDeepLink.shared)
    ) { _ in
      // Only a Settings surface the user can actually see may take the one-shot
      // target. A hosted-but-hidden instance (closed scene, XCTest host) must
      // leave it for whichever surface opens next.
      guard hostWindow?.isVisible == true else { return }
      consumePendingDeepLink()
    }
  }

  /// The toolbar toggle, a divider drag to collapse and ⌃⌘S all write the
  /// one persisted flag, so every path keeps the same choice.
  private var sidebarVisibility: Binding<NavigationSplitViewVisibility> {
    CSSidebarVisibility.binding(expanded: $sidebarExpanded)
  }

  /// The wordmark toolbar is the visible title. The window keeps its name for
  /// Mission Control, App Exposé and the Window menu, and never miniaturises:
  /// its minimise control and ⌘M hide it (`DockPresence`), so no empty Exposé
  /// tile is left behind; Open Settings and ⌘, bring it back.
  ///
  /// The window titlebar and backing are configured to allow native sidebar
  /// translucency (`.sidebar` material with `.behindWindow` blending) while
  /// keeping the integrated titlebar/toolbar intact and content areas opaque.
  private func adoptHostWindow(_ window: NSWindow?) {
    hostWindow = window
    guard let window else { return }
    // This owner keeps the Window-menu title while the toolbar paints the
    // wordmark. A separate navigationTitle would restore a second visible title.
    window.title = String(localized: "Settings")
    window.titleVisibility = .hidden
    window.titlebarAppearsTransparent = true
    window.styleMask.insert(.fullSizeContentView)
    window.isOpaque = false
    window.backgroundColor = .clear
    // The toolbar style belongs to the scene (`.windowToolbarStyle(.unified)`
    // in App.swift): SwiftUI's toolbar bridging rewrites the NSWindow value
    // after this adoption, so setting it here is not what the window keeps.
    // The strip that once cut the sidebar beneath the titlebar came from a
    // visible toolbar background, not from the style; see `CSWindowHeaderChrome`.
    DockPresence.adopt(window)
  }

  /// The bar carries only the wordmark: the selected sidebar row and the
  /// pane's own headline already name the pane, so the bar does not repeat it.
  private var brandToolbar: some ToolbarContent {
    ToolbarItem(placement: .navigation) {
      CSWindowHeaderItem {
        HStack(spacing: 16) {
          Wordmark(size: 16)
            .fixedSize(horizontal: true, vertical: false)
          Text(verbatim: "v\(model.appVersion)")
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(Color.secondary)
        }
      }
    }
  }

  /// The window's own rows inside the shared sidebar column: grouped
  /// sections, SF Symbol rows and system selection. One flat row per
  /// section — a pane's parts are tabs inside the pane, not child rows.
  /// The list style, the search field that matches panel names AND what each
  /// panel does, the material and the column bounds belong to
  /// `CSSidebarColumn`.
  private var sidebar: some View {
    List(selection: $sidebarSelection) {
      ForEach(SettingsSectionGroup.allCases) { group in
        let items = matchedSections.filter { $0.group == group }
        if !items.isEmpty {
          Section(group.title) {
            ForEach(items) { item in
              Label(item.title, systemImage: item.symbol)
                // Rows carry the shared scroller policy into the list's own
                // scroll view (Founder, 2026-10-18).
                .csAutohidingScrollers()
                .tag(item)
                .accessibilityIdentifier("settings-rail-\(item.rawValue)")
            }
          }
        }
      }
    }
    .onChange(of: sidebarSelection) { _, selection in
      // Clearing/filtering the native selection does not clear the detail.
      guard let selection, selection != model.section else { return }
      model.select(selection)
    }
    .onChange(of: model.section) { _, section in
      sidebarSelection = section
      landOnSearchHit(in: section)
    }
  }

  /// Sections the current query reveals, directly or through one of their tabs.
  private var matchedSections: [SettingsSection] {
    SettingsSection.revealed(by: search)
  }

  /// A section opened while searching lands on the tab the query named, so
  /// "mcp" opens Agent › MCP servers rather than Agent's first tab.
  private func landOnSearchHit(in section: SettingsSection) {
    guard let tab = SettingsTab.searchLanding(in: section, query: search),
      model.currentTab != tab
    else { return }
    model.select(tab)
  }

  private func consumePendingDeepLink() {
    guard let target = SettingsDeepLink.shared.consume() else { return }
    model.select(target)
    pendingScrollAnchor = target.anchor
  }

  @ViewBuilder
  private var detail: some View {
    ScrollViewReader { proxy in
      Group {
        switch model.section.destination {
        case .dictation:
          EnginePanel(model: model)
        case .agent:
          AgentPanel(model: model)
        default:
          ScrollView {
            untabbedDetail
              .frame(maxWidth: .infinity, alignment: .leading)
              // `.scrollIndicators(.hidden)` stood here and could not keep its
              // promise: with a mouse attached the system still handed this
              // pane the legacy bar (Founder, 2026-10-18). `CSScrollerPin` is
              // the one owner of the policy.
              .csAutohidingScrollers()
          }
          .scrollContentBackground(.hidden)
        }
      }
      .onChange(of: pendingScrollAnchor) { _, anchor in
        guard let anchor else { return }
        DispatchQueue.main.async {
          proxy.scrollTo(anchor, anchor: .top)
          pendingScrollAnchor = nil
        }
      }
    }
  }

  @ViewBuilder
  private var untabbedDetail: some View {
    switch model.section.destination {
    case .shortcuts:
      ShortcutsPanel(model: model)
    case .providers:
      ProvidersPanel(model: model)
    case .user:
      UserPanel(model: model)
    case .dictionary:
      VoiceLabPanel(model: model)
    case .audio:
      AudioPanel(model: model)
    case .creator:
      CreatorPanel(model: model)
    case .lab:
      LabPanel(model: model)
    case .dictation, .agent:
      EmptyView()
    }
  }
}

// MARK: - Rail

/// Runtime-truth footer pinned under the sidebar. It is the one part of the old
/// hand-drawn rail that carried information rather than chrome: a live health
/// line that deep-links to the section owning the problem.
struct SettingsHealthFooter: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    let health = model.settingsHealth
    // No message means nothing operational to say: the sidebar has no footer.
    if let message = health.message {
      Group {
        if let target = health.targetSection {
          Button {
            model.select(target)
          } label: {
            content(health, message: message)
          }
          .csFocusRing()
          .help("Open \(target.title) settings")
        } else {
          content(health, message: message)
        }
      }
      .accessibilityIdentifier("settings-health-footer")
    }
  }

  /// The coloured dot is the whole status signal. The line beside it is
  /// ordinary secondary text in the system face, so the column ends quietly
  /// instead of shouting a second time in colour (Monika, 2026-10-18).
  private func content(_ health: SettingsHealthState, message: String) -> some View {
    HStack(spacing: 8) {
      Circle().fill(health.level.color).frame(width: 6, height: 6)
      Text(message)
        .font(.caption)
        .foregroundStyle(Color.secondary)
        .lineLimit(2)
      Spacer(minLength: 0)
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 12)
    .contentShape(Rectangle())
    .overlay(alignment: .top) {
      Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
    }
  }
}

extension SettingsHealthLevel {
  fileprivate var color: Color {
    switch self {
    case .healthy: return CSColor.oliveLight
    case .degraded: return CSColor.amber
    case .offline: return CSColor.terracotta
    case .unknown: return Color.secondary
    }
  }
}

// MARK: - Shared Settings chrome (consumed by every panel)

/// The one headline of a page, above everything that page contains.
struct SettingsPageHeader: View {
  let title: String
  var blurb: String?

  init(_ title: String, blurb: String? = nil) {
    self.title = title
    self.blurb = blurb
  }

  var body: some View {
    VStack(alignment: .leading, spacing: SettingsScale.titleGap) {
      Text(title)
        .font(SettingsScale.pageTitle)
        .foregroundStyle(.primary)
        .accessibilityAddTraits(.isHeader)
      if let blurb, !blurb.isEmpty {
        Text(blurb)
          .font(SettingsScale.pageBlurb)
          .foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }
}

/// The one title of a peer section, above the cards it introduces. It reads at
/// primary strength and one rung above a setting's own name, because a header
/// that is quieter than the rows beneath it stops being a header. Every
/// section of a page is a peer, so there is no rung above this one.
struct SettingsSectionLabel: View {
  let text: String
  init(_ text: String) {
    self.text = text
  }
  var body: some View {
    Text(text)
      .font(SettingsScale.sectionTitle)
      .foregroundStyle(.primary)
      .accessibilityAddTraits(.isHeader)
  }
}

/// Section label with one trailing action on the same line, so a helper such
/// as refresh or a System Settings link does not take a row of its own. This
/// is the one header owner of the central pane; a panel that needs a title
/// without an action uses the `EmptyView` init below or `SettingsSectionLabel`.
struct SettingsSectionHeaderRow<Action: View>: View {
  let text: String
  @ViewBuilder var action: () -> Action
  init(_ text: String, @ViewBuilder action: @escaping () -> Action) {
    self.text = text
    self.action = action
  }
  var body: some View {
    HStack(alignment: .firstTextBaseline, spacing: CSSpace.md) {
      SettingsSectionLabel(text)
        .frame(maxWidth: .infinity, alignment: .leading)
      action()
        .controlSize(.small)
    }
  }
}

extension SettingsSectionHeaderRow where Action == EmptyView {
  init(_ text: String) {
    self.init(text) { EmptyView() }
  }
}

struct SettingsMenuLabel: View {
  let text: String
  var mono: Bool = false

  var body: some View {
    HStack(spacing: CSSpace.xs) {
      Text(text)
        .font(mono ? SettingsScale.monoValue : SettingsScale.settingName)
        .foregroundStyle(.primary)
        .lineLimit(1)
      CSIconView(icon: .chevronUpDown, size: 9, weight: .semibold, color: Color.secondary)
    }
    .accessibilityElement(children: .combine)
  }
}

extension View {
  /// The one switch of Settings: a small macOS switch, tinted, its label
  /// carried by the row that holds it. The regular size reads as an iPad
  /// control on the desktop (Founder, 2026-10-20), so no switch sets its own.
  func settingsSwitch() -> some View {
    self
      .toggleStyle(.switch)
      .controlSize(.small)
      .labelsHidden()
      .tint(CSColor.chromeAccent)
  }

  /// The one card of the central pane: a grouped inset that follows the
  /// system appearance. Setting names live inside it; the section title that
  /// introduces it sits above it.
  func settingsGroupedInset(padding: CGFloat = SettingsScale.cardPadding) -> some View {
    self
      .padding(padding)
      .background(
        RoundedRectangle(cornerRadius: SettingsScale.cardRadius, style: .continuous)
          .fill(Color.primary.opacity(0.05))
      )
      .overlay(
        RoundedRectangle(cornerRadius: SettingsScale.cardRadius, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
      )
  }
}

/// Read-only key/value row for runtime-truth blocks (Dictation and Providers).
struct RuntimeRow: View {
  enum Trailing {
    case none
    case dot(Color)
    case text(String, Color)
  }

  let key: String
  let value: String
  var tint: Bool = false
  var mono: Bool = false
  var trailing: Trailing = .none
  /// Key column; a table whose Polish keys run long widens it once for all rows.
  var keyWidth: CGFloat = SettingsScale.rowKeyWidth

  var body: some View {
    HStack(spacing: CSSpace.md) {
      Text(key)
        .font(SettingsScale.metaKey)
        .foregroundStyle(Color.secondary)
        .frame(width: keyWidth, alignment: .leading)
      Text(value)
        .font(mono ? SettingsScale.monoValue : SettingsScale.settingValue)
        .foregroundStyle(.primary)
        .lineLimit(1)
        .truncationMode(.middle)
        .frame(maxWidth: .infinity, alignment: .leading)
      trailingView
    }
    .padding(.horizontal, SettingsScale.rowInsetH)
    .padding(.vertical, SettingsScale.rowInsetV)
    .background(tint ? Color.primary.opacity(0.04) : Color.clear)
  }

  @ViewBuilder
  private var trailingView: some View {
    switch trailing {
    case .none:
      EmptyView()
    case .dot(let color):
      Circle().fill(color).frame(width: 7, height: 7)
    case .text(let label, let color):
      Text(label)
        .font(SettingsScale.statusBadge)
        .foregroundStyle(color)
    }
  }
}

#if DEBUG
  #Preview("Settings — Creator") {
    SettingsView(model: SettingsViewModel.preview(.creator))
      .frame(width: 960, height: 620)
  }

  #Preview("Settings — Dictation") {
    SettingsView(model: SettingsViewModel.preview(.engine))
      .frame(width: 960, height: 620)
  }

  #Preview("Settings — Providers") {
    SettingsView(model: SettingsViewModel.preview(.keys))
      .frame(width: 960, height: 620)
  }

  #Preview("Settings — Agent") {
    SettingsView(model: SettingsViewModel.preview(.agent))
      .frame(width: 960, height: 620)
  }
#endif

/// Hands the hosting NSWindow to SwiftUI so event handlers can ask whether this
/// surface is actually on screen. The callback fires on every window move,
/// including detach (nil), so a stale handle cannot pass the visibility gate.
private struct HostingWindowReader: NSViewRepresentable {
  let onWindow: (NSWindow?) -> Void

  func makeNSView(context: Context) -> WindowReportingView {
    let view = WindowReportingView()
    view.onWindow = onWindow
    return view
  }

  func updateNSView(_ view: WindowReportingView, context: Context) {
    view.onWindow = onWindow
  }

  final class WindowReportingView: NSView {
    var onWindow: ((NSWindow?) -> Void)?

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      let window = self.window
      DispatchQueue.main.async { [weak self] in
        guard let self else { return }
        self.onWindow?(window ?? self.window)
      }
    }
  }
}
