import SwiftUI

/// The agent channel in one fixed header slot.
///
/// Budget: "to ma być 1 char budżetu" (Founder, quoted in the Codex handoff,
/// Annex A3/A4, 2026-09-29). The state → glyph table below is the Codex root's
/// PROPOSAL, not a Founder decision; it is kept in one place so a different
/// verdict is a one-table change.
///
/// Every state is a pure function of the bus projection
/// (`OverlayChannelDelivery`): no clock, no click, no local counter feeds it.
/// ␆ lights only when a `codescribe.agent-ack.v1` row names the current
/// delivery, channel and agent — the row `app/presentation/agent_ack.rs`
/// appends after the agent ran `bus-demux.py --ack`. A durable marker alone,
/// another channel's ack, or elapsed time never produce it.
enum OverlayAgentGlyph: CaseIterable, Equatable, Sendable {
  /// ❖ bound agent, nothing sealed for it yet.
  case attached
  /// ❖ in the listening hue: the microphone feeds this agent's channel now.
  case open
  /// Roster spinner: a sealed utterance is out and no receipt names it yet.
  case awaitingReceipt
  /// ␆ the agent confirmed receipt of the newest delivery.
  case acknowledged
  /// ⚠︎ the channel status could not be read; last state retained.
  case unavailable

  /// Worst news wins the single slot: an unreadable status, then a delivery
  /// still waiting, then a confirmed one, then plain attachment.
  static func resolve(channels: [OverlayChannelDelivery], unavailable: Bool)
    -> OverlayAgentGlyph?
  {
    if unavailable { return .unavailable }
    guard !channels.isEmpty else { return nil }
    let stages = channels.compactMap(\.stage)
    if stages.contains(where: { $0 != .received }) { return .awaitingReceipt }
    if !stages.isEmpty { return .acknowledged }
    return channels.contains(where: \.isOpen) ? .open : .attached
  }

  var character: String? {
    switch self {
    case .attached, .open: "\u{2756}"
    case .awaitingReceipt: nil
    case .acknowledged: "\u{2406}"
    // U+FE0E pins the text presentation: a monochrome sign, never the emoji.
    case .unavailable: "\u{26A0}\u{FE0E}"
    }
  }

  /// VoiceOver label and tooltip lead; one sentence per state.
  var label: String {
    switch self {
    case .attached: String(localized: "Agent attached")
    case .open: String(localized: "Agent channel open")
    case .awaitingReceipt: String(localized: "Waiting for the agent to confirm receipt")
    case .acknowledged: String(localized: "Agent confirmed receipt")
    case .unavailable: String(localized: "Agent channel status unavailable")
    }
  }

  func tone(in palette: OverlayAppearancePalette) -> OverlayColorToken {
    switch self {
    case .attached: palette.mutedText
    case .open: palette.listeningStatus
    case .awaitingReceipt, .unavailable: palette.processingStatus
    case .acknowledged: palette.successStatus
    }
  }

  /// One fixed slot sized for the widest glyph, so a state change never moves
  /// the recording light, the timer or Stop.
  static let slotSize = CGSize(width: 18, height: 22)
}

/// Display-only projection of the controller's per-digit roster snapshot.
struct OverlayChannelHudProjection: Equatable {
  let open: Bool
  let loud: Bool
  let autosealDeadline: Date?
  /// Nil means the controller made no liveness claim.
  let followerAlive: Bool?
  var provider: String? = nil
  var providerSessionID: String? = nil
  /// Whether the live follower's lease names a vc-frame pane. False means a
  /// focus request has nowhere to land; nil means no claim.
  var frameDetected: Bool? = nil
  /// Whether the live follower's lease names an iTerm2 or Terminal.app tab
  /// the follower can bring to front; nil means no claim.
  var terminalDetected: Bool? = nil
  /// `codex://threads/<id>` of a Codex agent's thread in the Codex app,
  /// known whether or not its follower is alive; nil means no thread.
  var codexThreadUrl: String? = nil
  /// Whether the local Codex store holds that thread, so the link opens;
  /// false is a Codex Cloud thread.
  var codexThreadLocal: Bool? = nil
  /// When the live follower's oldest unacknowledged delivery was emitted.
  /// The second liveness layer: a reader process can heartbeat while the
  /// conversation behind it reads nothing (Founder, 2026-10-14).
  var unreadSince: Date? = nil

  /// Alive by heartbeat, yet nothing read for longer than the stall horizon.
  func isStalled(now: Date = Date()) -> Bool {
    guard followerAlive == true, let unreadSince else { return false }
    return now.timeIntervalSince(unreadSince) >= OverlayState.followerStallHorizon
  }
}

/// Shared by the header and roster; animation never owns delivery state.
struct OverlayAgentStatusMark: View {
  let reduceMotion: Bool
  let glyph: OverlayAgentGlyph
  let palette: OverlayAppearancePalette
  let animates: Bool
  let fontSize: CGFloat

  var showsSpinner: Bool { glyph == .awaitingReceipt }

  var rotates: Bool {
    showsSpinner && animates && !reduceMotion
  }

  func rotation(at time: TimeInterval) -> Angle {
    .degrees(
      rotates ? time.truncatingRemainder(dividingBy: 1) * 360 : 0)
  }

  var body: some View {
    Group {
      if showsSpinner {
        if rotates {
          TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
            spinner.rotationEffect(
              rotation(at: timeline.date.timeIntervalSinceReferenceDate)
            )
          }
        } else {
          spinner
        }
      } else if let character = glyph.character {
        // A status sign, not code: the system face carries it (Founder's mono
        // policy, 2026-10-18). The slot below already fixes its box.
        Text(character)
          .font(.system(size: fontSize, weight: .medium))
          .fixedSize()
      }
    }
    .foregroundStyle(glyph.tone(in: palette).color)
    .frame(width: OverlayAgentGlyph.slotSize.width, height: OverlayAgentGlyph.slotSize.height)
  }

  private var spinner: some View {
    Circle()
      .trim(from: 0.08, to: 0.82)
      .stroke(style: StrokeStyle(lineWidth: 1.5, lineCap: .round))
      .frame(width: fontSize, height: fontSize)
      .accessibilityIdentifier("overlay-agent-spinner")
  }
}

/// The one horizontal flip the overlay's megaphones share: the header's agent
/// button and the broadcast row both face left (Maciej, 2026-10-20).
struct OverlayHorizontalMirror: ViewModifier {
  let mirrored: Bool

  func body(content: Content) -> some View {
    content.scaleEffect(x: mirrored ? -1 : 1, y: 1)
  }
}

extension View {
  func overlayMirrored(_ mirrored: Bool = true) -> some View {
    modifier(OverlayHorizontalMirror(mirrored: mirrored))
  }
}

/// Compact circular glyph for the widget header; capture stays with its owner.
struct OverlayMicrophoneGlyph: View {
  @Environment(\.displayScale) private var displayScale
  private let mark: Mark
  let tint: Color
  var plateOpacity: Double = 0.12
  /// Flips a symbol horizontally; only the agent megaphone, which faces left
  /// toward the wordmark (Maciej, 2026-10-20).
  var mirrored = false
  static let diameter: CGFloat = 22

  private enum Mark {
    case symbol(String)
    /// One text sign (`OverlayHeaderGlyph`), sized to sit like the symbols.
    case character(String, pointSize: CGFloat)
  }

  init(symbol: String, tint: Color, plateOpacity: Double = 0.12, mirrored: Bool = false) {
    mark = .symbol(symbol)
    self.tint = tint
    self.plateOpacity = plateOpacity
    self.mirrored = mirrored
  }

  init(character: String, tint: Color, plateOpacity: Double = 0.12) {
    mark = .character(character, pointSize: OverlayHeaderGlyph.pointSize(character))
    self.tint = tint
    self.plateOpacity = plateOpacity
  }

  var body: some View {
    markView
      .foregroundStyle(tint)
      .frame(width: Self.diameter, height: Self.diameter)
      .background { Circle().fill(tint.opacity(plateOpacity)) }
      .overlay {
        Circle()
          .strokeBorder(tint.opacity(0.42), lineWidth: 1 / max(displayScale, 1))
          .accessibilityHidden(true)
      }
  }

  @ViewBuilder
  private var markView: some View {
    switch mark {
    case .symbol(let symbol):
      Image(systemName: symbol)
        .font(.system(size: 9, weight: .semibold))
        .overlayMirrored(mirrored)
    case .character(let character, let pointSize):
      Text(verbatim: character)
        .font(.system(size: pointSize))
        .fixedSize()
    }
  }
}

/// A bound channel whose reader process is gone keeps its row, greyed and
/// faded, so the roster shows at a glance who cannot hear (Maciej,
/// 2026-10-17: "takie sesje na starcie dawać od razu wyszarzonymi"). The
/// audio pair sleeps; the cross and reception policy remain actionable.
struct OverlayDisconnectedRowDim: ViewModifier {
  static let opacity: Double = 0.45
  let disconnected: Bool

  func body(content: Content) -> some View {
    content
      .opacity(disconnected ? Self.opacity : 1)
      .grayscale(disconnected ? 1 : 0)
  }
}

/// The two pane actions. The canvas edge mounts its own control; the roster
/// header mounts only its own fold. Both use the same state commands.
struct OverlayAgentsSidebarControls {
  /// The conversation, or the transcription, is on screen beside the list.
  var canvasShown: Bool
  var canvasIsTranscription: Bool
  var onToggleCanvas: () -> Void
  var onFoldList: () -> Void

  var canvasTitle: String {
    switch (canvasShown, canvasIsTranscription) {
    case (true, true): String(localized: "Hide transcription")
    case (true, false): String(localized: "Hide conversation")
    case (false, true): String(localized: "Show transcription")
    case (false, false): String(localized: "Show conversation")
    }
  }
}

/// A corner button of the agents list, blended into the background (Maciej,
/// 2026-10-19: "wtopione w tło"): a muted glyph with the rows' hover wash.
struct OverlaySidebarToggleButton: View {
  let symbol: String
  let title: String
  let identifier: String
  let palette: OverlayAppearancePalette
  let textScale: CGFloat
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Image(systemName: symbol)
        .font(.system(size: 13 * textScale))
        .frame(width: 22 * textScale, height: 22 * textScale)
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .focusable(false)
    .foregroundStyle(palette.mutedText.color)
    .modifier(OverlayRowHoverHighlight(palette: palette, cornerRadius: 6))
    .accessibilityLabel(Text(verbatim: title))
    .accessibilityHint(Text(verbatim: title))
    .accessibilityIdentifier(identifier)
  }
}

/// The drawer's clickable rows answer the pointer the way the tray's plain
/// rows do: the whole row is the hover target, a raised wash appears on
/// enter, and leave clears it. The wash does not hit-test, so the row's own
/// buttons keep clicks, audio, and selection. Icons keep their resting color.
/// `outset` lets a row without its own padding keep its text clear of the
/// wash's edge.
struct OverlayRowHoverHighlight: ViewModifier {
  let palette: OverlayAppearancePalette
  var cornerRadius: CGFloat = CSRadius.chip
  var outset: CGFloat = 0
  var enabled = true
  @State private var hovered = false

  /// A veil, not a surface: `surfaceRaised` lifts dark glass with 5 % white
  /// but paints paper with 80 % of it, which is the milky blob the Founder
  /// saw in the drawer (2026-10-18). `hairline` keeps the same white veil on
  /// ink and shades paper instead of bleaching it. Half the strength of
  /// `OverlayRosterRowSelection`, so pointing at a row never reads as being
  /// on it. The clear branch still names the row's canvas so a disabled row
  /// stays on its palette.
  private var wash: Color {
    guard hovered, enabled else { return palette.desktopBackground.color.opacity(0) }
    return CSColor.hairline(0.05)
  }

  func body(content: Content) -> some View {
    content
      .background {
        RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
          .fill(wash)
          .padding(.horizontal, -outset)
          .allowsHitTesting(false)
          .accessibilityHidden(true)
      }
      .contentShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
      .background {
        OverlayMenuCursorRegion { hovered = enabled && $0 }
      }
  }
}

/// One mark for "this is the open conversation", worn by every roster row:
/// the shared channel and the agents under it, in the drawer. Before the
/// Founder's 2026-10-18 review the roster carried two — a grey wash on an
/// agent row, an accent wash on "0 · All" — and both read as milky glass.
/// The mark is a slim accent bar on the leading edge, where no other row
/// chrome lives, over a veil only a step deeper than hover: the bar says
/// which row, the veil says the whole row. `OverlayGestureFocusRing` keeps
/// the full accent outline, so the middle-click chooser's focus and the open
/// conversation stay two different marks even on the same row.
struct OverlayRosterRowSelection: ViewModifier {
  let palette: OverlayAppearancePalette
  let selected: Bool
  var cornerRadius: CGFloat = CSRadius.chip

  func body(content: Content) -> some View {
    content
      .background {
        if selected {
          RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
            .fill(CSColor.hairline(0.09))
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
      }
      .overlay(alignment: .leading) {
        if selected {
          Capsule()
            .fill(palette.agentAccent.color)
            .frame(width: 3)
            .padding(.vertical, 3)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
      }
  }
}

/// The middle-click chooser's focus (Maciej, 2026-10-17 12:19Z): an accent
/// ring on the focused row, and the pointer entering a row moves the focus
/// there. Hover rides the same tracking region as the row wash, so it answers
/// while Codescribe is not the active app.
struct OverlayGestureFocusRing<RingShape: InsettableShape>: ViewModifier {
  let focused: Bool
  let shape: RingShape
  let palette: OverlayAppearancePalette
  let targetID: String?
  let onHover: ((String) -> Void)?

  func body(content: Content) -> some View {
    content
      .overlay {
        if focused {
          shape
            .strokeBorder(palette.agentAccent.color, lineWidth: 2)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
      }
      .background {
        if let targetID, let onHover {
          OverlayMenuCursorRegion { inside in
            if inside { onHover(targetID) }
          }
        }
      }
  }
}

/// The system owns button material, contrast and pointer feedback alongside the composer.
/// `chrome: false` keeps the hit target and drops the glass: the Intercom paints
/// the same glyphs straight onto its fiszka (Maciej, 2026-10-23: "ta sama
/// kolejność to nie te same okrągłe pile — robisz to prosto na powierzchni").
struct OverlayAgentControlStyle: ViewModifier {
  var diameter: CGFloat? = nil
  var chrome = true
  @ViewBuilder
  func body(content: Content) -> some View {
    if !chrome {
      content.buttonStyle(OverlayFlatControlButtonStyle(diameter: diameter ?? 22))
    } else if let diameter {
      content.buttonStyle(OverlaySizedAudioButtonStyle(diameter: diameter))
    } else if #available(macOS 26.0, *) {
      content.buttonStyle(.glass).buttonBorderShape(.circle).controlSize(.regular)
    } else {
      content.buttonStyle(.bordered).buttonBorderShape(.circle).controlSize(.regular)
    }
  }
}

private struct OverlaySizedAudioButtonStyle: ButtonStyle {
  let diameter: CGFloat
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .frame(width: diameter, height: diameter)
      .contentShape(Circle())
      .modifier(OverlayControlGlass(cornerRadius: diameter / 2))
      .opacity(configuration.isPressed ? 0.7 : 1)
  }
}

/// Sets a glyph's ink only when it has one of its own; nil inherits.
struct OverlayGlyphInk: ViewModifier {
  let ink: AnyShapeStyle?
  @ViewBuilder
  func body(content: Content) -> some View {
    if let ink { content.foregroundStyle(ink) } else { content }
  }
}

/// Audio and reception controls share one disabled-aware native hover owner.
/// Off rests in muted ink; on keeps its canonical accent even without motion.
/// The button supplies ink so an inner glyph cannot defeat hover feedback.
private struct OverlayAudioControlInk: ViewModifier {
  let palette: OverlayAppearancePalette
  let lit: Color?
  var unlitOpacity: Double = 1

  func body(content: Content) -> some View {
    // The resolved primary ink darkens a light canvas and brightens a dark
    // one. Keep a lit control's hue while giving it its own hover step.
    let hover =
      lit.map { ink in
        let color = NSColor(ink)
        return Color(
          nsColor: color.blended(withFraction: 0.3, of: palette.primaryText.nsColor) ?? color)
      } ?? palette.primaryText.color
    content.conversationActionHover(
      restingColor: lit ?? palette.mutedText.color.opacity(unlitOpacity),
      hoverColor: hover)
  }
}

/// The same square hit target without a surface of its own: the host's
/// capsule is the surface.
private struct OverlayFlatControlButtonStyle: ButtonStyle {
  let diameter: CGFloat
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .frame(width: diameter, height: diameter)
      .contentShape(Rectangle())
      .opacity(configuration.isPressed ? 0.6 : 1)
  }
}

/// Identical controls in the roster and conversation; neither owns capture or playback.
struct OverlayAgentAudioControls: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var renderVisible = false
  let open: Bool
  let muted: Bool?
  let microphoneEnabled: Bool
  let playbackEnabled: Bool
  let palette: OverlayAppearancePalette
  let onMicrophone: () -> Void
  let onPlayback: () -> Void
  var broadcast = false
  /// Founder, 2026-10-14: "Chciałbym wiedzieć kto gada, żeby był jakiś
  /// indikator na głośniku." The speaker of the agent whose reply is playing
  /// turns violet and animates its waves.
  var speaking = false
  /// All keeps a matched pair, one third more prominent than agent controls;
  /// a surface with one control height (the Intercom bar) opts out.
  var broadcastEmphasis = true
  /// Glass circles in the roster and the conversation header; the Intercom
  /// fiszka is itself the surface and paints the pair flat on it.
  var chrome = true
  /// The conversation header supplies its measured base height; All scales
  /// both controls together from that base.
  var diameter: CGFloat? = nil
  /// A narrow Intercom row has one place for the pair (Maciej, 2026-10-23:
  /// "może być na jednym miejscu z mikrofonem"): the microphone, which
  /// yields to the speaker while the agent talks so it can be muted.
  var shared = false

  private var speakingNow: Bool { speaking && muted == false }

  /// An open microphone keeps the shared place: that take is still running.
  static func sharedShowsSpeaker(speaking: Bool, muted: Bool?, open: Bool) -> Bool {
    speaking && muted == false && !open
  }

  /// Symbol activity follows the existing open input and this actual host.
  /// Reduced motion and hidden windows retain the colored open-state glyph.
  static func microphoneActivityActive(open: Bool, renderVisible: Bool, reduceMotion: Bool) -> Bool
  {
    open && renderVisible && !reduceMotion
  }
  private var showsSpeaker: Bool {
    !shared || Self.sharedShowsSpeaker(speaking: speaking, muted: muted, open: open)
  }
  private var showsMicrophone: Bool {
    !shared || !Self.sharedShowsSpeaker(speaking: speaking, muted: muted, open: open)
  }

  static let broadcastScale: CGFloat = 1.33
  static let controlFrame: CGFloat = 16
  private var controlScale: CGFloat { broadcast && broadcastEmphasis ? Self.broadcastScale : 1 }
  private var glyphSize: CGFloat {
    (diameter.map { min(24, max(12, $0 * 0.42)) } ?? 12) * controlScale
  }
  /// Flat Intercom slots are the full hit square. Glass roster slots stay the
  /// small glyph box and let the chrome frame center them.
  private var glyphBox: CGFloat {
    let base = chrome ? Self.controlFrame : (diameter ?? Self.controlFrame)
    return base * controlScale
  }

  /// Keep the flat pair close without pressing the symbols against each other.
  /// At the usual 30pt hit square, each symbol retreats 4pt from the shared edge.
  private var flatPairInset: CGFloat {
    chrome || shared ? 0 : glyphBox * (4.0 / 30.0)
  }

  /// The broadcast row speaks through a left-facing outline megaphone; agent
  /// rows wear the live-speech glyph, a microphone with a waveform (Maciej,
  /// 2026-10-23: "to jest ewidentnie symbol dyktowania live"). The plain
  /// microphone is the composer's dictation verb. Open state takes the same
  /// violet and native layer activity as a speaker that is playing a reply.
  static let liveSpeechSymbol = "waveform.badge.mic"

  var microphoneSymbol: String {
    broadcast ? "megaphone" : Self.liveSpeechSymbol
  }

  /// Recording shares the appearance-aware active-speaker accent.
  private var recordingAccent: Color { palette.agentAccent.color }

  /// Founder, 2026-10-15: the lit speaker swallowed its glyph, because the
  /// glass took the whole accent and so did the symbol; "Wyraźne, ale nie
  /// wyraziste". A lit control washes its glass with a pastel of the colour
  /// and keeps the glyph in the whole colour.
  static let litGlassOpacity = 0.3

  static func litGlass(_ color: Color?) -> Color? {
    color?.opacity(litGlassOpacity)
  }

  private var microphoneLabel: String {
    if broadcast {
      return open
        ? String(localized: "Stop speaking to everyone (Control–0)")
        : String(localized: "Speak to everyone (Control–0)")
    }
    return open ? String(localized: "Stop speaking to agent") : String(localized: "Speak to agent")
  }
  private var microphoneHint: String {
    microphoneEnabled
      ? microphoneLabel
      : String(localized: "Microphone unavailable", comment: "Disabled agent microphone control")
  }
  private var playbackLabel: String {
    if muted == nil { return String(localized: "Playback status unavailable") }
    if broadcast {
      return muted == true
        ? String(localized: "Unmute replies on all channels")
        : String(localized: "Mute replies on all channels")
    }
    return muted == true
      ? String(localized: "Unmute agent replies") : String(localized: "Mute agent replies")
  }
  /// The sentence the old system tooltip showed, including "Speaking" while a reply plays.
  private var speakerHint: String {
    speakingNow ? String(localized: "Speaking") + ". " + playbackLabel : playbackLabel
  }

  /// Names a speaker that cannot run. A missing mute state keeps the status
  /// sentence; a refused control says the action is unavailable.
  private var speakerAccessibilityHint: String {
    if muted == nil { return String(localized: "Playback status unavailable") }
    if !playbackEnabled {
      return String(
        localized: "Playback unavailable", comment: "Disabled agent speaker control")
    }
    return speakerHint
  }

  private var playbackValue: String {
    if muted == nil { return String(localized: "Playback status unavailable") }
    if broadcast {
      return muted == true
        ? String(localized: "All channels muted")
        : String(localized: "Global playback on")
    }
    if speakingNow { return String(localized: "Speaking") }
    return muted == true
      ? String(localized: "Automatic playback muted")
      : String(localized: "Automatic playback on")
  }

  var body: some View {
    // Glass rows keep 6pt between circles. Flat hit squares stay 2pt apart;
    // the inner glyph insets leave 10pt between the symbols at the base size.
    HStack(spacing: chrome ? 6 : 2) {
      if showsMicrophone {
        Button(action: onMicrophone) {
          Image(systemName: microphoneSymbol)
            .overlayMirrored(broadcast)
            .font(.system(size: glyphSize, weight: .semibold))
            .symbolEffect(
              .pulse,
              isActive: Self.microphoneActivityActive(
                open: open, renderVisible: renderVisible, reduceMotion: reduceMotion)
            )
            .padding(.trailing, flatPairInset)
            .frame(
              width: glyphBox, height: glyphBox,
              alignment: chrome ? .center : .trailing)
        }
        .modifier(OverlayAudioControlInk(palette: palette, lit: open ? recordingAccent : nil))
        .tint(Self.litGlass(open ? recordingAccent : nil))
        .disabled(!microphoneEnabled)
        .overlayFastHint(microphoneHint, palette: palette)
        .accessibilityLabel(Text(verbatim: microphoneLabel))
        .accessibilityValue(
          Text(
            verbatim: open
              ? String(localized: "Microphone active") : String(localized: "Microphone off"))
        )
        .accessibilityHint(Text(verbatim: microphoneHint))
        .accessibilityIdentifier("overlay-agent-microphone")
      }

      if showsSpeaker {
        Button(action: onPlayback) {
          Image(
            systemName: muted == nil
              ? "speaker.badge.exclamationmark"
              : muted == true
                ? "speaker.slash.fill" : speakingNow ? "speaker.wave.3.fill" : "speaker.wave.2"
          )
          .font(.system(size: glyphSize, weight: .semibold))
          .symbolEffect(.variableColor.iterative, isActive: speakingNow && !reduceMotion)
          .padding(.leading, flatPairInset)
          .frame(
            width: glyphBox, height: glyphBox,
            alignment: chrome ? .center : .leading)
        }
        .modifier(
          OverlayAudioControlInk(
            palette: palette, lit: muted == false ? palette.agentAccent.color : nil)
        )
        .tint(Self.litGlass(speakingNow ? palette.agentAccent.color : nil))
        .disabled(!playbackEnabled || muted == nil)
        .overlayFastHint(
          playbackEnabled ? playbackLabel : speakerAccessibilityHint, palette: palette
        )
        .accessibilityLabel(Text(verbatim: playbackLabel))
        .accessibilityValue(Text(verbatim: playbackValue))
        .accessibilityHint(Text(verbatim: speakerAccessibilityHint))
        .accessibilityIdentifier("overlay-agent-speaker")
      }
    }
    .modifier(
      OverlayAgentControlStyle(diameter: diameter.map { $0 * controlScale }, chrome: chrome)
    )
    // Founder, 2026-10-15: an I-beam showed between and over these buttons.
    // They float above message and transcript text, whose tracking set the
    // text cursor; the pair and its gap claim the arrow like the drawer does.
    .background { OverlayMenuCursorRegion() }
    .background {
      OverlayRenderVisibility(
        onHidden: { renderVisible = false }, onChange: { renderVisible = $0 }
      )
      .allowsHitTesting(false)
      .accessibilityHidden(true)
    }
  }
}

/// A rendered command carries its real availability and existing owner callback.
/// It never owns a second conversation, recorder or preference.
struct OverlayAgentPillMenuAction {
  let title: String
  let symbol: String
  let identifier: String
  let enabled: Bool
  let perform: () -> Void
}

struct OverlayAgentPillMenuActions {
  let write: OverlayAgentPillMenuAction
  let microphone: OverlayAgentPillMenuAction?
  let playback: OverlayAgentPillMenuAction?
  let showInAgents: OverlayAgentPillMenuAction?
  let focus: OverlayAgentPillMenuAction?
  let archive: OverlayAgentPillMenuAction?
}

/// The one owner of reception changes (Copilot on #183). The marks, the pill
/// menu and the Intercom each kept their own pending flag and built the next
/// policy from the roster's cached receipt, which the poll refreshes only
/// every 500 ms: the moon and then the antenna inside that window wrote `.off`
/// over the lit moon. A change now starts from the latest read-back, holds
/// every surface for that agent until the helper's new receipt is read back,
/// and a poll that began before the write cannot paint the old receipt over it.
@MainActor
@Observable
final class OverlayBroadcastPolicyLedger {
  static let shared = OverlayBroadcastPolicyLedger()

  private(set) var pending: Set<AgentPlaybackIdentity> = []
  /// Receipts read back after this app's own writes, shown until a poll that
  /// started after the last write reports.
  private(set) var readBacks: [AgentPlaybackIdentity: AgentBroadcastPolicy] = [:]
  @ObservationIgnored private(set) var revision: UInt64 = 0
  @ObservationIgnored private let write:
    @MainActor (AgentBroadcastPolicy, AgentPlaybackIdentity) async throws -> Void
  @ObservationIgnored private let readBack:
    @MainActor (AgentPlaybackIdentity) async -> AgentBroadcastPolicy?

  init(
    write:
      @escaping @MainActor (AgentBroadcastPolicy, AgentPlaybackIdentity) async throws ->
      Void = { policy, identity in
        try await OverlayBroadcastPolicyMenu.setPolicy(policy, for: identity)
      },
    readBack: @escaping @MainActor (AgentPlaybackIdentity) async -> AgentBroadcastPolicy? = {
      identity in
      await RealAgentBridgeInstaller.broadcastPolicySnapshot(for: [identity])[identity]
    }
  ) {
    self.write = write
    self.readBack = readBack
  }

  func policy(for identity: AgentPlaybackIdentity, polled: AgentBroadcastPolicy)
    -> AgentBroadcastPolicy
  {
    readBacks[identity] ?? polled
  }

  func isPending(_ identity: AgentPlaybackIdentity?) -> Bool {
    identity.map { pending.contains($0) } ?? false
  }

  /// Edits one axis of the latest policy. A second change while one is in
  /// flight for the same agent, or an edit that changes nothing, writes
  /// nothing.
  func change(
    _ identity: AgentPlaybackIdentity, polled: AgentBroadcastPolicy,
    _ edit: (inout AgentBroadcastPolicy) -> Void
  ) async throws {
    guard !pending.contains(identity) else { return }
    let base = policy(for: identity, polled: polled)
    var next = base
    edit(&next)
    guard next != base else { return }
    pending.insert(identity)
    revision &+= 1
    defer {
      pending.remove(identity)
      revision &+= 1
    }
    try await write(next, identity)
    // Published before the hold lifts, so the next click starts from it.
    readBacks[identity] = await readBack(identity) ?? next
  }

  /// A poll that started at `revision` may paint its receipts only when no
  /// change began or ended meanwhile; it is then newer than every read-back.
  func admitsPoll(startedAt revision: UInt64) -> Bool {
    guard revision == self.revision, pending.isEmpty else { return false }
    if !readBacks.isEmpty { readBacks = [:] }
    return true
  }
}

/// Wake policy lives on the agent's own row (Maciej, 2026-10-17, "Słowo"):
/// a right-click context menu with a checkmark replaces the disclosure list
/// that used to hang under 0 · All. A single member gives the row its own
/// actions; the 0 · All row carries every member as a submenu, so nine or
/// fifteen agents never need a second column. The command path is unchanged.
struct OverlayBroadcastPolicyMenu: ViewModifier {
  let members: [OverlayChannelDelivery]
  let palette: OverlayAppearancePalette
  let enabled: Bool
  var errorAlignment: HorizontalAlignment = .leading
  var agentActions: OverlayAgentPillMenuActions? = nil
  var showsInlineError = true
  var showsButton = false
  var focusState: OverlayAgentPanelFocusState? = nil
  @State private var commandError: String?

  @MainActor
  static func setPolicy(
    _ policy: AgentBroadcastPolicy, for identity: AgentPlaybackIdentity,
    installer: RealAgentBridgeInstaller = RealAgentBridgeInstaller()
  ) async throws {
    try await RealAgentBridgeInstaller.setBroadcastPolicy(
      policy, for: identity, installer: installer)
  }

  func body(content: Content) -> some View {
    if members.isEmpty {
      content
    } else {
      VStack(alignment: errorAlignment, spacing: 2) {
        if showsButton {
          Menu { menuContents } label: {
            Image(systemName: "ellipsis")
              .font(.system(size: 12, weight: .semibold))
              .frame(width: 16, height: 16)
              .overlay(alignment: .topTrailing) {
                if let focusState {
                  if focusState.isRequesting {
                    ProgressView().controlSize(.mini)
                      .scaleEffect(0.6)
                      .offset(x: 7, y: -5)
                  } else {
                    Image(systemName: focusState.isFailed ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                      .font(.system(size: 8, weight: .semibold))
                      .foregroundStyle(
                        focusState.isFailed ? palette.errorStatus.color : palette.successStatus.color)
                      .offset(x: 5, y: -4)
                  }
                }
              }
          }
          .menuStyle(.borderlessButton)
          .fixedSize()
          .accessibilityLabel(Text("Agent actions"))
          .accessibilityValue(Text(verbatim: focusState?.statusText ?? ""))
          .help(Text(verbatim: focusState?.statusText ?? String(localized: "Agent actions")))
        } else {
          content.contextMenu { menuContents }
        }
        if let commandError, showsInlineError {
          Text(verbatim: commandError)
            .foregroundStyle(palette.errorStatus.color)
            .accessibilityIdentifier("overlay-broadcast-policy-error")
        }
      }
    }
  }

  @ViewBuilder
  private var menuContents: some View {
    if let commandError, !showsInlineError {
      Text(verbatim: commandError)
      Divider()
    }
    if members.count == 1, let member = members.first {
      if let agentActions {
        agentCommands(agentActions)
        Divider()
      }
      policyActions(for: member)
      if let archive = agentActions?.archive {
        Divider()
        agentCommand(archive, role: .destructive)
      }
    } else {
      ForEach(members) { member in
        Menu {
          policyActions(for: member)
        } label: {
          Label {
            Text(verbatim: "\(member.channel) · \(member.agent)")
          } icon: {
            Image(systemName: member.broadcastPolicy.statusSymbol)
          }
        }
      }
    }
  }

  @ViewBuilder
  private func agentCommands(_ actions: OverlayAgentPillMenuActions) -> some View {
    agentCommand(actions.write)
    if let microphone = actions.microphone { agentCommand(microphone) }
    if let playback = actions.playback { agentCommand(playback) }
    if let showInAgents = actions.showInAgents { agentCommand(showInAgents) }
    if let focus = actions.focus { agentCommand(focus) }
  }

  private func agentCommand(
    _ command: OverlayAgentPillMenuAction, role: ButtonRole? = nil
  ) -> some View {
    Button(role: role, action: command.perform) {
      Label(command.title, systemImage: command.symbol)
    }
    .disabled(!command.enabled)
    .accessibilityIdentifier(command.identifier)
  }

  @ViewBuilder
  private func policyActions(for channel: OverlayChannelDelivery) -> some View {
    // Channel 0 is membership; Do not disturb is the moon. Two axes, each its
    // own box, neither greyed by the other (Maciej, 2026-10-23: the moon must
    // light "w dowolnym momencie, nie tylko jeśli kanał 0 jest wyłączony").
    let locked =
      !enabled || channel.broadcastIdentity == nil
      || OverlayBroadcastPolicyLedger.shared.isPending(channel.broadcastIdentity)
    Toggle(
      isOn: Binding(
        get: { channel.broadcastPolicy.member },
        set: { member in apply(to: channel) { $0.member = member } }
      )
    ) {
      Text("Channel 0", comment: "Pill context menu: membership in channel-zero broadcasts")
    }
    .disabled(locked)
    Toggle(
      isOn: Binding(
        get: { channel.broadcastPolicy.moon },
        set: { moon in apply(to: channel) { $0.moon = moon } }
      )
    ) {
      Text(
        "Do not disturb",
        comment:
          "Agent pill menu: ticked, deliveries wait in the mailbox without waking the agent"
      )
    }
    .disabled(locked)
  }

  private func apply(
    to channel: OverlayChannelDelivery, _ edit: @escaping (inout AgentBroadcastPolicy) -> Void
  ) {
    let ledger = OverlayBroadcastPolicyLedger.shared
    guard let identity = channel.broadcastIdentity, !ledger.isPending(identity) else { return }
    let polled = channel.broadcastPolicy
    commandError = nil
    Task { @MainActor in
      do {
        try await ledger.change(identity, polled: polled, edit)
      } catch {
        commandError = String(localized: "Couldn't change broadcast reception.")
      }
    }
  }
}

/// The agent's two reception marks beside its name, each a one-click switch
/// (Maciej, 2026-10-23: "symbole broadcastu zrób klikalnymi"): the antenna
/// joins or leaves channel 0, the moon lights or clears do-not-disturb. They
/// move independently and neither greys the other. The roster, the Intercom
/// fiszki and the pill menu all issue the same command through the helper.
struct OverlayReceptionMarks: View {
  let channel: OverlayChannelDelivery
  let palette: OverlayAppearancePalette
  let textScale: CGFloat
  var enabled = true

  private var policy: AgentBroadcastPolicy { channel.broadcastPolicy }
  /// Any surface's change in flight holds these marks too.
  private var pending: Bool {
    OverlayBroadcastPolicyLedger.shared.isPending(channel.broadcastIdentity)
  }

  var body: some View {
    HStack(spacing: 3) {
      mark(
        symbol: policy.statusSymbol, lit: policy.member, litColor: palette.agentAccent.color,
        hint: policy.member
          ? String(localized: "Leave channel 0") : String(localized: "Join channel 0"),
        help: policy.statusHelp, identifier: "overlay-broadcast-policy-\(channel.channel)"
      ) {
        apply { $0.member.toggle() }
      }
      // The moon speaks through its shape alone: a dimmed outline when clear,
      // a filled moon in the row's own text colour when lit. The amber of
      // `processingStatus` made it a yellow warning beside the name (Maciej,
      // 2026-10-23: "żółty księżyc zamiast kontur ↔ wypełnienie, brzydko").
      mark(
        symbol: policy.quietSymbol, lit: policy.moon, litColor: palette.primaryText.color,
        unlitOpacity: 0.45,
        hint: policy.moon
          ? String(localized: "Turn off do not disturb") : String(localized: "Do not disturb"),
        help: policy.quietHelp, identifier: "overlay-quiet-policy-\(channel.channel)"
      ) {
        apply { $0.moon.toggle() }
      }
    }
    .accessibilityIdentifier("overlay-reception-marks-\(channel.channel)")
  }

  private func mark(
    symbol: String, lit: Bool, litColor: Color, unlitOpacity: Double = 1,
    hint: String, help: String, identifier: String,
    action: @escaping () -> Void
  ) -> some View {
    Button(action: action) {
      Image(systemName: symbol)
        .font(.system(size: 10 * textScale))
        .frame(width: 14 * textScale, height: 14 * textScale)
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .modifier(
      OverlayAudioControlInk(
        palette: palette, lit: lit ? litColor : nil, unlitOpacity: unlitOpacity)
    )
    .disabled(!enabled || pending || channel.broadcastIdentity == nil)
    .overlayFastHint(hint, palette: palette)
    .accessibilityLabel(Text(verbatim: help))
    .accessibilityIdentifier(identifier)
  }

  private func apply(_ edit: @escaping (inout AgentBroadcastPolicy) -> Void) {
    guard let identity = channel.broadcastIdentity, !pending else { return }
    let polled = policy
    Task { @MainActor in
      // A refused command leaves the mark as the receipt still says; the pill
      // menu's inline error is the place that names the refusal.
      try? await OverlayBroadcastPolicyLedger.shared.change(identity, polled: polled, edit)
    }
  }
}

/// A quiet notification affordance opens the full monitor on the overlay canvas.
struct OverlayChannelStatusView: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  /// Not `@Environment`: the overlay calls `monitorBody` on this value from its
  /// own body, so the struct is never installed and an environment property
  /// reads its default. ⌘+ grew the docked column and the conversation while
  /// the roster's text stayed at 1.0 (Maciej, 2026-10-19). `monitorBody` sets
  /// the scale read where the column is installed.
  private var textScale: CGFloat = 1

  let channels: [OverlayChannelDelivery]
  let rosterReady: Bool
  let unavailable: Bool
  let palette: OverlayAppearancePalette
  /// False while the panel is hidden or occluded: the waiting spinner must not
  /// keep the render loop awake.
  let animates: Bool
  let hudStates: [String: OverlayChannelHudProjection]
  let agentDescriptors: [String: String]
  let onToggleChannel: ((UInt8) -> Void)?
  let toggleError: String?
  var pendingToggleChannels: Set<String> = []
  let conversations: [OverlayConversation]
  let selectedConversationID: String?
  let unreadCounts: [String: Int]
  let onSelectConversation: ((String?) -> Void)?
  let onShowMonitor: (() -> Void)?
  /// Explicitly show this agent in the list. Unlike the header toggle, this
  /// callback never closes a panel or stops a recording.
  var onShowAgentInMonitor: ((String) -> Void)? = nil
  var headerSelected = false
  /// The recording agent is on screen: a press stops its capture.
  var agentMicrophoneStops = false
  var mutedChannels: [String: Bool] = [:]
  var pendingMuteChannels: Set<String> = []
  var onTogglePlayback: ((String) -> Void)?
  var playbackError: String?
  var archiveCandidates: [String: OverlayConversationOwner] = [:]
  var pendingArchives: Set<OverlayConversationOwner> = []
  var archivedOwners: Set<OverlayConversationOwner> = []
  var onArchiveAgent: ((OverlayConversationOwner) -> Void)?
  var archiveError: String?
  var panelFocusStates: [String: OverlayAgentPanelFocusState] = [:]
  var panelFocusAvailable = false
  var onFocusAgentPanel: ((OverlayConversationOwner) -> Void)?
  /// The row the middle-click chooser has focused, and the hover that moves it.
  var gestureFocusID: String?
  var onGestureHover: ((String) -> Void)?
  /// The list header's corner buttons; nil leaves the header bare.
  var sidebarControls: OverlayAgentsSidebarControls?
  var searchModel: OverlayConversationSearchModel?
  @State private var internalSearchModel = OverlayConversationSearchModel()

  var activeSearchModel: OverlayConversationSearchModel {
    searchModel ?? internalSearchModel
  }

  init(
    channels: [OverlayChannelDelivery], unavailable: Bool,
    palette: OverlayAppearancePalette, animates: Bool,
    hudStates: [String: OverlayChannelHudProjection] = [:],
    agentDescriptors: [String: String] = [:],
    onToggleChannel: ((UInt8) -> Void)? = nil,
    toggleError: String? = nil,
    pendingToggleChannels: Set<String> = [],
    conversations: [OverlayConversation] = [], selectedConversationID: String? = nil,
    unreadCounts: [String: Int] = [:], onSelectConversation: ((String?) -> Void)? = nil,
    onShowMonitor: (() -> Void)? = nil,
    searchModel: OverlayConversationSearchModel? = nil,
    rosterReady: Bool = true
  ) {
    self.channels = channels
    self.rosterReady = rosterReady
    self.unavailable = unavailable
    self.palette = palette
    self.animates = animates
    self.hudStates = hudStates
    self.agentDescriptors = agentDescriptors
    self.onToggleChannel = onToggleChannel
    self.toggleError = toggleError
    self.pendingToggleChannels = pendingToggleChannels
    self.conversations = conversations
    self.selectedConversationID = selectedConversationID
    self.unreadCounts = unreadCounts
    self.onSelectConversation = onSelectConversation
    self.onShowMonitor = onShowMonitor
    self.searchModel = searchModel
  }

  /// Common roster and command projection. A floating card can override only
  /// conversation presentation; the execution and broadcast policy stay shared.
  static func intercomRoster(
    state: OverlayState, palette: OverlayAppearancePalette, animates: Bool,
    onRevealConversation: (() -> Void)? = nil,
    onOpenConversation: ((String) -> Void)? = nil
  ) -> OverlayChannelStatusView {
    let ownedConversations = Self.currentConversations(
      conversations: state.conversations, archivedOwners: state.archivedAgentOwners,
      hudStates: state.channelHudStates)
    var view = OverlayChannelStatusView(
      channels: state.visibleChannelRows.filter { $0.channel != "0" },
      unavailable: state.channelStatusUnavailable,
      palette: palette, animates: animates,
      hudStates: state.channelHudStates,
      agentDescriptors: state.channelAgentDescriptors,
      onToggleChannel: { digit in
        guard let conversation = ownedConversations.first(where: { $0.channel == String(digit) })
        else { return }
        Task {
          await state.toggleConversationMicrophone(
            conversation, keepsAgentList: true, preservesPresentation: onOpenConversation != nil)
        }
      },
      toggleError: state.channelToggleError,
      pendingToggleChannels: state.pendingChannelToggles,
      conversations: state.conversations,
      selectedConversationID: state.selectedConversationID,
      unreadCounts: Dictionary(
        uniqueKeysWithValues: state.conversations.map {
          ($0.id, state.unreadReplies(in: $0))
        }),
      // A row or a rail pill: in the agents view the conversation slides out.
      onSelectConversation: { id in
        if let id {
          if let onOpenConversation {
            onOpenConversation(id)
          } else {
            state.chooseConversation(id)
            onRevealConversation?()
          }
        } else {
          state.selectConversation(nil)
        }
      },
      onShowMonitor: state.toggleAgentSidebar,
      // The drawer value is rebuilt on every body pass and only its
      // `monitorBody(...)` is installed, so a `@State` on it never persists:
      // each HUD tick handed the drawer a fresh, empty search model and the
      // query vanished mid-typing. The overlay state owns the model.
      searchModel: state.savedConversationSearch,
      rosterReady: state.hasLoadedChannelRoster
    )
    view.mutedChannels = state.channelPlaybackMuted
    view.headerSelected =
      !state.isCollapsed && state.windowContentSurface == .agents
    view.agentMicrophoneStops = state.agentsButtonStopsRecording
    view.pendingMuteChannels = state.pendingPlaybackChannels
    view.onTogglePlayback = { channel in
      guard let conversation = ownedConversations.first(where: { $0.channel == channel }) else {
        return
      }
      Task { await state.toggleConversationPlayback(conversation) }
    }
    view.playbackError = state.playbackPreferenceError
    view.archiveCandidates = Dictionary(
      uniqueKeysWithValues: state.visibleChannelRows.compactMap {
        state.archiveCandidate(for: $0.channel).map { ($0.channel, $0) }
      })
    view.pendingArchives = state.pendingAgentArchives
    view.archivedOwners = state.archivedAgentOwners
    view.onArchiveAgent = { owner in Task { await state.archiveAgent(owner) } }
    view.archiveError = state.agentArchiveError
    view.panelFocusStates = state.agentPanelFocusStates
    view.panelFocusAvailable = state.agentPanelFocusAvailable
    // The view resolves the destination per row; `vc-frame` gates only Frame.
    view.onFocusAgentPanel = { state.focusAgentPanel($0) }
    view.gestureFocusID = state.agentGestureFocusID
    view.onGestureHover = { state.hoverAgentGestureTarget($0) }
    view.onShowAgentInMonitor = { [view] digit in
      if let channel = view.channels.first(where: { $0.channel == digit }),
        let conversation = view.conversation(for: channel)
      {
        state.chooseConversation(conversation.id)
      }
      state.showAgentMonitor()
      state.focusAgentRoster()
      onRevealConversation?()
    }
    return view
  }

  static func toggleDigit(for channel: String) -> UInt8? {
    guard let digit = UInt8(channel), (0...9).contains(digit) else { return nil }
    return digit
  }

  func toggle(_ channel: OverlayChannelDelivery) {
    guard let digit = Self.toggleDigit(for: channel.channel) else { return }
    onToggleChannel?(digit)
  }

  func isOpen(_ channel: OverlayChannelDelivery) -> Bool {
    hudStates[channel.channel]?.open ?? channel.isOpen
  }

  func hasDeadFollower(_ channel: OverlayChannelDelivery) -> Bool {
    hudStates[channel.channel]?.followerAlive == false
  }

  /// Nobody reads this channel: its follower is gone, or alive with a backlog
  /// unread past the stall horizon.
  func isInactive(_ channel: OverlayChannelDelivery) -> Bool {
    hasDeadFollower(channel) || hasStalledFollower(channel)
  }

  func hasStalledFollower(_ channel: OverlayChannelDelivery) -> Bool {
    hudStates[channel.channel]?.isStalled() == true
  }

  /// Whole minutes since the stalled follower's oldest unread delivery.
  func stalledMinutes(_ channel: OverlayChannelDelivery) -> Int? {
    guard let since = hudStates[channel.channel]?.unreadSince, hasStalledFollower(channel)
    else { return nil }
    return max(1, Int(Date().timeIntervalSince(since) / 60))
  }

  var glyph: OverlayAgentGlyph {
    OverlayAgentGlyph.resolve(channels: channels, unavailable: unavailable) ?? .attached
  }

  func showMonitor() { onShowMonitor?() }

  func notificationTitle(for channel: OverlayChannelDelivery) -> String {
    let title = channel.channel + " · " + channel.agent
    guard let conversation = conversation(for: channel),
      let count = unreadCounts[conversation.id], count > 0
    else { return title }
    return title + " (" + String(count) + ")"
  }

  /// Open agent capture is violet and pulsing. Its glyph distinguishes an
  /// individual microphone from an explicit channel-zero broadcast.
  var agentCaptureOpen: Bool {
    hudStates.values.contains(where: \.open)
  }

  private var headerTitle: String {
    guard agentCaptureOpen else { return String(localized: "Agents") }
    let action =
      agentMicrophoneStops ? String(localized: "Stop recording") : String(localized: "Agents")
    return action + ". " + OverlayRecordingLight.agent.tooltip
  }

  @ViewBuilder
  private var headerGlyph: some View {
    if agentCaptureOpen {
      let broadcasting = hudStates["0"]?.open == true
      OverlayMicrophoneGlyph(
        symbol: broadcasting ? "megaphone" : OverlayAgentAudioControls.liveSpeechSymbol,
        tint: OverlayRecordingLight.agent.color, mirrored: broadcasting
      )
      .symbolEffect(.pulse, isActive: animates && !reduceMotion)
    } else {
      // The chrome owns the selected wash; the glyph drops its own plate.
      OverlayMicrophoneGlyph(
        character: OverlayHeaderGlyph.agents,
        tint: headerSelected
          ? OverlayHeaderAccent.agents.selectionTint(in: palette) : palette.mutedText.color,
        plateOpacity: headerSelected ? 0 : 0.12
      )
    }
  }

  var body: some View {
    Button(action: showMonitor) {
      headerGlyph
        .contentShape(Circle())
        .overlay(alignment: .topTrailing) {
          if unreadCounts.values.contains(where: { $0 > 0 }) {
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
        title: headerTitle, accent: .agents, palette: palette,
        selected: headerSelected)
    )
    .accessibilityLabel(
      agentMicrophoneStops ? String(localized: "Stop recording") : String(localized: "Agents")
    )
    .accessibilityAddTraits(headerSelected ? .isSelected : [])
    .accessibilityValue(agentCaptureOpen ? OverlayRecordingLight.agent.name : glyph.label)
    .accessibilityIdentifier("overlay-agent-glyph")
  }

  var monitorBody: some View {
    monitorBody(maximumHeight: 480)
  }

  var broadcastMembers: [OverlayChannelDelivery] {
    channels.filter {
      (1...9).contains(Int($0.channel) ?? 0)
        && hudStates[$0.channel]?.provider != nil
        && hudStates[$0.channel]?.providerSessionID != nil
    }
  }

  var broadcastHeader: String {
    let listening = broadcastMembers.filter { $0.broadcastPolicy.receivesWake }.count
    let bound = broadcastMembers.count
    return String(
      localized: "0 · All (\(listening) of \(bound))",
      comment: "Channel zero: agents woken by broadcasts out of all bound agents")
  }

  /// The roster body. Wide windows dock it as the right-hand column, so the
  /// header carries the agent count, not a close button; `focusRevision` is the
  /// docked name pill's channel for focusing the selected row. The middle-click
  /// chooser's focus scrolls into view. Below the threshold the same body
  /// covers the whole window.
  func monitorBody(maximumHeight: CGFloat, focusRevision: UInt64 = 0) -> some View {
    OverlayTextScaleReader { scale in
      var installed = self
      installed.textScale = scale
      return installed.scaledMonitorBody(
        maximumHeight: maximumHeight, focusRevision: focusRevision)
    }
  }

  /// The folded list (Maciej, 2026-10-19: "zostawia pigułki"): one pill per
  /// row of the list, channel 0 first, under the `sidebar.right` that
  /// unfolds it. A pill switches the conversation and the list stays folded.
  func railBody(maximumHeight: CGFloat, onUnfold: @escaping () -> Void) -> some View {
    OverlayTextScaleReader { scale in
      var installed = self
      installed.textScale = scale
      return installed.scaledRailBody(maximumHeight: maximumHeight, onUnfold: onUnfold)
    }
  }

  private func scaledRailBody(
    maximumHeight: CGFloat, onUnfold: @escaping () -> Void
  ) -> some View {
    let unfold = 22 * textScale
    let viewportHeight = max(0, maximumHeight - 8 - unfold - 8)
    let rowCount = channels.count
      + (currentConversations.contains(where: { $0.channel == "0" }) ? 1 : 0)
      + (currentConversations.contains(where: \.isHumanInbox) ? 1 : 0)
    let gapCount = max(0, rowCount - 1)
    let rowSpacing = gapCount == 0 ? 0 : min(
      6, max(0, (viewportHeight - CGFloat(rowCount) * 24 * textScale - 4) / CGFloat(gapCount)))
    return VStack(spacing: 8) {
      OverlaySidebarToggleButton(
        symbol: "sidebar.right", title: String(localized: "Show agents list"),
        identifier: "overlay-agent-rail-unfold", palette: palette, textScale: textScale,
        action: onUnfold)
      ScrollView(.vertical, showsIndicators: false) {
        VStack(spacing: rowSpacing) {
          if let broadcast = currentConversations.first(where: { $0.channel == "0" }) {
            railPill(broadcast, channel: nil)
          }
          if let inbox = currentConversations.first(where: \.isHumanInbox) {
            railPill(inbox, channel: nil)
          }
          ForEach(channels) { channel in
            railPill(conversation(for: channel), channel: channel)
          }
        }
        .padding(.vertical, 2)
        .frame(maxWidth: .infinity)
        .csAutohidingScrollers(.verticalHidden)
      }
      .csRailOverflowHints(color: palette.mutedText.color)
      .frame(maxHeight: viewportHeight)
    }
    // The list header sits 8 points into the roster content: the unfold
    // button lands where `sidebar.right` folded it.
    .padding(.top, 8)
    .frame(maxWidth: .infinity)
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-agent-rail-pills")
  }

  /// Status dot and channel number; the open conversation in the agent
  /// accent, unread replies as the row badge's colour. The pill itself is the
  /// shared `OverlayChannelPill`, the one drawing of a channel's digit.
  private func railPill(
    _ conversation: OverlayConversation?, channel: OverlayChannelDelivery?
  ) -> some View {
    let inbox = conversation?.isHumanInbox == true
    let digit = channel?.channel ?? conversation?.channel ?? "0"
    let title =
      inbox
      ? conversation?.name ?? String(localized: "Your Inbox")
      : channel.map { notificationTitle(for: $0) } ?? broadcastHeader
    let canOpen = conversation != nil && onSelectConversation != nil
    let hint =
      canOpen
      ? title + ". "
        + String(localized: "View conversation without changing the microphone")
      : String(
        localized: "No conversation on this channel",
        comment: "Disabled mini-rail channel pill")
    return OverlayChannelPill(
      form: .rail,
      display: railPillDisplay(conversation, channel: channel),
      hint: hint, accessibilityText: title, identifier: "overlay-agent-rail-\(digit)",
      palette: palette, textScale: textScale
    ) {
      if let conversation { onSelectConversation?(conversation.id) }
    }
    .accessibilityHint(Text(verbatim: hint))
    .modifier(
      inbox
        ? OverlayBroadcastPolicyMenu(members: [], palette: palette, enabled: false)
        : agentPillMenu(for: digit))
  }

  func railPillDisplay(
    _ conversation: OverlayConversation?, channel: OverlayChannelDelivery?
  ) -> OverlayChannelPill.Display {
    let inbox = conversation?.isHumanInbox == true
    return OverlayChannelPill.Display(
      figure: inbox ? "" : channel?.channel ?? "0", symbol: inbox ? "tray" : nil,
      dot: inbox ? nil : channel.map { statusDotKind(for: $0) } ?? .broadcast,
      disconnected: channel.map { hasDeadFollower($0) } ?? false,
      unreadCount: conversation.flatMap { unreadCounts[$0.id] } ?? 0,
      selected: conversation != nil && conversation?.id == selectedConversationID,
      enabled: conversation != nil && onSelectConversation != nil)
  }

  /// The collapsed overlay's channel map: another surface of this roster's
  /// projections. Bound digits come from the roster snapshot (`hudStates`),
  /// names from the same rows the list shows, unread marks from the same
  /// counts, the selected chip from the same selection as a rail pill's, and
  /// a chip is clickable only where a rail pill would be.
  var channelMapModel: OverlayChannelMapModel {
    let current = onSelectConversation == nil ? [] : currentConversations
    return OverlayChannelMapModel(
      channels: channels, hudStates: hudStates,
      selectedDigit: currentConversations.first(where: { $0.id == selectedConversationID })?
        .channel,
      selectableDigits: Set(current.map(\.channel)),
      unreadDigits: Set(current.filter { (unreadCounts[$0.id] ?? 0) > 0 }.map(\.channel)))
  }

  /// A map chip's click: the digit's current conversation, through the same
  /// selection path and the same owner match as its rail pill. A digit with
  /// no current conversation selects nothing; capture is never touched.
  func selectChannelConversation(_ digit: String) {
    guard let conversation = currentConversations.first(where: { $0.channel == digit }) else {
      return
    }
    onSelectConversation?(conversation.id)
  }

  /// Write/Forward opens this owner's existing composer even when its
  /// conversation is already selected; it never turns into panel focus.
  func writeToAgent(_ channel: OverlayChannelDelivery) {
    guard let conversation = conversation(for: channel) else { return }
    onSelectConversation?(conversation.id)
  }

  func showAgentInMonitor(_ channel: OverlayChannelDelivery) {
    onShowAgentInMonitor?(channel.channel)
  }

  func focusAgentPanel(_ channel: OverlayChannelDelivery) {
    guard showsPanelFocusButton(for: channel), !panelFocusUnreachable(channel),
      panelFocusStates[channel.channel]?.isRequesting != true,
      let owner = archiveCandidates[channel.channel]
    else { return }
    onFocusAgentPanel?(owner)
  }

  func archiveAgent(_ channel: OverlayChannelDelivery) {
    guard !isOpen(channel), let owner = archiveCandidates[channel.channel],
      !pendingArchives.contains(owner)
    else { return }
    onArchiveAgent?(owner)
  }

  /// One menu projection for the header, folded rail, list and floating
  /// intercom. Broadcast policy execution remains in its existing modifier.
  func agentPillMenu(
    for digit: String, showsInlineError: Bool = true, showsButton: Bool = false
  ) -> OverlayBroadcastPolicyMenu {
    guard let channel = channels.first(where: { $0.channel == digit }), digit != "0",
      let actions = agentPillMenuActions(for: digit, showsFocusState: true)
    else {
      return OverlayBroadcastPolicyMenu(members: [], palette: palette, enabled: false)
    }
    return OverlayBroadcastPolicyMenu(
      members: [channel], palette: palette, enabled: !unavailable,
      agentActions: actions, showsInlineError: showsInlineError, showsButton: showsButton,
      focusState: showsButton ? panelFocusStates[digit] : nil)
  }

  /// Both the card controls and the action menu use the same command guards.
  func agentMicrophoneEnabled(for channel: OverlayChannelDelivery) -> Bool {
    !unavailable && onToggleChannel != nil
      && !pendingToggleChannels.contains(channel.channel)
      && Self.toggleDigit(for: channel.channel) != nil
      && (isOpen(channel) || !hasDeadFollower(channel))
  }

  func agentPlaybackEnabled(for channel: OverlayChannelDelivery) -> Bool {
    onTogglePlayback != nil && mutedChannels[channel.channel] != nil
      && !pendingMuteChannels.contains(channel.channel)
  }

  /// The commands behind one agent's pill menu, with their real availability.
  /// The Intercom draws its focus button from the same `focus` action, so a
  /// fiszka and the menu never disagree on where the agent can be reached.
  func agentPillMenuActions(
    for digit: String, showsFocusState: Bool = false
  ) -> OverlayAgentPillMenuActions? {
    guard let channel = channels.first(where: { $0.channel == digit }), digit != "0" else {
      return nil
    }
    let microphoneEnabled = agentMicrophoneEnabled(for: channel)
    let playbackEnabled = agentPlaybackEnabled(for: channel)
    let muted = mutedChannels[digit]
    let focusState = panelFocusStates[digit]
    let owner = archiveCandidates[digit]
    return OverlayAgentPillMenuActions(
      write: OverlayAgentPillMenuAction(
        title: String(localized: "Write / Forward…"), symbol: "square.and.pencil",
        identifier: "overlay-agent-menu-write-\(digit)",
        enabled: conversation(for: channel) != nil && onSelectConversation != nil,
        perform: { writeToAgent(channel) }),
      microphone: onToggleChannel == nil
        ? nil
        : OverlayAgentPillMenuAction(
          title: isOpen(channel)
            ? String(localized: "Stop speaking to agent")
            : String(localized: "Speak to agent"),
          symbol: OverlayAgentAudioControls.liveSpeechSymbol,
          identifier: "overlay-agent-menu-microphone-\(digit)", enabled: microphoneEnabled,
          perform: { if microphoneEnabled { toggle(channel) } }),
      playback: onTogglePlayback == nil
        ? nil
        : OverlayAgentPillMenuAction(
          title: muted == nil
            ? String(localized: "Playback status unavailable")
            : muted == true
              ? String(localized: "Unmute agent replies")
              : String(localized: "Mute agent replies"),
          symbol: muted == true ? "speaker.slash" : "speaker.wave.2",
          identifier: "overlay-agent-menu-playback-\(digit)",
          enabled: playbackEnabled,
          perform: { if playbackEnabled { onTogglePlayback?(digit) } }),
      showInAgents: onShowAgentInMonitor == nil
        ? nil
        : OverlayAgentPillMenuAction(
          title: String(localized: "Show in Agents"), symbol: "person.2",
          identifier: "overlay-agent-menu-show-\(digit)", enabled: true,
          perform: { showAgentInMonitor(channel) }),
      focus: !showsPanelFocusButton(for: channel)
        ? nil
        : OverlayAgentPillMenuAction(
          title: showsFocusState
            ? (focusState?.statusText ?? panelFocusHelp(for: channel, state: nil))
            : (panelFocusTarget(for: channel)?.help ?? panelFocusHelp(for: channel, state: nil)),
          symbol: panelFocusSymbol(
            showsFocusState ? focusState : nil, target: panelFocusTarget(for: channel)),
          identifier: "overlay-agent-menu-focus-\(digit)",
          enabled: !panelFocusUnreachable(channel) && focusState?.isRequesting != true,
          perform: { focusAgentPanel(channel) }),
      archive: owner == nil || onArchiveAgent == nil
        ? nil
        : OverlayAgentPillMenuAction(
          title: hasDeadFollower(channel)
            ? String(localized: "Remove from list and move to archive")
            : String(localized: "Disconnect and archive agent"),
          symbol: "archivebox",
          identifier: "overlay-agent-menu-archive-\(digit)",
          enabled: !isOpen(channel) && owner.map { !pendingArchives.contains($0) } == true,
          perform: { archiveAgent(channel) }))
  }

  private func scaledMonitorBody(maximumHeight: CGFloat, focusRevision: UInt64) -> some View {
    OverlayAgentMonitorBody(
      maximumHeight: maximumHeight, palette: palette,
      focusRevision: focusRevision, focusedRowID: selectedConversationID,
      scrollTargetID: gestureFocusID.flatMap { id in
        currentConversations.contains(where: {
          $0.id == id && ($0.channel == "0" || $0.isHumanInbox)
        }) ? nil : id
      }
    ) {
      HStack(spacing: 6) {
        Text("Agents").font(.system(size: 13 * textScale, weight: .semibold))
        Spacer()
        Text(verbatim: String(channels.count))
          .font(.system(size: 11 * textScale))
          .monospacedDigit()
          .foregroundStyle(palette.mutedText.color)
          .accessibilityHidden(true)
        if let sidebarControls {
          OverlaySidebarToggleButton(
            symbol: "sidebar.right", title: String(localized: "Hide agents list"),
            identifier: "overlay-agent-sidebar-fold", palette: palette,
            textScale: textScale, action: sidebarControls.onFoldList)
        }
      }
    } pinned: { rowFocus in
      if onSelectConversation != nil,
        currentConversations.contains(where: { $0.channel == "0" || $0.isHumanInbox })
      {
        VStack(alignment: .leading, spacing: 2) {
          if let broadcast = currentConversations.first(where: { $0.channel == "0" }),
            onSelectConversation != nil
          {
            rowGlassContainer(
              VStack(alignment: .leading, spacing: 2) {
                broadcastRow(broadcast, rowFocus: rowFocus)
                // The pair sits beside the preview, not in the Agents header
                // and not in the agent-row footer. Three lines belong to All.
                HStack(alignment: .center, spacing: 8) {
                  if let last = broadcast.messages.last {
                    let unread = unreadCounts[broadcast.id] ?? 0
                    // The preview opens the conversation like an agent row's
                    // preview does; only the All pair beside it keeps its own
                    // actions (Founder, 2026-10-23: a click on it did nothing).
                    Button {
                      onSelectConversation?(broadcast.id)
                    } label: {
                      Text(verbatim: Self.drawerPreview(message: last))
                        .font(.system(size: 13 * textScale))
                        .foregroundStyle(
                          unread > 0 ? palette.bodyText.color : palette.mutedText.color
                        )
                        .lineLimit(3)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityHint(
                      Text(
                        verbatim: String(
                          localized: "View conversation without changing the microphone"))
                    )
                    .accessibilityIdentifier("overlay-all-preview")
                  } else {
                    Spacer(minLength: 0)
                  }
                  OverlayAgentAudioControls(
                    open: hudStates["0"]?.open == true, muted: mutedChannels["0"],
                    microphoneEnabled: !unavailable && hudStates["0"] != nil
                      && onToggleChannel != nil && !pendingToggleChannels.contains("0"),
                    playbackEnabled: onTogglePlayback != nil && !pendingMuteChannels.contains("0"),
                    palette: palette, onMicrophone: { onToggleChannel?(0) },
                    onPlayback: { onTogglePlayback?("0") }, broadcast: true,
                    speaking: broadcast.replySpeaking
                  )
                  .fixedSize()
                  .accessibilityIdentifier("overlay-channel-toggle-0")
                }
              }
              .frame(maxWidth: .infinity, alignment: .leading)
            )
            // The shared channel is a roster row: the same padding, the same
            // selection mark and the same hover as the agents under it.
            .padding(.vertical, 3)
            .padding(.horizontal, 5)
            .modifier(
              OverlayRosterRowSelection(
                palette: palette, selected: selectedConversationID == broadcast.id)
            )
            .modifier(OverlayRowHoverHighlight(palette: palette, cornerRadius: CSRadius.chip))
            .modifier(
              OverlayGestureFocusRing(
                focused: broadcast.id == gestureFocusID,
                shape: RoundedRectangle(cornerRadius: CSRadius.chip), palette: palette,
                targetID: broadcast.id, onHover: onGestureHover)
            )
            .id(broadcast.id)
            .modifier(
              OverlayBroadcastPolicyMenu(
                members: broadcastMembers, palette: palette, enabled: !unavailable))
          }
          if let inbox = currentConversations.first(where: \.isHumanInbox) {
            conversationRow(inbox, saved: false, rowFocus: rowFocus)
              .font(.system(size: 13 * textScale, weight: .medium))
              .foregroundStyle(palette.primaryText.color)
              .padding(.horizontal, 5)
          }
        }
        .padding(CSSpace.xxs)
        .modifier(OverlayControlGlass(cornerRadius: CSRadius.card))
      }
    } content: { rowFocus in
      VStack(alignment: .leading, spacing: 6) {
        detailsContent(rowFocus: rowFocus)
          .font(.system(size: 13 * textScale, weight: .medium))
          .foregroundStyle(palette.primaryText.color)
      }
    }
    .font(.system(size: 13 * textScale))
    .accessibilityIdentifier("overlay-agent-monitor")
  }

  var currentConversations: [OverlayConversation] {
    Self.currentConversations(
      conversations: conversations, archivedOwners: archivedOwners, hudStates: hudStates)
  }

  /// Static so the overlay state's middle-click chooser walks the same rows.
  static func currentConversations(
    conversations: [OverlayConversation], archivedOwners: Set<OverlayConversationOwner>,
    hudStates: [String: OverlayChannelHudProjection]
  ) -> [OverlayConversation] {
    // Count matching sessions once, including archived leases: archiving one
    // of two leases must never make the other look like the unique live owner.
    var matchingCounts: [String: Int] = [:]
    for conversation in conversations {
      guard let owner = conversation.owner, let hud = hudStates[conversation.channel],
        hud.provider == owner.provider, hud.providerSessionID == owner.providerSessionID
      else { continue }
      matchingCounts[conversation.channel, default: 0] += 1
    }
    var archivedIDs: [String: Set<String>] = [:]
    for owner in archivedOwners { archivedIDs[owner.channel, default: []].insert(owner.id) }
    return conversations.filter { conversation in
      if conversation.channel == "0" || conversation.isHumanInbox { return true }
      guard let owner = conversation.owner,
        archivedIDs[owner.channel]?.contains(owner.id) != true,
        let hud = hudStates[conversation.channel],
        hud.provider == owner.provider, hud.providerSessionID == owner.providerSessionID
      else { return false }
      return matchingCounts[conversation.channel] == 1
    }
  }

  var rawSavedConversations: [OverlayConversation] {
    let current = Set(currentConversations.map(\.id))
    return conversations.filter { !current.contains($0.id) }
  }

  var savedConversations: [OverlayConversation] {
    activeSearchModel.filterAndSort(rawSavedConversations) { [self] conversation in
      self.lastActivityDate(in: conversation)
    }
  }

  func conversation(for channel: OverlayChannelDelivery) -> OverlayConversation? {
    currentConversations.first { $0.channel == channel.channel }
  }

  /// Where this row's focus button lands: the Frame pane when one is known
  /// and `vc-frame` can reach it, otherwise the agent's terminal tab,
  /// otherwise a Codex agent's thread in the Codex app.
  func panelFocusTarget(for channel: OverlayChannelDelivery) -> OverlayAgentPanelFocusTarget? {
    OverlayAgentPanelFocusTarget.resolve(
      hudStates[channel.channel], frameFocusAvailable: panelFocusAvailable)
  }

  /// The row's only known destination is a Codex Cloud thread.
  func hasCodexCloudThread(_ channel: OverlayChannelDelivery) -> Bool {
    OverlayAgentPanelFocusTarget.isCodexCloudThread(
      hudStates[channel.channel], frameFocusAvailable: panelFocusAvailable)
  }

  /// Frame and terminal destinations need a live follower; a Codex thread
  /// does not, and a Codex Cloud thread shows its button greyed.
  func showsPanelFocusButton(for channel: OverlayChannelDelivery) -> Bool {
    guard archiveCandidates[channel.channel] != nil, onFocusAgentPanel != nil else {
      return false
    }
    if hasCodexCloudThread(channel) { return true }
    guard let target = panelFocusTarget(for: channel) else { return false }
    return !target.needsLiveFollower || hudStates[channel.channel]?.followerAlive == true
  }

  /// The focus button is rendered but cannot act: a disconnected follower
  /// for Frame or a terminal, no destination at all, or a Codex Cloud thread.
  /// A local Codex thread opens with the follower disconnected.
  func panelFocusUnreachable(_ channel: OverlayChannelDelivery) -> Bool {
    if hasCodexCloudThread(channel) { return true }
    if panelFocusTarget(for: channel)?.needsLiveFollower == false { return false }
    return hasDeadFollower(channel) || hasNoTerminalTarget(channel)
  }

  func viewConversation(_ channel: OverlayChannelDelivery) {
    guard let conversation = conversation(for: channel) else { return }
    if selectedConversationID == conversation.id,
      showsPanelFocusButton(for: channel),
      let owner = archiveCandidates[channel.channel],
      !panelFocusUnreachable(channel),
      let onFocusAgentPanel
    {
      onFocusAgentPanel(owner)
      return
    }
    onSelectConversation?(conversation.id)
  }

  private func rowGlassContainer<Content: View>(_ content: Content) -> some View {
    overlayGlassGroup(content)
  }

  func channelRow(
    _ channel: OverlayChannelDelivery, rowFocus: FocusState<String?>.Binding? = nil
  ) -> some View {
    let conversation = conversation(for: channel)
    let receiptText = deliveryReceiptDisplay(for: channel, conversation: conversation)?.text
      ?? deliveryReceiptText(for: channel, conversation: conversation)
    let descriptor = agentDescriptor(for: channel)
    let status = statusHelp(for: channel)
    // Keep the full descriptor in the tooltip as well as the visible row.
    var tooltipParts: [String] = []
    if let descriptor, !descriptor.isEmpty { tooltipParts.append(descriptor) }
    if let receiptText { tooltipParts.append(receiptText) }
    tooltipParts.append(status)
    if let conversation, let count = unreadCounts[conversation.id], count > 0 {
      tooltipParts.append(String(localized: "\(count) unread replies"))
    }
    tooltipParts.append(channel.broadcastPolicy.statusHelp)
    let tooltip = tooltipParts.joined(separator: ". ")
    let identityHint =
      conversation == nil
      ? String(localized: "No messages yet")
      : String(localized: "View conversation without changing the microphone")
    return rowGlassContainer(
      VStack(alignment: .leading, spacing: 4) {
        HStack(alignment: .top, spacing: 4) {
          Group {
            if let conversation, onSelectConversation != nil {
              Button { viewConversation(channel) } label: {
                rosterRowLabel(
                  title: conversation.name, statusChannel: channel, conversation: conversation,
                  dot: statusDotKind(for: channel), headerOnly: true)
              }
              .buttonStyle(.plain)
              .modifier(OptionalRowFocus(binding: rowFocus, id: conversation.id))
            } else if showsPanelFocusButton(for: channel),
              let owner = archiveCandidates[channel.channel],
              !panelFocusUnreachable(channel), let onFocusAgentPanel
            {
              Button { onFocusAgentPanel(owner) } label: {
                rosterRowLabel(
                  title: channel.agent, statusChannel: channel, conversation: conversation,
                  dot: statusDotKind(for: channel), headerOnly: true)
              }
              .buttonStyle(.plain)
            } else {
              rosterRowLabel(
                title: channel.agent, statusChannel: channel, conversation: conversation,
                dot: statusDotKind(for: channel), headerOnly: true)
            }
          }
          .frame(maxWidth: .infinity, alignment: .leading)
          .foregroundStyle(palette.primaryText.color)
          .accessibilityLabel(Text(verbatim: notificationTitle(for: channel)))
          .accessibilityValue(Text(verbatim: tooltip))
          .accessibilityHint(Text(verbatim: identityHint))
          .accessibilityIdentifier("overlay-view-conversation-\(conversation?.id ?? channel.channel)")
          compactAgentActions(for: channel)
        }
        if let conversation, onSelectConversation != nil {
          Button { viewConversation(channel) } label: {
            rosterRowLabel(
              title: conversation.name, statusChannel: channel, conversation: conversation,
              dot: statusDotKind(for: channel), contentOnly: true)
          }
          .buttonStyle(.plain)
          .accessibilityLabel(Text(verbatim: identityHint))
        } else {
          rosterRowLabel(
            title: channel.agent, statusChannel: channel, conversation: conversation,
            dot: statusDotKind(for: channel), contentOnly: true)
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
    )
    .modifier(agentPillMenu(for: channel.channel, showsInlineError: false))
    .padding(.vertical, 3)
    .padding(.horizontal, 5)
    .modifier(
      OverlayRosterRowSelection(
        palette: palette,
        selected: conversation != nil && selectedConversationID == conversation?.id)
    )
    .modifier(
      OverlayRowHoverHighlight(
        palette: palette, cornerRadius: CSRadius.chip,
        enabled: (conversation != nil && onSelectConversation != nil)
          || (showsPanelFocusButton(for: channel) && !panelFocusUnreachable(channel))
      )
    )
    .modifier(
      OverlayGestureFocusRing(
        focused: conversation != nil && conversation?.id == gestureFocusID,
        shape: RoundedRectangle(cornerRadius: CSRadius.chip), palette: palette,
        targetID: conversation?.id, onHover: onGestureHover)
    )
    .id(conversation?.id ?? "overlay-channel-\(channel.channel)")
    .modifier(OverlayDisconnectedRowDim(disconnected: hasDeadFollower(channel)))
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-agent-row-\(channel.channel)")
  }

  /// Compose display values here; the visual shell never reads state or
  /// executes commands. The header and content are separate so its buttons
  /// remain siblings of the conversation navigation button.
  private func rosterRowLabel(
    title: String, statusChannel: OverlayChannelDelivery?,
    conversation: OverlayConversation?, dot: OverlayAgentDotKind,
    includePreview: Bool = true, titleAccessory: AnyView? = nil,
    headerOnly: Bool = false, contentOnly: Bool = false
  ) -> some View {
    let last = conversation?.messages.last
    return OverlayAgentRosterLabel(
      title: title, channel: statusChannel?.channel, conversationID: conversation?.id,
      descriptor: statusChannel.flatMap { agentDescriptor(for: $0) },
      preview: includePreview ? last.map { Self.drawerPreview(message: $0) } : nil,
      timestamp: last.flatMap { MessageTimestamp.date(fromISO8601: $0.emittedAt) },
      unreadCount: conversation.flatMap { unreadCounts[$0.id] } ?? 0,
      dot: dot, statusLabel: statusChannel.map { shortStatus(for: $0) } ?? "",
      statusDetail: statusChannel.map { detail(for: $0) } ?? "",
      titleAccessory: titleAccessory ?? statusChannel.map {
        AnyView(compactReceptionIndicators(for: $0))
      }, palette: palette,
      textScale: textScale, headerOnly: headerOnly, contentOnly: contentOnly,
      receipt: statusChannel.flatMap { deliveryReceiptDisplay(for: $0, conversation: conversation) })
  }

  func compactAgentActions(for channel: OverlayChannelDelivery) -> some View {
    HStack(spacing: 6) {
      OverlayAgentAudioControls(
        open: isOpen(channel), muted: mutedChannels[channel.channel],
        microphoneEnabled: agentMicrophoneEnabled(for: channel),
        playbackEnabled: agentPlaybackEnabled(for: channel),
        palette: palette, onMicrophone: { toggle(channel) },
        onPlayback: { onTogglePlayback?(channel.channel) },
        speaking: conversation(for: channel)?.replySpeaking == true)
      .accessibilityIdentifier("overlay-channel-toggle-\(channel.channel)")
      EmptyView().modifier(agentPillMenu(
        for: channel.channel, showsInlineError: false, showsButton: true))
    }
    .fixedSize()
    .accessibilityIdentifier("overlay-agent-row-controls-\(channel.channel)")
  }

  func compactReceptionIndicators(for channel: OverlayChannelDelivery) -> some View {
    HStack(spacing: 4) {
      if channel.broadcastPolicy.moon {
        Image(systemName: "moon.fill")
          .help(Text("Do not disturb"))
          .accessibilityLabel(Text("Do not disturb"))
      }
      if !channel.broadcastPolicy.member {
        Image(systemName: "antenna.radiowaves.left.and.right.slash")
          .help(Text("Off channel 0"))
          .accessibilityLabel(Text("Off channel 0"))
      }
    }
    .font(.system(size: 11 * textScale))
    .foregroundStyle(palette.mutedText.color)
  }

  func deliveryReceiptDisplay(
    for channel: OverlayChannelDelivery, conversation: OverlayConversation?
  ) -> OverlayRosterReceipt? {
    guard !unavailable else { return nil }
    if let conversation, let owner = conversation.owner {
      for message in conversation.messages.reversed()
      where message.kind == .user && message.showsDeliveryReceipts {
        guard let recipient = message.recipients.first(where: {
          $0.owner.provider == owner.provider
            && $0.owner.providerSessionID == owner.providerSessionID
            && $0.owner.leaseID == owner.leaseID
        }) else { continue }
        guard message.withdrawn != true else { return nil }
        func display(_ symbol: String, _ text: String) -> OverlayRosterReceipt {
          let detail: String
          if let date = MessageTimestamp.date(fromISO8601: message.emittedAt) {
            let timestamp = date.formatted(date: .abbreviated, time: .shortened)
            detail = String(localized: "Last sent message at \(timestamp): \(text)")
          } else {
            detail = String(localized: "Last sent message: \(text)")
          }
          return OverlayRosterReceipt(symbol: symbol, text: text, helpText: detail)
        }
        if message.holdSecondsRemaining != nil {
          return display("pause.circle", String(localized: "Held"))
        }
        if let deliveryID = recipient.deliveryID,
          let state = message.receipt?.deliveries.first(where: { $0.deliveryID == deliveryID })?.state
        {
          switch state {
          case .read: return display("double-check", String(localized: "Read"))
          case .delivered: return display("checkmark", String(localized: "Delivered"))
          case .skipped: return display("forward.end", OverlayConversationView.skippedBroadcastText)
          case .noReceipt: return display("circle.dotted", OverlayConversationView.noReceiptText)
          }
        }
        if recipient.acknowledged {
          return display("double-check", String(localized: "Read"))
        }
        let symbol = recipient.broadcastSkipped == true ? "forward.end" : "clock"
        return display(symbol, OverlayConversationView.receiptStatusText(for: recipient))
      }
    }
    return nil
  }

  private func rosterRowState(
    _ channel: OverlayChannelDelivery, receiptText: String?
  ) -> some View {
    HStack(spacing: 5) {
      if panelFocusStates[channel.channel]?.isRequesting == true {
        ProgressView()
          .controlSize(.mini)
          .accessibilityIdentifier("overlay-agent-focus-spinner-\(channel.channel)")
      }
      Text(verbatim: rowStateText(for: channel, receiptText: receiptText))
        .font(.system(size: 11 * textScale, weight: .medium))
        .foregroundStyle(rowStateColor(for: channel))
        .lineLimit(1)
        .truncationMode(.tail)
        .accessibilityIdentifier("overlay-agent-state-\(channel.channel)")
    }
  }

  /// The receipt of the newest delivery, then the reader's state when there is
  /// no receipt or the reader is gone or stalled: one line, never two.
  ///
  /// A failed Frame focus never takes this line: Maciej, 2026-10-19, "czemu te
  /// errors nie mogą się chować do ładnych mini glifków statusu, tylko straszą
  /// do końca". The failure lives on the focus button's glyph and its help.
  func rowStateText(for channel: OverlayChannelDelivery, receiptText: String?) -> String {
    if let focusState = panelFocusStates[channel.channel], !focusState.isFailed {
      return focusState.statusText
    }
    guard let receiptText else { return shortStatus(for: channel) }
    guard hasDeadFollower(channel) || hasStalledFollower(channel) else { return receiptText }
    return receiptText + " · " + shortStatus(for: channel)
  }

  private func rowStateColor(for channel: OverlayChannelDelivery) -> Color {
    guard let focusState = panelFocusStates[channel.channel] else {
      return palette.mutedText.color
    }
    switch focusState.phase {
    case .requesting, .activating: return palette.processingStatus.color
    case .succeeded: return palette.successStatus.color
    case .failed: return palette.mutedText.color
    }
  }

  /// The live follower named neither a vc-frame pane nor a terminal tab, so a
  /// focus request has nowhere to land: the control rests greyed instead of
  /// failing on click (Maciej, 2026-10-19: "przycisków kierowania do frame
  /// brak gdy frame session nie wykryty u agenta - wyszarzony"). No claim at
  /// all is not a refusal.
  func hasNoTerminalTarget(_ channel: OverlayChannelDelivery) -> Bool {
    guard let hud = hudStates[channel.channel] else { return false }
    return hud.frameDetected != true && hud.terminalDetected != true
      && (hud.frameDetected == false || hud.terminalDetected == false)
  }

  func panelFocusSymbol(
    _ state: OverlayAgentPanelFocusState?, target: OverlayAgentPanelFocusTarget? = nil
  ) -> String {
    if state?.isSucceeded == true { return "checkmark" }
    if state?.isFailed == true { return "exclamationmark.triangle" }
    switch state?.target ?? target {
    case .some(.terminal): return "terminal"
    case .some(.codexThread): return "bubble.left.and.text.bubble.right"
    case .some(.frame), nil: return "macwindow.on.rectangle"
    }
  }

  func panelFocusHelp(
    for channel: OverlayChannelDelivery, state: OverlayAgentPanelFocusState?
  ) -> String {
    if hasCodexCloudThread(channel) {
      return OverlayAgentPanelFocusTarget.codexCloudThreadHelp
    }
    if panelFocusTarget(for: channel) == .codexThread {
      return state?.statusText ?? OverlayAgentPanelFocusTarget.codexThread.help
    }
    if hasDeadFollower(channel) {
      return String(localized: "Agent is disconnected")
    }
    if hasNoTerminalTarget(channel) {
      // A follower older than terminal focus only ever claimed a pane.
      return hudStates[channel.channel]?.terminalDetected == nil
        ? String(
          localized: "Agent is not in a Frame pane",
          comment: "Frame focus help: the follower attached without vc-frame pane coordinates")
        : String(
          localized: "No terminal known for this agent",
          comment: "Focus result: the follower knows neither a Frame pane nor a terminal tab")
    }
    if let state {
      return state.statusText
    }
    return (panelFocusTarget(for: channel) ?? .frame).help
  }

  /// The "0 · All" row keeps the roster's model; its title carries the
  /// broadcast wake count and its context menu every member's policy.
  ///
  /// The shared channel stands apart from the agent rows through structure,
  /// not colour: it leads the roster, it wears the antenna where an agent
  /// wears its status dot, and a divider parts it from the agents under it.
  /// Both decisions are recorded here — Maciej, 2026-10-17: "wyróżnić bardziej
  /// kanał ogólny", which arrived as an accent wash behind the row; Founder,
  /// 2026-10-18: that wash "wygląda tragicznie" and goes. The intent stayed,
  /// its paint did not.
  private func broadcastRow(
    _ broadcast: OverlayConversation, rowFocus: FocusState<String?>.Binding?
  ) -> some View {
    Button {
      onSelectConversation?(broadcast.id)
    } label: {
      // Preview lives beside the All controls, so this button keeps the
      // identity line (name, antenna, clock) and does not nest those controls.
      rosterRowLabel(
        title: broadcastHeader, statusChannel: nil, conversation: broadcast, dot: .broadcast,
        includePreview: false)
    }
    .buttonStyle(.plain)
    .modifier(OptionalRowFocus(binding: rowFocus, id: broadcast.id))
    .foregroundStyle(palette.primaryText.color)
    .overlayFastHint(
      String(localized: "View conversation without changing the microphone"), palette: palette
    )
    .accessibilityHint(
      Text(verbatim: String(localized: "View conversation without changing the microphone"))
    )
    .accessibilityIdentifier("overlay-view-conversation-\(broadcast.id)")
  }

  private func conversationRow(
    _ conversation: OverlayConversation, saved: Bool,
    rowFocus: FocusState<String?>.Binding? = nil
  ) -> some View {
    Button {
      onSelectConversation?(conversation.id)
    } label: {
      HStack(spacing: 10) {
        Image(systemName: conversation.isHumanInbox ? "tray" : saved ? "clock" : "bubble.left")
          .font(.system(size: 12))
          .foregroundStyle(palette.mutedText.color)
          .frame(width: 16)
        VStack(alignment: .leading, spacing: 2) {
          if conversation.channel == "0" {
            Text(verbatim: broadcastHeader)
          } else {
            Text(verbatim: conversation.displayTitle)
          }
          if saved, let owner = conversation.owner {
            Text(verbatim: savedConversationDetail(conversation, owner: owner))
              .font(.system(size: 11 * textScale))
              .foregroundStyle(palette.mutedText.color)
              .lineLimit(1)
          }
        }
        Spacer(minLength: 8)
        if saved, !conversation.historyLoaded, selectedConversationID == conversation.id {
          ProgressView()
            .controlSize(.small)
            .accessibilityIdentifier("overlay-saved-conversation-history-loading")
        }
        if let count = unreadCounts[conversation.id], count > 0 {
          OverlayAgentUnreadBadge(count: count, palette: palette, textScale: textScale)
        }
        Image(systemName: selectedConversationID == conversation.id ? "checkmark" : "chevron.right")
          .font(.system(size: 10, weight: .medium))
          .foregroundStyle(palette.mutedText.color)
      }
      .padding(.vertical, 7)
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .modifier(OptionalRowFocus(binding: rowFocus, id: conversation.id))
    .modifier(OverlayRowHoverHighlight(palette: palette, cornerRadius: CSRadius.chip, outset: 6))
    .overlayFastHint(
      String(localized: "View conversation without changing the microphone"), palette: palette
    )
    .accessibilityHint(
      Text(verbatim: String(localized: "View conversation without changing the microphone"))
    )
    .accessibilityIdentifier("overlay-view-conversation-\(conversation.id)")
  }

  private func savedConversationDetail(
    _ conversation: OverlayConversation, owner: OverlayConversationOwner
  ) -> String {
    guard let date = lastActivityDate(in: conversation) else { return owner.provider }
    return "\(owner.provider) · \(date.formatted(date: .abbreviated, time: .shortened))"
  }

  private func lastActivityDate(in conversation: OverlayConversation) -> Date? {
    // Messages are in history order. Parse once per row, outside the sort
    // comparator; an unloaded archive has no verified message date yet.
    if let date = conversation.messages.reversed().lazy.compactMap({
      MessageTimestamp.date(fromISO8601: $0.emittedAt)
    }).first {
      return date
    }
    return activeSearchModel.backendLastActivityDates[conversation.id]
  }

  var showsConnectionHint: Bool {
    rosterReady && channels.isEmpty
      && !currentConversations.contains(where: { $0.owner != nil })
  }

  var showsRosterLoading: Bool {
    !rosterReady
  }

  private func detailsContent(rowFocus: FocusState<String?>.Binding?) -> some View {
    VStack(alignment: .leading, spacing: 2) {
      if showsRosterLoading {
        Text("Checking the connection…")
          .foregroundStyle(palette.mutedText.color)
          .padding(.vertical, 12)
          .accessibilityIdentifier("overlay-agent-roster-loading")
      } else if showsConnectionHint {
        Text(
          "No agents connected. To connect an agent to Codescribe, run the `codescribe` skill in its session."
        )
        .foregroundStyle(palette.mutedText.color)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.vertical, 12)
        .accessibilityIdentifier("overlay-agent-connection-hint")
      }
      ForEach(channels) { channel in
        channelRow(channel, rowFocus: rowFocus)
      }
      if onSelectConversation != nil {
        // A passive conversation can precede the next roster snapshot.
        // Keep it visible without inventing a microphone binding.
        ForEach(
          currentConversations.filter { conversation in
            conversation.channel != "0" && !conversation.isHumanInbox
              && !channels.contains { $0.channel == conversation.channel }
          }
        ) { conversation in
          conversationRow(conversation, saved: false, rowFocus: rowFocus)
        }
        if !rawSavedConversations.isEmpty {
          Divider().padding(.vertical, 4)
          DisclosureGroup("Saved conversations") {
            savedConversationsContent
          }
          .font(.system(size: 13 * textScale))
        }
      }
      if unavailable {
        Text("Channel status unavailable — last open state retained")
          .font(.system(size: 13 * textScale, weight: .medium))
          .foregroundStyle(palette.processingStatus.color)
          .accessibilityIdentifier("overlay-channel-status-unavailable")
      }
      if let toggleError {
        // A refused toggle is an error, not a pending state (N roster palette).
        Text("Channel toggle failed: \(toggleError)")
          .font(.system(size: 13 * textScale, weight: .medium))
          .foregroundStyle(palette.errorStatus.color)
          .accessibilityIdentifier("overlay-channel-toggle-error")
      }
      if let playbackError {
        Text(verbatim: playbackError)
          .font(.system(size: 11 * textScale))
          .foregroundStyle(palette.errorStatus.color)
          .accessibilityIdentifier("overlay-agent-playback-error")
      }
      if let archiveError {
        Text(verbatim: archiveError)
          .font(.system(size: 11 * textScale))
          .foregroundStyle(palette.errorStatus.color)
          .accessibilityIdentifier("overlay-agent-archive-error")
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  @ViewBuilder
  private var savedConversationsContent: some View {
    VStack(alignment: .leading, spacing: 6) {
      HStack(spacing: 6) {
        HStack(spacing: 5) {
          Image(systemName: "magnifyingglass")
            .font(.system(size: 11 * textScale))
            .foregroundStyle(palette.mutedText.color)
          TextField(
            "Search conversations…",
            text: Binding(
              get: { activeSearchModel.query },
              set: { activeSearchModel.query = $0 }
            )
          )
          .textFieldStyle(.plain)
          .font(.system(size: 11 * textScale))
          .foregroundStyle(palette.primaryText.color)
          .accessibilityIdentifier("overlay-saved-conversations-search-field")

          if activeSearchModel.isLoading {
            ProgressView()
              .controlSize(.small)
              .accessibilityIdentifier("overlay-saved-conversations-search-loading")
          } else if !activeSearchModel.query.isEmpty {
            Button {
              activeSearchModel.clear()
            } label: {
              Image(systemName: "xmark.circle.fill")
                .font(.system(size: 11 * textScale))
                .foregroundStyle(palette.mutedText.color)
            }
            .buttonStyle(.plain)
            .overlayFastHint(String(localized: "Clear search"), palette: palette)
            .accessibilityLabel(Text("Clear search"))
            .accessibilityHint(Text("Clear search"))
            .accessibilityIdentifier("overlay-saved-conversations-search-clear")
          }
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 4)
        .background(
          palette.mutedText.color.opacity(0.08), in: RoundedRectangle(cornerRadius: CSRadius.chip))

        Menu {
          Picker(
            "Sort order",
            selection: Binding(
              get: { activeSearchModel.sortOrder },
              set: { activeSearchModel.sortOrder = $0 }
            )
          ) {
            ForEach(OverlayConversationSortOrder.allCases, id: \.self) { order in
              Text(order.title).tag(order)
            }
          }
        } label: {
          Image(systemName: "line.3.horizontal.decrease.circle")
            .font(.system(size: 12 * textScale))
            .foregroundStyle(palette.mutedText.color)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .accessibilityIdentifier("overlay-saved-conversations-sort-menu")
      }
      .padding(.vertical, 2)

      if let error = activeSearchModel.effectiveSearchError {
        Text("Search failed")
          .font(.system(size: 10 * textScale, weight: .medium))
          .foregroundStyle(palette.errorStatus.color)
          .overlayFastHint(error, palette: palette)
          .accessibilityHint(Text(verbatim: error))
          .accessibilityIdentifier("overlay-saved-conversations-search-error")
      } else if activeSearchModel.isEffectivePartial {
        HStack(spacing: 4) {
          Image(systemName: "exclamationmark.triangle")
            .font(.system(size: 10 * textScale))
          Text("Partial results")
            .font(.system(size: 10 * textScale))
        }
        .foregroundStyle(palette.processingStatus.color)
        .accessibilityIdentifier("overlay-saved-conversations-search-partial")
      }

      if activeSearchModel.hasExhaustiveNoMatch(in: savedConversations) {
        Text("No matching conversations")
          .font(.system(size: 11 * textScale))
          .foregroundStyle(palette.mutedText.color)
          .padding(.vertical, 4)
          .accessibilityIdentifier("overlay-saved-conversations-empty-matches")
      } else {
        ForEach(savedConversations) { conversation in
          conversationRow(conversation, saved: true)
        }
      }
    }
    .task(id: rawSavedConversations.map(\.id)) {
      await activeSearchModel.loadArchiveMetadata()
    }
  }

  func archiveHelp(for channel: OverlayChannelDelivery) -> String {
    if isOpen(channel) {
      return String(
        localized: "Hang up the channel first, then remove the agent from the list",
        comment: "Drawer cross on an agent whose microphone channel is open")
    }
    if hasDeadFollower(channel) {
      return String(localized: "Remove from list and move to archive")
    }
    return String(
      localized: "Stop the agent's reader, remove from list and move to archive",
      comment: "Drawer cross on an agent whose reader process is still alive")
  }

  /// The newest completed delivery to this frozen owner, never a reply or another lease.
  /// Liveness and the Founder's unread replies remain separate row metadata.
  func deliveryReceiptText(
    for channel: OverlayChannelDelivery, conversation: OverlayConversation?
  ) -> String? {
    guard !unavailable else { return nil }
    if let conversation, let owner = conversation.owner {
      for message in conversation.messages.reversed()
      where message.kind == .user && message.showsDeliveryReceipts {
        guard
          let recipient = message.recipients.first(where: {
            $0.owner.provider == owner.provider
              && $0.owner.providerSessionID == owner.providerSessionID
              && $0.owner.leaseID == owner.leaseID
          })
        else { continue }
        guard message.withdrawn != true else { return nil }
        if message.holdSecondsRemaining != nil { return String(localized: "Held") }
        // Skipped only on the follower's receipt for this delivery and owner
        // (`receiptStatusText`). Today's channel-0 policy is the row's mark,
        // never this delivery's receipt (Lena's and Olek's review, 2026-10-17).
        return OverlayConversationView.receiptStatusText(for: recipient)
      }
    }
    switch channel.stage {
    case .held: return String(localized: "Held")
    case .sent where channel.broadcastSkipped: return OverlayConversationView.skippedBroadcastText
    case .sent: return String(localized: "Addressed")
    case .queued: return String(localized: "Queued")
    case .received: return String(localized: "Read")
    case nil: return nil
    }
  }

  private func shortStatus(for channel: OverlayChannelDelivery) -> String {
    if unavailable { return String(localized: "Status unavailable") }
    if hasDeadFollower(channel) { return String(localized: "Disconnected") }
    if hasStalledFollower(channel) {
      return String(localized: "Not reading", comment: "Agent alive, backlog unread for long")
    }
    if isOpen(channel) { return String(localized: "Listening") }
    if channel.broadcastSkipped { return String(localized: "Ready") }
    switch channel.stage {
    case nil: return String(localized: "Ready")
    case .held: return String(localized: "Held")
    case .sent, .queued: return String(localized: "Awaiting receipt")
    case .received: return String(localized: "Received")
    }
  }

  func agentDescriptor(for channel: OverlayChannelDelivery) -> String? {
    let descriptor = agentDescriptors[channel.channel]?.trimmingCharacters(
      in: .whitespacesAndNewlines)
    if let descriptor, !descriptor.isEmpty { return descriptor }
    let provider = hudStates[channel.channel]?.provider?.trimmingCharacters(
      in: .whitespacesAndNewlines)
    return provider.flatMap { $0.isEmpty ? nil : $0 }
  }

  func statusSymbol(for channel: OverlayChannelDelivery) -> String {
    if unavailable { return "exclamationmark.triangle" }
    if hasDeadFollower(channel) { return "xmark.circle" }
    if hasStalledFollower(channel) { return "zzz" }
    if isOpen(channel) { return OverlayAgentAudioControls.liveSpeechSymbol }
    switch channel.stage {
    case nil: return "circle"
    case .held: return "hourglass"
    case .sent, .queued: return "clock"
    case .received: return "checkmark"
    }
  }

  /// The roster dot shares the row's status predicates: lit while listening or
  /// with delivery progress, a ring for an asleep channel or one with no
  /// deliveries yet (mockup `.dot`).
  func statusDotKind(for channel: OverlayChannelDelivery) -> OverlayAgentDotKind {
    if unavailable { return .idle }
    if hasDeadFollower(channel) { return .sleeping }
    if isOpen(channel) { return .listening }
    if hasStalledFollower(channel) { return .sleeping }
    if channel.broadcastSkipped { return .idle }
    switch channel.stage {
    case .received: return .received
    case .held: return .waiting
    case .sent, .queued: return .waiting
    case nil: return .idle
    }
  }

  func statusHelp(for channel: OverlayChannelDelivery) -> String {
    shortStatus(for: channel) + ". " + detail(for: channel)
  }

  func detail(for channel: OverlayChannelDelivery) -> String {
    if unavailable { return String(localized: "status unavailable") }
    let delivery: String
    switch channel.stage {
    case nil: delivery = String(localized: "no sealed utterance")
    case .held: delivery = String(localized: "Held")
    case .sent where channel.broadcastSkipped:
      delivery = OverlayConversationView.skippedBroadcastText
    case .sent: delivery = String(localized: "sent · waiting for receipt")
    case .queued: delivery = String(localized: "queued · waiting for receipt")
    case .received: delivery = String(localized: "receipt confirmed by the agent")
    }
    if let minutes = stalledMinutes(channel) {
      return String(
        localized: "\(delivery) · nothing read for \(minutes) min",
        comment:
          "Channel row; first placeholder is the delivery state, second the whole minutes since the oldest unread message"
      )
    }
    guard hasDeadFollower(channel) else { return delivery }
    return String(
      localized: "\(delivery) · nobody listening",
      comment: "Channel row; the placeholder is the delivery state of that channel")
  }

  /// Channel 0 first, the human inbox, then bound channels in roster order,
  /// followed by passive conversations the roster snapshot has not caught up with.
  static func rosterOrder(
    current: [OverlayConversation], channels: [OverlayChannelDelivery]
  ) -> [OverlayConversation] {
    var ordered: [OverlayConversation] = []
    if let zero = current.first(where: { $0.channel == "0" }) {
      ordered.append(zero)
    }
    if let inbox = current.first(where: \.isHumanInbox) {
      ordered.append(inbox)
    }
    for channel in channels where channel.channel != "0" {
      if let conversation = current.first(where: { $0.channel == channel.channel }) {
        ordered.append(conversation)
      }
    }
    for conversation in current
    where conversation.channel != "0" && !ordered.contains(where: { $0.id == conversation.id }) {
      ordered.append(conversation)
    }
    return ordered
  }

  /// Who addressed whom, from the message's own source channel and frozen
  /// recipients. The row or conversation showing the message does not choose
  /// the names. An empty audience means there is no arrow.
  struct MessageRoute: Equatable {
    var speaker: String
    var audience: String?

    var label: String {
      guard let audience, !audience.isEmpty else { return speaker }
      return "\(speaker) → \(audience)"
    }
  }

  static func messageRoute(of message: OverlayConversationMessage) -> MessageRoute {
    let named = recipientNames(of: message)
    if message.kind == .user {
      if message.sourceChannel == "0" {
        return MessageRoute(speaker: String(localized: "You"), audience: String(localized: "All"))
      }
      if let named { return MessageRoute(speaker: String(localized: "You"), audience: named) }
      if let channel = message.sourceChannel {
        return MessageRoute(speaker: String(localized: "You"), audience: channel)
      }
      return MessageRoute(speaker: String(localized: "You"), audience: nil)
    }
    guard let sender = message.owner?.name, !sender.isEmpty else {
      return MessageRoute(speaker: "", audience: nil)
    }
    // A reply without a destination, or one addressed on channel 10, answered
    // the Founder. Channel 0 is the broadcast, whatever the current row is.
    guard let destination = message.sourceChannel, destination != "10" else {
      return MessageRoute(speaker: sender, audience: String(localized: "You"))
    }
    if destination == "0" {
      return MessageRoute(speaker: sender, audience: String(localized: "All"))
    }
    return MessageRoute(speaker: sender, audience: named ?? destination)
  }

  /// The drawer's one-glance preview of the last message, naming who
  /// addressed whom (Maciej, 2026-10-17: "brak informacji jak who addressed
  /// whom"). The route comes from the message itself — its source channel and
  /// frozen recipients — never from the row showing it: a broadcast copied
  /// into an agent's row still reads "→ All". Newlines fold so the two-line
  /// cap holds. Attachment-only messages still carry the route.
  static func drawerPreview(message: OverlayConversationMessage) -> String {
    let body = message.bodyText.replacingOccurrences(of: "\n", with: " ")
    let route = messageRoute(of: message)
    guard !route.speaker.isEmpty else { return body }
    return "\(route.label): \(body)"
  }

  /// The frozen recipients' names in publication order, at most two named.
  private static func recipientNames(of message: OverlayConversationMessage) -> String? {
    var names: [String] = []
    for recipient in message.recipients
    where !recipient.owner.name.isEmpty && !names.contains(recipient.owner.name) {
      names.append(recipient.owner.name)
    }
    guard let first = names.first else { return nil }
    if names.count == 1 { return first }
    let named = names.prefix(2).joined(separator: ", ")
    return names.count == 2 ? named : "\(named) +\(names.count - 2)"
  }
}

/// Roster status dot kinds, mapped to palette tones by `OverlayAgentStatusDot`;
/// the shared channel's `broadcast` draws an antenna instead of a dot.
enum OverlayAgentDotKind {
  case broadcast, listening, received, waiting, sleeping, idle
}

/// Glass rows and their controls morph as one group on macOS 26; older
/// systems render the content as it is. Main-actor like the views that call
/// it: a nonisolated generic cannot hand a `View` conformance to
/// `GlassEffectContainer` (Swift 6.2 isolated conformances).
@MainActor @ViewBuilder
private func overlayGlassGroup<Content: View>(_ content: Content) -> some View {
  if #available(macOS 26.0, *) {
    GlassEffectContainer(spacing: 6) { content }
  } else {
    content
  }
}

/// The roster's status dot: filled while listening or with delivery progress,
/// a ring for an asleep channel or one without deliveries (mockup `.dot`).
/// The shared channel wears an antenna: it has no reader of its own.
struct OverlayAgentStatusDot: View {
  @Environment(\.csTextScale) private var textScale
  let kind: OverlayAgentDotKind
  let palette: OverlayAppearancePalette

  private var filled: Bool {
    switch kind {
    case .broadcast, .listening, .received, .waiting: true
    case .sleeping, .idle: false
    }
  }

  private var tone: OverlayColorToken {
    switch kind {
    case .broadcast: palette.agentAccent
    case .listening: palette.listeningStatus
    case .received: palette.successStatus
    case .waiting: palette.processingStatus
    case .sleeping, .idle: palette.mutedText
    }
  }

  @ViewBuilder
  var body: some View {
    if kind == .broadcast {
      Image(systemName: "antenna.radiowaves.left.and.right")
        .font(.system(size: 10 * textScale, weight: .semibold))
        .foregroundStyle(tone.color)
        .frame(width: 12 * textScale, height: 8 * textScale)
        .accessibilityHidden(true)
    } else {
      Circle()
        .fill(filled ? tone.color : .clear)
        .overlay {
          if !filled {
            Circle()
              .strokeBorder(tone.color.opacity(kind == .idle ? 0.5 : 1), lineWidth: 1.5)
          }
        }
        .frame(width: 8 * textScale, height: 8 * textScale)
    }
  }
}

/// Applies `.focused(equals:)` when the docked column handed a binding down;
/// test harnesses and legacy call sites render the row without one.
private struct OptionalRowFocus: ViewModifier {
  let binding: FocusState<String?>.Binding?
  let id: String

  @ViewBuilder
  func body(content: Content) -> some View {
    if let binding {
      content.focused(binding, equals: id)
    } else {
      content
    }
  }
}

/// The embedded monitor shares the overlay's appearance and readable palette.
struct ChannelRosterStyle: Equatable {
  let surface: OverlayColorToken
  let border: OverlayColorToken
  let primaryText: OverlayColorToken
  let bodyText: OverlayColorToken
  let mutedText: OverlayColorToken
  let colorScheme: ColorScheme

  init(palette: OverlayAppearancePalette) {
    surface = palette.desktopBackground
    border = palette.border
    primaryText = palette.primaryText
    bodyText = palette.bodyText
    mutedText = palette.mutedText
    colorScheme = palette.appearance == .dark ? .dark : .light
  }
}

/// Reads `\.csTextScale` where it is installed and hands it to a builder.
private struct OverlayTextScaleReader<Content: View>: View {
  @Environment(\.csTextScale) private var textScale
  let content: (CGFloat) -> Content

  var body: some View { content(textScale) }
}

struct ChannelRosterContent<Content: View>: View {
  let palette: OverlayAppearancePalette
  @ViewBuilder let content: Content

  var style: ChannelRosterStyle { ChannelRosterStyle(palette: palette) }

  var body: some View {
    content
      .padding(.horizontal, 4)
      .padding(.vertical, 8)
      .frame(maxWidth: .infinity, alignment: .leading)
      .preferredColorScheme(style.colorScheme)
  }
}

/// A roster hugs its measured content but never grows past the canvas budget.
/// Both measurements are SwiftUI points; Retina changes pixel density, not layout.
/// The header stays outside the scroll viewport so the channel count is always
/// visible. The focus state is the docked column's name-pill channel: bumping
/// `focusRevision` moves keyboard focus to the selected conversation's row.
/// `scrollTargetID` is the middle-click chooser's focus, kept in view.
private struct OverlayAgentMonitorBody<Header: View, Pinned: View, Content: View>: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  let maximumHeight: CGFloat
  let palette: OverlayAppearancePalette
  var focusRevision: UInt64 = 0
  var focusedRowID: String? = nil
  var scrollTargetID: String? = nil
  @State private var headerHeight: CGFloat = 22
  @State private var pinnedHeight: CGFloat = 0
  @State private var contentHeight: CGFloat?
  @FocusState private var rowFocus: String?
  @ViewBuilder let header: Header
  let pinnedBuilder: (FocusState<String?>.Binding) -> Pinned
  let contentBuilder: (FocusState<String?>.Binding) -> Content

  init(
    maximumHeight: CGFloat, palette: OverlayAppearancePalette,
    focusRevision: UInt64 = 0, focusedRowID: String? = nil, scrollTargetID: String? = nil,
    @ViewBuilder header: () -> Header,
    @ViewBuilder pinned: @escaping (FocusState<String?>.Binding) -> Pinned,
    @ViewBuilder content: @escaping (FocusState<String?>.Binding) -> Content
  ) {
    self.maximumHeight = maximumHeight
    self.palette = palette
    self.focusRevision = focusRevision
    self.focusedRowID = focusedRowID
    self.scrollTargetID = scrollTargetID
    self.header = header()
    pinnedBuilder = pinned
    contentBuilder = content
  }

  private var viewportHeight: CGFloat {
    OverlayAgentMonitorLayout.viewportHeight(
      maximumHeight: maximumHeight, headerHeight: headerHeight, pinnedHeight: pinnedHeight,
      contentHeight: contentHeight)
  }

  var body: some View {
    ChannelRosterContent(palette: palette) {
      VStack(alignment: .leading, spacing: 8) {
        header
          .fixedSize(horizontal: false, vertical: true)
          .onGeometryChange(for: CGFloat.self) {
            $0.size.height
          } action: {
            headerHeight = $0
          }
        pinnedBuilder($rowFocus)
          .fixedSize(horizontal: false, vertical: true)
          .onGeometryChange(for: CGFloat.self) {
            $0.size.height
          } action: {
            pinnedHeight = $0
          }
          .accessibilityIdentifier("overlay-agent-monitor-pinned-mailboxes")
        ScrollViewReader { proxy in
          ScrollView(.vertical) {
            contentBuilder($rowFocus)
              .fixedSize(horizontal: false, vertical: true)
              .onGeometryChange(for: CGFloat.self) {
                $0.size.height
              } action: {
                contentHeight = $0
              }
              .csAutohidingScrollers()
          }
          .frame(height: viewportHeight)
          .clipped()
          .accessibilityIdentifier("overlay-agent-monitor-scroll")
          .onChange(of: scrollTargetID) { _, target in
            guard let target else { return }
            withAnimation(reduceMotion ? nil : .easeOut(duration: CSMotion.Tempo.hover)) {
              proxy.scrollTo(target, anchor: .center)
            }
          }
        }
      }
    }
    // With keyboard navigation on (System Settings › Keyboard) every row is
    // focusable, and the system ring stayed on "0 · All", the first row, while
    // clicks moved the selection bar: two marks naming two rows, and a message
    // typed under the ring went to the bar's agent (Maciej, 2026-10-19, div0).
    // Focus entering the roster lands on the open conversation, and a focused
    // row follows the selection.
    .defaultFocus($rowFocus, focusedRowID)
    .onChange(of: focusRevision) { _, _ in
      rowFocus = focusedRowID
    }
    .onChange(of: focusedRowID) { _, id in
      if rowFocus != nil, let id { rowFocus = id }
    }
  }
}

/// Measured header and pinned mailboxes consume space before the scrolling agents.
enum OverlayAgentMonitorLayout {
  static func viewportHeight(
    maximumHeight: CGFloat, headerHeight: CGFloat, pinnedHeight: CGFloat, contentHeight: CGFloat?
  ) -> CGFloat {
    let available = max(0, maximumHeight - headerHeight - pinnedHeight - 16 - 16)
    return min(contentHeight ?? available, available)
  }
}
