import SwiftUI

/// Non-interactive hint. Every caller uses the header's anchor and the one
/// shared panel, so a scroll view cannot clip the label and a system tooltip
/// never appears beside it. `floating` places the hint beside a narrow rail;
/// otherwise it sits below the control, as on the header. A nil accent draws
/// no ring and does not change the control's own hover wash.
struct OverlayMiniTooltip: ViewModifier {
  let title: String
  let palette: OverlayAppearancePalette
  var enabled = true
  var floating = false
  var withinWindow = false

  func body(content: Content) -> some View {
    content.modifier(
      OverlayHeaderControlChrome(
        title: title, accent: nil, palette: palette, hintLeading: floating,
        hintWithinWindow: withinWindow,
        showsHint: enabled && !title.isEmpty))
  }
}

extension View {
  /// Immediate click-through hint on `OverlayHintPanel`. The hint does not
  /// take keyboard or mouse focus. `leading` is the rail placement.
  func overlayFastHint(
    _ title: String,
    palette: OverlayAppearancePalette,
    leading: Bool = false,
    enabled: Bool = true
  ) -> some View {
    modifier(
      OverlayMiniTooltip(
        title: title, palette: palette, enabled: enabled, floating: leading))
  }
}

/// The action pill's hint: one caption, or a warning sentence that wraps at
/// the layout's width cap instead of running past the panel edge. The header's
/// floating hint paints the same chip over the system tooltip material
/// (`filled: false`).
struct OverlayTooltipChip: View {
  let title: String
  let palette: OverlayAppearancePalette
  var filled = true

  var body: some View {
    Text(title)
      .font(CSFont.ui(10))
      .foregroundStyle(palette.primaryText.color)
      .lineLimit(4)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.horizontal, 5)
      .padding(.vertical, 2)
      .background {
        if filled {
          RoundedRectangle(cornerRadius: CSRadius.chip).fill(.regularMaterial)
        }
      }
      .accessibilityHidden(true)
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

/// Resting ink of an enabled control. Header and coverage chrome stay solid.
/// The transcript rail rests slightly dim and strengthens on hover: dark ink
/// darkens on paper and light ink brightens on a dark canvas. That dim is
/// presentation only: the control stays enabled, keeps its hit target and
/// does not change size.
enum OverlayHoverEmphasis: Equatable {
  case solid
  case transcriptRail

  /// Disabled ink stays at the historical 0.35. Rail rest is only a little
  /// below full ink, so it cannot be read as the disabled state.
  func opacity(enabled: Bool, hovered: Bool) -> Double {
    guard enabled else { return 0.35 }
    switch self {
    case .solid:
      return 1
    case .transcriptRail:
      return hovered ? 1 : 0.72
    }
  }
}

/// Hover describes; only button activation opens details or executes an action.
struct OverlayHoverControl<LabelContent: View, Detail: View>: View {
  @Environment(\.isEnabled) private var environmentEnabled
  let id: String
  let title: String
  let palette: OverlayAppearancePalette
  @Binding var presented: String?
  var action: (() -> Void)? = nil
  var showsTooltip = true
  /// A caller that passes false greys the control and blocks its action and
  /// detail. The tooltip still names it. The transcript rail does not use
  /// this for a missing action; it omits the control. Dim rail ink is
  /// `emphasis`, and it stays enabled.
  var isEnabled = true
  var emphasis: OverlayHoverEmphasis = .solid
  /// Transcript actions hint over their document, inside the owning window.
  /// Header and footer controls keep their existing compact hint placement.
  var keepsHintInsideWindow = false
  /// The transcript column sits on the trailing edge, so its menus open over
  /// the document. Other chrome keeps the menu below the control.
  var popoverArrow: Edge = .top
  @ViewBuilder var label: LabelContent
  @ViewBuilder var detail: (@escaping () -> Void) -> Detail
  @State private var hovered = false

  var body: some View {
    let enabled = isEnabled && environmentEnabled
    let selected = presented == id
    Button {
      if let action {
        presented = nil
        action()
      } else {
        presented = presented == id ? nil : id
      }
    } label: {
      label
        .foregroundStyle(
          enabled && selected && emphasis == .transcriptRail
            ? CSColor.chromeAccent : palette.primaryText.color
        )
        .opacity(emphasis.opacity(enabled: enabled, hovered: hovered || selected))
        // A menu keeps its accent ink after the pointer leaves for its
        // popover. Emphasis never adds a halo or changes the hit rectangle.
        .padding(2)
        .contentShape(Rectangle())
    }
    .csFocusRing()
    .disabled(!isEnabled)
    .focusable(false)
    .accessibilityLabel(title)
    .accessibilityIdentifier(id)
    .accessibilityAddTraits(selected ? .isSelected : [])
    .background {
      OverlayMenuCursorRegion { hovered = $0 }
    }
    .modifier(
      OverlayMiniTooltip(
        title: title, palette: palette, enabled: showsTooltip && presented == nil,
        floating: popoverArrow == .trailing, withinWindow: keepsHintInsideWindow)
    )
    .popover(
      isPresented: Binding(
        get: { presented == id },
        set: { if !$0 && presented == id { presented = nil } }
      ),
      attachmentAnchor: .rect(.bounds), arrowEdge: popoverArrow
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
