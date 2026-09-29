import AppKit
import SwiftUI

/// Agent window appearance.
///
/// The shared palette's cream text and near-black slabs assume a forced-dark
/// canvas. This window tracks Aqua and Dark Aqua instead, so structural ink
/// comes from semantic system colors. Terracotta and the status colors stay
/// on `CSColor` — they carry brand and state, not chrome.
enum AgentChrome {
  static let followsSystemAppearance = true
  /// `nil` means the host must not apply `preferredColorScheme`.
  static let forcedColorScheme: ColorScheme? = nil
  static let paintsFixedDarkWindow = false

  static let primary = Color.primary
  static let secondary = Color.secondary
  static let tertiary = Color(nsColor: .tertiaryLabelColor)

  static let windowCanvas = Color(nsColor: .windowBackgroundColor)
  static let codeWell = Color(nsColor: .textBackgroundColor)
  static let controlFill = Color(nsColor: .controlBackgroundColor)

  /// Brand text that has to read on both canvases. Peach (`terracottaLight`)
  /// is for a terracotta fill or the dark code theme, not for labels on the
  /// window.
  static let brandLabel = CSColor.terracotta

  static func lift(_ opacity: Double) -> Color {
    Color.primary.opacity(opacity)
  }

  static func separator(_ opacity: Double) -> Color {
    Color.primary.opacity(opacity)
  }

  /// Applies `forcedColorScheme` only when one is set. The current policy
  /// leaves it nil, so the window keeps the system appearance.
  static func host<V: View>(_ view: V) -> some View {
    applyColorScheme(forcedColorScheme, to: view)
  }

  @ViewBuilder
  private static func applyColorScheme<V: View>(_ scheme: ColorScheme?, to view: V) -> some View {
    if let scheme {
      view.preferredColorScheme(scheme)
    } else {
      view
    }
  }

  static func codeCSS(dark: Bool) -> String {
    dark ? CodeTheme.darkCSS : CodeTheme.lightCSS
  }

  static func composerTextColor() -> NSColor { .labelColor }
  static func composerPlaceholderColor() -> NSColor { .placeholderTextColor }
  static func composerCaretColor() -> NSColor { NSColor(CSColor.terracotta) }
  static func transcriptTextColor() -> NSColor { .labelColor }

  enum CodePalette: String, Equatable, CaseIterable {
    case light
    case dark

    static func resolve(_ scheme: ColorScheme) -> CodePalette {
      scheme == .dark ? .dark : .light
    }

    var usesDarkCSS: Bool { self == .dark }
  }

  enum Sidebar {
    static let paintsCustomWash = false
    static let title = "codescribe"
    static let titleSize: CGFloat = 13
    static let emptyTitle = "New thread"
    static let emptyDetail = "Write in the composer, or dictate. The reply stays in this thread."
    /// Two-line rail rows stay list-dense. 11pt of vertical padding plus the
    /// title and meta was reading as a stack of cards.
    static let rowVerticalPadding: CGFloat = 7
  }
}

/// Sidebar identity. The shared wordmark paints cream (`textHigh`) and only
/// holds on a dark slab, so the rail draws the same name in semantic ink.
struct AgentSidebarTitle: View {
  var body: some View {
    HStack(spacing: 8) {
      Circle()
        .fill(CSColor.terracotta)
        .frame(width: 8, height: 8)
        .accessibilityHidden(true)
      Text(AgentChrome.Sidebar.title)
        .font(CSFont.ui(AgentChrome.Sidebar.titleSize, .semibold))
        .foregroundStyle(AgentChrome.primary)
    }
    .accessibilityElement(children: .combine)
    .accessibilityLabel(AgentChrome.Sidebar.title)
  }
}

/// Quiet first screen for a thread that has no turns yet.
struct AgentEmptyThread: View {
  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      Text(AgentChrome.Sidebar.emptyTitle)
        .font(CSFont.ui(15, .semibold))
        .foregroundStyle(AgentChrome.primary)
      Text(AgentChrome.Sidebar.emptyDetail)
        .font(CSFont.ui(13, .regular))
        .foregroundStyle(AgentChrome.secondary)
        .fixedSize(horizontal: false, vertical: true)
    }
    .frame(maxWidth: 420, alignment: .leading)
    .padding(.top, 28)
    .accessibilityElement(children: .combine)
  }
}
