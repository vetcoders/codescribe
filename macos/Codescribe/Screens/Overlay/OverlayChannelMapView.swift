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
/// the counter opens a microphone or retargets a capture. Wide strips add
/// separate microphone, playback and known panel actions beside each name.
struct OverlayChannelMapView: View {
  enum Placement: Equatable {
    /// Compact's header: named chips with controls, names, digits or counter.
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
  var controls: [String: OverlayChannelMapControls] = [:]
  var onToggleMicrophone: ((String) -> Void)? = nil
  var onTogglePlayback: ((String) -> Void)? = nil
  var onFocusChannel: ((String) -> Void)? = nil

  private enum Actions: Equatable {
    case all, microphone, none
  }

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
    let rich = rowWidth(actions: .all, scale: scale)
    let microphones = rowWidth(actions: .microphone, scale: scale)
    return OverlayChannelMapSlot(
      namedWidth: named, digitsWidth: digits, counterWidth: counter, leadingGap: Self.leadingGap,
      height: OverlayChannelPill.headerHeight, preferredWidths: [rich, microphones]
    ) {
      // The slot proposes exactly the width it claimed, so this reads the
      // same decision the slot took; no state, no feedback loop.
      GeometryReader { proxy in
        if proxy.size.width + 0.5 >= rich {
          chips(.chips, actions: .all, scale: scale)
        } else if proxy.size.width + 0.5 >= microphones {
          chips(.chips, actions: .microphone, scale: scale)
        } else {
          switch OverlayChannelMapModel.presentation(
            availableWidth: proxy.size.width, namedWidth: named, digitsWidth: digits,
            counterWidth: counter)
          {
          case .chips: chips(.chips, actions: .none, scale: scale)
          case .digits: chips(.digits, actions: .none, scale: scale)
          case .counter: counterPill(scale: scale)
          case .hidden: Color.clear.accessibilityHidden(true)
          }
        }
      }
    }
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-channel-map")
  }

  private func chips(
    _ presentation: OverlayChannelMapPresentation, actions: Actions, scale: CGFloat
  ) -> some View {
    HStack(spacing: Self.chipSpacing) {
      ForEach(model.chips) { chip in
        HStack(spacing: OverlayChannelPill.actionGap) {
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
            palette: palette, textScale: scale, showsHint: false
          ) {
            onSelectChannel(chip.digit)
          }
          if actions != .none, !chip.isBroadcast, let control = controls[chip.digit] {
            agentActions(chip, control: control, actions: actions, scale: scale)
          }
        }
        .fixedSize()
        .accessibilityElement(children: .contain)
      }
    }
  }

  /// Full names have priority over every optional control. The same count
  /// and fixed action frames are used here and by the rendered row.
  private func rowWidth(actions: Actions, scale: CGFloat) -> CGFloat {
    let widths = model.chips.map { chip in
      let named = OverlayChannelPill.chipWidth(name: chip.isBroadcast ? nil : chip.name, scale: scale)
      guard !chip.isBroadcast, let control = controls[chip.digit] else { return named }
      var count = onToggleMicrophone == nil ? 0 : 1
      if actions == .all {
        if onTogglePlayback != nil, control.muted != nil { count += 1 }
        if onFocusChannel != nil, control.focusSymbol != nil { count += 1 }
      }
      return named + CGFloat(count) * (OverlayChannelPill.actionWidth + OverlayChannelPill.actionGap)
    }
    return OverlayChannelMapModel.rowWidth(chipWidths: widths, spacing: Self.chipSpacing)
  }

  @ViewBuilder
  private func agentActions(
    _ chip: OverlayChannelMapChip, control: OverlayChannelMapControls, actions: Actions,
    scale: CGFloat
  ) -> some View {
    if let onToggleMicrophone {
      OverlayChannelMapActionButton(
        symbol: chip.microphoneOpen ? "mic.fill" : "mic",
        label: chip.name + ": " + (chip.microphoneOpen
          ? String(localized: "Stop speaking to agent") : String(localized: "Speak to agent")),
        value: chip.microphoneOpen ? String(localized: "Microphone active")
          : String(localized: "Microphone off"),
        identifier: "overlay-channel-map-microphone-\(chip.digit)", palette: palette, scale: scale,
        enabled: control.microphoneEnabled, active: chip.microphoneOpen,
        pulses: chip.microphoneOpen && animates
      ) { onToggleMicrophone(chip.digit) }
    }
    if actions == .all {
      if let onTogglePlayback, let muted = control.muted {
        OverlayChannelMapActionButton(
          symbol: muted ? "speaker.slash.fill"
            : control.speaking ? "speaker.wave.3.fill" : "speaker.wave.2",
          label: chip.name + ": " + (muted ? String(localized: "Unmute agent replies")
            : String(localized: "Mute agent replies")),
          value: control.speaking && !muted ? String(localized: "Speaking")
            : muted ? String(localized: "Automatic playback muted")
              : String(localized: "Automatic playback on"),
          identifier: "overlay-channel-map-playback-\(chip.digit)", palette: palette, scale: scale,
          enabled: control.playbackEnabled, active: control.speaking && !muted,
          pulses: control.speaking && !muted && animates
        ) { onTogglePlayback(chip.digit) }
      }
      if let onFocusChannel, let symbol = control.focusSymbol {
        OverlayChannelMapActionButton(
          symbol: symbol, label: chip.name + ": " + control.focusLabel,
          identifier: "overlay-channel-map-focus-\(chip.digit)", palette: palette, scale: scale,
          enabled: control.focusEnabled, pending: control.focusPending,
          succeeded: control.focusSucceeded, failed: control.focusFailed
        ) { onFocusChannel(chip.digit) }
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

/// Claims the width of what the map shows: a named row with optional actions,
/// the digit-only row, the counter, or nothing. A flexible frame would keep the
/// leftover beside a counter; reporting the shown width hands it back to the
/// waveform. A map that cannot fit even the counter claims no width, so it
/// never pushes the actions or the fold button out of the strip.
struct OverlayChannelMapSlot: Layout {
  let namedWidth: CGFloat
  let digitsWidth: CGFloat
  let counterWidth: CGFloat
  let leadingGap: CGFloat
  let height: CGFloat
  var preferredWidths: [CGFloat] = []

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
      counterWidth: counterWidth, preferredWidths: preferredWidths)
    return occupied > 0 ? occupied + leadingGap : 0
  }
}
