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
    /// A channel of the compact map: the circled digit, and the agent's name
    /// where the strip has room for names (bus intercom spec §13.1).
    case chip
    /// The collapsed map in compact and in mini: the agent count and `▾`, or
    /// the circled digits of the channels the microphone feeds.
    case counter
  }

  struct Display: Equatable {
    /// The channel digit; for the counter, the number of agents.
    let figure: String
    /// The agent's name on a chip that shows it; a digit-only chip, the rail
    /// and the counter carry none.
    var name: String? = nil
    /// The rail's status dot.
    var dot: OverlayAgentDotKind? = nil
    /// The follower is gone: the pill dims as the roster's Disconnected row.
    var disconnected = false
    /// The microphone feeds this channel now (chip and counter forms): the
    /// digit carries the microphone badge and the pill the recording tint.
    var microphoneOpen = false
    /// The counter's circled digits while the microphone is open.
    var openDigits: [String] = []
    var unread = false
    var selected = false
    var enabled = true
    /// False while the panel is hidden or occluded: no pulse keeps the render
    /// loop awake.
    var animates = false
  }

  @Environment(\.accessibilityReduceMotion) private var reduceMotion
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
          if display.unread {
            Circle()
              .fill(palette.processingStatus.color)
              .frame(width: 6 * textScale, height: 6 * textScale)
              .offset(x: 1, y: -1)
              .accessibilityHidden(true)
          }
        }
        .contentShape(Capsule(style: .continuous))
    }
    .buttonStyle(.plain)
    .csFocusOutline(cornerRadius: height / 2)
    .disabled(!display.enabled)
    .opacity(display.enabled ? 1 : 0.35)
    .modifier(OverlayDisconnectedRowDim(disconnected: display.disconnected))
    .modifier(OverlayChannelPillHint(form: form, hint: hint, palette: palette, enabled: showsHint))
    .accessibilityLabel(Text(verbatim: accessibilityText))
    .accessibilityValue(Text(verbatim: accessibilityValue))
    .accessibilityAddTraits(display.selected ? .isSelected : [])
    .accessibilityIdentifier(identifier)
  }

  private var height: CGFloat {
    form == .rail ? 24 * textScale : Self.headerHeight
  }

  private var recording: Bool { display.microphoneOpen && form != .rail }

  /// Recording takes the active-speaker violet, as the roster's open
  /// microphone and the `.agent` light do (`OverlayAgentAudioControls`).
  private var tint: Color {
    recording ? palette.agentAccent.color : palette.primaryText.color
  }

  private var fill: Color {
    if recording {
      return palette.agentAccent.color.opacity(OverlayAgentAudioControls.litGlassOpacity)
    }
    return display.selected ? palette.agentAccent.color.opacity(0.22) : CSColor.hairline(0.06)
  }

  @ViewBuilder
  private var label: some View {
    Group {
      switch form {
      case .rail:
        HStack(spacing: 3) {
          OverlayAgentStatusDot(kind: display.dot ?? .idle, palette: palette)
          figure(display.figure, size: 12)
        }
        .padding(.horizontal, 6 * textScale)
      case .chip:
        HStack(spacing: Self.chipGap * textScale) {
          circledDigit(display.figure)
          if let name = display.name {
            Text(verbatim: name)
              .font(Self.nameFont(textScale))
              .lineLimit(1)
              .truncationMode(.tail)
              .frame(width: Self.nameWidth(name, scale: textScale), alignment: .leading)
          }
        }
        .padding(.horizontal, Self.chipPadding * textScale)
      case .counter:
        if display.microphoneOpen {
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

  /// A chip's or the recording counter's digit in its circle, with the
  /// microphone badge while the microphone feeds the channel (spec §13.3).
  /// The badge overlaps the circle, so it adds no width.
  private func circledDigit(_ digit: String) -> some View {
    figure(digit, size: Self.digitSize)
      .frame(width: Self.digitCircle * textScale, height: Self.digitCircle * textScale)
      .overlay {
        Circle().strokeBorder(tint.opacity(0.5), lineWidth: 1)
          .accessibilityHidden(true)
      }
      .overlay(alignment: .bottomTrailing) {
        if recording { microphoneBadge }
      }
  }

  private var microphoneBadge: some View {
    Image(systemName: "mic.fill")
      .font(.system(size: Self.badgeGlyphSize * textScale, weight: .bold))
      .foregroundStyle(palette.desktopBackground.color)
      .symbolEffect(.pulse, isActive: display.animates && !reduceMotion)
      .frame(width: Self.badgeDiameter * textScale, height: Self.badgeDiameter * textScale)
      .background(Circle().fill(palette.agentAccent.color))
      .offset(x: Self.badgeOffset * textScale, y: Self.badgeOffset * textScale)
      .accessibilityHidden(true)
  }
}

// MARK: Header metrics

/// The compact map's widths come from the same values the pill draws with,
/// so the collapse thresholds are measured, not constants (the approach of
/// `OverlayAgentDrawerLayout.railPillWidth`).
extension OverlayChannelPill {
  /// The header's control diameter: chips stand as tall as its buttons.
  static let headerHeight: CGFloat = OverlayMicrophoneGlyph.diameter
  /// Separate mic, speaker and panel actions share the header pill height.
  static let actionWidth: CGFloat = headerHeight
  static let actionGap: CGFloat = 2
  /// Text scale stops growing the strip's pills where a 22 pt pill still
  /// holds its circled digit; the header row stays 46 pt at any scale.
  static let headerScaleCeiling: CGFloat = 1.25
  static let chipPadding: CGFloat = 6
  static let chipGap: CGFloat = 4
  static let counterPadding: CGFloat = 5
  /// The recording counter hugs its circled digit; mini, which is as wide as
  /// its content, narrows with it (`DictationOverlayWindow.miniNaturalWidth`).
  static let counterRecordingPadding: CGFloat = 2
  static let counterGap: CGFloat = 2
  static let digitCircle: CGFloat = 15
  static let digitSize: CGFloat = 10
  static let nameSize: CGFloat = 11
  static let badgeDiameter: CGFloat = 9
  static let badgeGlyphSize: CGFloat = 5.5
  /// How far the badge sits past the circle's bottom-right edge.
  static let badgeOffset: CGFloat = 3
  /// A long agent name truncates instead of collapsing the whole map.
  static let maxNameWidth: CGFloat = 96
  static let caret = "\u{25BE}"

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

  /// A chip's width: padding, the circled digit and, when the chip shows
  /// one, the name. The microphone badge overlaps the circle and adds none.
  @MainActor
  static func chipWidth(name: String?, scale: CGFloat) -> CGFloat {
    var width = 2 * chipPadding * scale + digitCircle * scale
    if let name { width += chipGap * scale + nameWidth(name, scale: scale) }
    return ceil(width)
  }

  /// The counter's width: `4 ▾`, or the circled digits of the open channels.
  @MainActor
  static func counterWidth(figure: String, openDigitCount: Int, scale: CGFloat) -> CGFloat {
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

/// A sibling of the name button, never a nested hit target. The compact
/// capsule follows the channel pill's native shell and fixed header metrics.
struct OverlayChannelMapActionButton: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  let symbol: String
  let label: String
  var value = ""
  let identifier: String
  let palette: OverlayAppearancePalette
  let scale: CGFloat
  var enabled = true
  var active = false
  var pulses = false
  var pending = false
  var succeeded = false
  var failed = false
  let action: () -> Void

  private var tint: Color {
    if failed { return palette.errorStatus.color }
    if succeeded { return palette.successStatus.color }
    return active ? palette.agentAccent.color : palette.primaryText.color
  }

  var body: some View {
    Button(action: action) {
      Group {
        if pending {
          ProgressView().controlSize(.mini)
        } else {
          Image(systemName: symbol)
            .font(.system(size: 11 * scale, weight: .semibold))
            .symbolEffect(.pulse, isActive: pulses && !reduceMotion)
        }
      }
      .foregroundStyle(tint)
      .frame(width: OverlayChannelPill.actionWidth, height: OverlayChannelPill.headerHeight)
      .background(
        Capsule(style: .continuous)
          .fill(active ? palette.agentAccent.color.opacity(OverlayAgentAudioControls.litGlassOpacity)
            : CSColor.hairline(0.06)))
      .contentShape(Capsule(style: .continuous))
    }
    .buttonStyle(.plain)
    .csFocusOutline(cornerRadius: OverlayChannelPill.headerHeight / 2)
    .disabled(!enabled || pending)
    .opacity(enabled ? 1 : 0.35)
    .accessibilityLabel(Text(verbatim: label))
    .accessibilityValue(Text(verbatim: value))
    .accessibilityIdentifier(identifier)
  }
}
