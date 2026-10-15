import AppKit
import SwiftUI

/// Opens a link from rendered markdown. A refusal (missing file, no handler)
/// is audible instead of a click that silently does nothing.
struct MarkdownLinkOpener: Sendable {
  var open: @MainActor @Sendable (URL) -> Bool
  var refused: @MainActor @Sendable () -> Void

  static let workspace = MarkdownLinkOpener(
    open: { NSWorkspace.shared.open($0) },
    refused: { NSSound.beep() })

  @MainActor func callAsFunction(_ url: URL) {
    if ConversationLocalFileLinks.open(url, open: open) != .opened {
      refused()
    }
  }
}

private struct MarkdownLinkOpenerKey: EnvironmentKey {
  static let defaultValue = MarkdownLinkOpener.workspace
}

extension EnvironmentValues {
  var markdownLinkOpener: MarkdownLinkOpener {
    get { self[MarkdownLinkOpenerKey.self] }
    set { self[MarkdownLinkOpenerKey.self] = newValue }
  }
}

/// Marks a link run inside rendered markdown so the text layout reports where
/// each link sits. `ordinal` keeps two links to the same URL apart.
struct MarkdownLinkRun: TextAttribute {
  let url: URL
  let ordinal: Int
}

/// Inline markdown whose links answer the pointer the way native links do:
/// the hovered link gets a wash of the system accent, and inside the overlay
/// panel the pointer over a link is the arrow, not the text I-beam (the panel
/// owns motion over `OverlayMenuCursorRegion`s). Clicks stay with the text's
/// own link handling and `OpenURLAction`; selection stays on, because the
/// regions never hit-test.
struct MarkdownLinkText: View {
  let attributed: AttributedString
  var lineSpacing: CGFloat

  /// Line fragments under the pointer. A link wrapped over two lines has two
  /// fragments and lights up whole while either is hovered.
  @State private var hoveredFragments: Set<String> = []

  var body: some View {
    Self.text(attributed)
      .lineSpacing(lineSpacing)
      .backgroundPreferenceValue(Text.LayoutKey.self) { layouts in
        GeometryReader { proxy in
          let fragments = Self.fragments(layouts, in: proxy)
          let lit = Set(fragments.filter { hoveredFragments.contains($0.id) }.map(\.ordinal))
          ForEach(fragments.filter { lit.contains($0.ordinal) }) { fragment in
            RoundedRectangle(cornerRadius: 3, style: .continuous)
              .fill(CSColor.chromeAccent.opacity(0.22))
              .frame(width: fragment.rect.width + 4, height: fragment.rect.height)
              .position(x: fragment.rect.midX, y: fragment.rect.midY)
          }
        }
      }
      .overlayPreferenceValue(Text.LayoutKey.self) { layouts in
        GeometryReader { proxy in
          ForEach(Self.fragments(layouts, in: proxy)) { fragment in
            OverlayMenuCursorRegion { inside in
              if inside {
                hoveredFragments.insert(fragment.id)
              } else {
                hoveredFragments.remove(fragment.id)
              }
            }
            .frame(width: fragment.rect.width, height: fragment.rect.height)
            .position(x: fragment.rect.midX, y: fragment.rect.midY)
          }
        }
      }
  }

  struct Fragment: Identifiable, Equatable {
    let id: String
    let ordinal: Int
    let rect: CGRect
  }

  /// Link runs of the laid-out text, in this view's coordinate space.
  static func fragments(_ layouts: Text.LayoutKey.Value, in proxy: GeometryProxy) -> [Fragment] {
    var fragments: [Fragment] = []
    for (layoutIndex, anchored) in layouts.enumerated() {
      let origin = proxy[anchored.origin]
      for (lineIndex, line) in anchored.layout.enumerated() {
        for (runIndex, run) in line.enumerated() {
          guard let link = run[MarkdownLinkRun.self] else { continue }
          fragments.append(
            Fragment(
              id: "\(layoutIndex).\(lineIndex).\(runIndex)", ordinal: link.ordinal,
              rect: run.typographicBounds.rect.offsetBy(dx: origin.x, dy: origin.y)))
        }
      }
    }
    return fragments
  }

  /// One `Text` from the attributed runs, link runs tagged for the layout.
  /// Joined through `Text` interpolation (`Text + Text` is deprecated), built
  /// at runtime: the format is only `%@` placeholders, which no catalog entry
  /// translates, and no literal reaches string extraction.
  static func text(_ attributed: AttributedString) -> Text {
    let pieces = attributed.runs[\.link]
    var interpolation = LocalizedStringKey.StringInterpolation(
      literalCapacity: 0, interpolationCount: pieces.count)
    var ordinal = 0
    for (link, range) in pieces {
      let piece = Text(AttributedString(attributed[range]))
      if let link {
        interpolation.appendInterpolation(
          piece.customAttribute(MarkdownLinkRun(url: link, ordinal: ordinal)))
        ordinal += 1
      } else {
        interpolation.appendInterpolation(piece)
      }
    }
    return Text(LocalizedStringKey(stringInterpolation: interpolation))
  }

  static func hasLinks(_ attributed: AttributedString) -> Bool {
    attributed.runs.contains { $0.link != nil }
  }
}
