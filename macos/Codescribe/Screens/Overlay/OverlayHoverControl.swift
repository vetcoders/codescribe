import SwiftUI

/// Immediate, non-interactive hint. It never changes layout or opens a panel.
struct OverlayMiniTooltip: ViewModifier {
  let title: String
  let palette: OverlayAppearancePalette
  var enabled = true
  @State private var hovered = false

  func body(content: Content) -> some View {
    OverlayTooltipLayout {
      content
        .onHover { hovered = $0 }
      if hovered && enabled {
        // A caption stays one line; a sentence-long warning wraps at the
        // layout's width cap instead of running past the panel edge.
        Text(title)
          .font(CSFont.ui(10, .medium))
          .foregroundStyle(palette.primaryText.color)
          .lineLimit(4)
          .fixedSize(horizontal: false, vertical: true)
          .padding(.horizontal, 7)
          .padding(.vertical, 4)
          .background(.regularMaterial, in: RoundedRectangle(cornerRadius: CSRadius.chip))
          .overlay {
            RoundedRectangle(cornerRadius: CSRadius.chip).strokeBorder(palette.border.color)
          }
          .allowsHitTesting(false)
          .accessibilityHidden(true)
      }
    }
    .zIndex(hovered && enabled ? 1 : 0)
  }
}

/// Keep the hint entirely above its anchor, independent of alignment guides
/// propagated by the surrounding glass. The gap also clears the rail padding.
struct OverlayTooltipLayout: Layout {
  /// Widest hint before it wraps; fits inside the 320 pt minimum panel.
  static let maxHintWidth: CGFloat = 260

  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    subviews[0].sizeThatFits(proposal)
  }

  func placeSubviews(
    in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()
  ) {
    subviews[0].place(at: bounds.origin, proposal: ProposedViewSize(bounds.size))
    if subviews.count > 1 {
      let hint = subviews[1]
      let wraps = hint.sizeThatFits(.unspecified).width > Self.maxHintWidth
      hint.place(
        at: CGPoint(x: bounds.midX, y: bounds.minY - 12),
        anchor: .bottom,
        proposal: wraps ? ProposedViewSize(width: Self.maxHintWidth, height: nil) : .unspecified)
    }
  }
}

/// Hover describes; only button activation opens details or executes an action.
struct OverlayHoverControl<LabelContent: View, Detail: View>: View {
  let id: String
  let title: String
  let palette: OverlayAppearancePalette
  @Binding var presented: String?
  var action: (() -> Void)? = nil
  @ViewBuilder var label: LabelContent
  @ViewBuilder var detail: (@escaping () -> Void) -> Detail
  @State private var hovered = false

  var body: some View {
    Button {
      if let action {
        presented = nil
        action()
      } else {
        presented = presented == id ? nil : id
      }
    } label: {
      label
        .foregroundStyle(palette.primaryText.color)
        .padding(2)
        .background {
          RoundedRectangle(cornerRadius: CSRadius.chip)
            .fill(palette.primaryText.color.opacity(hovered ? 0.14 : 0))
        }
        .contentShape(Rectangle())
    }
    .csFocusRing()
    .accessibilityLabel(title)
    .accessibilityIdentifier(id)
    .onHover { hovered = $0 }
    .modifier(OverlayMiniTooltip(title: title, palette: palette, enabled: presented == nil))
    .popover(
      isPresented: Binding(
        get: { presented == id },
        set: { if !$0 && presented == id { presented = nil } }
      ),
      attachmentAnchor: .rect(.bounds), arrowEdge: .top
    ) {
      detail { presented = nil }
        .font(CSFont.ui(12, .medium))
        .foregroundStyle(palette.primaryText.color)
        .padding(10)
        .frame(maxWidth: 280)
        .fixedSize(horizontal: false, vertical: true)
        .background(.regularMaterial)
        .contentShape(Rectangle())
        .onExitCommand { presented = nil }
    }
    .onExitCommand { presented = nil }
  }
}
