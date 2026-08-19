import AppKit
import Combine
import SwiftUI

// Settings window: a NATIVE `NavigationSplitView` whose sidebar is a real
// `List(selection:)` in `.sidebar` style — the same chrome the Agent window
// already uses, and the reason that window survives an OS bump without visual
// drift while this one used to.
//
// The previous shell was a plain `HStack` with a 212pt hand-drawn rail. It was
// written that way to dodge one concrete problem: a NavigationSplitView reserves
// a toolbar strip above the sidebar, which pushed the rail ~70px down. Dropping
// the split view took the whole native surface with it — no collapse, no search,
// no section headers, no system material, no keyboard navigation, and a rail
// that had to re-implement selection state by hand.
//
// The strip is not dead space, it is the toolbar: the wordmark and version live
// in it, and it hosts the system sidebar toggle. That turns the reason for the
// fork into the feature the operator asked for.
struct SettingsView: View {
  @StateObject private var model: SettingsViewModel
  @State private var columnVisibility: NavigationSplitViewVisibility = .all
  @State private var search: String = ""

  init(model: SettingsViewModel? = nil) {
    _model = StateObject(wrappedValue: model ?? SettingsViewModel())
  }

  var body: some View {
    NavigationSplitView(columnVisibility: $columnVisibility) {
      sidebar
        .navigationSplitViewColumnWidth(min: 196, ideal: 216, max: 300)
        .safeAreaInset(edge: .bottom, spacing: 0) { SettingsHealthFooter(model: model) }
    } detail: {
      detail
    }
    .navigationTitle("")
    .toolbar {
      ToolbarItem(placement: .navigation) {
        HStack(spacing: 9) {
          Wordmark(size: 14)
            .fixedSize(horizontal: true, vertical: false)
          Text("v\(model.appVersion)")
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(CSColor.textFaintAlt)
        }
      }
    }
    .csFocusPolicy()
    .frame(minWidth: 880, maxWidth: .infinity, minHeight: 620, maxHeight: .infinity)
    .background(SettingsWindowCapabilities())
    // The panels still paint hand-picked dark tokens, so the window stays
    // pinned to dark until the palette itself is theme-aware. Removing this
    // line before that work lands would render dark text on a light system
    // background — the sidebar is native either way.
    .preferredColorScheme(.dark)
    .onAppear {
      model.refresh()
      consumePendingDeepLink()
    }
    .onReceive(NotificationCenter.default.publisher(for: SettingsDeepLink.pendingSectionDidChange))
    { _ in
      consumePendingDeepLink()
    }
  }

  /// Native sidebar: grouped sections, SF Symbol rows, system selection, and a
  /// search field that matches panel names AND what each panel does.
  private var sidebar: some View {
    List(selection: routeSelection) {
      ForEach(SettingsSectionGroup.allCases) { group in
        let items = matchedSections.filter { $0.group == group }
        if !items.isEmpty {
          Section(group.title) {
            ForEach(items) { item in
              sidebarRow(item)
            }
          }
        }
      }
    }
    .listStyle(.sidebar)
    .searchable(
      text: $search,
      placement: .sidebar,
      prompt: "Search settings"
    )
  }

  /// A paginated section renders as an expandable parent whose children are
  /// its pages; everything else stays a plain row. While a search is active
  /// the tree is pre-expanded — a hit the user cannot see is not a hit.
  @ViewBuilder
  private func sidebarRow(_ item: SettingsSection) -> some View {
    let pages = visiblePages(in: item)
    if pages.isEmpty {
      Label(item.title, systemImage: item.symbol)
        .tag(SettingsRoute.section(item))
        .accessibilityIdentifier("settings-rail-\(item.rawValue)")
    } else {
      DisclosureGroup(isExpanded: expansion(for: item)) {
        ForEach(pages) { page in
          Label(page.title, systemImage: page.symbol)
            .tag(SettingsRoute.page(page))
            .accessibilityIdentifier("settings-rail-page-\(page.rawValue)")
        }
      } label: {
        Label(item.title, systemImage: item.symbol)
          .tag(SettingsRoute.section(item))
          .accessibilityIdentifier("settings-rail-\(item.rawValue)")
      }
    }
  }

  /// Sections the current query reveals: a title/keyword hit on the section
  /// itself, or on any of its pages (so "mcp" surfaces Agent).
  private var matchedSections: [SettingsSection] {
    let direct = Set(SettingsSection.matching(query: search))
    let viaPages = Set(SettingsPage.matching(query: search).map(\.section))
    let hits =
      search.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
      ? direct
      : direct.union(viaPages)
    return SettingsSection.allCases.filter { hits.contains($0) }
  }

  /// Pages to show under a section: all of them normally, only the matches
  /// while searching — unless the section itself matched, which means the user
  /// asked for the section and deserves its full contents.
  private func visiblePages(in section: SettingsSection) -> [SettingsPage] {
    let all = SettingsPage.pages(in: section)
    guard !search.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
      !SettingsSection.matching(query: search).contains(section)
    else { return all }
    let matched = Set(SettingsPage.matching(query: search))
    return all.filter { matched.contains($0) }
  }

  private func expansion(for section: SettingsSection) -> Binding<Bool> {
    Binding(
      get: {
        !search.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
          || model.section == section
      },
      set: { expanded in
        // Expanding a collapsed parent is also a navigation intent.
        if expanded, model.section != section { model.select(section) }
      }
    )
  }

  /// `List` selection is optional by contract; a nil write (⌘-click clearing a
  /// row) must not blank the detail pane, so it is dropped instead of applied.
  private var routeSelection: Binding<SettingsRoute?> {
    Binding(
      get: { model.route },
      set: { if let value = $0 { model.select(value) } }
    )
  }

  private func consumePendingDeepLink() {
    guard let target = SettingsDeepLink.consume() else { return }
    model.select(target)
  }

  @ViewBuilder
  private var detail: some View {
    ScrollView {
      Group {
        switch model.section.destination {
        case .dictation:
          EnginePanel(model: model)
        case .shortcuts:
          ShortcutsPanel(model: model)
        case .providers:
          KeysPanel(model: model)
        case .agent:
          AgentPanel(model: model)
        case .prompts:
          PromptPanel(model: model)
        case .user:
          UserPanel(model: model)
        case .dictionary:
          VoiceLabPanel(model: model)
        case .audio:
          AudioPanel(model: model)
        case .license:
          LicensePanel(model: model)
        case .creator:
          CreatorPanel(model: model)
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
    }
    .scrollContentBackground(.hidden)
    .background(Self.windowGradient)
  }

  /// linear-gradient(135deg,#15110e,#0b0c10 55%,#0d1012) from the mock.
  static let windowGradient = LinearGradient(
    stops: [
      .init(color: Color(hex: 0x15110E), location: 0.0),
      .init(color: Color(hex: 0x0B0C10), location: 0.55),
      .init(color: Color(hex: 0x0D1012), location: 1.0),
    ],
    startPoint: .topLeading,
    endPoint: .bottomTrailing
  )
}

/// SwiftUI's Settings scene can silently keep the content-sized AppKit style
/// mask even when `.windowResizability` is present (notably after restoring an
/// older saved frame). Enforce normal macOS window capabilities on the actual
/// host window so Settings can resize, zoom and enter native full screen.
private struct SettingsWindowCapabilities: NSViewRepresentable {
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
    window.styleMask.formUnion([.resizable, .miniaturizable, .fullSizeContentView])
    window.collectionBehavior.insert(.fullScreenPrimary)
    let minimum = NSSize(width: 880, height: 620)
    window.minSize = minimum
    window.level = .normal
    window.standardWindowButton(.zoomButton)?.isEnabled = true
    window.standardWindowButton(.miniaturizeButton)?.isEnabled = true

    var frame = window.frame
    frame.size.width = max(frame.width, minimum.width)
    frame.size.height = max(frame.height, minimum.height)
    if let screen = window.screen ?? NSScreen.main {
      frame = window.constrainFrameRect(frame, to: screen)
    }
    if frame != window.frame {
      window.setFrame(frame, display: false)
    }
  }
}

// MARK: - Rail

/// Runtime-truth footer pinned under the sidebar. It is the one part of the old
/// hand-drawn rail that carried information rather than chrome: a live health
/// line that deep-links to the section owning the problem.
private struct SettingsHealthFooter: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    let health = model.settingsHealth
    Group {
      if let target = health.targetSection {
        Button {
          model.select(target)
        } label: {
          content(health)
        }
        .csFocusRing(cornerRadius: 8)
        .help("Open \(target.title) settings")
      } else {
        content(health)
      }
    }
    .accessibilityIdentifier("settings-health-footer")
  }

  private func content(_ health: SettingsHealthState) -> some View {
    HStack(spacing: 8) {
      Circle().fill(health.level.color).frame(width: 6, height: 6)
      Text(health.message)
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(health.level.color)
        .lineLimit(2)
      Spacer(minLength: 0)
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 12)
    .contentShape(Rectangle())
    .overlay(alignment: .top) {
      Rectangle().fill(CSColor.hairline(0.06)).frame(height: 1)
    }
  }
}

extension SettingsHealthLevel {
  fileprivate var color: Color {
    switch self {
    case .healthy: return CSColor.oliveLight
    case .degraded: return CSColor.amber
    case .offline: return CSColor.terracottaLight
    case .unknown: return CSColor.textFaint
    }
  }
}

// MARK: - Shared Settings chrome (consumed by every panel)

struct SettingsSectionLabel: View {
  let text: String
  init(_ text: String) { self.text = text }
  var body: some View {
    Text(text.uppercased())
      .font(CSFont.mono(12, .semibold))
      .tracking(0.5)
      .foregroundStyle(CSColor.textMuted)
  }
}

struct SettingsMenuLabel: View {
  let text: String
  var mono: Bool = false
  var chrome: Bool = false

  var body: some View {
    if chrome {
      content
        .padding(.horizontal, 11)
        .padding(.vertical, 7)
        .background(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .fill(CSColor.surfaceRaised(0.03))
        )
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
        )
        .contentShape(Rectangle())
    } else {
      content
    }
  }

  private var content: some View {
    HStack(spacing: 6) {
      Text(text)
        .font(mono ? CSFont.mono(12.5, .semibold) : CSFont.ui(12.5, .semibold))
        .foregroundStyle(CSColor.textHigh)
        .lineLimit(1)
      CSIconView(icon: .chevronUpDown, size: 9, weight: .semibold, color: CSColor.textFaint)
    }
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

  var body: some View {
    HStack(spacing: 12) {
      Text(key)
        .font(CSFont.mono(12, .medium))
        .foregroundStyle(CSColor.textMutedAlt)
        .frame(width: 160, alignment: .leading)
      Text(value)
        .font(mono ? CSFont.mono(12.5, .semibold) : CSFont.ui(12.5, .semibold))
        .foregroundStyle(mono ? CSColor.textBodyAlt : CSColor.textHigh)
        .lineLimit(1)
        .truncationMode(.middle)
        .frame(maxWidth: .infinity, alignment: .leading)
      trailingView
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 13)
    .background(tint ? CSColor.surfaceRaised(0.02) : Color.clear)
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
        .font(CSFont.mono(10, .semibold))
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

  #Preview("Settings — Prompts") {
    SettingsView(model: SettingsViewModel.preview(.prompts))
      .frame(width: 960, height: 620)
  }
#endif
