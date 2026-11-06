import AppKit
import SwiftUI

/// The one numbered channel pill (bus intercom spec §5.5). The folded Agents
/// rail and the overlay's channel map, in the bar and the window, draw a
/// channel's digit only through this view; they differ in the form and in the
/// action they hand it.
///
/// It receives immutable display values, a hint, accessibility text and one
/// action. It does not receive the overlay state, a controller, a conversation,
/// a command or a bus reader: the caller resolves the conversation and its
/// exact owner, and the action never opens a microphone.
/// See docs/OVERLAY_PRESENTATION.md before changing the visual shell.
struct OverlayChannelPill: View {
  enum Form: Equatable {
    /// The folded Agents list: status dot and digit.
    case rail
    /// A channel of the expanded bar's second row (6–9 agents): the circled
    /// digit with the agent's name beside it.
    case chip
    /// A channel of the expanded bar's single row (1–5 agents): the circled
    /// digit with the agent's name under it, one button for both (bus brief
    /// 2026-11-04 §4). The bar grows by the caption line, never the circle.
    case stacked
    /// The vertical layout's collapsed map in the bar and the window: the
    /// stacked cards, or the circled digits of the channels the microphone
    /// feeds. Mini has no counter (Founder, 2026-11-04); during a take it
    /// draws each open channel as this form's single circled digit (bus
    /// brief §6, §11D).
    case counter
  }

  struct Display: Equatable {
    /// The channel digit; for the counter, the number of agents.
    let figure: String
    /// A numberless mailbox uses a glyph in the folded rail and in the
    /// agents list's chip, in the circled digit's place.
    var symbol: String? = nil
    /// The agent's name on a chip or stacked pill; channel 0, an agent with
    /// an empty name, the rail and the counter carry none.
    var name: String? = nil
    /// The width the map's row gives the name (`OverlayChannelMapNameWidths`);
    /// a narrower width than the name truncates it with an ellipsis, never a
    /// smaller font. Nil is the name's own width up to `maxNameWidth`.
    var nameWidth: CGFloat? = nil
    /// The presence dot: the rail draws it before the digit, the agents
    /// list's chip between the digit and the name (`nr · dot · name`, as the
    /// Bus window's roster rows; Founder, 2026-11-04). The bar's chips carry
    /// none: their hover card says the presence.
    var dot: OverlayAgentDotKind? = nil
    /// The follower is gone: the pill dims as the roster's Disconnected row.
    var disconnected = false
    /// The microphone feeds this channel now (chip and counter forms): the
    /// circle and the pill wear the recording violet (bus intercom spec §14;
    /// the extra pulsing dot on the circle went on 2026-11-04, decision 3).
    var microphoneOpen = false
    /// The counter's circled digits while the microphone is open.
    var openDigits: [String] = []
    /// The vertical layout's counter: stacked cards with the agent count as a
    /// badge, the face of a button that opens the Intercom group.
    var stacked = false
    var unread = false
    var unreadCount = 0
    var selected = false
    var enabled = true
    /// False while the panel is hidden or occluded. Nothing in the pill
    /// pulses any more; the map keeps passing it for its own visibility.
    var animates = false
  }

  let form: Form
  let display: Display
  let hint: String
  let accessibilityText: String
  var accessibilityValue = ""
  let identifier: String
  let palette: OverlayAppearancePalette
  /// The rail passes the roster's text scale; mini and the expanded bar pass
  /// `headerScale(_:)`, which also sizes the bar's rows.
  let textScale: CGFloat
  /// The floating hint: the agent's hover card on the bar's chips
  /// (`hintCard`), else `0 · Ctrl+0`; the agent list on the counter, the
  /// roster's own hints on the rail.
  var showsHint = true
  /// A bar chip's hover card (`OverlayChannelChipHoverCard`) in place of
  /// the one-line hint; the rail and the counter keep their text hints.
  var hintCard: HintCard? = nil
  let action: () -> Void

  /// Content for the header's floating hint panel, with the size measured
  /// from it. `title` is what the card says, so a change of any fact
  /// refreshes an open card.
  struct HintCard {
    let title: String
    let content: AnyView
    let size: CGSize
  }

  var body: some View {
    Button(action: action) {
      label
        .overlay(alignment: .bottomTrailing) {
          if stackedFace {
            stackedBadge.padding(3)
          }
        }
        .overlay(alignment: .topTrailing) {
          if stackedFace {
            if display.unread {
              Circle()
                .fill(palette.primaryText.color)
                .frame(width: 4, height: 4)
                .padding(3)
                .accessibilityHidden(true)
            }
          } else if form == .rail && display.unreadCount > 0 {
            railUnreadBadge
          } else if display.unread && form != .stacked {
            Circle()
              .fill(palette.primaryText.color)
              .frame(width: 6 * textScale, height: 6 * textScale)
              .offset(x: 1, y: -1)
              .accessibilityHidden(true)
          }
        }
        // A stacked pill's caption belongs to its button, so the hover and
        // the click over the name never fall to a neighbour.
        .contentShape(
          form == .stacked ? AnyShape(Rectangle()) : AnyShape(Capsule(style: .continuous)))
    }
    .buttonStyle(.plain)
    .csFocusOutline(cornerRadius: form == .stacked ? Self.stackedFocusRadius : height / 2)
    .disabled(!display.enabled)
    .opacity(display.enabled ? 1 : 0.35)
    .modifier(OverlayDisconnectedRowDim(disconnected: display.disconnected))
    .modifier(
      OverlayChannelPillHint(
        form: form, hint: hint, card: hintCard, palette: palette, enabled: showsHint,
        stacked: stackedFace)
    )
    .accessibilityLabel(Text(verbatim: accessibilityText))
    .accessibilityValue(Text(verbatim: spokenValue))
    .accessibilityAddTraits(display.selected ? .isSelected : [])
    .accessibilityIdentifier(identifier)
  }

  private var height: CGFloat {
    switch form {
    case .rail: 24 * textScale
    case .stacked: Self.stackedHeight(scale: textScale)
    case .chip, .counter: Self.headerHeight
    }
  }

  private var recording: Bool { display.microphoneOpen && form != .rail }

  /// The folded rail's unread count: its own small capsule outside the pill,
  /// at the pill's top-right shoulder, so the pill and its channel digit stay
  /// whole (Founder's mock-up, 2026-11-06). It grows with its figure; the
  /// rail keeps room for two digits beside the pill
  /// (`OverlayAgentDrawerLayout.railWidth`). Light: white with a hairline
  /// and dark figures; dark: grey with light figures. Opaque, so nothing
  /// behind it shows through. The roster rows keep `OverlayAgentUnreadBadge`,
  /// which has a whole row's width to sit in.
  private var railUnreadBadge: some View {
    // In the pill's coordinates with a zero-width pill: `maxX` is how far
    // the capsule's trailing edge lies past the pill's trailing edge.
    let frame = Self.railBadgeFrame(count: display.unreadCount, pillSize: .zero, scale: textScale)
    let dark = palette.appearance == .dark
    return Text(verbatim: String(display.unreadCount))
      .font(.system(size: Self.railBadgeFontSize * textScale, weight: .semibold))
      .monospacedDigit()
      .lineLimit(1)
      .fixedSize()
      .foregroundStyle(palette.primaryText.color)
      .frame(width: frame.width, height: frame.height)
      .background {
        ZStack {
          Capsule(style: .continuous).fill(palette.desktopBackground.color)
          Capsule(style: .continuous)
            .fill(dark ? palette.primaryText.color.opacity(0.28) : Color.white)
        }
      }
      .overlay {
        if !dark {
          Capsule(style: .continuous).strokeBorder(palette.border.color, lineWidth: 1)
        }
      }
      // The overlay lays it inside the pill's top-trailing corner; move it
      // out to its frame beside the pill.
      .offset(x: frame.maxX, y: frame.minY)
      .accessibilityLabel("\(display.unreadCount) unread replies")
  }

  private var spokenValue: String {
    guard form == .counter, display.stacked, display.unread else { return accessibilityValue }
    return [accessibilityValue, String(localized: "Unread replies")]
      .filter { !$0.isEmpty }.joined(separator: ". ")
  }

  /// Recording keeps the circled digits in both layouts; at rest the stacked
  /// counter shows its cards.
  private var stackedFace: Bool { form == .counter && display.stacked && !recording }

  /// The count remains neutral; the separate dot means unread replies. The
  /// map contains at most nine agents, so the count fits inside the circle.
  private var stackedBadge: some View {
    Text(verbatim: display.figure)
      .font(.system(size: Self.badgeSize, weight: .semibold))
      .monospacedDigit()
      .foregroundStyle(palette.primaryText.color)
      .frame(width: Self.badgeDiameter, height: Self.badgeDiameter)
      .background(palette.desktopBackground.color, in: Circle())
      .overlay { Circle().strokeBorder(palette.primaryText.color.opacity(0.25), lineWidth: 0.5) }
      .accessibilityHidden(true)
  }

  /// Recording takes the active-speaker violet, as the roster's open
  /// microphone and the `.agent` light do (`OverlayAgentAudioControls`).
  private var tint: Color {
    recording ? palette.agentAccent.color : palette.primaryText.color
  }

  private var fill: Color {
    if recording {
      return palette.agentAccent.color.opacity(OverlayAgentAudioControls.litGlassOpacity)
    }
    // The selected conversation is neutral, in the text tone: in the bus
    // violet means your voice and terracotta the microphone (Founder,
    // 2026-11-04, decision 4).
    return display.selected ? palette.primaryText.color.opacity(0.12) : CSColor.hairline(0.06)
  }

  @ViewBuilder
  private var label: some View {
    Group {
      switch form {
      case .rail:
        HStack(spacing: 3) {
          if let symbol = display.symbol {
            mailboxSymbol(symbol)
          } else {
            OverlayAgentStatusDot(kind: display.dot ?? .idle, palette: palette)
            figure(display.figure, size: 12)
          }
        }
        .padding(.horizontal, 6 * textScale)
      case .chip:
        HStack(spacing: Self.chipGap * textScale) {
          if let symbol = display.symbol {
            mailboxSymbol(symbol)
              .frame(width: Self.digitCircle * textScale, height: Self.digitCircle * textScale)
          } else {
            circledDigit(display.figure)
          }
          if let dot = display.dot {
            OverlayAgentStatusDot(kind: dot, palette: palette)
              .accessibilityHidden(true)
          }
          if let name = display.name {
            Text(verbatim: name)
              .font(Self.nameFont(textScale))
              .lineLimit(1)
              .truncationMode(.tail)
              .frame(
                width: display.nameWidth ?? Self.nameWidth(name, scale: textScale),
                alignment: .leading)
          }
        }
        .padding(.horizontal, Self.chipPadding * textScale)
      case .stacked:
        VStack(spacing: Self.captionGap * textScale) {
          circledDigit(display.figure)
          if let name = display.name {
            Text(verbatim: name)
              .font(Self.nameFont(textScale))
              .lineLimit(1)
              .truncationMode(.tail)
              .frame(width: display.nameWidth ?? Self.nameWidth(name, scale: textScale))
          }
        }
        .padding(.horizontal, Self.stackedPadding * textScale)
      case .counter:
        if stackedFace {
          OverlayMicrophoneGlyph(
            symbol: Self.stackedCardsSymbol, tint: palette.primaryText.color,
            markOffset: CGSize(width: -2.5, height: -2.5))
        } else if display.microphoneOpen {
          HStack(spacing: Self.counterGap * textScale) {
            ForEach(display.openDigits, id: \.self) { digit in
              circledDigit(digit)
            }
          }
          .padding(.horizontal, Self.counterRecordingPadding * textScale)
        } else {
          HStack(spacing: Self.counterGap * textScale) {
            figure(display.figure, size: Self.nameSize)
            Text(verbatim: Self.caret)
              .font(Self.nameFont(textScale))
              .frame(width: Self.textWidth(Self.caret, size: Self.nameSize * textScale))
          }
          .padding(.horizontal, Self.counterPadding * textScale)
        }
      }
    }
    .foregroundStyle(tint)
    .frame(height: height, alignment: form == .stacked ? .top : .center)
    // A stacked pill wears the selection and recording fill on its circle;
    // a capsule around the circle and the caption would read as a card.
    .background(
      Capsule(style: .continuous).fill(stackedFace || form == .stacked ? .clear : fill))
  }

  /// The digit: drawn here once, whatever the form.
  private func figure(_ text: String, size: CGFloat) -> some View {
    Text(verbatim: text)
      .font(.system(size: size * textScale, weight: .semibold))
      .monospacedDigit()
      .lineLimit(1)
      .fixedSize()
  }

  /// The numberless mailbox's symbol: drawn here once, whatever the form.
  private func mailboxSymbol(_ name: String) -> some View {
    Image(systemName: name)
      .font(.system(size: 12 * textScale))
  }

  /// A chip's or the recording counter's digit in its circle; while the
  /// microphone feeds the channel the circle and the pill wear the recording
  /// violet (spec §14; the Agents button carries the speaking and broadcast
  /// signs).
  private func circledDigit(_ digit: String) -> some View {
    figure(digit, size: Self.digitSize)
      .frame(width: Self.digitCircle * textScale, height: Self.digitCircle * textScale)
      .background {
        if form == .stacked { Circle().fill(fill) }
      }
      .overlay {
        Circle().strokeBorder(tint.opacity(0.5), lineWidth: 1)
          .accessibilityHidden(true)
      }
      .overlay(alignment: .topTrailing) {
        // On a stacked pill the unread dot marks the circle, where the chip
        // form draws it at the capsule's corner.
        if form == .stacked && display.unread {
          Circle()
            .fill(palette.primaryText.color)
            .frame(width: 6 * textScale, height: 6 * textScale)
            .offset(x: 1, y: -1)
            .accessibilityHidden(true)
        }
      }
  }

}

// MARK: Header metrics

/// The bar's widths and heights come from the same values the pill draws with,
/// so the bar's natural width, its resize minimum and its row heights are
/// measured, not constants (the approach of
/// `OverlayAgentDrawerLayout.railPillWidth`).
extension OverlayChannelPill {
  /// The header's control diameter: chips stand as tall as its buttons.
  static let headerHeight: CGFloat = OverlayMicrophoneGlyph.diameter
  /// Text scale stops growing the header's pills where a 22 pt pill still
  /// holds its circled digit. The bar's stacked row grows with it up to here.
  static let headerScaleCeiling: CGFloat = 1.25
  static let chipPadding: CGFloat = 6
  static let chipGap: CGFloat = 4
  /// A stacked pill's side margin: with the map's 4 pt chip spacing two
  /// captions stand at least 8 pt apart, and a caption never overlaps.
  static let stackedPadding: CGFloat = 2
  static let stackedFocusRadius: CGFloat = 6
  static let counterPadding: CGFloat = 5
  /// The recording counter hugs its circled digit.
  static let counterRecordingPadding: CGFloat = 2
  static let counterGap: CGFloat = 2
  static let digitCircle: CGFloat = 15
  static let digitSize: CGFloat = 10
  static let nameSize: CGFloat = 11
  /// The stacked pill's gap between the circle and its caption: the
  /// former open-channel dot's offset, kept so the bar's height does not
  /// change.
  static let captionGap: CGFloat = 2
  /// A long agent name truncates instead of collapsing the whole map; the
  /// widest a bar the user widens gives any one name.
  static let maxNameWidth: CGFloat = 96
  /// Names up to six letters are shown whole in the default bar; a longer
  /// one shows its first five and an ellipsis (`Konst…`, bus brief
  /// 2026-11-04 §5). The widths are those strings measured in the pill's
  /// own font, so the threshold follows the letters, not a fixed width.
  static let preferredNameLength = 6
  static let preferredNamePrefix = 5
  /// The shortest a narrowed bar cuts a name to: four letters and the
  /// ellipsis, so `Lena` and `Lena…` (Lena2) still read differently and
  /// names of up to five letters never lose a letter.
  static let shortestNamePrefix = 4
  static let ellipsis = "\u{2026}"
  static let caret = "\u{25BE}"
  /// Overlapping rectangles read as a deck, unlike a three-line list.
  static let stackedCardsSymbol = "rectangle.stack"
  static let badgeSize: CGFloat = 6
  static let badgeDiameter: CGFloat = 9
  /// The folded rail's unread capsule: 13 pt tall with a 9 pt figure, its
  /// top 2 pt above the pill's and its leading edge 1 pt inside the pill's
  /// trailing edge, where the pill's rounded end leaves it clear.
  static let railBadgeHeight: CGFloat = 13
  static let railBadgeFontSize: CGFloat = 9
  static let railBadgePadding: CGFloat = 3.5
  /// One monospaced 9 pt semibold digit, as `railPillWidth` counts 7.2 at 12.
  static let railBadgeDigitAdvance: CGFloat = 5.4
  static let railBadgeRise: CGFloat = 2
  static let railBadgeOverlap: CGFloat = 1
  /// The rail keeps room beside its pills for counts of up to two digits.
  static let railBadgeReservedCount = 99

  /// The rail capsule's width: a circle for one digit, wider with each more.
  static func railBadgeWidth(count: Int, scale: CGFloat) -> CGFloat {
    let digits = CGFloat(String(count).count)
    return ceil(max(railBadgeHeight, digits * railBadgeDigitAdvance + 2 * railBadgePadding) * scale)
  }

  /// The rail capsule's frame in a rail pill's coordinates (origin at the
  /// pill's top-leading corner, y down): beside the pill's top-right
  /// shoulder, outside the pill's capsule.
  static func railBadgeFrame(count: Int, pillSize: CGSize, scale: CGFloat) -> CGRect {
    CGRect(
      x: pillSize.width - railBadgeOverlap * scale, y: -railBadgeRise * scale,
      width: railBadgeWidth(count: count, scale: scale), height: railBadgeHeight * scale)
  }

  /// How far a reserved-width capsule reaches past a rail pill's trailing edge.
  static func railBadgeReach(scale: CGFloat) -> CGFloat {
    railBadgeFrame(count: railBadgeReservedCount, pillSize: .zero, scale: scale).maxX
  }

  static func headerScale(_ textScale: CGFloat) -> CGFloat {
    min(textScale, headerScaleCeiling)
  }

  static func nameFont(_ scale: CGFloat) -> Font {
    .system(size: nameSize * scale, weight: .semibold)
  }

  /// One semibold line, rounded up with a point of slack for the renderer.
  @MainActor
  static func textWidth(_ text: String, size: CGFloat, monospacedDigits: Bool = false) -> CGFloat {
    let font =
      monospacedDigits
      ? NSFont.monospacedDigitSystemFont(ofSize: size, weight: .semibold)
      : NSFont.systemFont(ofSize: size, weight: .semibold)
    return ceil((text as NSString).size(withAttributes: [.font: font]).width) + 1
  }

  @MainActor
  static func nameWidth(_ name: String, scale: CGFloat) -> CGFloat {
    min(textWidth(name, size: nameSize * scale), maxNameWidth * scale)
  }

  /// The name cut to its first `prefix` letters and the ellipsis, measured;
  /// a name no longer than that is its own width. Never wider than the name.
  @MainActor
  static func truncatedNameWidth(_ name: String, prefix: Int, scale: CGFloat) -> CGFloat {
    let full = nameWidth(name, scale: scale)
    guard name.count > prefix else { return full }
    return min(full, textWidth(String(name.prefix(prefix)) + ellipsis, size: nameSize * scale))
  }

  /// The caption line under a stacked pill's circle: the name font's line
  /// height (ascender, descender and leading), rounded up.
  @MainActor
  static func captionLineHeight(scale: CGFloat) -> CGFloat {
    let font = NSFont.systemFont(ofSize: nameSize * scale, weight: .semibold)
    return ceil(font.ascender - font.descender + font.leading)
  }

  /// A stacked pill's height: the circle, the open-channel dot's reach below
  /// it and the caption line, all at the header scale.
  @MainActor
  static func stackedHeight(scale: CGFloat) -> CGFloat {
    ceil(digitCircle * scale + captionGap * scale + captionLineHeight(scale: scale))
  }

  /// A map pill's width for the name width its row gives it (zero: no name).
  /// A chip is padding, the circle and the name beside it; a stacked pill is
  /// the wider of the circle and the name under it, with its margins. The
  /// open-channel dot overlaps the circle and adds none.
  static func chipWidth(
    nameWidth: CGFloat, form: Form, scale: CGFloat, presenceDot: Bool = false
  ) -> CGFloat {
    switch form {
    case .stacked:
      return ceil(max(digitCircle * scale, nameWidth) + 2 * stackedPadding * scale)
    case .chip, .rail, .counter:
      var width = 2 * chipPadding * scale + digitCircle * scale
      if presenceDot { width += chipGap * scale + OverlayAgentStatusDot.diameter * scale }
      if nameWidth > 0 { width += chipGap * scale + nameWidth }
      return ceil(width)
    }
  }

  /// The counter's width: `4 ▾`, the stacked cards (a header circle; the
  /// badge stays inside it), or the circled digits of the open channels.
  @MainActor
  static func counterWidth(
    figure: String, openDigitCount: Int, scale: CGFloat, stacked: Bool = false
  ) -> CGFloat {
    if stacked && openDigitCount == 0 { return headerHeight }
    guard openDigitCount == 0 else {
      let digits = CGFloat(openDigitCount)
      return ceil(
        2 * counterRecordingPadding * scale + digits * digitCircle * scale
          + (digits - 1) * counterGap * scale)
    }
    return ceil(
      2 * counterPadding * scale
        + textWidth(figure, size: nameSize * scale, monospacedDigits: true)
        + counterGap * scale + textWidth(caret, size: nameSize * scale))
  }
}

/// The rail shows the shared floating hint beside the pill, as the bubbles and
/// header controls do. In mini and the expanded bar the header is the whole
/// panel, so the bar's pills use the header's floating hint window: a chip
/// or stacked pill with a card shows the card there (non-interactive, after
/// the same delay as the one-line hint), anything else its text.
private struct OverlayChannelPillHint: ViewModifier {
  let form: OverlayChannelPill.Form
  let hint: String
  let card: OverlayChannelPill.HintCard?
  let palette: OverlayAppearancePalette
  let enabled: Bool
  let stacked: Bool

  @ViewBuilder
  func body(content: Content) -> some View {
    if enabled {
      switch form {
      case .rail:
        content.overlayFastHint(hint, palette: palette, leading: true)
      case .chip, .stacked:
        content.modifier(
          OverlayHeaderControlChrome(
            title: card?.title ?? hint, accent: nil, palette: palette,
            hintContent: card?.content, hintSize: card?.size))
      case .counter:
        content.modifier(
          OverlayHeaderControlChrome(title: hint, accent: stacked ? .state : nil, palette: palette))
      }
    } else {
      content
    }
  }
}
