import SwiftUI

/// Presentation only. A short exit grace bridges the gap to the panel;
/// entering a control never waits and never invokes its action.
struct OverlayHoverRegion {
  var anchorInside = false
  var panelInside = false
  var isInside: Bool { anchorInside || panelInside }
}

struct OverlayHoverControl<LabelContent: View, Detail: View>: View {
  let id: String
  let title: String
  let palette: OverlayAppearancePalette
  @Binding var presented: String?
  var action: (() -> Void)? = nil
  @ViewBuilder var label: LabelContent
  @ViewBuilder var detail: (@escaping () -> Void) -> Detail
  @State private var hover = OverlayHoverRegion()
  @FocusState private var focused: Bool

  var body: some View {
    Button {
      if let action {
        presented = nil
        action()
      } else {
        presented = id
      }
    } label: {
      label
        .foregroundStyle(palette.primaryText.color)
        .padding(2)
        .background {
          RoundedRectangle(cornerRadius: CSRadius.chip)
            .fill(palette.primaryText.color.opacity(hover.anchorInside ? 0.14 : 0))
        }
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .focused($focused)
    .accessibilityLabel(title)
    .accessibilityIdentifier(id)
    .onHover { inside in
      hover.anchorInside = inside
      if inside { presented = id }
    }
    .onChange(of: focused) { _, value in
      if value { presented = id }
    }
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
        .onHover { hover.panelInside = $0 }
        .onDisappear { hover.panelInside = false }
        .onExitCommand { presented = nil }
    }
    .task(id: hover.isInside) {
      guard !hover.isInside, !focused, presented == id else { return }
      do { try await Task.sleep(for: .milliseconds(180)) } catch { return }
      guard !hover.isInside, presented == id else { return }
      presented = nil
    }
    .onExitCommand { presented = nil }
  }
}
