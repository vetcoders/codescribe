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
        Text(character)
          .font(.system(size: fontSize, weight: .medium, design: .monospaced))
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

  /// Tray plain rows use `CSColor.surfaceRaised(0.05)`. The clear branch
  /// still names the row's canvas so a disabled row stays on its palette.
  private var wash: Color {
    guard hovered, enabled else { return palette.desktopBackground.color.opacity(0) }
    return CSColor.surfaceRaised(0.05)
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
          .font(.system(size: 12, weight: .semibold))
          .foregroundStyle(open ? Color.red : palette.primaryText.color)
          .symbolEffect(.pulse, isActive: open && !reduceMotion)
          .frame(width: 16, height: 16)
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
                if let symbol = member.broadcastPolicy.statusSymbol {
                  Label {
                    Text(verbatim: "\(member.channel) · \(member.agent)")
                  } icon: {
                    Image(systemName: symbol)
                  }
                } else {
                  Text(verbatim: "\(member.channel) · \(member.agent)")
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
      Text(
        "Steer (otherwise queued)",
        comment: "Pill context menu: deliveries wake the agent; unticked, they queue silently")
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
  @Environment(\.csTextScale) private var textScale

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
  var mutedChannels: [String: Bool] = [:]
  var pendingMuteChannels: Set<String> = []
  var onTogglePlayback: ((String) -> Void)?
  var playbackError: String?
  var archiveCandidates: [String: OverlayConversationOwner] = [:]
  var pendingArchives: Set<OverlayConversationOwner> = []
  var archivedOwners: Set<OverlayConversationOwner> = []
  var onArchiveAgent: ((OverlayConversationOwner) -> Void)?
  var archiveError: String?
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

  var body: some View {
    Button(action: showMonitor) {
      // The chrome owns the selected wash; the glyph drops its own plate.
      OverlayMicrophoneGlyph(
        character: OverlayHeaderGlyph.agents,
        tint: headerSelected
          ? OverlayHeaderAccent.agents.selectionTint(in: palette) : palette.mutedText.color,
        plateOpacity: headerSelected ? 0 : 0.12
      )
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
        title: String(localized: "Agents"), accent: .agents, palette: palette,
        selected: headerSelected)
    )
    .accessibilityLabel("Agents")
    .accessibilityAddTraits(headerSelected ? .isSelected : [])
    .accessibilityValue(glyph.label)
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
  /// docked name pill's channel for focusing the selected row. The narrow
  /// window's surface is `pillStack(onSelect:onDismiss:)`.
  func monitorBody(maximumHeight: CGFloat, focusRevision: UInt64 = 0) -> some View {
    OverlayAgentMonitorBody(
      maximumHeight: maximumHeight, palette: palette,
      focusRevision: focusRevision, focusedRowID: selectedConversationID
    ) {
      HStack {
        Text("Agents").font(.system(size: 13 * textScale, weight: .semibold))
        Spacer()
        Text(verbatim: String(channels.count))
          .font(.system(size: 11 * textScale, design: .monospaced))
          .foregroundStyle(palette.mutedText.color)
          .accessibilityHidden(true)
      }
    } content: { rowFocus in
      VStack(alignment: .leading, spacing: 8) {
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
            }
          )
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

  func viewConversation(_ channel: OverlayChannelDelivery) {
    guard let conversation = conversation(for: channel) else { return }
    onSelectConversation?(conversation.id)
  }

  private func rowGlassContainer<Content: View>(_ content: Content) -> some View {
    overlayGlassGroup(content)
  }

  private func channelRow(
    _ channel: OverlayChannelDelivery, rowFocus: FocusState<String?>.Binding? = nil
  ) -> some View {
    let conversation = conversation(for: channel)
    let open = isOpen(channel)
    let descriptor = agentDescriptor(for: channel)
    let status = statusHelp(for: channel)
    // Keep the full descriptor in the tooltip as well as the visible row.
    var tooltipParts: [String] = []
    if let descriptor, !descriptor.isEmpty { tooltipParts.append(descriptor) }
    tooltipParts.append(status)
    let tooltip = tooltipParts.joined(separator: ". ")
    let label = rosterRowLabel(
      title: "\(channel.channel) · \(conversation?.name ?? channel.agent)",
      statusChannel: channel, conversation: conversation, dot: statusDotKind(for: channel))
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
        .accessibilityValue(Text(verbatim: descriptor.map { $0 + ". " + status } ?? status))
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
    .padding(.vertical, 4)
    .padding(.horizontal, 6)
    .background(
      selectedConversationID == conversation?.id && conversation != nil
        ? palette.mutedText.color.opacity(0.10) : .clear,
      in: RoundedRectangle(cornerRadius: 10)
    )
    .modifier(
      OverlayRowHoverHighlight(
        palette: palette, cornerRadius: 10,
        enabled: conversation != nil && onSelectConversation != nil)
    )
    .help(tooltip)
    .modifier(OverlayDisconnectedRowDim(disconnected: hasDeadFollower(channel)))
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-agent-row-\(channel.channel)")
    .modifier(
      OverlayBroadcastPolicyMenu(
        members: broadcastMembers.filter { $0.channel == channel.channel },
        palette: palette, enabled: !unavailable))
  }

  /// The roster keeps identity above the preview in both wide and narrow modes.
  /// Unread rows brighten the preview; the badge sits before the time.
  @ViewBuilder
  private func rosterRowLabel(
    title: String, statusChannel: OverlayChannelDelivery?,
    conversation: OverlayConversation?, dot: OverlayAgentPillEntry.DotKind
  ) -> some View {
    HStack(alignment: .top, spacing: 7) {
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
      VStack(alignment: .leading, spacing: 2) {
        HStack(spacing: 6) {
          // Maciej 2026-10-17: names were cut to "6…r" in the sidebar because
          // the fixed-size time won the squeeze. The name is offered room
          // first and the time is compact.
          Text(verbatim: title)
            .font(.system(size: 13.5 * textScale, weight: .semibold))
            .lineLimit(1)
            .truncationMode(.middle)
            .layoutPriority(1)
          if let statusChannel, let symbol = statusChannel.broadcastPolicy.statusSymbol {
            Image(systemName: symbol)
              .font(.system(size: 10 * textScale))
              .foregroundStyle(palette.mutedText.color)
              .help(String(localized: "Broadcasts do not wake this agent"))
              .accessibilityLabel(String(localized: "Broadcasts do not wake this agent"))
              .accessibilityIdentifier("overlay-broadcast-quiet-\(statusChannel.channel)")
          }
          Spacer(minLength: 4)
          if let conversation, let count = unreadCounts[conversation.id], count > 0 {
            unreadBadge(count)
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
            .lineLimit(2)
            .truncationMode(.tail)
            .help(descriptor)
            .accessibilityIdentifier("overlay-agent-descriptor-\(statusChannel.channel)")
        }
        if let conversation, let last = conversation.messages.last {
          Text(verbatim: Self.drawerPreview(message: last, channel: conversation.channel))
            .font(.system(size: 12.5 * textScale))
            .foregroundStyle(
              (unreadCounts[conversation.id] ?? 0) > 0
                ? palette.bodyText.color : palette.mutedText.color
            )
            .lineLimit(2)
            .fixedSize(horizontal: false, vertical: true)
        }
      }
    }
    .contentShape(Rectangle())
  }

  /// The "0 · All" row keeps the roster's two-line model; its title carries
  /// the broadcast wake count and the disclosure under it the policy menus.
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
    // The shared channel stands apart from the agent rows: an accent wash,
    // deeper while selected (Maciej, 2026-10-17: "wyróżnić bardziej kanał ogólny").
    .background(
      palette.agentAccent.color.opacity(selectedConversationID == broadcast.id ? 0.18 : 0.08),
      in: RoundedRectangle(cornerRadius: 10)
    )
    .accessibilityIdentifier("overlay-view-conversation-\(broadcast.id)")
  }

  private func unreadBadge(_ count: Int) -> some View {
    Text(verbatim: String(count))
      .font(.system(size: 11 * textScale, weight: .semibold, design: .monospaced))
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
        }
      }
      if unavailable {
        Text("Channel status unavailable — last open state retained")
          .font(.system(size: 13, weight: .medium))
          .foregroundStyle(palette.processingStatus.color)
          .accessibilityIdentifier("overlay-channel-status-unavailable")
      }
      if let toggleError {
        // A refused toggle is an error, not a pending state (N roster palette).
        Text("Channel toggle failed: \(toggleError)")
          .font(.system(size: 13, weight: .medium))
          .foregroundStyle(palette.errorStatus.color)
          .accessibilityIdentifier("overlay-channel-toggle-error")
      }
      if let playbackError {
        Text(verbatim: playbackError)
          .font(.system(size: 11))
          .foregroundStyle(palette.errorStatus.color)
          .accessibilityIdentifier("overlay-agent-playback-error")
      }
      if let archiveError {
        Text(verbatim: archiveError)
          .font(.system(size: 11))
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

  private func shortStatus(for channel: OverlayChannelDelivery) -> String {
    if unavailable { return String(localized: "Status unavailable") }
    if hasDeadFollower(channel) { return String(localized: "Disconnected") }
    if hasStalledFollower(channel) {
      return String(localized: "Not reading", comment: "Agent alive, backlog unread for long")
    }
    if isOpen(channel) { return String(localized: "Listening") }
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
  func statusDotKind(for channel: OverlayChannelDelivery) -> OverlayAgentPillEntry.DotKind {
    if unavailable { return .idle }
    if isOpen(channel) { return .listening }
    if hasDeadFollower(channel) || hasStalledFollower(channel) { return .sleeping }
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

  /// One pill per conversation other than the selected one, 0 · All first.
  /// Transcription is not a row: the header's own control opens it (Maciej,
  /// 2026-10-17: "te transkrypcje możemy wywalić z listy").
  var pillEntries: [OverlayAgentPillEntry] {
    rosterOrderedConversations
      .filter { $0.id != selectedConversationID }
      .map(pillEntry(for:))
  }

  /// Channel 0 first, then the bound channels in roster order, then any
  /// passive current conversation the roster snapshot has not caught up with.
  var rosterOrderedConversations: [OverlayConversation] {
    var ordered: [OverlayConversation] = []
    if let zero = currentConversations.first(where: { $0.channel == "0" }) {
      ordered.append(zero)
    }
    for channel in channels where channel.channel != "0" {
      if let conversation = conversation(for: channel) {
        ordered.append(conversation)
      }
    }
    for conversation in currentConversations
    where conversation.channel != "0" && !ordered.contains(where: { $0.id == conversation.id }) {
      ordered.append(conversation)
    }
    return ordered
  }

  private func pillEntry(for conversation: OverlayConversation) -> OverlayAgentPillEntry {
    let channel = channels.first { $0.channel == conversation.channel }
    let descriptor = channel.flatMap { agentDescriptor(for: $0) } ?? conversation.owner?.provider
    return OverlayAgentPillEntry(
      id: conversation.id, conversationID: conversation.id,
      title:
        conversation.channel == "0"
        ? String(localized: "0 · All") : "\(conversation.channel) · \(conversation.name)",
      // The shared channel's wake count lives on its broadcast disclosure.
      descriptor: conversation.channel == "0" ? nil : descriptor,
      unread: unreadCounts[conversation.id] ?? 0,
      dot: conversation.channel == "0"
        ? .broadcast : (channel.map { statusDotKind(for: $0) } ?? .received),
      quietSymbol: channel?.broadcastPolicy.statusSymbol,
      isBroadcast: conversation.channel == "0",
      disconnected: channel.map(hasDeadFollower) ?? false,
      controls: pillControls(for: conversation, channel: channel))
  }

  /// The pill row's microphone and speaker, wired exactly like the docked
  /// column's row (`channelRow`) and its 0 · All row (Founder, 2026-10-16:
  /// "identyczne piguły mikrofon + głośnik + nazwa + agent/model/repo jakie te
  /// agenty mają na swoich kartach").
  private func pillControls(
    for conversation: OverlayConversation, channel: OverlayChannelDelivery?
  ) -> OverlayAgentPillEntry.Controls {
    let id = conversation.channel
    let digit = Self.toggleDigit(for: id)
    let broadcast = id == "0"
    // A dead reader cannot be spoken to; its recorded replies still play.
    let disconnected = channel.map(hasDeadFollower) ?? false
    return OverlayAgentPillEntry.Controls(
      channel: id,
      open: channel.map { isOpen($0) } ?? (hudStates[id]?.open == true),
      muted: mutedChannels[id],
      microphoneEnabled: !unavailable && onToggleChannel != nil && digit != nil
        && !pendingToggleChannels.contains(id) && (!broadcast || hudStates[id] != nil)
        && !disconnected,
      playbackEnabled: onTogglePlayback != nil && !pendingMuteChannels.contains(id),
      broadcast: broadcast,
      speaking: conversation.replySpeaking,
      onMicrophone: { if let digit { onToggleChannel?(digit) } },
      onPlayback: { onTogglePlayback?(id) })
  }

  /// The drawer's one-glance preview of the last message: own messages take
  /// the localized "You: " prefix, channel 0 names the sender, agent chats
  /// stay bare. Newlines fold so the two-line cap holds.
  static func drawerPreview(message: OverlayConversationMessage, channel: String) -> String {
    let body = message.bodyText.replacingOccurrences(of: "\n", with: " ")
    if message.kind == .user {
      return String(
        localized: "You: \(body)",
        comment: "Drawer preview of the user's own message; the placeholder is the text")
    }
    if channel == "0", let owner = message.owner {
      return "\(owner.name): \(body)"
    }
    return body
  }

  /// The narrow-window roster surface under the name pill, built from the
  /// same projection as the docked column.
  func pillStack(
    onSelect: @escaping (String?) -> Void, onDismiss: @escaping () -> Void
  ) -> OverlayAgentPillStack {
    OverlayAgentPillStack(
      palette: palette, entries: pillEntries,
      broadcastMembers: broadcastMembers, broadcastEnabled: !unavailable,
      onSelect: onSelect, onDismiss: onDismiss)
  }
}

/// One narrow-mode pill: a conversation switch target under the name pill.
struct OverlayAgentPillEntry: Identifiable {
  /// Status dot kinds, mapped to palette tones by the pill view; the shared
  /// channel's `broadcast` draws an antenna instead of a dot.
  enum DotKind {
    case broadcast, listening, received, waiting, sleeping, idle
  }

  let id: String
  let conversationID: String
  let title: String
  let descriptor: String?
  let unread: Int
  let dot: DotKind
  /// Slashed-antenna symbol when broadcasts do not wake this agent.
  let quietSymbol: String?
  let isBroadcast: Bool
  /// The roster keeps this channel's binding but its reader process is gone
  /// (the shape a restarted host leaves): the row dims and its microphone sleeps.
  let disconnected: Bool
  /// The row's microphone and speaker.
  let controls: Controls

  /// What `OverlayAgentAudioControls` needs for one row, resolved by the
  /// roster view from the same projection as the docked column's rows.
  struct Controls {
    let channel: String
    let open: Bool
    let muted: Bool?
    let microphoneEnabled: Bool
    let playbackEnabled: Bool
    let broadcast: Bool
    let speaking: Bool
    let onMicrophone: () -> Void
    let onPlayback: () -> Void
  }
}

/// Glass pills and their controls morph as one group on macOS 26; older
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
  let kind: OverlayAgentPillEntry.DotKind
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

/// The narrow-window roster: one glass row per conversation directly under
/// the name pill, a mirror of the header's own cluster: the row's microphone
/// and speaker lead, the pill (dot, name, agent/model/repo, unread) trails,
/// right edges under the name pill, so pills stand in one column and controls
/// under controls (Founder, 2026-10-16: "czy ten placement nie powinien być
/// precyzyjniejszy"; host contract, klaudiusz). Clicking a pill selects; Esc
/// closes; wake policy rides each row's context menu (Maciej, 2026-10-17,
/// "Słowo"). Rows stagger in unless the user asked for reduced motion.
struct OverlayAgentPillStack: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.csTextScale) private var textScale
  let palette: OverlayAppearancePalette
  let entries: [OverlayAgentPillEntry]
  let broadcastMembers: [OverlayChannelDelivery]
  let broadcastEnabled: Bool
  let onSelect: (String?) -> Void
  let onDismiss: () -> Void
  @State private var revealed = false

  var body: some View {
    ScrollView(.vertical) {
      overlayGlassGroup(
        VStack(alignment: .trailing, spacing: 6) {
          ForEach(Array(entries.enumerated()), id: \.element.id) { index, entry in
            Group {
              pillEntryView(entry)
              if entry.isBroadcast {
                // A hairline parts the shared channel from the agents under it.
                Rectangle()
                  .fill(palette.agentAccent.color.opacity(0.35))
                  .frame(height: 1)
                  .padding(.horizontal, 10)
                  .accessibilityHidden(true)
              }
            }
            .opacity(revealed ? 1 : 0)
            .offset(y: revealed ? 0 : -10)
            .animation(
              reduceMotion ? nil : .easeOut(duration: 0.2).delay(Double(index) * 0.026),
              value: revealed)
          }
        }
      )
      .padding(.vertical, 2)
    }
    .scrollIndicators(.hidden)
    .frame(width: OverlayAgentDrawerLayout.pillWidth(textScale: textScale))
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-agent-pill-stack")
    .onAppear { revealed = true }
    .onExitCommand { onDismiss() }
    .background {
      GeometryReader { proxy in
        Color.clear.preference(
          key: OverlayDrawerFramePreferenceKey.self,
          value: proxy.frame(in: .named("overlay-canvas")))
      }
      .allowsHitTesting(false)
    }
  }

  @ViewBuilder
  private func pillEntryView(_ entry: OverlayAgentPillEntry) -> some View {
    let pill = Button {
      onSelect(entry.conversationID)
    } label: {
      HStack(spacing: 9) {
        OverlayAgentStatusDot(kind: entry.dot, palette: palette)
        VStack(alignment: .leading, spacing: 1) {
          HStack(spacing: 4) {
            Text(verbatim: entry.title)
              .font(.system(size: 13 * textScale, weight: .bold))
            if let quietSymbol = entry.quietSymbol {
              Image(systemName: quietSymbol)
                .font(.system(size: 10 * textScale))
                .foregroundStyle(palette.mutedText.color)
                .help(String(localized: "Broadcasts do not wake this agent"))
                .accessibilityLabel(String(localized: "Broadcasts do not wake this agent"))
            }
          }
          .lineLimit(1)
          .truncationMode(.middle)
          if let descriptor = entry.descriptor, !descriptor.isEmpty {
            Text(verbatim: descriptor)
              .font(.system(size: 11 * textScale))
              .foregroundStyle(palette.mutedText.color)
              .lineLimit(1)
              .truncationMode(.middle)
          }
        }
        Spacer(minLength: 4)
        if entry.unread > 0 {
          Text(verbatim: String(entry.unread))
            .font(.system(size: 11 * textScale, weight: .semibold, design: .monospaced))
            .foregroundStyle(palette.processingStatus.color)
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(palette.processingStatus.color.opacity(0.12), in: Capsule())
            .accessibilityLabel("\(entry.unread) unread replies")
        }
      }
      .foregroundStyle(palette.primaryText.color)
      .padding(.horizontal, 12)
      .padding(.vertical, 5)
      .frame(maxWidth: .infinity, minHeight: 28, alignment: .leading)
      .contentShape(Capsule())
    }
    .buttonStyle(.plain)
    .csFocusOutline()
    .background(
      entry.isBroadcast ? palette.agentAccent.color.opacity(0.14) : .clear, in: Capsule()
    )
    .modifier(OverlayNamePillGlass(interactive: true))
    .help(
      entry.disconnected
        ? String(localized: "Disconnected")
        : String(localized: "View conversation without changing the microphone")
    )
    .accessibilityIdentifier("overlay-agent-pill-\(entry.id)")
    let controls = entry.controls
    HStack(spacing: 6) {
      OverlayAgentAudioControls(
        open: controls.open, muted: controls.muted,
        microphoneEnabled: controls.microphoneEnabled,
        playbackEnabled: controls.playbackEnabled,
        palette: palette, onMicrophone: controls.onMicrophone,
        onPlayback: controls.onPlayback, broadcast: controls.broadcast,
        speaking: controls.speaking
      )
      .fixedSize()
      .accessibilityIdentifier("overlay-agent-pill-controls-\(controls.channel)")
      pill
    }
    .modifier(OverlayDisconnectedRowDim(disconnected: entry.disconnected))
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-agent-pill-row-\(entry.id)")
    // Maciej, 2026-10-17 01:15Z: the stack floats above the transcript, whose
    // tracking set the I-beam, and the panel resets the cursor only above its
    // registered regions. The row, its audio pair and the gap between claim
    // the arrow the way the docked drawer column does.
    .background { OverlayMenuCursorRegion() }
    .modifier(
      OverlayBroadcastPolicyMenu(
        members: policyMembers(for: entry), palette: palette,
        enabled: broadcastEnabled, errorAlignment: .trailing))
  }

  /// The 0 · All row carries every member's wake policy; an agent's own row
  /// carries only its own.
  private func policyMembers(for entry: OverlayAgentPillEntry) -> [OverlayChannelDelivery] {
    if entry.isBroadcast { return broadcastMembers }
    return broadcastMembers.filter { $0.channel == entry.controls.channel }
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
private struct OverlayAgentMonitorBody<Header: View, Content: View>: View {
  let maximumHeight: CGFloat
  let palette: OverlayAppearancePalette
  var focusRevision: UInt64 = 0
  var focusedRowID: String? = nil
  @State private var headerHeight: CGFloat = 22
  @State private var contentHeight: CGFloat?
  @FocusState private var rowFocus: String?
  @ViewBuilder let header: Header
  let contentBuilder: (FocusState<String?>.Binding) -> Content

  init(
    maximumHeight: CGFloat, palette: OverlayAppearancePalette,
    focusRevision: UInt64 = 0, focusedRowID: String? = nil,
    @ViewBuilder header: () -> Header,
    @ViewBuilder content: @escaping (FocusState<String?>.Binding) -> Content
  ) {
    self.maximumHeight = maximumHeight
    self.palette = palette
    self.focusRevision = focusRevision
    self.focusedRowID = focusedRowID
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
        ScrollView(.vertical) {
          contentBuilder($rowFocus)
            .fixedSize(horizontal: false, vertical: true)
            .onGeometryChange(for: CGFloat.self) {
              $0.size.height
            } action: {
              contentHeight = $0
            }
        }
        .scrollIndicators(.visible)
        .frame(height: viewportHeight)
        .clipped()
      }
    }
    .onChange(of: focusRevision) { _, _ in
      rowFocus = focusedRowID
    }
  }
}
