import SwiftUI

/// A bounded label of the existing PCM-ordered projection, without document authority.
enum OverlayEvidencePresentation {
  static func chip(evidence: [CsUnanchoredEvidence]) -> (count: Int, line: String)? {
    guard !evidence.isEmpty else { return nil }
    return (evidence.count, evidence.suffix(12).map(\.text).joined(separator: " · "))
  }

  static func isExpanded(hovered: Bool, focused: Bool, actionsOpen: Bool) -> Bool {
    !actionsOpen && (hovered || focused)
  }
}

/// Read-only evidence occupies one bottom-bar capsule, never transcript height.
/// Only this view observes the live evidence projection; labels are not identity.
struct OverlayEvidenceChip: View {
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var hovered = false
  @FocusState private var focused: Bool
  let state: OverlayState
  let palette: OverlayAppearancePalette
  let actionsOpen: Bool
  let glassNamespace: Namespace.ID

  private var expanded: Bool {
    OverlayEvidencePresentation.isExpanded(
      hovered: hovered, focused: focused, actionsOpen: actionsOpen)
  }

  var body: some View {
    if let chip = OverlayEvidencePresentation.chip(evidence: state.liveEvidence) {
      surface(count: chip.count, line: chip.line)
        .contentShape(Capsule())
        .focusable()
        .focused($focused)
        .onHover { hovered = $0 }
        .help("Also heard · not committed: \(chip.line)")
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
      Text(expanded ? line : "\(count)")
        .lineLimit(1)
        .truncationMode(.head)
    }
    .font(.system(size: 11, weight: .medium))
    .padding(.horizontal, 10)
    .frame(height: OverlayResizeChrome.actionsHeight)
    .frame(maxWidth: expanded ? .infinity : nil, alignment: .trailing)
    .fixedSize(horizontal: !expanded, vertical: true)
  }
}
