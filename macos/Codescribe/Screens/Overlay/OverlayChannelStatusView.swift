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

/// One microphone treatment for the header and channel controls; actions stay with their owners.
struct OverlayMicrophoneGlyph: View {
  @Environment(\.displayScale) private var displayScale
  let symbol: String
  let tint: Color
  static let diameter: CGFloat = 22

  var body: some View {
    Image(systemName: symbol)
      .font(.system(size: 9, weight: .semibold))
      .foregroundStyle(tint)
      .frame(width: Self.diameter, height: Self.diameter)
      .background { Circle().fill(tint.opacity(0.12)) }
      .overlay {
        Circle()
          .strokeBorder(tint.opacity(0.42), lineWidth: 1 / max(displayScale, 1))
          .accessibilityHidden(true)
      }
  }
}

/// A quiet notification affordance opens the full monitor on the overlay canvas.
struct OverlayChannelStatusView: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.displayScale) private var displayScale

  let channels: [OverlayChannelDelivery]
  let unavailable: Bool
  let palette: OverlayAppearancePalette
  /// False while the panel is hidden or occluded: the waiting spinner must not
  /// keep the render loop awake.
  let animates: Bool
  let hudStates: [String: OverlayChannelHudProjection]
  let onToggleChannel: ((UInt8) -> Void)?
  let toggleError: String?
  let conversations: [OverlayConversation]
  let selectedConversationID: String?
  let unreadCounts: [String: Int]
  let onSelectConversation: ((String?) -> Void)?
  let onShowMonitor: (() -> Void)?

  init(
    channels: [OverlayChannelDelivery], unavailable: Bool,
    palette: OverlayAppearancePalette, animates: Bool,
    hudStates: [String: OverlayChannelHudProjection] = [:],
    onToggleChannel: ((UInt8) -> Void)? = nil,
    toggleError: String? = nil,
    conversations: [OverlayConversation] = [], selectedConversationID: String? = nil,
    unreadCounts: [String: Int] = [:], onSelectConversation: ((String?) -> Void)? = nil,
    onShowMonitor: (() -> Void)? = nil
  ) {
    self.channels = channels
    self.unavailable = unavailable
    self.palette = palette
    self.animates = animates
    self.hudStates = hudStates
    self.onToggleChannel = onToggleChannel
    self.toggleError = toggleError
    self.conversations = conversations
    self.selectedConversationID = selectedConversationID
    self.unreadCounts = unreadCounts
    self.onSelectConversation = onSelectConversation
    self.onShowMonitor = onShowMonitor
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

  /// Native menu labels accept images, but omit Shapes and reset text colors.
  /// The header uses the full bound-agent symbol as a muted navigation affordance.
  /// The projected glyph still owns receipt labels, accessibility and roster marks.
  private var nativeHeaderImage: CGImage? {
    let renderer = ImageRenderer(
      content: OverlayAgentStatusMark(
        reduceMotion: true, glyph: .attached, palette: palette, animates: false, fontSize: 13))
    renderer.scale = displayScale
    return renderer.cgImage
  }

  var body: some View {
    Menu {
      Button("My dictation", systemImage: "waveform") { onSelectConversation?(nil) }
      ForEach(channels) { channel in
        Button {
          viewConversation(channel)
        } label: {
          Label {
            Text(verbatim: notificationTitle(for: channel))
          } icon: {
            Image(systemName: "bubble.left")
          }
        }
      }
      if !savedConversations.isEmpty {
        Menu("Saved conversations") {
          ForEach(savedConversations) { conversation in
            Button {
              onSelectConversation?(conversation.id)
            } label: {
              Text(verbatim: conversation.name)
            }
          }
        }
      }
      Divider()
      Button("Capture channels", systemImage: "person.2") { showMonitor() }
    } label: {
      Group {
        if let image = nativeHeaderImage {
          Image(decorative: image, scale: displayScale)
            .renderingMode(.original)
        }
      }
      .frame(width: OverlayAgentGlyph.slotSize.width, height: OverlayAgentGlyph.slotSize.height)
      .contentShape(Rectangle())
      .overlay(alignment: .topTrailing) {
        if unreadCounts.values.contains(where: { $0 > 0 }) {
          Circle()
            .fill(palette.processingStatus.color)
            .frame(width: 5, height: 5)
            .offset(x: 2, y: -2)
            .accessibilityLabel("Unread replies")
            .accessibilityIdentifier("overlay-unread-replies")
        }
      }
    }
    .menuStyle(.borderlessButton)
    .menuIndicator(.hidden)
    .frame(width: OverlayAgentGlyph.slotSize.width, height: OverlayAgentGlyph.slotSize.height)
    .fixedSize()
    .help(
      Text(
        "\(glyph.label) — show details",
        comment: "Agent glyph tooltip; the placeholder is the channel state label")
    )
    .accessibilityLabel(glyph.label)
    .accessibilityHint("Choose a conversation or show agent channel details")
    .accessibilityIdentifier("overlay-agent-glyph")
  }

  var monitorBody: some View {
    ChannelRosterContent(palette: palette) {
      details
        .font(.system(size: 13, weight: .medium))
        .foregroundStyle(palette.primaryText.color)
    }
    .accessibilityIdentifier("overlay-agent-monitor")
  }

  var currentConversations: [OverlayConversation] {
    conversations.filter { conversation in
      if conversation.channel == "0" { return true }
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

  var savedConversations: [OverlayConversation] {
    let current = Set(currentConversations.map(\.id))
    return conversations.filter { !current.contains($0.id) }
  }

  func conversation(for channel: OverlayChannelDelivery) -> OverlayConversation? {
    currentConversations.first { $0.channel == channel.channel }
  }

  func viewConversation(_ channel: OverlayChannelDelivery) {
    guard let conversation = conversation(for: channel) else { return }
    onSelectConversation?(conversation.id)
  }

  private func channelRow(_ channel: OverlayChannelDelivery) -> some View {
    let conversation = conversation(for: channel)
    let open = isOpen(channel)
    return HStack(spacing: 12) {
      Button {
        viewConversation(channel)
      } label: {
        HStack(spacing: 10) {
          Image(systemName: "bubble.left")
            .foregroundStyle(palette.mutedText.color)
            .frame(width: 16)
          VStack(alignment: .leading, spacing: 3) {
            if channel.channel == "0" {
              Text("0 · All")
            } else {
              Text(verbatim: "\(channel.channel) · \(conversation?.name ?? channel.agent)")
                .lineLimit(1).truncationMode(.middle)
            }
            HStack(spacing: 6) {
              if open {
                Text("Microphone active")
                  .foregroundStyle(palette.listeningStatus.color)
                  .accessibilityIdentifier("overlay-channel-open-\(channel.channel)")
              } else {
                let rowGlyph =
                  OverlayAgentGlyph.resolve(channels: [channel], unavailable: unavailable)
                  ?? .attached
                OverlayAgentStatusMark(
                  reduceMotion: reduceMotion, glyph: rowGlyph, palette: palette, animates: animates,
                  fontSize: 10
                )
                .accessibilityHidden(true)
              }
              Text(detail(for: channel))
                .foregroundStyle(palette.bodyText.color)
                .lineLimit(1).truncationMode(.tail)
                .accessibilityIdentifier("overlay-channel-delivery-\(channel.channel)")
            }
            .font(.system(size: 11, weight: .medium))
          }
          Spacer(minLength: 8)
          if let conversation, let count = unreadCounts[conversation.id], count > 0 {
            Text(verbatim: String(count))
              .font(.system(size: 11, weight: .semibold, design: .monospaced))
              .foregroundStyle(palette.processingStatus.color)
              .padding(.horizontal, 6).padding(.vertical, 2)
              .background(palette.processingStatus.color.opacity(0.12), in: Capsule())
              .accessibilityLabel("\(count) unread replies")
          }
          if conversation != nil {
            Image(
              systemName: selectedConversationID == conversation?.id ? "checkmark" : "chevron.right"
            )
            .font(.system(size: 10, weight: .medium))
            .foregroundStyle(palette.mutedText.color)
          }
        }
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .disabled(onSelectConversation == nil || conversation == nil)
      .help("View conversation without changing the microphone")
      .accessibilityIdentifier("overlay-view-conversation-\(conversation?.id ?? channel.channel)")

      Button {
        toggle(channel)
      } label: {
        OverlayMicrophoneGlyph(
          symbol: open ? "mic.fill" : "mic", tint: palette.listeningStatus.color
        )
        .contentShape(Circle())
      }
      .buttonStyle(.plain)
      .csFocusOutline()
      .disabled(onToggleChannel == nil || Self.toggleDigit(for: channel.channel) == nil)
      .help(open ? "Hang up channel" : "Open channel")
      .accessibilityLabel(Text(verbatim: "\(channel.channel) · \(channel.agent)"))
      .accessibilityHint(open ? "Hang up channel" : "Open channel")
      .accessibilityIdentifier("overlay-channel-toggle-\(channel.channel)")
    }
    .padding(.vertical, 7)
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
              .font(.system(size: 11))
              .foregroundStyle(palette.mutedText.color)
              .lineLimit(1)
          }
        }
        Spacer(minLength: 8)
        if let count = unreadCounts[conversation.id], count > 0 {
          Text(verbatim: String(count))
            .font(.system(size: 11, weight: .semibold, design: .monospaced))
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
    .help("View conversation without changing the microphone")
    .accessibilityIdentifier("overlay-view-conversation-\(conversation.id)")
  }

  private func savedConversationDetail(
    _ conversation: OverlayConversation, owner: OverlayConversationOwner
  ) -> String {
    guard let stamp = conversation.messages.last?.emittedAt else { return owner.provider }
    let parser = ISO8601DateFormatter()
    parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    let date = parser.date(from: stamp) ?? ISO8601DateFormatter().date(from: stamp)
    guard let date else { return owner.provider }
    return "\(owner.provider) · \(date.formatted(date: .abbreviated, time: .shortened))"
  }

  private var details: some View {
    VStack(alignment: .leading, spacing: 4) {
      if onSelectConversation != nil {
        Button {
          onSelectConversation?(nil)
        } label: {
          HStack {
            Image(systemName: "waveform").frame(width: 16)
            Text("My dictation")
            Spacer()
            Image(systemName: "chevron.right").font(.system(size: 10, weight: .medium))
              .foregroundStyle(palette.mutedText.color)
          }
        }
        .buttonStyle(.plain)
        .padding(.vertical, 7)
        .accessibilityIdentifier("overlay-view-my-dictation")
      }
      ForEach(channels) { channel in
        channelRow(channel)
      }
      if onSelectConversation != nil {
        // A passive conversation can precede the next roster snapshot.
        // Keep it visible without inventing a microphone binding.
        ForEach(
          currentConversations.filter { conversation in
            !channels.contains { $0.channel == conversation.channel }
          }
        ) { conversation in
          conversationRow(conversation, saved: false)
        }
        if !savedConversations.isEmpty {
          Divider().padding(.vertical, 4)
          Text("Saved conversations")
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(palette.mutedText.color)
            .padding(.top, 8)
          ForEach(savedConversations) { conversation in
            conversationRow(conversation, saved: true)
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
    }
    .frame(maxWidth: .infinity, alignment: .leading)
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
