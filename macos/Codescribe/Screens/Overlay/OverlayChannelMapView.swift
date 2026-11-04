import SwiftUI

/// The overlay's channel map (bus intercom spec §5.1, §13; bus brief
/// 2026-11-04 §4, §5).
///
/// In the expanded bar it is every bound agent, in digit order: with 1–5
/// agents one row of stacked pills beside the session timer (the name under
/// the circle), with 6–9 the bar's second row of chips (the name beside the
/// circle). The bar's layout tells the map its row through the height it
/// proposes (`OverlayChannelMapSlot`); the map gives the names the width the
/// row leaves them, cutting the longest first, and never drops a channel, a
/// name or a digit. In mini it is the counter alone, in the place the "cs"
/// wordmark had.
///
/// A chip selects that channel's conversation through the roster's own
/// selection path, exactly as its rail pill does (decision D1), and names the
/// agent and the chord in its hint. The counter changes the overlay's form
/// through its action. Neither a chip nor the counter opens a microphone or
/// retargets a capture.
struct OverlayChannelMapView: View {
  enum Placement: Equatable {
    /// The expanded bar: stacked pills in its single row or chips in its
    /// second row.
    case strip
    /// Mini's header: the counter only; mini's width follows it.
    case mini
  }

  /// The map's distance from the session timer in the bar's single row.
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
  /// The counter's click (spec §13.4): mini switches to the expanded bar.
  let onCounter: () -> Void
  /// The roster supplies the same menu as the rail and floating intercom.
  var agentPillMenu: ((String) -> OverlayBroadcastPolicyMenu)? = nil
  /// The vertical layout's mini counter: stacked cards that open the
  /// Intercom group. The header decides; the map only draws it.
  var stacked = false

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
    let names = OverlayChannelMapNameWidths(chips: model.chips, scale: scale)
    let stackedHeight = OverlayChannelPill.stackedHeight(scale: scale)
    return OverlayChannelMapSlot(
      stackedHeight: stackedHeight, chipHeight: OverlayChannelPill.headerHeight,
      stackedWidths: rowWidth(names.shortest, form: .stacked, scale: scale)
        ... rowWidth(names.full, form: .stacked, scale: scale),
      chipWidths: rowWidth(names.shortest, form: .chip, scale: scale)
        ... rowWidth(names.full, form: .chip, scale: scale)
    ) {
      // The slot proposes exactly the size it claimed, so this reads the
      // same row the bar chose; no state, no feedback loop.
      GeometryReader { proxy in
        let form: OverlayChannelPill.Form =
          proxy.size.height + 0.5 >= stackedHeight ? .stacked : .chip
        chips(
          form: form,
          nameWidths: names.fitted(available: proxy.size.width) {
            rowWidth($0, form: form, scale: scale)
          },
          scale: scale)
      }
    }
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-channel-map")
  }

  /// A row of pills at the given name widths, as the window measures it.
  private func rowWidth(
    _ nameWidths: [CGFloat], form: OverlayChannelPill.Form, scale: CGFloat
  ) -> CGFloat {
    OverlayChannelMapModel.rowWidth(
      nameWidths: nameWidths, form: form, scale: scale, spacing: Self.chipSpacing)
  }

  /// One button per channel, its caption included, each with its own hit
  /// target, hint and menu. Pills keep the map's spacing; circles never
  /// overlap.
  private func chips(
    form: OverlayChannelPill.Form, nameWidths: [CGFloat], scale: CGFloat
  ) -> some View {
    HStack(alignment: .top, spacing: Self.chipSpacing) {
      ForEach(Array(model.chips.enumerated()), id: \.element.id) { index, chip in
        let nameWidth = index < nameWidths.count ? nameWidths[index] : 0
        OverlayChannelPill(
          form: form,
          display: OverlayChannelPill.Display(
            figure: chip.digit, name: nameWidth > 0 ? chip.name : nil, nameWidth: nameWidth,
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
        .fixedSize()
        .modifier(
          agentPillMenu?(chip.digit)
            ?? OverlayBroadcastPolicyMenu(members: [], palette: palette, enabled: false))
      }
    }
  }

  private func counterPill(scale: CGFloat) -> some View {
    OverlayChannelPill(
      form: .counter,
      display: OverlayChannelPill.Display(
        figure: String(model.count), microphoneOpen: model.microphoneOpen,
        openDigits: model.openDigits, stacked: stacked, unread: model.anyUnread,
        animates: animates),
      hint: model.counterHint,
      accessibilityText: String(
        localized: "Channel map",
        comment: "Compact overlay: the collapsed counter that stands for every mapped channel"),
      accessibilityValue: model.counterAccessibilityValue,
      identifier: "overlay-channel-map-toggle",
      palette: palette, textScale: scale, action: onCounter)
  }
}

/// Claims the map's width for the bar's proposal and tells the map its row.
/// A proposal at least as tall as a stacked pill is the bar's single row;
/// a chip-tall one is its second row. The width is the proposal held between
/// the row with the shortest names and the row with every name whole, so the
/// map never claims a hole beside its pills and never less than its floor.
struct OverlayChannelMapSlot: Layout {
  let stackedHeight: CGFloat
  let chipHeight: CGFloat
  let stackedWidths: ClosedRange<CGFloat>
  let chipWidths: ClosedRange<CGFloat>

  func stacks(_ proposal: ProposedViewSize) -> Bool {
    (proposal.height ?? .infinity) + 0.5 >= stackedHeight
  }

  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    let isStacked = stacks(proposal)
    return CGSize(
      width: width(proposed: proposal.width, range: isStacked ? stackedWidths : chipWidths),
      height: isStacked ? stackedHeight : chipHeight)
  }

  func placeSubviews(
    in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()
  ) {
    for subview in subviews {
      subview.place(at: bounds.origin, proposal: ProposedViewSize(bounds.size))
    }
  }

  func width(proposed: CGFloat?, range: ClosedRange<CGFloat>) -> CGFloat {
    guard let proposed, proposed.isFinite else { return range.upperBound }
    return min(max(proposed, range.lowerBound), range.upperBound)
  }
}
