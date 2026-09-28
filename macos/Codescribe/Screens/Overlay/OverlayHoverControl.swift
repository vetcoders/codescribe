import SwiftUI

/// Immediate, non-interactive hint. It never changes layout or opens a panel.
struct OverlayMiniTooltip: ViewModifier {
  let title: String
  let palette: OverlayAppearancePalette
  var enabled = true
  @State private var hovered = false

  func body(content: Content) -> some View {
    content
      .onHover { hovered = $0 }
      .overlay(alignment: .top) {
        if hovered && enabled {
          Text(title)
            .font(.system(size: 10, weight: .medium))
            .foregroundStyle(palette.primaryText.color)
            .lineLimit(1)
            .fixedSize()
            .padding(.horizontal, 7)
            .padding(.vertical, 4)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 6))
            .overlay {
              RoundedRectangle(cornerRadius: 6).strokeBorder(palette.border.color)
            }
            .alignmentGuide(.top) { $0[.bottom] + 6 }
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
      }
      .zIndex(hovered && enabled ? 1 : 0)
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
    .buttonStyle(.plain)
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
        .font(.system(size: 12, weight: .medium))
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
