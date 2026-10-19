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

/// Compact circular glyph for the widget header; capture stays with its owner.
struct OverlayMicrophoneGlyph: View {
  @Environment(\.displayScale) private var displayScale
  private let mark: Mark
  let tint: Color
  var plateOpacity: Double = 0.12
  static let diameter: CGFloat = 22

  private enum Mark {
    case symbol(String)
    /// One text sign (`OverlayHeaderGlyph`), sized to sit like the symbols.
    case character(String, pointSize: CGFloat)
  }

  init(symbol: String, tint: Color, plateOpacity: Double = 0.12) {
    mark = .symbol(symbol)
    self.tint = tint
    self.plateOpacity = plateOpacity
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
/// microphone sleeps; the cross still archives it; recorded replies still play.
struct OverlayDisconnectedRowDim: ViewModifier {
  static let opacity: Double = 0.45
  let disconnected: Bool

  func body(content: Content) -> some View {
    content
      .opacity(disconnected ? Self.opacity : 1)
      .grayscale(disconnected ? 1 : 0)
  }
}

/// What the list header's corner buttons act on (Maciej, 2026-10-19:
/// `sidebar.left` and `sidebar.right`). The overlay supplies it where it
/// installs the list.
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
    .foregroundStyle(palette.mutedText.color)
    .modifier(OverlayRowHoverHighlight(palette: palette, cornerRadius: 6))
    .help(title)
    .accessibilityLabel(Text(verbatim: title))
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
  var cornerRadius: CGFloat = 9
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
  var cornerRadius: CGFloat = 10

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
struct OverlayAgentControlStyle: ViewModifier {
  @ViewBuilder
  func body(content: Content) -> some View {
    if #available(macOS 26.0, *) {
      content.buttonStyle(.glass).buttonBorderShape(.circle).controlSize(.regular)
    } else {
      content.buttonStyle(.bordered).buttonBorderShape(.circle).controlSize(.regular)
    }
  }
}

/// Identical controls in the roster and conversation; neither owns capture or playback.
struct OverlayAgentAudioControls: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
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

  private var speakingNow: Bool { speaking && muted == false }

  /// Maciej, 2026-10-18: the channel-0 microphone "dwa razy większa w
  /// średnicy". Only the broadcast microphone grows; its speaker stays.
  private var microphoneScale: CGFloat { broadcast ? 2 : 1 }

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
    HStack(spacing: 6) {
      Button(action: onMicrophone) {
        Image(systemName: open ? "mic.fill" : "mic")
          .font(.system(size: 12 * microphoneScale, weight: .semibold))
          .foregroundStyle(open ? Color.red : palette.primaryText.color)
          .symbolEffect(.pulse, isActive: open && !reduceMotion)
          .frame(width: 16 * microphoneScale, height: 16 * microphoneScale)
      }
      .tint(Self.litGlass(open ? Color.red : nil))
      .csFocusOutline()
      .disabled(!microphoneEnabled)
      .help(microphoneLabel)
      .accessibilityLabel(microphoneLabel)
      .accessibilityValue(
        open ? String(localized: "Microphone active") : String(localized: "Microphone off")
      )
      .accessibilityIdentifier("overlay-agent-microphone")

      Button(action: onPlayback) {
        Image(
          systemName: muted == nil
            ? "speaker.badge.exclamationmark"
            : muted == true
              ? "speaker.slash.fill" : speakingNow ? "speaker.wave.3.fill" : "speaker.wave.2"
        )
        .font(.system(size: 12, weight: .semibold))
        .foregroundStyle(speakingNow ? palette.agentAccent.color : palette.primaryText.color)
        .symbolEffect(.variableColor.iterative, isActive: speakingNow && !reduceMotion)
        .frame(width: 16, height: 16)
      }
      .tint(Self.litGlass(speakingNow ? palette.agentAccent.color : nil))
      .csFocusOutline()
      .disabled(!playbackEnabled || muted == nil)
      .help(speakingNow ? String(localized: "Speaking") + ". " + playbackLabel : playbackLabel)
      .accessibilityLabel(playbackLabel)
      .accessibilityValue(playbackValue)
      .accessibilityIdentifier("overlay-agent-speaker")
    }
    .modifier(OverlayAgentControlStyle())
    // Founder, 2026-10-15: an I-beam showed between and over these buttons.
    // They float above message and transcript text, whose tracking set the
    // text cursor; the pair and its gap claim the arrow like the drawer does.
    .background { OverlayMenuCursorRegion() }
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
  @State private var pending: Set<AgentPlaybackIdentity> = []
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
        content.contextMenu {
          if members.count == 1, let member = members.first {
            policyActions(for: member)
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
        if let commandError {
          Text(verbatim: commandError)
            .foregroundStyle(palette.errorStatus.color)
            .accessibilityIdentifier("overlay-broadcast-policy-error")
        }
      }
    }
  }

  @ViewBuilder
  private func policyActions(for channel: OverlayChannelDelivery) -> some View {
    // Two STATE checkboxes in the menu's native state column (Maciej,
    // 2026-10-17): "Channel 0" is membership in the zero broadcasts, "Steer"
    // is wake-versus-queue — and since the soft-queue decision, Steer governs
    // every delivery, direct and peer included. (channel0, steer) maps onto
    // the wire policy: (on, on) = on, (on, off) = quiet, (off, _) = off.
    let locked =
      !enabled || channel.broadcastIdentity == nil
      || channel.broadcastIdentity.map { pending.contains($0) } == true
    Toggle(
      isOn: Binding(
        get: { channel.broadcastPolicy != .off },
        set: { member in apply(member ? .on : .off, to: channel) }
      )
    ) {
      Text("Channel 0", comment: "Pill context menu: membership in channel-zero broadcasts")
    }
    .disabled(locked)
    Toggle(
      isOn: Binding(
        // Off keeps directs waking (the wire has no off+quiet state), so the
        // disabled Steer box must still read ticked — an unticked box over
        // deliveries that ring would lie (stasiu's falsifier, 2026-10-17).
        get: { channel.broadcastPolicy != .quiet },
        set: { steer in apply(steer ? .on : .quiet, to: channel) }
      )
    ) {
      // The user-facing label is "Wake the agent" (Founder, 2026-10-18); the
      // wire policy and the code keep the `steer` name.
      Text(
        "Wake the agent",
        comment:
          "Pill context menu: ticked, deliveries wake the agent; unticked, they wait in its mailbox"
      )
    }
    .disabled(locked || channel.broadcastPolicy == .off)
  }

  private func apply(_ policy: AgentBroadcastPolicy, to channel: OverlayChannelDelivery) {
    guard policy != channel.broadcastPolicy, let identity = channel.broadcastIdentity,
      !pending.contains(identity)
    else { return }
    pending.insert(identity)
    commandError = nil
    Task { @MainActor in
      defer { pending.remove(identity) }
      do {
        try await Self.setPolicy(policy, for: identity)
      } catch {
        commandError = String(localized: "Couldn't change broadcast reception.")
      }
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

  /// An open channel records for its agent; the button then is that agent's
  /// microphone, violet and pulsing (Maciej, 2026-10-19 08:52), drawn as a
  /// megaphone so it never reads as a second dictation microphone (09:36).
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
      OverlayMicrophoneGlyph(symbol: "megaphone.fill", tint: OverlayRecordingLight.agent.color)
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
    return VStack(spacing: 8) {
      OverlaySidebarToggleButton(
        symbol: "sidebar.right", title: String(localized: "Show agents list"),
        identifier: "overlay-agent-rail-unfold", palette: palette, textScale: textScale,
        action: onUnfold)
      ScrollView(.vertical) {
        VStack(spacing: 6) {
          if let broadcast = currentConversations.first(where: { $0.channel == "0" }) {
            railPill(broadcast, channel: nil)
          }
          ForEach(channels) { channel in
            railPill(conversation(for: channel), channel: channel)
          }
        }
        .padding(.vertical, 2)
        .frame(maxWidth: .infinity)
        .csAutohidingScrollers()
      }
      .frame(maxHeight: max(0, maximumHeight - unfold - 8))
    }
    // The list header sits 8 points into the roster content: the unfold
    // button lands where `sidebar.right` folded it.
    .padding(.top, 8)
    .frame(maxWidth: .infinity)
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-agent-rail-pills")
  }

  /// Status dot and channel number; the open conversation in the agent
  /// accent, unread replies as the row badge's colour.
  private func railPill(
    _ conversation: OverlayConversation?, channel: OverlayChannelDelivery?
  ) -> some View {
    let digit = channel?.channel ?? "0"
    let selected = conversation != nil && conversation?.id == selectedConversationID
    let unread = conversation.flatMap { unreadCounts[$0.id] } ?? 0
    let title = channel.map { notificationTitle(for: $0) } ?? broadcastHeader
    return Button {
      if let conversation { onSelectConversation?(conversation.id) }
    } label: {
      HStack(spacing: 3) {
        OverlayAgentStatusDot(
          kind: channel.map { statusDotKind(for: $0) } ?? .broadcast, palette: palette)
        Text(verbatim: digit)
          .font(.system(size: 12 * textScale, weight: .semibold))
          .monospacedDigit()
      }
      .foregroundStyle(palette.primaryText.color)
      .padding(.horizontal, 6 * textScale)
      .frame(height: 24 * textScale)
      .background(
        Capsule(style: .continuous)
          .fill(selected ? palette.agentAccent.color.opacity(0.22) : CSColor.hairline(0.06))
      )
      .overlay(alignment: .topTrailing) {
        if unread > 0 {
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
    .disabled(conversation == nil || onSelectConversation == nil)
    .modifier(OverlayDisconnectedRowDim(disconnected: channel.map { hasDeadFollower($0) } ?? false))
    .help(title)
    .accessibilityLabel(Text(verbatim: title))
    .accessibilityAddTraits(selected ? .isSelected : [])
    .accessibilityIdentifier("overlay-agent-rail-\(digit)")
  }

  private func scaledMonitorBody(maximumHeight: CGFloat, focusRevision: UInt64) -> some View {
    OverlayAgentMonitorBody(
      maximumHeight: maximumHeight, palette: palette,
      focusRevision: focusRevision, focusedRowID: selectedConversationID,
      scrollTargetID: gestureFocusID
    ) {
      HStack(spacing: 6) {
        if let sidebarControls {
          OverlaySidebarToggleButton(
            symbol: "sidebar.left", title: sidebarControls.canvasTitle,
            identifier: "overlay-agent-sidebar-toggle-canvas", palette: palette,
            textScale: textScale, action: sidebarControls.onToggleCanvas)
        }
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
    } content: { rowFocus in
      VStack(alignment: .leading, spacing: 6) {
        if let broadcast = currentConversations.first(where: { $0.channel == "0" }),
          onSelectConversation != nil
        {
          rowGlassContainer(
            HStack(spacing: 6) {
              broadcastRow(broadcast, rowFocus: rowFocus)
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
              // Maciej, 2026-10-18: the big microphone "powinien być w linii z
              // resztą mikrofonów". Agent rows end with the archive cross and a
              // 6 pt inset; keeping that empty slot puts the broadcast speaker
              // in the agents' speaker column and its microphone beside it.
              if onArchiveAgent != nil, !archiveCandidates.isEmpty { archiveSlot }
            }
            .padding(.trailing, 6)
          )
          // The shared channel is a roster row: the same padding, the same
          // selection mark and the same hover as the agents under it.
          .padding(.vertical, 3)
          .padding(.horizontal, 5)
          .modifier(
            OverlayRosterRowSelection(
              palette: palette, selected: selectedConversationID == broadcast.id)
          )
          .modifier(OverlayRowHoverHighlight(palette: palette, cornerRadius: 10))
          .modifier(
            OverlayGestureFocusRing(
              focused: broadcast.id == gestureFocusID,
              shape: RoundedRectangle(cornerRadius: 10), palette: palette,
              targetID: broadcast.id, onHover: onGestureHover)
          )
          .id(broadcast.id)
          .modifier(
            OverlayBroadcastPolicyMenu(
              members: broadcastMembers, palette: palette, enabled: !unavailable))
        }
        Divider()
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
    conversations.filter { conversation in
      if conversation.channel == "0" { return true }
      if let owner = conversation.owner,
        archivedOwners.contains(where: { $0.id == owner.id && $0.channel == owner.channel })
      {
        return false
      }
      guard let owner = conversation.owner, let hud = hudStates[conversation.channel],
        hud.provider == owner.provider, hud.providerSessionID == owner.providerSessionID
      else { return false }
      // A roster without a lease cannot distinguish two leases of the same
      // provider session. Keep both as saved conversations rather than guess.
      return conversations.filter {
        $0.channel == conversation.channel && $0.owner?.provider == hud.provider
          && $0.owner?.providerSessionID == hud.providerSessionID
      }.count == 1
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

  func showsPanelFocusButton(for channel: OverlayChannelDelivery) -> Bool {
    panelFocusAvailable && archiveCandidates[channel.channel] != nil && onFocusAgentPanel != nil
  }

  func viewConversation(_ channel: OverlayChannelDelivery) {
    guard let conversation = conversation(for: channel) else { return }
    if selectedConversationID == conversation.id,
      showsPanelFocusButton(for: channel),
      let owner = archiveCandidates[channel.channel],
      !hasDeadFollower(channel),
      !hasNoFramePane(channel),
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

  /// The archive cross's footprint, laid out but never drawn or focused.
  private var archiveSlot: some View {
    Button {
    } label: {
      Image(systemName: "xmark")
        .font(.system(size: 12, weight: .semibold))
        .frame(width: 16, height: 16)
    }
    .modifier(OverlayAgentControlStyle())
    .disabled(true)
    .hidden()
    .accessibilityHidden(true)
  }

  func channelRow(
    _ channel: OverlayChannelDelivery, rowFocus: FocusState<String?>.Binding? = nil
  ) -> some View {
    let conversation = conversation(for: channel)
    let receiptText = deliveryReceiptText(for: channel, conversation: conversation)
    let open = isOpen(channel)
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
    let tooltip = tooltipParts.joined(separator: ". ")
    let label = rosterRowLabel(
      title: "\(channel.channel) · \(conversation?.name ?? channel.agent)",
      statusChannel: channel, conversation: conversation, dot: statusDotKind(for: channel),
      receiptText: receiptText)
    return rowGlassContainer(
      HStack(spacing: 6) {
        Group {
          if let conversation, onSelectConversation != nil {
            Button {
              viewConversation(channel)
            } label: {
              label
            }
            .buttonStyle(.plain)
            .modifier(OptionalRowFocus(binding: rowFocus, id: conversation.id))
          } else if showsPanelFocusButton(for: channel),
            let owner = archiveCandidates[channel.channel],
            !hasDeadFollower(channel),
            let onFocusAgentPanel
          {
            Button {
              onFocusAgentPanel(owner)
            } label: {
              label
            }
            .buttonStyle(.plain)
          } else {
            label.accessibilityElement(children: .combine)
          }
        }
        .foregroundStyle(palette.primaryText.color)
        .help(
          (conversation == nil
            ? String(localized: "No messages yet")
            : String(localized: "View conversation without changing the microphone"))
            + ". " + status
        )
        .accessibilityLabel(Text(verbatim: notificationTitle(for: channel)))
        .accessibilityValue(Text(verbatim: tooltip))
        .accessibilityIdentifier("overlay-view-conversation-\(conversation?.id ?? channel.channel)")

        OverlayAgentAudioControls(
          open: open, muted: mutedChannels[channel.channel],
          microphoneEnabled: !unavailable && onToggleChannel != nil
            && !pendingToggleChannels.contains(channel.channel)
            && Self.toggleDigit(for: channel.channel) != nil
            && !hasDeadFollower(channel),
          playbackEnabled: onTogglePlayback != nil
            && !pendingMuteChannels.contains(channel.channel),
          palette: palette, onMicrophone: { toggle(channel) },
          onPlayback: { onTogglePlayback?(channel.channel) },
          speaking: conversation?.replySpeaking == true
        )
        .fixedSize()
        .accessibilityIdentifier("overlay-channel-toggle-\(channel.channel)")
        if showsPanelFocusButton(for: channel),
          let owner = archiveCandidates[channel.channel],
          let onFocusAgentPanel
        {
          let focusState = panelFocusStates[channel.channel]
          let focusHelp = panelFocusHelp(for: channel, state: focusState)
          Button {
            onFocusAgentPanel(owner)
          } label: {
            Group {
              if focusState?.isRequesting == true {
                ProgressView()
                  .controlSize(.mini)
              } else {
                Image(systemName: panelFocusSymbol(focusState))
                  .font(.system(size: 11, weight: .semibold))
                  .foregroundStyle(
                    focusState?.isSucceeded == true
                      ? palette.successStatus.color
                      : focusState?.isFailed == true
                        ? palette.errorStatus.color
                        : hasNoFramePane(channel)
                          ? palette.mutedText.color : palette.primaryText.color
                  )
              }
            }
            .frame(width: 16, height: 16)
          }
          .modifier(OverlayAgentControlStyle())
          .csFocusOutline()
          .disabled(
            focusState?.isRequesting == true || hasDeadFollower(channel)
              || hasNoFramePane(channel)
          )
          .modifier(
            OverlayHeaderControlChrome(title: focusHelp, accent: nil, palette: palette)
          )
          .accessibilityLabel(Text(verbatim: focusHelp))
          .accessibilityIdentifier("overlay-focus-panel-\(channel.channel)")
        }
        // The cross is on every bound row (Founder, 2026-10-14: "x dostępny
        // tam zawsze"); only an open microphone or an archive in flight
        // disables it, and the help says which.
        if let owner = archiveCandidates[channel.channel], onArchiveAgent != nil {
          let archiveHelp = archiveHelp(for: channel)
          Button {
            onArchiveAgent?(owner)
          } label: {
            Image(systemName: "xmark")
              .font(.system(size: 12, weight: .semibold))
              .foregroundStyle(palette.primaryText.color)
              .frame(width: 16, height: 16)
          }
          .modifier(OverlayAgentControlStyle())
          .csFocusOutline()
          .disabled(pendingArchives.contains(owner) || open)
          .help(archiveHelp)
          .accessibilityLabel(Text(verbatim: archiveHelp))
          .accessibilityIdentifier("overlay-archive-agent-\(channel.channel)")
        }
      }
    )
    .padding(.vertical, 3)
    .padding(.horizontal, 5)
    .modifier(
      OverlayRosterRowSelection(
        palette: palette,
        selected: conversation != nil && selectedConversationID == conversation?.id)
    )
    .modifier(
      OverlayRowHoverHighlight(
        palette: palette, cornerRadius: 10,
        enabled: (conversation != nil && onSelectConversation != nil)
          || (showsPanelFocusButton(for: channel) && !hasDeadFollower(channel))
      )
    )
    .modifier(
      OverlayGestureFocusRing(
        focused: conversation != nil && conversation?.id == gestureFocusID,
        shape: RoundedRectangle(cornerRadius: 10), palette: palette,
        targetID: conversation?.id, onHover: onGestureHover)
    )
    .id(conversation?.id ?? "overlay-channel-\(channel.channel)")
    .help(tooltip)
    .modifier(OverlayDisconnectedRowDim(disconnected: hasDeadFollower(channel)))
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-agent-row-\(channel.channel)")
    .modifier(
      OverlayBroadcastPolicyMenu(
        members: broadcastMembers.filter { $0.channel == channel.channel },
        palette: palette, enabled: !unavailable))
  }

  /// One roster row in at most five lines (Maciej, 2026-10-17: "ustalmy budżet
  /// 5 wierszy"): identity with the channel-0 mark, the unread badge and the
  /// time; the provider descriptor; a two-line preview that names who
  /// addressed whom; one state line. The badge counts replies the Founder has
  /// not opened, the state line carries the agent's receipts, so the two
  /// never mix. The five lines are a budget, not a shape: they sit on one
  /// leading, so the card stays dense beside the conversation it steals
  /// width from (Founder, 2026-10-18, on the drawer's padding).
  @ViewBuilder
  private func rosterRowLabel(
    title: String, statusChannel: OverlayChannelDelivery?,
    conversation: OverlayConversation?, dot: OverlayAgentDotKind,
    receiptText: String? = nil
  ) -> some View {
    HStack(alignment: .top, spacing: 6) {
      OverlayAgentStatusDot(kind: dot, palette: palette)
        .padding(.top, 5 * textScale)
        .accessibilityLabel(
          Text(verbatim: statusChannel.map { shortStatus(for: $0) } ?? "")
        )
        .accessibilityValue(
          Text(verbatim: statusChannel.map { detail(for: $0) } ?? "")
        )
        .accessibilityIdentifier(
          statusChannel.map { "overlay-channel-delivery-\($0.channel)" }
            ?? "overlay-channel-delivery"
        )
      VStack(alignment: .leading, spacing: 1) {
        HStack(spacing: 6) {
          // Maciej 2026-10-17: names were cut to "6…r" in the sidebar because
          // the fixed-size time won the squeeze. The name is offered room
          // first and the time is compact.
          Text(verbatim: title)
            .font(.system(size: 13.5 * textScale, weight: .semibold))
            .lineLimit(1)
            .truncationMode(.middle)
            .layoutPriority(1)
          if let statusChannel {
            broadcastPolicyMark(statusChannel.broadcastPolicy)
              .accessibilityIdentifier("overlay-broadcast-policy-\(statusChannel.channel)")
          }
          Spacer(minLength: 4)
          if let conversation, let count = unreadCounts[conversation.id], count > 0 {
            unreadBadge(count)
              .accessibilityIdentifier("overlay-agent-unread-replies-\(conversation.id)")
          }
          if let last = conversation?.messages.last,
            let date = MessageTimestamp.date(fromISO8601: last.emittedAt)
          {
            MessageTimestampLabel(date: date, compact: true)
              .font(.system(size: 11 * textScale))
              .foregroundStyle(palette.mutedText.color)
          }
        }
        if let statusChannel, let descriptor = agentDescriptor(for: statusChannel) {
          Text(verbatim: descriptor)
            .font(.system(size: 11 * textScale))
            .foregroundStyle(palette.mutedText.color)
            .lineLimit(1)
            .truncationMode(.middle)
            .help(descriptor)
            .accessibilityIdentifier("overlay-agent-descriptor-\(statusChannel.channel)")
        }
        if let conversation, let last = conversation.messages.last {
          Text(verbatim: Self.drawerPreview(message: last))
            .font(.system(size: 12.5 * textScale))
            .foregroundStyle(
              (unreadCounts[conversation.id] ?? 0) > 0
                ? palette.bodyText.color : palette.mutedText.color
            )
            .lineLimit(2)
            .fixedSize(horizontal: false, vertical: true)
        }
        if let statusChannel {
          HStack(spacing: 5) {
            if panelFocusStates[statusChannel.channel]?.isRequesting == true {
              ProgressView()
                .controlSize(.mini)
                .accessibilityIdentifier("overlay-agent-focus-spinner-\(statusChannel.channel)")
            }
            Text(verbatim: rowStateText(for: statusChannel, receiptText: receiptText))
              .font(.system(size: 11 * textScale, weight: .medium))
              .foregroundStyle(rowStateColor(for: statusChannel))
              .lineLimit(1)
              .truncationMode(.tail)
              .accessibilityIdentifier("overlay-agent-state-\(statusChannel.channel)")
          }
        }
      }
    }
    .contentShape(Rectangle())
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
    case .requesting: return palette.processingStatus.color
    case .succeeded: return palette.successStatus.color
    case .failed: return palette.mutedText.color
    }
  }

  /// The live follower attached without vc-frame pane coordinates, so a focus
  /// request has nowhere to land: the control rests greyed instead of failing
  /// on click (Maciej, 2026-10-19: "przycisków kierowania do frame brak gdy
  /// frame session nie wykryty u agenta - wyszarzony").
  func hasNoFramePane(_ channel: OverlayChannelDelivery) -> Bool {
    hudStates[channel.channel]?.frameDetected == false
  }

  func panelFocusSymbol(_ state: OverlayAgentPanelFocusState?) -> String {
    if state?.isSucceeded == true { return "checkmark" }
    if state?.isFailed == true { return "exclamationmark.triangle" }
    return "macwindow.on.rectangle"
  }

  func panelFocusHelp(
    for channel: OverlayChannelDelivery, state: OverlayAgentPanelFocusState?
  ) -> String {
    if hasDeadFollower(channel) {
      return String(localized: "Agent is disconnected")
    }
    if hasNoFramePane(channel) {
      return String(localized: "Agent is not in a Frame pane")
    }
    if let state {
      return state.statusText
    }
    return String(localized: "Focus agent panel in Frame")
  }

  /// Every agent row and pill wears its channel-0 state: the antenna in the
  /// accent while deliveries wake, the queue tray while they wait, the
  /// slashed antenna off channel 0.
  private func broadcastPolicyMark(_ policy: AgentBroadcastPolicy) -> some View {
    Image(systemName: policy.statusSymbol)
      .font(.system(size: 10 * textScale))
      .foregroundStyle(policy == .on ? palette.agentAccent.color : palette.mutedText.color)
      .help(policy.statusHelp)
      .accessibilityLabel(policy.statusHelp)
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
      rosterRowLabel(
        title: broadcastHeader, statusChannel: nil, conversation: broadcast, dot: .broadcast)
    }
    .buttonStyle(.plain)
    .modifier(OptionalRowFocus(binding: rowFocus, id: broadcast.id))
    .foregroundStyle(palette.primaryText.color)
    .help(String(localized: "View conversation without changing the microphone"))
    .accessibilityIdentifier("overlay-view-conversation-\(broadcast.id)")
  }

  private func unreadBadge(_ count: Int) -> some View {
    Text(verbatim: String(count))
      .font(.system(size: 11 * textScale, weight: .semibold))
      .monospacedDigit()
      .foregroundStyle(palette.processingStatus.color)
      .padding(.horizontal, 6).padding(.vertical, 2)
      .background(palette.processingStatus.color.opacity(0.12), in: Capsule())
      .accessibilityLabel("\(count) unread replies")
  }

  private func conversationRow(
    _ conversation: OverlayConversation, saved: Bool,
    rowFocus: FocusState<String?>.Binding? = nil
  ) -> some View {
    Button {
      onSelectConversation?(conversation.id)
    } label: {
      HStack(spacing: 10) {
        Image(systemName: saved ? "clock" : "bubble.left")
          .font(.system(size: 12))
          .foregroundStyle(palette.mutedText.color)
          .frame(width: 16)
        VStack(alignment: .leading, spacing: 2) {
          if conversation.channel == "0" {
            Text(verbatim: broadcastHeader)
          } else {
            Text(verbatim: "\(conversation.channel) · \(conversation.name)")
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
          unreadBadge(count)
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
    .modifier(OverlayRowHoverHighlight(palette: palette, cornerRadius: 9, outset: 6))
    .help("View conversation without changing the microphone")
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
    rosterReady && channels.isEmpty && !currentConversations.contains(where: { $0.channel != "0" })
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
            conversation.channel != "0" && !channels.contains { $0.channel == conversation.channel }
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
            .accessibilityLabel(Text("Clear search"))
            .accessibilityIdentifier("overlay-saved-conversations-search-clear")
          }
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 4)
        .background(palette.mutedText.color.opacity(0.08), in: RoundedRectangle(cornerRadius: 6))

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
          .help(error)
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
    if isOpen(channel) { return "mic.fill" }
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
    if isOpen(channel) { return .listening }
    if hasDeadFollower(channel) || hasStalledFollower(channel) { return .sleeping }
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

  /// Channel 0 first, then the bound channels in roster order, then any
  /// passive current conversation the roster snapshot has not caught up with.
  static func rosterOrder(
    current: [OverlayConversation], channels: [OverlayChannelDelivery]
  ) -> [OverlayConversation] {
    var ordered: [OverlayConversation] = []
    if let zero = current.first(where: { $0.channel == "0" }) {
      ordered.append(zero)
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

  /// The drawer's one-glance preview of the last message, naming who
  /// addressed whom (Maciej, 2026-10-17: "brak informacji jak who addressed
  /// whom"). The route comes from the message itself — its source channel and
  /// frozen recipients — never from the row showing it: a broadcast copied
  /// into an agent's row still reads "→ All". Newlines fold so the two-line
  /// cap holds.
  static func drawerPreview(message: OverlayConversationMessage) -> String {
    let body = message.bodyText.replacingOccurrences(of: "\n", with: " ")
    let target = recipientNames(of: message) ?? message.sourceChannel
    if message.kind == .user {
      if message.sourceChannel == "0" {
        return String(
          localized: "You → All: \(body)",
          comment: "Drawer preview of the user's channel-0 broadcast; the placeholder is the text")
      }
      guard let target else {
        return String(
          localized: "You: \(body)",
          comment: "Drawer preview of the user's own message; the placeholder is the text")
      }
      return String(
        localized: "You → \(target): \(body)",
        comment: "Drawer preview of the user's direct message: the agent names, then the text")
    }
    guard let sender = message.owner?.name, !sender.isEmpty else { return body }
    // A peer message records its destination channel; a reply without one or
    // addressed to channel 10 answered the Founder.
    guard let destination = message.sourceChannel, destination != "10" else {
      return String(
        localized: "\(sender) → You: \(body)",
        comment: "Drawer preview of an agent's reply to the user: the agent, then the text")
    }
    if destination == "0" {
      return String(
        localized: "\(sender) → All: \(body)",
        comment: "Drawer preview of an agent's message to channel 0: the agent, then the text")
    }
    return "\(sender) → \(target ?? destination): \(body)"
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
private struct OverlayAgentMonitorBody<Header: View, Content: View>: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  let maximumHeight: CGFloat
  let palette: OverlayAppearancePalette
  var focusRevision: UInt64 = 0
  var focusedRowID: String? = nil
  var scrollTargetID: String? = nil
  @State private var headerHeight: CGFloat = 22
  @State private var contentHeight: CGFloat?
  @FocusState private var rowFocus: String?
  @ViewBuilder let header: Header
  let contentBuilder: (FocusState<String?>.Binding) -> Content

  init(
    maximumHeight: CGFloat, palette: OverlayAppearancePalette,
    focusRevision: UInt64 = 0, focusedRowID: String? = nil, scrollTargetID: String? = nil,
    @ViewBuilder header: () -> Header,
    @ViewBuilder content: @escaping (FocusState<String?>.Binding) -> Content
  ) {
    self.maximumHeight = maximumHeight
    self.palette = palette
    self.focusRevision = focusRevision
    self.focusedRowID = focusedRowID
    self.scrollTargetID = scrollTargetID
    self.header = header()
    contentBuilder = content
  }

  private var viewportHeight: CGFloat {
    let available = max(0, maximumHeight - headerHeight - 8 - 16)
    return min(contentHeight ?? available, available)
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
          .onChange(of: scrollTargetID) { _, target in
            guard let target else { return }
            withAnimation(reduceMotion ? nil : .easeOut(duration: 0.15)) {
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
