import SwiftUI

/// The collapsed overlay's channel map (bus intercom spec §5.1, §13).
///
/// In compact it is one chip per agent between the session timer and the
/// actions. Where the strip has no room for the names, the chips drop them
/// (the selected conversation's chip keeps its own); where even the digits do
/// not fit, the row collapses into one counter. In mini it is the counter
/// alone, in the place the "cs" wordmark had.
///
/// A chip selects that channel's conversation through the roster's own
/// selection path, exactly as its rail pill does (decision D1). The counter
/// changes the overlay's form or width through its action. Neither a chip nor
/// the counter opens a microphone or retargets a capture.
struct OverlayChannelMapView: View {
  enum Placement: Equatable {
    /// Compact's header: named chips, digit-only chips, the counter or nothing.
    case strip
    /// Mini's header: the counter only; mini's width follows it.
    case mini
  }

  /// The map's distance from the session timer. The map carries it, so a map
  /// that yields its place costs the strip nothing.
  static let leadingGap: CGFloat = 8
  static let chipSpacing: CGFloat = 4

  let model: OverlayChannelMapModel
  let placement: Placement
  let modifier: OverlayChannelChordModifier
  let palette: OverlayAppearancePalette
  let textScale: CGFloat
  let animates: Bool
  /// A chip's digit; the roster resolves the conversation and its owner.
  let onSelectChannel: (String) -> Void
  /// The counter's click (spec §13.4): mini switches to compact; a compact
  /// strip the user narrowed returns to its natural width.
  let onCounter: () -> Void

  var body: some View {
    switch placement {
    case .mini:
      counterPill(scale: OverlayChannelPill.headerScale(textScale))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("overlay-channel-map")
    case .strip:
      strip
    }
  }

  private var strip: some View {
    let scale = OverlayChannelPill.headerScale(textScale)
    let named = model.rowWidth(for: .chips, scale: scale, spacing: Self.chipSpacing)
    let digits = model.rowWidth(for: .digits, scale: scale, spacing: Self.chipSpacing)
    let counter = OverlayChannelPill.counterWidth(
      figure: String(model.count), openDigitCount: model.openDigits.count, scale: scale)
    return OverlayChannelMapSlot(
      namedWidth: named, digitsWidth: digits, counterWidth: counter, leadingGap: Self.leadingGap,
      height: OverlayChannelPill.headerHeight
    ) {
      // The slot proposes exactly the width it claimed, so this reads the
      // same decision the slot took; no state, no feedback loop.
      GeometryReader { proxy in
        switch OverlayChannelMapModel.presentation(
          availableWidth: proxy.size.width, namedWidth: named, digitsWidth: digits,
          counterWidth: counter)
        {
        case .chips: chips(.chips, scale: scale)
        case .digits: chips(.digits, scale: scale)
        case .counter: counterPill(scale: scale)
        case .hidden: Color.clear.accessibilityHidden(true)
        }
      }
    }
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-channel-map")
  }

  private func chips(_ presentation: OverlayChannelMapPresentation, scale: CGFloat) -> some View {
    HStack(spacing: Self.chipSpacing) {
      ForEach(model.chips) { chip in
        OverlayChannelPill(
          form: .chip,
          display: OverlayChannelPill.Display(
            figure: chip.digit, name: chip.showsName(in: presentation) ? chip.name : nil,
            disconnected: chip.disconnected, microphoneOpen: chip.microphoneOpen,
            unread: chip.unread, selected: chip.selected, enabled: chip.selectable,
            animates: animates),
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
        figure: String(model.count), microphoneOpen: model.microphoneOpen,
        openDigits: model.openDigits, unread: model.anyUnread, animates: animates),
      hint: model.counterHint,
      accessibilityText: String(
        localized: "Channel map",
        comment: "Compact overlay: the collapsed counter that stands for every mapped channel"),
      accessibilityValue: model.counterAccessibilityValue,
      identifier: "overlay-channel-map-toggle",
      palette: palette, textScale: scale, action: onCounter)
  }
}

/// Claims the width of what the map shows: the named row, the digit-only
/// row, the counter, or nothing. A plain flexible frame would keep the
/// leftover beside a counter; reporting the shown width hands it back to the
/// waveform. A map that cannot fit even the counter claims no width, so it
/// never pushes the actions or the fold button out of the strip.
struct OverlayChannelMapSlot: Layout {
  let namedWidth: CGFloat
  let digitsWidth: CGFloat
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
      availableWidth: available, namedWidth: namedWidth, digitsWidth: digitsWidth,
      counterWidth: counterWidth)
    return occupied > 0 ? occupied + leadingGap : 0
  }
}
