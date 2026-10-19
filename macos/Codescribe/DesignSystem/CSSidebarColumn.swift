import SwiftUI

// One sidebar for every window. Agent and Settings had the same native
// composition twice, with different widths, a different footer treatment and
// a toggle only one of them showed; the two columns read as two products
// (Monika, 2026-10-18: "musimy to ujednolicić"). Side by side she then chose
// the Settings column as the reference, so the Agent rail adopts it rather
// than the other way round.
//
// The column's chrome — material, width, search field, bottom area and the
// toggle that opens and closes it — lives here. A window supplies only what
// is its own: its rows and its footer.

/// Bounds of a sidebar column. A window names the kind of column it has
/// instead of repeating three numbers, so changing a window's proportions is
/// one word at its call site.
struct CSSidebarWidth: Equatable {
  let minimum: CGFloat
  let ideal: CGFloat
  let maximum: CGFloat

  /// A list of destinations: Settings' sections, and since 2026-10-18 the
  /// Agent window's threads. Content truncates inside these bounds; it never
  /// widens the column, and collapsing removes the column entirely.
  static let navigation = CSSidebarWidth(minimum: 196, ideal: 216, maximum: 300)

  /// The wider column the Agent rail used while it carried two-line rows.
  /// Kept as the other half of the choice: a window that needs it says so.
  static let content = CSSidebarWidth(minimum: 267, ideal: 267, maximum: 360)
}

extension View {
  /// The sidebar column bounds, for a call site that applies them itself.
  /// `CSSidebarColumn` already carries the width it was given.
  func csSidebarColumnWidth(_ width: CSSidebarWidth) -> some View {
    navigationSplitViewColumnWidth(
      min: width.minimum,
      ideal: width.ideal,
      max: width.maximum
    )
  }

  /// ⌃⌘S for the split view's own sidebar toggle.
  ///
  /// Both windows keep the system toggle beside the traffic lights: it
  /// reaches the AppKit-hosted Agent window as well as the Settings scene. A
  /// second, hand-placed toolbar button beside it was a duplicate on
  /// 2026-10-18 and is gone; this modifier only lends the shortcut.
  func csSidebarToggle(expanded: Binding<Bool>) -> some View {
    modifier(CSSidebarToggle(expanded: expanded))
  }
}

/// The sidebar column of a `NavigationSplitView`.
///
/// `content` is the window's own `List(selection:)` — navigation rows in
/// Settings, thread rows in Agent. Everything around it is identical by
/// construction: the system sidebar list style and its section headers,
/// selection and hover; the sidebar search field; the shared glass running
/// from the bottom edge up through the titlebar; and the column's bounds.
///
/// The two slots are where a window keeps what belongs to its own function
/// and nothing else: `header` under the search field, above the rows
/// (Agent's New conversation),
/// `footer` pinned under the list (Settings' health line). Either may be
/// left out, and the column then has no such area at all.
struct CSSidebarColumn<Content: View, Header: View, Footer: View>: View {
  private let width: CSSidebarWidth
  @Binding private var search: String
  private let prompt: Text
  private let onSearchSubmit: () -> Void
  private let content: Content
  private let header: Header
  private let footer: Footer

  init(
    width: CSSidebarWidth,
    search: Binding<String>,
    prompt: Text,
    onSearchSubmit: @escaping () -> Void = {},
    @ViewBuilder content: () -> Content,
    @ViewBuilder header: () -> Header,
    @ViewBuilder footer: () -> Footer
  ) {
    self.width = width
    self._search = search
    self.prompt = prompt
    self.onSearchSubmit = onSearchSubmit
    self.content = content()
    self.header = header()
    self.footer = footer()
  }

  var body: some View {
    content
      .listStyle(.sidebar)
      .scrollContentBackground(.hidden)
      .searchable(text: $search, placement: .sidebar, prompt: prompt)
      .onSubmit(of: .search) { onSearchSubmit() }
      .csSidebarSlot(.bottom) { footer }
      // The system search field stays first under the toolbar, so the header
      // lands between it and the rows.
      .csSidebarSlot(.top) { header }
      // On the List, not a stack around it: the list runs through the
      // titlebar, so the material does too.
      .background { CSSidebarMaterial().ignoresSafeArea() }
      .csSidebarColumnWidth(width)
  }
}

extension CSSidebarColumn where Header == EmptyView {
  /// A column with no action of its own above its rows.
  init(
    width: CSSidebarWidth,
    search: Binding<String>,
    prompt: Text,
    onSearchSubmit: @escaping () -> Void = {},
    @ViewBuilder content: () -> Content,
    @ViewBuilder footer: () -> Footer
  ) {
    self.init(
      width: width,
      search: search,
      prompt: prompt,
      onSearchSubmit: onSearchSubmit,
      content: content,
      header: { EmptyView() },
      footer: footer
    )
  }
}

extension CSSidebarColumn where Footer == EmptyView {
  /// A column that ends with its last row: no bottom bar, no pill.
  init(
    width: CSSidebarWidth,
    search: Binding<String>,
    prompt: Text,
    onSearchSubmit: @escaping () -> Void = {},
    @ViewBuilder content: () -> Content,
    @ViewBuilder header: () -> Header
  ) {
    self.init(
      width: width,
      search: search,
      prompt: prompt,
      onSearchSubmit: onSearchSubmit,
      content: content,
      header: header,
      footer: { EmptyView() }
    )
  }
}

extension View {
  /// A slot the window left empty adds no inset at all. A `safeAreaInset`
  /// around an `EmptyView` claims the whole remaining height of the column:
  /// on 2026-10-18 it pushed every Settings row under the health footer
  /// (683 pt of top inset) and gave the Agent list 593 pt of bottom inset.
  @ViewBuilder
  fileprivate func csSidebarSlot<Slot: View>(
    _ edge: VerticalEdge, @ViewBuilder _ slot: () -> Slot
  ) -> some View {
    if Slot.self == EmptyView.self {
      self
    } else {
      safeAreaInset(edge: edge, spacing: 0, content: slot)
    }
  }
}

/// Sidebar visibility for a window's `NavigationSplitView`. The toggle, a
/// divider drag and ⌃⌘S all write this one flag, so every path persists the
/// same choice and both windows behave alike.
enum CSSidebarVisibility {
  static func binding(expanded: Binding<Bool>) -> Binding<NavigationSplitViewVisibility> {
    Binding(
      get: { expanded.wrappedValue ? .all : .detailOnly },
      set: { expanded.wrappedValue = $0 != .detailOnly }
    )
  }
}

/// The key equivalent for the split view's own toggle. That item carries
/// none and the accessory app has no View menu to lend one, so the zero-size
/// button behind the content holds ⌃⌘S.
private struct CSSidebarToggle: ViewModifier {
  @Binding var expanded: Bool

  func body(content: Content) -> some View {
    content
      .background {
        Button(action: toggle) {
          EmptyView()
        }
        .buttonStyle(.plain)
        .keyboardShortcut("s", modifiers: [.command, .control])
        .frame(width: 0, height: 0)
        .opacity(0)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
      }
  }

  private func toggle() {
    withAnimation { expanded.toggle() }
  }
}
