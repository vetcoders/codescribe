import AppKit
import SwiftUI

/// The one numbered channel pill (bus intercom spec §5.5). The folded Agents
/// rail and the compact overlay's channel map draw a channel's digit only
/// through this view; they differ in the form and in the action they hand it.
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
    /// A channel of the expanded bar's agent map: the circled digit and the
    /// agent's name, under the circle in one row or beside it in the second
    /// row (Monika's bar brief, 2026-10-30).
    case chip
    /// The vertical layout's stacked cards, and the circled digits of the
    /// channels the microphone feeds: the stacked counter while recording
    /// and mini's Agents slot while Monika speaks to agents.
    case counter
  }

  struct Display: Equatable {
    /// The channel digit; for the counter, the number of agents.
    let figure: String
    /// A numberless mailbox uses a glyph in the folded rail.
    var symbol: String? = nil
    /// The agent's name on a chip, already truncated to the chip's room;
    /// channel 0, the rail and the counter carry none.
    var name: String? = nil
    /// The chip's name sits under its circle (the bar's one row) instead of
    /// beside it (the second row).
    var nameBelow = false
    /// The chip's slot in the one-row map, never narrower than the circle;
    /// the slot is the hit area.
    var slotWidth: CGFloat? = nil
    /// False where the map draws the unread dots on its own top layer, above
    /// every circle and name.
    var drawsUnreadDot = true
    /// The rail's status dot.
    var dot: OverlayAgentDotKind? = nil
    /// The follower is gone: the pill dims as the roster's Disconnected row.
    var disconnected = false
    /// The microphone feeds this channel now (chip and counter forms): the
    /// channel is a recipient of the current utterance. Its circle wears the
    /// violet ring and the violet open-channel dot (bus intercom spec §14).
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
    /// False while the panel is hidden or occluded: no pulse keeps the render
    /// loop awake.
    var animates = false
  }

  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.layoutDirection) private var layoutDirection
  let form: Form
  let display: Display
  let hint: String
  let accessibilityText: String
  var accessibilityValue = ""
  let identifier: String
  let palette: OverlayAppearancePalette
  /// The rail passes the roster's text scale; mini and compact pass
  /// `headerScale(_:)` so their 46 pt row keeps its height.
  let textScale: CGFloat
  /// Named horizontal chips are self-describing; the counter retains its
  /// list hint and the folded roster retains its existing hints.
  var showsHint = true
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      label
        .overlay(alignment: .topTrailing) {
          if stackedFace {
            stackedBadge
          } else if form == .rail && display.unreadCount > 0 {
            OverlayAgentUnreadBadge(
              count: display.unreadCount, palette: palette, textScale: textScale,
              presentation: .rail
            )
            .offset(
              x: (layoutDirection == .rightToLeft ? -1 : 1)
                * OverlayAgentUnreadBadge.railDiameter(
                  count: display.unreadCount, textScale: textScale) / 2,
              y: -OverlayAgentUnreadBadge.railDiameter(
                count: display.unreadCount, textScale: textScale) / 2
            )
            .allowsHitTesting(false)
            .accessibilityHidden(true)
          } else if display.unread && form != .rail && form != .chip {
            Circle()
              .fill(palette.processingStatus.color)
              .frame(width: 6 * textScale, height: 6 * textScale)
              .offset(x: 1, y: -1)
              .accessibilityHidden(true)
          }
        }
        .contentShape(
          form == .chip ? AnyShape(Rectangle()) : AnyShape(Capsule(style: .continuous)))
    }
    .buttonStyle(.plain)
    .csFocusOutline(cornerRadius: height / 2)
    .disabled(!display.enabled)
    .opacity(display.enabled ? 1 : 0.35)
    .modifier(OverlayDisconnectedRowDim(disconnected: display.disconnected))
    .modifier(OverlayChannelPillHint(form: form, hint: hint, palette: palette, enabled: showsHint))
    .accessibilityLabel(Text(verbatim: accessibilityText))
    .accessibilityValue(Text(verbatim: resolvedAccessibilityValue))
    .accessibilityAddTraits(display.selected ? .isSelected : [])
    .accessibilityIdentifier(identifier)
  }

  static func compactButtonWidth(textScale: CGFloat) -> CGFloat {
    38 * textScale
  }

  private var height: CGFloat {
    switch form {
    case .rail: 24 * textScale
    case .chip: display.nameBelow ? Self.mapRowHeight : OverlayBarGeometry.secondRowHeight
    case .counter: Self.headerHeight
    }
  }

  private var resolvedAccessibilityValue: String {
    let unread =
      form == .rail && display.unreadCount > 0
      ? String(localized: "\(display.unreadCount) unread replies") : ""
    return [accessibilityValue, unread].filter { !$0.isEmpty }.joined(separator: ", ")
  }

  private var recording: Bool { display.microphoneOpen && form != .rail }

  /// Recording keeps the circled digits in both layouts; at rest the stacked
  /// counter shows its cards.
  private var stackedFace: Bool { form == .counter && display.stacked && !recording }

  /// The agent count over the cards' corner, in the unread yellow while a
  /// reply waits, so the badge also carries the dot it replaces.
  private var stackedBadge: some View {
    Text(verbatim: display.figure)
      .font(.system(size: Self.badgeSize * textScale, weight: .bold))
      .monospacedDigit()
      .foregroundStyle(display.unread ? palette.desktopBackground.color : palette.primaryText.color)
      .padding(.horizontal, 3 * textScale)
      .frame(minWidth: Self.badgeDiameter * textScale, minHeight: Self.badgeDiameter * textScale)
      .background(
        Capsule(style: .continuous)
          .fill(display.unread ? palette.processingStatus.color : palette.border.color)
      )
      .offset(x: 4 * textScale, y: -4 * textScale)
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
    return display.selected
      ? palette.agentAccent.color.opacity(Self.selectedFillOpacity) : CSColor.hairline(0.06)
  }

  @ViewBuilder
  private var label: some View {
    if form == .chip {
      chipLabel
    } else {
      capsuleLabel
    }
  }

  /// No capsule around a chip: the bar stays slim and the circle alone
  /// carries the channel's marks.
  @ViewBuilder
  private var chipLabel: some View {
    if display.nameBelow {
      VStack(spacing: Self.nameBelowSpacing * textScale) {
        circledDigit(display.figure)
        if let name = display.name { nameText(name) }
      }
      // The circle keeps the row's top, so the map's unread layer finds it.
      .frame(width: display.slotWidth, height: Self.mapRowHeight, alignment: .top)
    } else {
      HStack(spacing: Self.chipGap * textScale) {
        circledDigit(display.figure)
        if let name = display.name { nameText(name) }
      }
      .frame(height: OverlayBarGeometry.secondRowHeight)
    }
  }

  private func nameText(_ name: String) -> some View {
    Text(verbatim: name)
      .font(Self.nameFont(textScale))
      .foregroundStyle(palette.primaryText.color)
      .lineLimit(1)
      .fixedSize()
  }

  private var capsuleLabel: some View {
    Group {
      switch form {
      case .rail:
        HStack(spacing: 3) {
          if let symbol = display.symbol {
            Image(systemName: symbol).font(.system(size: 12 * textScale))
          } else {
            OverlayAgentStatusDot(kind: display.dot ?? .idle, palette: palette)
            figure(display.figure, size: 12)
          }
        }
        .padding(.horizontal, 6 * textScale)
        .frame(width: Self.compactButtonWidth(textScale: textScale))
      case .chip, .counter:
        if stackedFace {
          Image(systemName: Self.stackedCardsSymbol)
            .font(.system(size: Self.nameSize * textScale, weight: .semibold))
            .foregroundStyle(palette.agentAccent.color)
            .frame(width: Self.headerHeight, height: Self.headerHeight)
        } else if display.microphoneOpen {
          // Several recipients overlap here, in mini's Agents slot only (the
          // bar's map never overlaps); the later circle lies on the earlier.
          HStack(spacing: -Self.counterOverlap * textScale) {
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
    .frame(height: height)
    .background(Capsule(style: .continuous).fill(fill))
  }

  /// The digit: drawn here once, whatever the form.
  private func figure(_ text: String, size: CGFloat) -> some View {
    Text(verbatim: text)
      .font(.system(size: size * textScale, weight: .semibold))
      .monospacedDigit()
      .lineLimit(1)
      .fixedSize()
  }

  /// A chip's or the recording counter's digit in its circle. Three marks
  /// stay apart (Monika's bar brief, 2026-10-30): a recipient of the
  /// current utterance wears the violet ring and the violet open-channel
  /// dot, the viewed conversation the muted violet fill, and unread replies
  /// the yellow dot. The opaque base lets overlapping circles lie on one
  /// another cleanly; the dots overlap the circle and add no width.
  private func circledDigit(_ digit: String) -> some View {
    let diameter = Self.digitCircle * textScale
    return figure(digit, size: Self.digitSize)
      .foregroundStyle(recording ? palette.agentAccent.color : palette.primaryText.color)
      .frame(width: diameter, height: diameter)
      .background {
        ZStack {
          Circle().fill(palette.desktopBackground.color)
          if form == .chip && display.selected {
            Circle().fill(palette.agentAccent.color.opacity(Self.selectedFillOpacity))
          }
        }
        .accessibilityHidden(true)
      }
      .overlay {
        Circle()
          .strokeBorder(
            recording ? palette.agentAccent.color : palette.primaryText.color.opacity(0.5),
            lineWidth: recording ? Self.recipientRingWidth : 1
          )
          .accessibilityHidden(true)
      }
      .overlay(alignment: .bottomTrailing) {
        if recording { openChannelDot }
      }
      .overlay(alignment: .topTrailing) {
        if form == .chip && display.unread && display.drawsUnreadDot {
          Self.unreadDot(palette: palette, scale: textScale)
        }
      }
  }

  /// The yellow unread dot on a circle's upper-right rim. A rim in the
  /// panel colour keeps it legible over a neighbour's circle.
  static func unreadDot(palette: OverlayAppearancePalette, scale: CGFloat) -> some View {
    Circle()
      .fill(palette.processingStatus.color)
      .overlay {
        Circle().strokeBorder(palette.desktopBackground.color, lineWidth: openDotRim)
      }
      .frame(width: unreadDotDiameter * scale, height: unreadDotDiameter * scale)
      .offset(x: unreadDotOffset * scale, y: -unreadDotOffset * scale)
      .allowsHitTesting(false)
      .accessibilityHidden(true)
  }

  /// The open channel's dot: the recording violet with a rim in the panel
  /// colour so it reads over the violet circle. It pulses as the recording
  /// light does, only while the pill animates and Reduce Motion is off.
  @ViewBuilder
  private var openChannelDot: some View {
    if display.animates && !reduceMotion {
      TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
        openChannelDotShape.opacity(
          OverlayRecordingLight.pulseOpacity(at: timeline.date.timeIntervalSinceReferenceDate))
      }
    } else {
      openChannelDotShape
    }
  }

  private var openChannelDotShape: some View {
    Circle()
      .fill(palette.agentAccent.color)
      .overlay {
        Circle().strokeBorder(palette.desktopBackground.color, lineWidth: Self.openDotRim)
      }
      .frame(width: Self.openDotDiameter * textScale, height: Self.openDotDiameter * textScale)
      .offset(x: Self.openDotOffset * textScale, y: Self.openDotOffset * textScale)
      .accessibilityHidden(true)
  }
}

// MARK: Header metrics

/// The bar's widths come from the same values the pill draws with, so its
/// layout thresholds are measured, not constants (the approach of
/// `OverlayAgentDrawerLayout.railPillWidth`).
extension OverlayChannelPill {
  /// The header's control diameter: the counter stands as tall as its buttons.
  static let headerHeight: CGFloat = OverlayMicrophoneGlyph.diameter
  /// The bar's one row: a circle with its name under it fits the header row.
  static let mapRowHeight: CGFloat = OverlayHeaderDensity.rowHeight
  /// The name tucks under the circle into the name line's own leading.
  static let nameBelowSpacing: CGFloat = -2
  /// Text scale stops growing the bar's map where its row still holds a
  /// circle and a name; the header row keeps its height at any scale.
  static let headerScaleCeiling: CGFloat = 1.25
  /// A circle and the name beside it in the second row.
  static let chipGap: CGFloat = 4
  static let counterPadding: CGFloat = 5
  /// The recording counter hugs its circled digits; mini, which is as wide
  /// as its content, narrows with it (`DictationOverlayWindow.miniNaturalWidth`).
  static let counterRecordingPadding: CGFloat = 2
  /// How far a recipient's circle lies on the previous one in the counter.
  static let counterOverlap: CGFloat = 5
  static let counterGap: CGFloat = 2
  static let digitCircle: CGFloat = 15
  static let digitSize: CGFloat = 10
  static let nameSize: CGFloat = 11
  /// The recipient's ring: heavier than the resting 1 pt hairline.
  static let recipientRingWidth: CGFloat = 1.5
  /// The viewed conversation's muted violet fill.
  static let selectedFillOpacity: Double = 0.22
  /// The open channel's dot. Its centre sits where the W2 microphone badge's
  /// did (9 pt, 3 pt past the edge), on the circle's bottom-right rim.
  static let openDotDiameter: CGFloat = 7
  static let openDotRim: CGFloat = 1
  /// How far the dot sits past the circle's bottom-right edge.
  static let openDotOffset: CGFloat = 2
  /// The unread dot on the circle's upper-right rim.
  static let unreadDotDiameter: CGFloat = 6
  static let unreadDotOffset: CGFloat = 1
  static let caret = "\u{25BE}"
  /// Three stacked cards: the Intercom group, as in Codex's first vertical
  /// proposal (Maciej, 2026-10-23: "znaczek tych układnych").
  static let stackedCardsSymbol = "rectangle.grid.1x3"
  static let badgeSize: CGFloat = 8
  static let badgeDiameter: CGFloat = 11

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

  /// A name's full width in the map's font.
  @MainActor
  static func nameWidth(_ name: String, scale: CGFloat) -> CGFloat {
    textWidth(name, size: nameSize * scale)
  }

  /// A shortened name keeps at least this many characters before `…`; a
  /// name of one more character or fewer is never shortened (Monika,
  /// 2026-10-30: one letter under a circle "wygląda po prostu źle").
  static let shortenedNameMinimumCharacters = 3

  /// The narrowest a name may become in the map: its first three
  /// characters and `…`, or the whole name when it has four characters or
  /// fewer. Measured in the chip's own font.
  @MainActor
  static func shortestNameWidth(_ name: String, scale: CGFloat) -> CGFloat {
    guard name.count > shortenedNameMinimumCharacters + 1 else {
      return nameWidth(name, scale: scale)
    }
    return nameWidth(
      String(name.prefix(shortenedNameMinimumCharacters)) + "\u{2026}", scale: scale)
  }

  /// A name cut to the width its slot allows: the full name, else its
  /// longest prefix with `…`, never shorter than three characters and `…`,
  /// and a name of four characters or fewer is never cut. The layout never
  /// offers less room than that (`OverlayBarGeometry.minimumNameWidths`); the
  /// hint carries the full name with its digit.
  @MainActor
  static func truncatedName(_ name: String, width: CGFloat, scale: CGFloat) -> String {
    guard name.count > shortenedNameMinimumCharacters + 1,
      nameWidth(name, scale: scale) > width + 0.5
    else { return name }
    var prefix = name
    while prefix.count > shortenedNameMinimumCharacters {
      prefix.removeLast()
      let candidate = prefix + "\u{2026}"
      if nameWidth(candidate, scale: scale) <= width + 0.5 { return candidate }
    }
    return String(name.prefix(shortenedNameMinimumCharacters)) + "\u{2026}"
  }

  /// The counter's width: `4 ▾`, the stacked cards (a header circle; the
  /// badge overhangs it), or the overlapping circled digits of the open
  /// channels.
  @MainActor
  static func counterWidth(
    figure: String, openDigitCount: Int, scale: CGFloat, stacked: Bool = false
  ) -> CGFloat {
    if stacked && openDigitCount == 0 { return headerHeight }
    guard openDigitCount == 0 else {
      let digits = CGFloat(openDigitCount)
      return ceil(
        2 * counterRecordingPadding * scale + digitCircle * scale
          + (digits - 1) * (digitCircle - counterOverlap) * scale)
    }
    return ceil(
      2 * counterPadding * scale
        + textWidth(figure, size: nameSize * scale, monospacedDigits: true)
        + counterGap * scale + textWidth(caret, size: nameSize * scale))
  }
}

/// The rail shows the shared floating hint beside the pill, as the bubbles and
/// header controls do. In mini and compact form the header is the whole 46 pt
/// panel, so the strip's pills use the header's floating hint window.
private struct OverlayChannelPillHint: ViewModifier {
  let form: OverlayChannelPill.Form
  let hint: String
  let palette: OverlayAppearancePalette
  let enabled: Bool

  @ViewBuilder
  func body(content: Content) -> some View {
    if enabled {
      switch form {
      case .rail:
        content.overlayFastHint(hint, palette: palette, leading: true)
      case .chip, .counter:
        content.modifier(OverlayHeaderControlChrome(title: hint, accent: nil, palette: palette))
      }
    } else {
      content
    }
  }
}
