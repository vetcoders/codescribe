import SwiftUI

/// The compact overlay's channel map (bus intercom spec §5.1, §5.4): one chip
/// per mapped channel between the session timer and the actions, collapsing
/// into one counter when the strip is too narrow for the row.
///
/// A chip selects that channel's conversation through the roster's own
/// selection path, exactly as its rail pill does (decision D1). Neither a chip
/// nor the counter opens a microphone or retargets a capture.
struct OverlayChannelMapView: View {
  /// The map's distance from the session timer. The map carries it, so a map
  /// that yields its place costs the strip nothing.
  static let leadingGap: CGFloat = 8
  static let chipSpacing: CGFloat = 4

  let model: OverlayChannelMapModel
  let modifier: OverlayChannelChordModifier
  let palette: OverlayAppearancePalette
  let textScale: CGFloat
  let animates: Bool
  /// A chip's digit; the roster resolves the conversation and its owner.
  let onSelectChannel: (String) -> Void
  /// The collapsed counter's click, the map's single entry point. In W1 it
  /// opens the Agents list as the Agents button does; W2 replaces it, and the
  /// counter's hint, with the map popover.
  let onOpenMap: () -> Void

  var body: some View {
    let scale = OverlayChannelPill.headerScale(textScale)
    let widths = model.chips.map {
      OverlayChannelPill.chipWidth(name: $0.name, microphoneOpen: $0.microphoneOpen, scale: scale)
    }
    let row = OverlayChannelMapModel.rowWidth(chipWidths: widths, spacing: Self.chipSpacing)
    let counter = OverlayChannelPill.counterWidth(
      figure: model.counterFigure, microphoneOpen: model.microphoneOpen, scale: scale)
    return OverlayChannelMapSlot(
      rowWidth: row, counterWidth: counter, leadingGap: Self.leadingGap,
      height: OverlayChannelPill.headerHeight
    ) {
      // The slot proposes exactly the width it claimed, so this reads the
      // same decision the slot took; no state, no feedback loop.
      GeometryReader { proxy in
        switch OverlayChannelMapModel.presentation(
          availableWidth: proxy.size.width, rowWidth: row, counterWidth: counter)
        {
        case .chips: chips(scale: scale)
        case .counter: counterPill(scale: scale)
        case .hidden: Color.clear.accessibilityHidden(true)
        }
      }
    }
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-channel-map")
  }

  private func chips(scale: CGFloat) -> some View {
    HStack(spacing: Self.chipSpacing) {
      ForEach(model.chips) { chip in
        OverlayChannelPill(
          form: .chip,
          display: OverlayChannelPill.Display(
            figure: chip.digit, name: chip.name, disconnected: chip.disconnected,
            microphoneOpen: chip.microphoneOpen, broadcast: chip.isBroadcast,
            unread: chip.unread, enabled: chip.selectable, animates: animates),
          hint: chip.hint(modifier: modifier),
          accessibilityText: chip.accessibilityLabel(modifier: modifier),
          accessibilityValue: chip.disconnected ? String(localized: "Disconnected") : "",
          identifier: "overlay-channel-chip-\(chip.digit)",
          palette: palette, textScale: scale
        ) {
          onSelectChannel(chip.digit)
        }
      }
    }
  }

  private func counterPill(scale: CGFloat) -> some View {
    OverlayChannelPill(
      form: .counter,
      display: OverlayChannelPill.Display(
        figure: model.counterFigure, microphoneOpen: model.microphoneOpen,
        broadcast: model.openDigits == ["0"], unread: model.anyUnread, animates: animates),
      hint: model.counterHint,
      accessibilityText: String(
        localized: "Channel map",
        comment: "Compact overlay: the collapsed counter that stands for every mapped channel"),
      accessibilityValue: model.counterAccessibilityValue,
      identifier: "overlay-channel-map-toggle",
      palette: palette, textScale: scale, action: onOpenMap)
  }
}

/// Claims the width of what the map shows: the whole row, the counter, or
/// nothing. A plain flexible frame would keep the leftover beside a counter;
/// reporting the counter's width hands it back to the waveform. A map that
/// cannot fit even the counter claims no width, so it never pushes the
/// actions or the fold button out of the strip.
struct OverlayChannelMapSlot: Layout {
  let rowWidth: CGFloat
  let counterWidth: CGFloat
  let leadingGap: CGFloat
  let height: CGFloat

  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    CGSize(width: width(proposed: proposal.width), height: height)
  }

  func placeSubviews(
    in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()
  ) {
    let content = max(0, bounds.width - leadingGap)
    for subview in subviews {
      subview.place(
        at: CGPoint(x: bounds.minX + leadingGap, y: bounds.minY),
        proposal: ProposedViewSize(width: content, height: bounds.height))
    }
  }

  func width(proposed: CGFloat?) -> CGFloat {
    let available = proposed.map { max(0, $0 - leadingGap) }
    let occupied = OverlayChannelMapModel.occupiedWidth(
      availableWidth: available, rowWidth: rowWidth, counterWidth: counterWidth)
    return occupied > 0 ? occupied + leadingGap : 0
  }
}
