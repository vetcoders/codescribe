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
  /// A rotating spinner: a sealed utterance is out and no receipt names it yet.
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

/// A quiet notification affordance opens the full monitor on the overlay canvas.
struct OverlayChannelStatusView: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

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

  var body: some View {
    Button(action: showMonitor) {
      OverlayAgentStatusMark(
        reduceMotion: reduceMotion, glyph: glyph, palette: palette, animates: animates, fontSize: 13
      )
      .contentShape(Rectangle())
      .overlay(alignment: .topTrailing) {
        if let count = unreadCounts["0"], count > 0 {
          Circle()
            .fill(palette.processingStatus.color)
            .frame(width: 5, height: 5)
            .offset(x: 2, y: -2)
            .accessibilityLabel("\(count) unread replies")
            .accessibilityIdentifier("overlay-unread-replies")
        }
      }
    }
    .buttonStyle(.plain)
    .help(
      Text(
        "\(glyph.label) — show details",
        comment: "Agent glyph tooltip; the placeholder is the channel state label")
    )
    .accessibilityLabel(glyph.label)
    .accessibilityHint("Shows agent channel details")
    .accessibilityIdentifier("overlay-agent-glyph")
  }

  var monitorBody: some View {
    ChannelRosterContent(palette: palette) {
      details
        .font(.system(size: 14, weight: .medium))
        .foregroundStyle(palette.primaryText.color)
    }
    .accessibilityIdentifier("overlay-agent-monitor")
  }

  private var details: some View {
    VStack(alignment: .leading, spacing: 4) {
      if onSelectConversation != nil {
        Button {
          onSelectConversation?(nil)
        } label: {
          HStack {
            Text("My dictation")
            Spacer()
            if selectedConversationID == nil { Image(systemName: "checkmark") }
          }
        }
        .buttonStyle(.plain)
        .padding(.vertical, 8)
        .accessibilityIdentifier("overlay-view-my-dictation")
        ForEach(conversations) { conversation in
          Button {
            onSelectConversation?(conversation.id)
          } label: {
            HStack {
              if conversation.channel == "0" {
                Text("0 · All")
              } else {
                Text(verbatim: "\(conversation.channel) · \(conversation.name)")
              }
              Spacer()
              if let count = unreadCounts[conversation.id], count > 0 {
                Text("\(count) unread")
                  .foregroundStyle(palette.processingStatus.color)
              }
              if selectedConversationID == conversation.id { Image(systemName: "checkmark") }
            }
          }
          .buttonStyle(.plain)
          .padding(.vertical, 8)
          .help("View conversation without changing the microphone")
          .accessibilityIdentifier("overlay-view-conversation-\(conversation.id)")
        }
        Divider()
        Text("Capture channels")
          .foregroundStyle(palette.mutedText.color)
      }
      ForEach(channels) { channel in
        if isOpen(channel) {
          Label("Microphone active · channel \(channel.channel)", systemImage: "mic.fill")
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(palette.listeningStatus.color)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("overlay-channel-open-\(channel.channel)")
        }
        Button {
          toggle(channel)
        } label: {
          HStack(spacing: 6) {
            Text(verbatim: "\(channel.channel) · \(channel.agent)")
              .lineLimit(1)
              .truncationMode(.middle)
              .foregroundStyle(
                (hasDeadFollower(channel) ? palette.bodyText : palette.primaryText).color)
            Spacer(minLength: 4)
            let rowGlyph =
              OverlayAgentGlyph.resolve(channels: [channel], unavailable: unavailable) ?? .attached
            OverlayAgentStatusMark(
              reduceMotion: reduceMotion, glyph: rowGlyph, palette: palette, animates: animates,
              fontSize: 11
            )
            .accessibilityHidden(true)
            Text(detail(for: channel))
              .foregroundStyle(palette.bodyText.color)
              .accessibilityIdentifier("overlay-channel-delivery-\(channel.channel)")
          }
          .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.vertical, 6)
        .disabled(onToggleChannel == nil || Self.toggleDigit(for: channel.channel) == nil)
        .font(.system(size: 13, weight: .medium))
        .accessibilityLabel(
          Text(verbatim: "\(channel.channel) · \(channel.agent), \(detail(for: channel))")
        )
        .accessibilityHint(isOpen(channel) ? "Hang up channel" : "Open channel")
        .accessibilityIdentifier("overlay-channel-toggle-\(channel.channel)")
        .accessibilityElement(children: .combine)
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
      .padding(16)
      .frame(maxWidth: .infinity, alignment: .leading)
      .background(style.surface.color)
      .overlay {
        RoundedRectangle(cornerRadius: 10, style: .continuous)
          .strokeBorder(style.border.color, lineWidth: 1)
          .allowsHitTesting(false)
      }
      .preferredColorScheme(style.colorScheme)
  }
}
