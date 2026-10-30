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
        // Later circles lie on earlier ones once they overlap; each slot,
        // not its circle, is the chip's hit area, so every channel stays
        // clickable.
        ForEach(chips, id: \.element.id) { index, chip in
          chipPill(chip, layout: layout, index: index, scale: scale)
            .offset(x: origins[index])
        }
        // The unread dots lie above every circle, so no overlap hides one.
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

/// Mini's Agents slot (Monika's bar brief, 2026-10-30). At rest the ✦
/// opens the Agents window as the roster's header button does; hovering it
/// lists the present agents, `N · Name` in channel order with their unread
/// dots, and the pointer can move into the list to pick one. While Monika
/// speaks to agents mini stays mini (her decision, 2026-10-30): the slot
/// shows the violet circled digits of every channel the microphone feeds,
/// overlapping when there are several, `0` for all; the hint names them.
/// The press is the roster's Agents press, which in mini stops that capture.
struct OverlayMiniAgentsControl: View {
  let model: OverlayChannelMapModel
  let modifier: OverlayChannelChordModifier
  let palette: OverlayAppearancePalette
  let textScale: CGFloat
  let animates: Bool
  /// The Agents surface is on screen.
  let selected: Bool
  /// A reply waits in some conversation.
  let unread: Bool
  /// The press stops the agent capture (`OverlayState.agentsButtonStopsRecording`).
  let stops: Bool
  let onAgents: () -> Void
  let onSelectChannel: (String) -> Void

  /// The slot's width: the ✦ circle, or the recipients' digits when wider.
  @MainActor
  static func width(map: OverlayChannelMapModel, textScale: CGFloat) -> CGFloat {
    guard map.microphoneOpen else { return OverlayMicrophoneGlyph.diameter }
    return max(
      OverlayMicrophoneGlyph.diameter,
      OverlayChannelPill.counterWidth(
        figure: String(map.count), openDigitCount: map.openDigits.count,
        scale: OverlayChannelPill.headerScale(textScale)))
  }

  private var actionLabel: String {
    stops ? String(localized: "Stop recording") : String(localized: "Agents")
  }

  var body: some View {
    if model.microphoneOpen {
      recipients
    } else {
      agentsButton
    }
  }

  private var recipients: some View {
    OverlayChannelPill(
      form: .counter,
      display: OverlayChannelPill.Display(
        figure: String(model.count), microphoneOpen: true, openDigits: model.openDigits,
        animates: animates),
      hint: "",
      accessibilityText: actionLabel,
      accessibilityValue: model.counterAccessibilityValue,
      identifier: "overlay-agent-glyph",
      palette: palette, textScale: OverlayChannelPill.headerScale(textScale),
      showsHint: false, action: onAgents
    )
    .frame(minWidth: OverlayMicrophoneGlyph.diameter)
    .modifier(
      OverlayHeaderControlChrome(
        title: actionLabel + ". " + model.recipientsHint, accent: nil, palette: palette))
  }

  private var agentsButton: some View {
    let agents = model.agents
    let list: AnyView? =
      agents.isEmpty
      ? nil
      : AnyView(
        OverlayAgentsHoverList(
          agents: agents, modifier: modifier, palette: palette, onSelect: onSelectChannel))
    return Button(action: onAgents) {
      OverlayMicrophoneGlyph(
        character: OverlayHeaderGlyph.agents,
        tint: selected
          ? OverlayHeaderAccent.agents.selectionTint(in: palette) : palette.mutedText.color,
        plateOpacity: selected ? 0 : 0.12
      )
      .contentShape(Circle())
      .overlay(alignment: .topTrailing) {
        if unread {
          Circle()
            .fill(palette.primaryText.color)
            .frame(width: 5, height: 5)
            .offset(x: 2, y: -2)
            .accessibilityLabel("Unread replies")
            .accessibilityIdentifier("overlay-unread-replies")
        }
      }
    }
    .buttonStyle(.plain)
    .csFocusOutline()
    .fixedSize()
    .modifier(
      OverlayHeaderControlChrome(
        title: actionLabel, accent: .agents, palette: palette, selected: selected,
        hintContent: list,
        hintSize: list == nil ? nil : OverlayAgentsHoverList.size(agents: agents),
        hintInteractive: list != nil)
    )
    .accessibilityLabel(actionLabel)
    .accessibilityAddTraits(selected ? .isSelected : [])
    .accessibilityIdentifier("overlay-agent-glyph")
  }
}

/// The present agents under mini's ✦: `N · Name` in channel order, the
/// yellow unread dot beside a waiting reply. A row selects that channel's
/// conversation through the same path as a map chip.
struct OverlayAgentsHoverList: View {
  let agents: [OverlayChannelMapChip]
  let modifier: OverlayChannelChordModifier
  let palette: OverlayAppearancePalette
  let onSelect: (String) -> Void

  static let rowHeight: CGFloat = 24
  static let horizontalPadding: CGFloat = 10
  static let verticalPadding: CGFloat = 4
  static let fontSize: CGFloat = 12

  static func rowTitle(_ chip: OverlayChannelMapChip) -> String {
    chip.digit + " · " + chip.name
  }

  @MainActor
  static func size(agents: [OverlayChannelMapChip]) -> CGSize {
    let text =
      agents.map {
        OverlayChannelPill.textWidth(rowTitle($0), size: fontSize)
      }.max() ?? 0
    let width =
      text + 2 * horizontalPadding + OverlayChannelPill.unreadDotDiameter
      + OverlayChannelPill.chipGap * 2
    return CGSize(
      width: min(260, max(140, ceil(width))),
      height: 2 * verticalPadding + rowHeight * CGFloat(agents.count))
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      ForEach(agents) { chip in
        Button {
          onSelect(chip.digit)
        } label: {
          HStack(spacing: OverlayChannelPill.chipGap) {
            Text(verbatim: Self.rowTitle(chip))
              .font(.system(size: Self.fontSize, weight: .medium))
              .foregroundStyle(palette.primaryText.color)
              .lineLimit(1)
              .truncationMode(.tail)
            Spacer(minLength: OverlayChannelPill.chipGap * 2)
            if chip.unread {
              Circle()
                .fill(palette.processingStatus.color)
                .frame(
                  width: OverlayChannelPill.unreadDotDiameter,
                  height: OverlayChannelPill.unreadDotDiameter)
                .accessibilityHidden(true)
            }
          }
          .padding(.horizontal, Self.horizontalPadding)
          .frame(height: Self.rowHeight)
          .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!chip.selectable)
        .opacity(chip.selectable ? 1 : 0.45)
        .modifier(OverlayDisconnectedRowDim(disconnected: chip.disconnected))
        .accessibilityLabel(Text(verbatim: chip.accessibilityLabel(modifier: modifier)))
        .accessibilityValue(chip.unread ? String(localized: "Unread replies") : "")
        .accessibilityIdentifier("overlay-agents-list-\(chip.digit)")
      }
    }
    .padding(.vertical, Self.verticalPadding)
  }
}
