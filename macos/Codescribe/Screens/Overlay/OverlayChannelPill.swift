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
    /// A channel of the compact map: circled digit and agent name, or
    /// `MIC → 5 Kuba` while the microphone feeds the channel.
    case chip
    /// The collapsed map: the channel count and `▾`, or `MIC → 5`.
    case counter
  }

  struct Display: Equatable {
    /// The channel digit; for the counter, its figure (the channel count, or
    /// the recording digits).
    let figure: String
    /// The agent's name on a chip; the rail and the counter carry none.
    var name: String? = nil
    /// The rail's status dot.
    var dot: OverlayAgentDotKind? = nil
    /// The follower is gone: the pill dims as the roster's Disconnected row.
    var disconnected = false
    /// The microphone feeds this channel now (chip and counter forms).
    var microphoneOpen = false
    /// Channel 0 speaks through the left-facing megaphone, as its row does.
    var broadcast = false
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
  /// The rail passes the roster's text scale; the compact strip passes
  /// `headerScale(_:)` so its 46 pt row keeps its height.
  let textScale: CGFloat
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
    .modifier(OverlayChannelPillHint(form: form, hint: hint, palette: palette))
    .accessibilityLabel(Text(verbatim: accessibilityText))
    .accessibilityValue(Text(verbatim: accessibilityValue))
    .accessibilityAddTraits(display.selected ? .isSelected : [])
    .accessibilityIdentifier(identifier)
  }

  private var height: CGFloat {
    form == .rail ? 24 * textScale : Self.headerHeight
  }

  /// Recording takes the active-speaker violet, as the roster's open
  /// microphone does (`OverlayAgentAudioControls`).
  private var tint: Color {
    display.microphoneOpen && form != .rail
      ? palette.agentAccent.color : palette.primaryText.color
  }

  private var fill: Color {
    if display.microphoneOpen && form != .rail {
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
          figure
        }
        .padding(.horizontal, 6 * textScale)
      case .chip:
        HStack(spacing: Self.chipGap * textScale) {
          if display.microphoneOpen { route }
          figure
            .frame(width: Self.digitCircle * textScale, height: Self.digitCircle * textScale)
            .overlay {
              Circle().strokeBorder(tint.opacity(0.5), lineWidth: 1)
                .accessibilityHidden(true)
            }
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
        HStack(spacing: Self.counterGap * textScale) {
          if display.microphoneOpen {
            route
            figure
          } else {
            figure
            Text(verbatim: Self.caret)
              .font(Self.nameFont(textScale))
              .frame(width: Self.textWidth(Self.caret, size: Self.nameSize * textScale))
          }
        }
        .padding(.horizontal, Self.counterPadding * textScale)
      }
    }
    .foregroundStyle(tint)
    .frame(height: height)
    .background(Capsule(style: .continuous).fill(fill))
  }

  /// The digit: drawn once, whatever the form.
  private var figure: some View {
    let size = form == .chip ? Self.digitSize : form == .counter ? Self.nameSize : 12
    return Text(verbatim: display.figure)
      .font(.system(size: size * textScale, weight: .semibold))
      .monospacedDigit()
      .lineLimit(1)
      .fixedSize()
  }

  /// `MIC →`: the microphone, or channel 0's left-facing megaphone, and the arrow.
  @ViewBuilder
  private var route: some View {
    Image(systemName: display.broadcast ? "megaphone" : "mic.fill")
      .font(.system(size: Self.glyphSize * textScale, weight: .semibold))
      .overlayMirrored(display.broadcast)
      .symbolEffect(.pulse, isActive: display.animates && !reduceMotion)
      .frame(width: Self.glyphWidth * textScale)
      .accessibilityHidden(true)
    Text(verbatim: Self.arrow)
      .font(Self.nameFont(textScale))
      .frame(width: Self.textWidth(Self.arrow, size: Self.nameSize * textScale))
      .accessibilityHidden(true)
  }
}

// MARK: Header metrics

/// The compact map's widths come from the same values the pill draws with,
/// so the collapse threshold is measured, not a constant (the approach of
/// `OverlayAgentDrawerLayout.railPillWidth`).
extension OverlayChannelPill {
  /// The header's control diameter: chips stand as tall as its buttons.
  static let headerHeight: CGFloat = OverlayMicrophoneGlyph.diameter
  /// Text scale stops growing the strip's pills where a 22 pt pill still
  /// holds its circled digit; the header row stays 46 pt at any scale.
  static let headerScaleCeiling: CGFloat = 1.25
  static let chipPadding: CGFloat = 6
  static let chipGap: CGFloat = 4
  static let counterPadding: CGFloat = 5
  static let counterGap: CGFloat = 2
  static let digitCircle: CGFloat = 15
  static let digitSize: CGFloat = 10
  static let nameSize: CGFloat = 11
  static let glyphSize: CGFloat = 9
  static let glyphWidth: CGFloat = 11
  /// A long agent name truncates instead of collapsing the whole map.
  static let maxNameWidth: CGFloat = 96
  static let arrow = "\u{2192}"
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

  @MainActor
  private static func routeWidth(gap: CGFloat, scale: CGFloat) -> CGFloat {
    glyphWidth * scale + gap * scale + textWidth(arrow, size: nameSize * scale) + gap * scale
  }

  /// A chip's width: padding, `MIC →` while recording, the circled digit and
  /// the name.
  @MainActor
  static func chipWidth(name: String, microphoneOpen: Bool, scale: CGFloat) -> CGFloat {
    var width = 2 * chipPadding * scale + digitCircle * scale
    if microphoneOpen { width += routeWidth(gap: chipGap, scale: scale) }
    width += chipGap * scale + nameWidth(name, scale: scale)
    return ceil(width)
  }

  /// The counter's width: `4 ▾`, or `MIC → 5`.
  @MainActor
  static func counterWidth(figure: String, microphoneOpen: Bool, scale: CGFloat) -> CGFloat {
    var width =
      2 * counterPadding * scale
      + textWidth(figure, size: nameSize * scale, monospacedDigits: true)
    if microphoneOpen {
      width += routeWidth(gap: counterGap, scale: scale)
    } else {
      width += counterGap * scale + textWidth(caret, size: nameSize * scale)
    }
    return ceil(width)
  }
}

/// The rail shows the shared floating hint beside the pill, as the bubbles and
/// header controls do. In mini and compact form the header is the whole 46 pt
/// panel, so the strip's pills use the header's floating hint window.
private struct OverlayChannelPillHint: ViewModifier {
  let form: OverlayChannelPill.Form
  let hint: String
  let palette: OverlayAppearancePalette

  @ViewBuilder
  func body(content: Content) -> some View {
    switch form {
    case .rail:
      content.overlayFastHint(hint, palette: palette, leading: true)
    case .chip, .counter:
      content.modifier(OverlayHeaderControlChrome(title: hint, accent: nil, palette: palette))
    }
  }
}
