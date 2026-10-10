import SwiftUI

/// A bounded label of the existing PCM-ordered projection, without document authority.
enum OverlayEvidencePresentation {
  static func chip(evidence: [CsUnanchoredEvidence]) -> (count: Int, line: String)? {
    guard !evidence.isEmpty else { return nil }
    return (evidence.count, evidence.suffix(12).map(\.text).joined(separator: " · "))
  }

  static func isExpanded(selected: Bool, actionsOpen: Bool) -> Bool {
    !actionsOpen && selected
  }
}

/// Read-only evidence occupies one bottom-bar capsule, never transcript height.
/// Only this view observes the live evidence projection; labels are not identity.
struct OverlayEvidenceChip: View {
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var selected = false
  let state: OverlayState
  let palette: OverlayAppearancePalette
  let actionsOpen: Bool
  let glassNamespace: Namespace.ID

  private var expanded: Bool {
    OverlayEvidencePresentation.isExpanded(
      selected: selected, actionsOpen: actionsOpen)
  }

  var body: some View {
    // CSDeveloperSurface is baked by install-app and forced off for release DMGs.
    // Confidence paint and word correction actions remain product features.
    if DeveloperSurface.isEnabled(),
      let chip = OverlayEvidencePresentation.chip(evidence: state.liveEvidence)
    {
      Button {
        selected.toggle()
      } label: {
        surface(count: chip.count, line: chip.line)
      }
      .buttonStyle(.plain)
      .contentShape(Capsule())
      .modifier(
        OverlayMiniTooltip(
          title: String(
            localized: "Also heard · not committed",
            comment: "Tooltip over the unanchored-evidence chip"),
          palette: palette)
      )
      .onExitCommand { selected = false }
      .onChange(of: actionsOpen) { _, open in if open { selected = false } }
      .accessibilityElement(children: .ignore)
      .accessibilityLabel("Also heard, not committed: \(chip.line), \(chip.count) total")
      .accessibilityIdentifier("overlay-unanchored-evidence")
      .animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: expanded)
      .transaction { transaction in
        if reduceMotion {
          transaction.animation = nil
          transaction.disablesAnimations = true
        }
      }
    }
  }

  @ViewBuilder
  private func surface(count: Int, line: String) -> some View {
    if reduceTransparency {
      label(count: count, line: line)
        .background(palette.desktopBackground.color, in: Capsule())
    } else if #available(macOS 26.0, *) {
      label(count: count, line: line)
        .glassEffect(.regular.interactive(), in: Capsule())
        .glassEffectID("overlay-evidence", in: glassNamespace)
        .glassEffectTransition(reduceMotion ? .identity : .matchedGeometry)
    } else {
      label(count: count, line: line)
        .background(.regularMaterial, in: Capsule())
        .overlay { Capsule().strokeBorder(palette.border.color, lineWidth: 1) }
    }
  }

  private func label(count: Int, line: String) -> some View {
    HStack(spacing: 4) {
      Image(systemName: "waveform")
        .font(.system(size: 11, weight: .medium))
      Text(expanded ? line : "\(count)")
        .font(CSFont.ui(11, .medium))
        .lineLimit(1)
        .truncationMode(.head)
    }
    .padding(.horizontal, 10)
    .frame(height: OverlayResizeChrome.actionsHeight)
    .frame(maxWidth: expanded ? .infinity : nil, alignment: .trailing)
    .fixedSize(horizontal: !expanded, vertical: true)
  }
}
