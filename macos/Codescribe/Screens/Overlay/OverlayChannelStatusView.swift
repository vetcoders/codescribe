import SwiftUI

/// The agent channel in one header character.
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
  /// ⣸ a sealed utterance is out and no receipt names it yet.
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

  var character: String {
    switch self {
    case .attached, .open: "\u{2756}"
    case .awaitingReceipt: "\u{28F8}"
    case .acknowledged: "\u{2406}"
    // U+FE0E pins the text presentation: a monochrome sign, never the emoji.
    case .unavailable: "\u{26A0}\u{FE0E}"
    }
  }

  /// VoiceOver label and tooltip lead; one sentence per state.
  var label: String {
    switch self {
    case .attached: "Agent attached"
    case .open: "Agent channel open"
    case .awaitingReceipt: "Waiting for the agent to confirm receipt"
    case .acknowledged: "Agent confirmed receipt"
    case .unavailable: "Agent channel status unavailable"
    }
  }

  var pulses: Bool { self == .awaitingReceipt }

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

/// Display-only projection of the controller's per-digit HUD state. The bridge
/// supplies this independently of the delivery mailbox once it is exposed.
struct OverlayChannelHudProjection: Equatable {
  let open: Bool
  let loud: Bool
  let autosealDeadline: Date?
  let followerAlive: Bool
}

/// A quiet header affordance. Delivery details belong to its popover, not the transcript.
struct OverlayChannelStatusView: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  let channels: [OverlayChannelDelivery]
  let unavailable: Bool
  let palette: OverlayAppearancePalette
  /// False while the panel is hidden or occluded: the waiting pulse must not
  /// keep the render loop awake.
  let animates: Bool
  let hudStates: [String: OverlayChannelHudProjection]
  let onToggleChannel: ((UInt8) -> Void)?

  init(
    channels: [OverlayChannelDelivery], unavailable: Bool,
    palette: OverlayAppearancePalette, animates: Bool,
    hudStates: [String: OverlayChannelHudProjection] = [:],
    onToggleChannel: ((UInt8) -> Void)? = nil
  ) {
    self.channels = channels
    self.unavailable = unavailable
    self.palette = palette
    self.animates = animates
    self.hudStates = hudStates
    self.onToggleChannel = onToggleChannel
  }

  static func toggleDigit(for channel: String) -> UInt8? {
    guard let digit = UInt8(channel), (1...9).contains(digit) else { return nil }
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

  @State private var showsDetails = false

  var glyph: OverlayAgentGlyph {
    OverlayAgentGlyph.resolve(channels: channels, unavailable: unavailable) ?? .attached
  }

  var body: some View {
    Button {
      showsDetails.toggle()
    } label: {
      Group {
        if glyph.pulses && animates && !reduceMotion {
          TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
            mark.opacity(
              OverlayRecordingLight.pulseOpacity(
                at: timeline.date.timeIntervalSinceReferenceDate))
          }
        } else {
          mark
        }
      }
      .frame(width: OverlayAgentGlyph.slotSize.width, height: OverlayAgentGlyph.slotSize.height)
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .help(glyph.label + " — show details")
    .accessibilityLabel(glyph.label)
    .accessibilityHint("Shows agent channel details")
    .accessibilityIdentifier("overlay-agent-glyph")
    .popover(isPresented: $showsDetails, arrowEdge: .bottom) {
      details
        .padding(16)
        .frame(width: 300)
    }
  }

  private var mark: some View {
    Text(glyph.character)
      .font(.system(size: 13, weight: .medium, design: .monospaced))
      .foregroundStyle(glyph.tone(in: palette).color)
      .fixedSize()
  }

  private var details: some View {
    VStack(alignment: .leading, spacing: 4) {
      ForEach(channels) { channel in
        if isOpen(channel) {
          Label("Microphone active · channel \(channel.channel)", systemImage: "mic.fill")
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(palette.listeningStatus.color)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("overlay-channel-open-\(channel.channel)")
        }
        Button {
          toggle(channel)
        } label: {
          HStack(spacing: 6) {
            Text("\(channel.channel) · \(channel.agent)")
              .lineLimit(1)
              .truncationMode(.middle)
            Spacer(minLength: 4)
            let rowGlyph =
              OverlayAgentGlyph.resolve(channels: [channel], unavailable: unavailable) ?? .attached
            Text(rowGlyph.character)
              .font(.system(size: 11, weight: .medium, design: .monospaced))
              .foregroundStyle(rowGlyph.tone(in: palette).color)
              .accessibilityHidden(true)
            Text(detail(for: channel))
              .accessibilityIdentifier("overlay-channel-delivery-\(channel.channel)")
          }
          .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(onToggleChannel == nil || Self.toggleDigit(for: channel.channel) == nil)
        .font(.system(size: 11, weight: .medium))
        .foregroundStyle(palette.mutedText.color)
        .opacity(hasDeadFollower(channel) ? 0.55 : 1)
        .accessibilityLabel("\(channel.channel) · \(channel.agent), \(detail(for: channel))")
        .accessibilityHint(isOpen(channel) ? "Hang up channel" : "Open channel")
        .accessibilityIdentifier("overlay-channel-toggle-\(channel.channel)")
        .accessibilityElement(children: .combine)
      }
      if unavailable {
        Text("Channel status unavailable — last open state retained")
          .font(.system(size: 11, weight: .medium))
          .foregroundStyle(palette.processingStatus.color)
          .accessibilityIdentifier("overlay-channel-status-unavailable")
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  func detail(for channel: OverlayChannelDelivery) -> String {
    if unavailable { return "status unavailable" }
    let delivery: String
    switch channel.stage {
    case nil: delivery = "no sealed utterance"
    case .sent: delivery = "sent · waiting for receipt"
    case .queued: delivery = "queued · waiting for receipt"
    case .received: delivery = "receipt confirmed by the agent"
    }
    return hasDeadFollower(channel) ? "\(delivery) · nobody listening" : delivery
  }
}
