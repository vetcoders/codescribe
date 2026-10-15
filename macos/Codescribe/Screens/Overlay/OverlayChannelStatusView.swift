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

  init(character: String, tint: Color) {
    mark = .character(character, pointSize: OverlayHeaderGlyph.pointSize(character))
    self.tint = tint
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
      .tint(open ? Color.red : nil)
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
            : muted == true ? "speaker.slash.fill" : "speaker.wave.2"
        )
        .font(.system(size: 12, weight: .semibold))
        .foregroundStyle(palette.primaryText.color)
        .frame(width: 16, height: 16)
      }
      .csFocusOutline()
      .disabled(!playbackEnabled || muted == nil)
      .help(playbackLabel)
      .accessibilityLabel(playbackLabel)
      .accessibilityValue(playbackValue)
      .accessibilityIdentifier("overlay-agent-speaker")
    }
    .modifier(OverlayAgentControlStyle())
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
  var onDismissMonitor: (() -> Void)?
  var onShowTranscription: (() -> Void)?
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
      OverlayMicrophoneGlyph(
        character: OverlayHeaderGlyph.agents,
        tint: headerSelected ? Color(nsColor: .controlAccentColor) : palette.mutedText.color
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

  func monitorBody(maximumHeight: CGFloat) -> some View {
    OverlayAgentMonitorBody(maximumHeight: maximumHeight, palette: palette) {
      HStack {
        Text("Agents").font(.system(size: 13 * textScale, weight: .semibold))
        Spacer()
        Button {
          onDismissMonitor?()
        } label: {
          OverlayMicrophoneGlyph(symbol: "xmark", tint: palette.mutedText.color)
        }
        .buttonStyle(.plain).csFocusOutline()
        .accessibilityLabel("Close agent sidebar")
        .help("Close agent sidebar")
      }
    } content: {
      VStack(alignment: .leading, spacing: 8) {
        Button {
          onShowTranscription?()
        } label: {
          HStack(spacing: 10) {
            Image(systemName: "text.alignleft")
              .foregroundStyle(palette.mutedText.color)
            Text("Transcription")
            Spacer()
            if selectedConversationID == nil {
              Image(systemName: "checkmark")
                .foregroundStyle(palette.mutedText.color)
            }
          }
          .padding(.horizontal, 10).padding(.vertical, 6)
          .frame(maxWidth: .infinity, alignment: .leading)
          .contentShape(RoundedRectangle(cornerRadius: 9))
          .background(
            selectedConversationID == nil ? palette.mutedText.color.opacity(0.10) : .clear,
            in: RoundedRectangle(cornerRadius: 9))
        }
        .buttonStyle(.plain).csFocusOutline()
        .modifier(
          OverlayRowHoverHighlight(
            palette: palette, cornerRadius: 9, enabled: onShowTranscription != nil)
        )
        .disabled(onShowTranscription == nil)
        .accessibilityIdentifier("overlay-show-transcription")
        if let broadcast = currentConversations.first(where: { $0.channel == "0" }),
          onSelectConversation != nil
        {
          rowGlassContainer(
            HStack(spacing: 6) {
              conversationRow(broadcast, saved: false)
              OverlayAgentAudioControls(
                open: hudStates["0"]?.open == true, muted: mutedChannels["0"],
                microphoneEnabled: !unavailable && hudStates["0"] != nil
                  && onToggleChannel != nil && !pendingToggleChannels.contains("0"),
                playbackEnabled: onTogglePlayback != nil && !pendingMuteChannels.contains("0"),
                palette: palette, onMicrophone: { onToggleChannel?(0) },
                onPlayback: { onTogglePlayback?("0") }, broadcast: true
              )
              .fixedSize()
              .accessibilityIdentifier("overlay-channel-toggle-0")
            }
          )
        }
        Divider()
        details
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

  @ViewBuilder
  private func rowGlassContainer<Content: View>(_ content: Content) -> some View {
    if #available(macOS 26.0, *) {
      GlassEffectContainer(spacing: 6) { content }
    } else {
      content
    }
  }

  private func channelRow(_ channel: OverlayChannelDelivery) -> some View {
    let conversation = conversation(for: channel)
    let open = isOpen(channel)
    let descriptor = agentDescriptor(for: channel)
    let status = statusHelp(for: channel)
    let label =
      HStack(spacing: 7) {
        HStack(spacing: 3) {
          Text(verbatim: channel.channel)
            .font(.system(size: 10 * textScale, weight: .semibold, design: .monospaced))
            .foregroundStyle(palette.mutedText.color)
            .frame(width: 10)
          Image(systemName: statusSymbol(for: channel))
            .font(.system(size: 9 * textScale, weight: .medium))
            .foregroundStyle(statusTone(for: channel).color)
            .frame(width: 12, height: 14)
            .help(status)
            .accessibilityLabel(Text(verbatim: shortStatus(for: channel)))
            .accessibilityValue(Text(verbatim: detail(for: channel)))
            .accessibilityIdentifier("overlay-channel-delivery-\(channel.channel)")
        }
        VStack(alignment: .leading, spacing: 2) {
          if channel.channel == "0" {
            Text("0 · All")
          } else {
            Text(verbatim: conversation?.name ?? channel.agent)
              .lineLimit(1).truncationMode(.middle)
          }
          if let descriptor {
            Text(verbatim: descriptor)
              .font(.system(size: 10 * textScale, weight: .light))
              .foregroundStyle(palette.mutedText.color)
              .lineLimit(1).truncationMode(.middle)
              .help(descriptor)
              .accessibilityIdentifier("overlay-agent-descriptor-\(channel.channel)")
          }
        }
        Spacer(minLength: 4)
        if let conversation, let count = unreadCounts[conversation.id], count > 0 {
          Text(verbatim: String(count))
            .font(.system(size: 11 * textScale, weight: .semibold, design: .monospaced))
            .foregroundStyle(palette.processingStatus.color)
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(palette.processingStatus.color.opacity(0.12), in: Capsule())
            .accessibilityLabel("\(count) unread replies")
        }
      }
      .contentShape(Rectangle())
    return rowGlassContainer(
      HStack(spacing: 6) {
        Group {
          if conversation != nil, onSelectConversation != nil {
            Button {
              viewConversation(channel)
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
        .accessibilityValue(Text(verbatim: descriptor.map { $0 + ". " + status } ?? status))
        .accessibilityIdentifier("overlay-view-conversation-\(conversation?.id ?? channel.channel)")

        OverlayAgentAudioControls(
          open: open, muted: mutedChannels[channel.channel],
          microphoneEnabled: !unavailable && onToggleChannel != nil
            && !pendingToggleChannels.contains(channel.channel)
            && Self.toggleDigit(for: channel.channel) != nil,
          playbackEnabled: onTogglePlayback != nil
            && !pendingMuteChannels.contains(channel.channel),
          palette: palette, onMicrophone: { toggle(channel) },
          onPlayback: { onTogglePlayback?(channel.channel) }
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
    .help(status)
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-agent-row-\(channel.channel)")
  }

  private func conversationRow(_ conversation: OverlayConversation, saved: Bool) -> some View {
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
            Text("0 · All")
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
          Text(verbatim: String(count))
            .font(.system(size: 11 * textScale, weight: .semibold, design: .monospaced))
            .foregroundStyle(palette.processingStatus.color)
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(palette.processingStatus.color.opacity(0.12), in: Capsule())
            .accessibilityLabel("\(count) unread replies")
        }
        Image(systemName: selectedConversationID == conversation.id ? "checkmark" : "chevron.right")
          .font(.system(size: 10, weight: .medium))
          .foregroundStyle(palette.mutedText.color)
      }
      .padding(.vertical, 7)
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
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

  private var details: some View {
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
        channelRow(channel)
      }
      if onSelectConversation != nil {
        // A passive conversation can precede the next roster snapshot.
        // Keep it visible without inventing a microphone binding.
        ForEach(
          currentConversations.filter { conversation in
            conversation.channel != "0" && !channels.contains { $0.channel == conversation.channel }
          }
        ) { conversation in
          conversationRow(conversation, saved: false)
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
    case .sent, .queued: return "clock"
    case .received: return "checkmark"
    }
  }

  private func statusTone(for channel: OverlayChannelDelivery) -> OverlayColorToken {
    if unavailable { return palette.processingStatus }
    if hasDeadFollower(channel) || hasStalledFollower(channel) { return palette.mutedText }
    if isOpen(channel) { return palette.listeningStatus }
    switch channel.stage {
    case nil: return palette.mutedText
    case .sent, .queued: return palette.processingStatus
    case .received: return palette.successStatus
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
/// The header stays outside the scroll viewport so Close is always reachable.
private struct OverlayAgentMonitorBody<Header: View, Content: View>: View {
  let maximumHeight: CGFloat
  let palette: OverlayAppearancePalette
  @ViewBuilder let header: Header
  @ViewBuilder let content: Content
  @State private var headerHeight: CGFloat = 22
  @State private var contentHeight: CGFloat?

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
          content
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
  }
}
