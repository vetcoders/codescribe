import SwiftUI

/// The bar's agent map and the vertical layout's counter (Monika's bar brief,
/// 2026-10-30; bus intercom spec §5.1, §13).
///
/// In the expanded bar the map shows every present agent in channel order:
/// the circled digit with the name under it in one row, or beside it on the
/// bar's second row. The bar decides the arrangement for its width
/// (`OverlayBarGeometry`); the map draws that decision and owns no width of
/// its own. In the vertical layout the map is the stacked counter beside the
/// brand, in every form.
///
/// A chip selects that channel's conversation through the roster's own
/// selection path, exactly as its rail pill does (decision D1). The counter
/// brings the Intercom group forward. Neither opens a microphone or
/// retargets a capture; a chip's secondary click carries the agent's
/// microphone, playback and panel actions.
struct OverlayChannelMapView: View {
  enum Placement: Equatable {
    /// The expanded bar's map at the layout the bar chose for its width.
    case strip(OverlayChannelMapLayout)
    /// The vertical layout's stacked counter.
    case counter
  }

  let model: OverlayChannelMapModel
  let placement: Placement
  let modifier: OverlayChannelChordModifier
  let palette: OverlayAppearancePalette
  let textScale: CGFloat
  let animates: Bool
  /// A chip's digit; the roster resolves the conversation and its owner.
  let onSelectChannel: (String) -> Void
  /// The counter's click: the vertical layout's Intercom group.
  let onCounter: () -> Void
  /// The roster supplies the same menu as the rail and floating intercom.
  var agentPillMenu: ((String) -> OverlayBroadcastPolicyMenu)? = nil

  var body: some View {
    switch placement {
    case .counter:
      counterPill(scale: OverlayChannelPill.headerScale(textScale))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("overlay-channel-map")
    case .strip(let layout):
      strip(layout, scale: OverlayChannelPill.headerScale(textScale))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("overlay-channel-map")
    }
  }

  @ViewBuilder
  private func strip(_ layout: OverlayChannelMapLayout, scale: CGFloat) -> some View {
    let chips = Array(model.chips.prefix(layout.slots.count).enumerated())
    switch layout.arrangement {
    case .oneRow:
      let origins = layout.origins
      ZStack(alignment: .topLeading) {
        // Each slot, never narrower than its circle, is the chip's hit
        // area, so every channel stays clickable.
        ForEach(chips, id: \.element.id) { index, chip in
          chipPill(chip, layout: layout, index: index, scale: scale)
            .offset(x: origins[index])
        }
        // The unread dots lie on their own top layer, above every circle.
        ForEach(chips, id: \.element.id) { index, chip in
          if chip.unread {
            OverlayChannelPill.unreadDot(palette: palette, scale: scale)
              .offset(
                x: origins[index] + (layout.slots[index] + layout.circle) / 2
                  - OverlayChannelPill.unreadDotDiameter * scale)
          }
        }
      }
      .frame(width: layout.width, height: OverlayChannelPill.mapRowHeight, alignment: .topLeading)
    case .twoRows:
      HStack(spacing: layout.spacing) {
        ForEach(chips, id: \.element.id) { index, chip in
          chipPill(chip, layout: layout, index: index, scale: scale)
        }
      }
      .fixedSize()
    }
  }

  private func chipPill(
    _ chip: OverlayChannelMapChip, layout: OverlayChannelMapLayout, index: Int, scale: CGFloat
  ) -> some View {
    let oneRow = layout.arrangement == .oneRow
    let name =
      chip.isBroadcast
      ? nil
      : OverlayChannelPill.truncatedName(
        chip.name, width: layout.nameWidths[index], scale: scale)
    return OverlayChannelPill(
      form: .chip,
      display: OverlayChannelPill.Display(
        figure: chip.digit, name: name, nameBelow: oneRow,
        slotWidth: oneRow ? layout.slots[index] : nil, drawsUnreadDot: !oneRow,
        disconnected: chip.disconnected, microphoneOpen: chip.microphoneOpen,
        unread: chip.unread, selected: chip.selected, enabled: chip.selectable,
        animates: animates),
      // `4 · Lena2 · Ctrl+4`: the full name tells similar or truncated names apart.
      hint: chip.hint(modifier: modifier),
      accessibilityText: chip.accessibilityLabel(modifier: modifier),
      accessibilityValue: chip.disconnected ? String(localized: "Disconnected") : "",
      identifier: "overlay-channel-chip-\(chip.digit)",
      palette: palette, textScale: scale
    ) {
      onSelectChannel(chip.digit)
    }
    .modifier(
      agentPillMenu?(chip.digit)
        ?? OverlayBroadcastPolicyMenu(members: [], palette: palette, enabled: false))
  }

  private func counterPill(scale: CGFloat) -> some View {
    OverlayChannelPill(
      form: .counter,
      display: OverlayChannelPill.Display(
        figure: String(model.count), microphoneOpen: model.microphoneOpen,
        openDigits: model.openDigits, stacked: true, unread: model.anyUnread,
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

/// The present agents under mini's ✦ (`OverlayChannelStatusView.miniAgentsButton`):
/// `N · Name` in channel order, then each channel's status, small and
/// secondary: the roster's status dot, its reception marks (the channel-0
/// antenna and the do-not-disturb moon, each its own switch) and the yellow
/// unread dot. Monika, 2026-10-30: per-agent icons leave the bar itself, so
/// the list shows the channels' statuses and a secondary click on a row
/// opens the agent's menu, the same menu as a map chip. A row's main button
/// selects that channel's conversation through the map's selection path; the
/// marks are its siblings, never nested in it. Every status comes from the
/// roster's own projections; the list computes none.
struct OverlayAgentsHoverList: View {
  struct Row: Identifiable {
    let chip: OverlayChannelMapChip
    /// The roster's row for the channel; its reception marks read it.
    let channel: OverlayChannelDelivery?
    /// `OverlayChannelStatusView.statusDotKind(for:)`.
    let dot: OverlayAgentDotKind
    /// `OverlayChannelStatusView.statusHelp(for:)`, read by VoiceOver.
    let statusHelp: String
    /// `OverlayChannelStatusView.agentPillMenu(for:)`: the chip's menu.
    let menu: OverlayBroadcastPolicyMenu

    var id: String { chip.id }
  }

  let rows: [Row]
  let modifier: OverlayChannelChordModifier
  let palette: OverlayAppearancePalette
  let onSelect: (String) -> Void

  static let rowHeight: CGFloat = 24
  static let horizontalPadding: CGFloat = 10
  static let verticalPadding: CGFloat = 4
  static let fontSize: CGFloat = 12
  /// The status dot and the two reception marks (14 pt each, 3 pt apart).
  static let statusWidth: CGFloat = 8 + 2 * 14 + 3

  static func rowTitle(_ chip: OverlayChannelMapChip) -> String {
    chip.digit + " · " + chip.name
  }

  @MainActor
  static func size(rows: [Row]) -> CGSize {
    let text =
      rows.map {
        OverlayChannelPill.textWidth(rowTitle($0.chip), size: fontSize)
      }.max() ?? 0
    let width =
      text + 2 * horizontalPadding + statusWidth + OverlayChannelPill.unreadDotDiameter
      + OverlayChannelPill.chipGap * 5
    return CGSize(
      width: min(300, max(160, ceil(width))),
      height: 2 * verticalPadding + rowHeight * CGFloat(rows.count))
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      ForEach(rows) { row in
        HStack(spacing: OverlayChannelPill.chipGap) {
          Button {
            onSelect(row.chip.digit)
          } label: {
            HStack(spacing: OverlayChannelPill.chipGap * 1.5) {
              // Number and name first: who is on which channel.
              Text(verbatim: Self.rowTitle(row.chip))
                .font(.system(size: Self.fontSize, weight: .medium))
                .foregroundStyle(palette.primaryText.color)
                .lineLimit(1)
                .truncationMode(.tail)
              OverlayAgentStatusDot(kind: row.dot, palette: palette)
                .accessibilityHidden(true)
              Spacer(minLength: OverlayChannelPill.chipGap)
            }
            .frame(height: Self.rowHeight)
            .contentShape(Rectangle())
          }
          .buttonStyle(.plain)
          .disabled(!row.chip.selectable)
          .opacity(row.chip.selectable ? 1 : 0.45)
          .accessibilityLabel(Text(verbatim: row.chip.accessibilityLabel(modifier: modifier)))
          .accessibilityValue(
            Text(
              verbatim: [
                row.statusHelp, row.chip.unread ? String(localized: "Unread replies") : "",
              ].filter { !$0.isEmpty }.joined(separator: ", ")))
          .accessibilityIdentifier("overlay-agents-list-\(row.chip.digit)")
          if let channel = row.channel {
            OverlayReceptionMarks(channel: channel, palette: palette, textScale: 1)
          }
          Circle()
            .fill(palette.processingStatus.color)
            .frame(
              width: OverlayChannelPill.unreadDotDiameter,
              height: OverlayChannelPill.unreadDotDiameter
            )
            .opacity(row.chip.unread ? 1 : 0)
            .accessibilityHidden(true)
        }
        .padding(.horizontal, Self.horizontalPadding)
        .contentShape(Rectangle())
        .modifier(OverlayDisconnectedRowDim(disconnected: row.chip.disconnected))
        .modifier(row.menu)
      }
    }
    .padding(.vertical, Self.verticalPadding)
  }
}
