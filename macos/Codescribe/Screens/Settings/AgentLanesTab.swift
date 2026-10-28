import SwiftUI

/// Agent › AI models: the lane editors, the voice conversation with a
/// Codescribe Bus agent, the built-in Agent's own timers, then the resolved
/// runtime truth — the answer to "which provider am I actually talking to" —
/// folded away as read-only proof of what resolved.
///
/// The two timer sections act on different things, so each one names its
/// target. "Voice conversation with an agent" is the `agent_channel` section of
/// settings.json: the pause that sends a spoken channel message, whether the
/// session keeps listening after it, how the message is finished, and the
/// hold in which a sent message can still be revised or withdrawn
/// (`docs/TRANSCRIPT_BUS.md`, "Held delivery"). "Built-in Agent" is unrelated
/// to the bus: its auto-send delay and the end of a take that hears no voice.
/// The hands-free dictation silence limit lives with Dictation settings.
struct AgentLanesTab: View {
  @ObservedObject var model: SettingsViewModel
  @State private var detailsExpanded = false
  /// "Adres API: Formatowanie" and "Klucze: Formatowanie" overflow the
  /// shared 160 pt key column.
  static let keyWidth: CGFloat = 184

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      LLMLanesSection(model: model)

      SettingsSectionLabel(
        String(
          localized: "Voice conversation with an agent",
          comment: "AI models section: how a spoken Codescribe Bus channel session behaves")
      )
      .padding(.top, SettingsScale.sectionGap)

      // The slider card is the width pattern: the rows below share its edges
      // instead of stretching to the pane like the lane editors above.
      PatternWidthStack {
        voiceConversationCard
      }
      .padding(.top, SettingsScale.headerGap)

      SettingsSectionLabel(
        String(
          localized: "Built-in Agent", comment: "AI models section: the built-in Agent's timers")
      )
      .padding(.top, SettingsScale.sectionGap)

      PatternWidthStack {
        builtInAgentCard
      }
      .padding(.top, SettingsScale.headerGap)

      // Collapsed by default: the editors above answer the everyday question;
      // this is where the endpoints and the settings keys live.
      DisclosureGroup(isExpanded: $detailsExpanded) {
        VStack(spacing: 0) {
          RuntimeRow(
            key: String(localized: "Formatting level"),
            value: model.formattingDescription,
            tint: true,
            trailing: .none,
            keyWidth: Self.keyWidth
          )
          ForEach(channelRuntimeRows, id: \.key) { row in
            divider
            RuntimeRow(
              key: row.key,
              value: row.value,
              tint: false,
              mono: row.mono,
              trailing: row.source.map { .text($0, CSColor.secondaryInk) } ?? .none,
              keyWidth: Self.keyWidth
            )
          }
          ForEach(LLMLane.allCases) { lane in
            let laneModel = model.llmLane(lane)
            divider
            RuntimeRow(
              key: String(localized: "\(lane.title) provider"),
              value: laneModel.providerDisplayName,
              tint: false,
              trailing: .mark(laneModel.availabilityTone),
              keyWidth: Self.keyWidth
            )
            divider
            RuntimeRow(
              key: String(localized: "\(lane.title) endpoint"),
              value: laneModel.resolvedEndpoint,
              tint: false,
              mono: true,
              trailing: .none,
              keyWidth: Self.keyWidth
            )
            divider
            RuntimeRow(
              key: String(localized: "\(lane.title) model"),
              value: laneModel.resolvedModel,
              tint: true,
              mono: true,
              trailing: .text(laneModel.availabilityDescription, laneModel.availabilityTint),
              keyWidth: Self.keyWidth
            )
            divider
            RuntimeRow(
              key: String(
                localized: "\(lane.title) settings keys",
                comment: "The placeholder is the lane title; the value lists its settings.json keys"
              ),
              value: "\(lane.providerKey) · \(lane.modelKey)",
              tint: false,
              mono: true,
              trailing: .none,
              keyWidth: Self.keyWidth
            )
          }
        }
        .clipShape(.rect(cornerRadius: CSRadius.composer))
        .overlay {
          RoundedRectangle(cornerRadius: CSRadius.composer)
            .strokeBorder(Color.primary.opacity(CSAlpha.stroke), lineWidth: 1)
        }
        .padding(.top, SettingsScale.headerGap)
      } label: {
        SettingsSectionLabel(String(localized: "Active configuration details"))
      }
      .padding(.top, SettingsScale.sectionGap)
      .accessibilityIdentifier("agent-lanes-details")
    }
  }

  private var voiceConversationCard: some View {
    let policy = model.agentChannelPolicy
    let finish = model.agentChannelFinish
    return VStack(alignment: .leading, spacing: 0) {
      SettingsControlRow(
        title: String(localized: "After the message"),
        subtitle: policy.summary
      ) {
        SettingsSegmentedChoice(
          titles: AgentChannelPolicy.allCases.map(\.title),
          selection: Binding(
            get: { AgentChannelPolicy.allCases.firstIndex(of: policy) ?? 0 },
            set: { model.setAgentChannelPolicy(AgentChannelPolicy.allCases[$0]) }
          ),
          accessibilityLabel: String(localized: "After the message"),
          accessibilityIdentifier: "agent-channel-session-policy"
        )
      }

      SettingsSecondsSlider(
        title: model.agentChannelSendPauseOverridden
          ? String(localized: "Pause before sending (env override)")
          : String(localized: "Pause before sending"),
        detail: String(
          localized:
            "Silence after you stop speaking before the message is sent. Applies from the next channel."
        ),
        seconds: model.settings.agentChannelSendPauseSec,
        value: $model.agentChannelSendPauseSlider,
        range: 1...15,
        step: 0.5
      )
      .disabled(model.agentChannelSendPauseOverridden)
      .padding(.top, SettingsScale.rowGap)
      .accessibilityIdentifier("agent-channel-send-pause")

      AgentChannelVoicelessSlider(model: model)
        .padding(.top, SettingsScale.rowGap)

      SettingsControlRow(
        title: String(localized: "Finish each message"),
        subtitle: finish.summary
      ) {
        SettingsSegmentedChoice(
          titles: AgentChannelFinish.allCases.map(\.title),
          selection: Binding(
            get: { AgentChannelFinish.allCases.firstIndex(of: finish) ?? 1 },
            set: { model.setAgentChannelFinish(AgentChannelFinish.allCases[$0]) }
          ),
          accessibilityLabel: String(localized: "Finish each message"),
          accessibilityIdentifier: "agent-channel-segment-finish"
        )
      }
      .padding(.top, SettingsScale.rowGap)

      SettingsControlRow(
        title: String(localized: "Wait for formatting"),
        subtitle: String(
          localized: "Past this, the message is sent with punctuation only.")
      ) {
        Stepper(value: model.agentChannelFormatWaitSeconds, in: 1...10) {
          Text("\(Int(model.settings.agentChannelFormatWaitSec)) sec")
            .monospacedDigit()
        }
        .accessibilityIdentifier("agent-channel-format-wait")
      }
      .disabled(finish != .formatter)
      .padding(.top, SettingsScale.rowGap)

      // The hold is a bus-only window: a held channel message can still be
      // revised or withdrawn before it lands.
      SettingsControlRow(
        title: String(localized: "Hold before sending to Codescribe Bus"),
        subtitle: String(
          localized: "Revise or withdraw the message during this window. 0 sends at once.")
      ) {
        Stepper(value: model.agentSendHoldSeconds, in: 0...30) {
          Text("\(Int(model.settings.agentSendHoldSec)) sec")
            .monospacedDigit()
        }
        .accessibilityIdentifier("agent-send-hold-seconds")
      }
      .padding(.top, SettingsScale.rowGap)

      Text(AgentChannelChain(settings: model.settings).caption)
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.top, SettingsScale.rowGap)
        .accessibilityIdentifier("agent-channel-chain")
    }
    .settingsGroupedInset()
  }

  /// Mirrors `OverlayState`: armed in Agent mode only, fires after
  /// `interaction.agent_auto_send_delay_sec`, cancelled the moment the
  /// transcript is edited.
  private var builtInAgentCard: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsControlRow(
        title: String(localized: "Send automatically"),
        subtitle: String(localized: "Sends the transcript unless you start editing it.")
      ) {
        Toggle(
          "",
          isOn: Binding(
            get: { model.settings.agentAutoSend },
            set: { model.setAgentAutoSend($0) }
          )
        )
        .settingsSwitch()
      }

      SettingsControlRow(
        title: String(localized: "After how many seconds"),
        subtitle: String(localized: "Countdown before the transcript is sent.")
      ) {
        Stepper(value: model.agentAutoSendDelaySeconds, in: 1...30) {
          Text("\(Int(model.settings.agentAutoSendDelaySec)) sec")
            .monospacedDigit()
        }
        .accessibilityIdentifier("agent-auto-send-delay")
      }
      .disabled(!model.settings.agentAutoSend)
      .padding(.top, SettingsScale.rowGap)

      SettingsControlRow(
        title: String(localized: "End the Agent recording without voice after"),
        subtitle: String(
          localized: "A hands-free Agent recording that hears no voice stops and sends nothing.")
      ) {
        Stepper(value: model.agentTakeVoicelessEndSeconds, in: 10...120, step: 5) {
          Text("\(Int(model.settings.agentTakeVoicelessEndSec)) sec")
            .monospacedDigit()
        }
        .accessibilityIdentifier("agent-take-voiceless-end")
      }
      .padding(.top, SettingsScale.rowGap)
    }
    .settingsGroupedInset()
  }

  private struct ChannelRuntimeRow {
    let key: String
    let value: String
    let mono: Bool
    let source: String?
  }

  /// The channel settings the next session freezes, with the source of the pause.
  private var channelRuntimeRows: [ChannelRuntimeRow] {
    let settings = model.settings
    let overridden = model.agentChannelSendPauseOverridden
    return [
      ChannelRuntimeRow(
        key: String(localized: "Channel session"),
        value: model.agentChannelPolicy.title, mono: false, source: nil),
      ChannelRuntimeRow(
        key: String(localized: "Channel send pause"),
        value: AgentChannelChain.seconds(settings.agentChannelSendPauseSec), mono: true,
        source: overridden
          ? String(
            localized: "settings.agentChannel.source.env", defaultValue: "env override",
            comment: "Where the channel send pause came from: the environment variable")
          : String(
            localized: "settings.agentChannel.source.settings", defaultValue: "settings",
            comment: "Where the channel send pause came from: settings.json")),
      ChannelRuntimeRow(
        key: String(localized: "Channel close without voice"),
        value: AgentChannelVoicelessSlider.label(settings.agentChannelVoicelessCloseSec),
        mono: true, source: nil),
      ChannelRuntimeRow(
        key: String(localized: "Channel message finish"),
        value: model.agentChannelFinish.title, mono: false, source: nil),
      ChannelRuntimeRow(
        key: String(localized: "Channel formatting wait"),
        value: AgentChannelChain.seconds(Float(settings.agentChannelFormatWaitSec)), mono: true,
        source: nil),
    ]
  }

  private var divider: some View {
    Rectangle().fill(Color.primary.opacity(CSAlpha.stroke)).frame(height: 1)
  }
}

/// Stacks cards at the width the first one takes for the offered width; every
/// later card is offered exactly that width. A plain `VStack` cannot do this:
/// `SettingsControlRow` fills whatever it is offered, so the stack would grow
/// to the pane instead of following the slider card.
private struct PatternWidthStack: Layout {
  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    let width = columnWidth(offered: proposal.width, subviews: subviews)
    let height = subviews.reduce(0) {
      $0 + $1.sizeThatFits(ProposedViewSize(width: width, height: nil)).height
    }
    return CGSize(width: width, height: height)
  }

  func placeSubviews(
    in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()
  ) {
    let width = columnWidth(offered: bounds.width, subviews: subviews)
    var y = bounds.minY
    for subview in subviews {
      let height = subview.sizeThatFits(ProposedViewSize(width: width, height: nil)).height
      subview.place(
        at: CGPoint(x: bounds.minX, y: y), anchor: .topLeading,
        proposal: ProposedViewSize(width: width, height: height))
      y += height
    }
  }

  private func columnWidth(offered: CGFloat?, subviews: Subviews) -> CGFloat {
    guard let pattern = subviews.first else { return 0 }
    return pattern.sizeThatFits(ProposedViewSize(width: offered, height: nil)).width
  }
}

/// `agent_channel.session_policy`; `rawValue` is the settings.json value.
enum AgentChannelPolicy: String, CaseIterable {
  case continuous
  case endAfterMessage = "end_after_message"

  var title: String {
    switch self {
    case .continuous:
      String(
        localized: "Keep listening", comment: "Channel session policy: continuous conversation")
    case .endAfterMessage:
      String(localized: "End the session", comment: "Channel session policy: one message per press")
    }
  }

  var summary: String {
    switch self {
    case .continuous:
      String(localized: "Each pause sends a message; the conversation lasts until you hang up.")
    case .endAfterMessage:
      String(localized: "The pause sends the message and hangs up; press again for the next one.")
    }
  }

  /// The voiceless close a policy starts with, as the core seeds it.
  var defaultVoicelessCloseSec: UInt64 {
    switch self {
    case .continuous: 0
    case .endAfterMessage: 30
    }
  }
}

/// `agent_channel.segment_finish`; `rawValue` is the settings.json value.
enum AgentChannelFinish: String, CaseIterable {
  case off
  case lightPlus = "light_plus"
  case formatter

  var title: String {
    switch self {
    case .off:
      String(
        localized: "settings.agentChannel.finish.off", defaultValue: "Off",
        comment: "Channel message finish: literal text. Short segment label")
    case .lightPlus:
      String(localized: "Punctuation (Light+)", comment: "Channel message finish: end punctuation")
    case .formatter:
      String(localized: "Formatting", comment: "Channel message finish: one formatter pass")
    }
  }

  var summary: String {
    switch self {
    case .off: String(localized: "The message is sent exactly as transcribed.")
    case .lightPlus: String(localized: "Adds the closing punctuation; nothing else changes.")
    case .formatter:
      String(localized: "One formatting pass at your formatting level, within the wait below.")
    }
  }
}

/// The live caption under the conversation card: what happens, in order,
/// after you stop speaking.
struct AgentChannelChain {
  let settings: CsSettings

  var caption: String {
    var steps = [
      String(
        localized: "Pause \(Self.seconds(settings.agentChannelSendPauseSec))",
        comment: "Channel chain step. The placeholder is a duration such as '5 s'")
    ]
    if settings.agentChannelSegmentFinish == AgentChannelFinish.formatter.rawValue {
      steps.append(
        String(
          localized: "formatting ≤ \(Self.seconds(Float(settings.agentChannelFormatWaitSec)))",
          comment: "Channel chain step. The placeholder is a duration such as '3 s'"))
    }
    if settings.agentSendHoldSec > 0 {
      steps.append(
        String(
          localized: "hold \(Self.seconds(Float(settings.agentSendHoldSec)))",
          comment: "Channel chain step. The placeholder is a duration such as '5 s'"))
    }
    let continuous =
      settings.agentChannelSessionPolicy != AgentChannelPolicy.endAfterMessage.rawValue
    steps.append(
      continuous
        ? String(
          localized: "sent; the conversation lasts until you hang up",
          comment: "Channel chain: last step of a continuous session")
        : String(
          localized: "sent and hung up",
          comment: "Channel chain: last step of a one-message session")
    )
    return steps.joined(separator: " → ")
  }

  static func seconds(_ value: Float) -> String {
    String(
      localized: "\(value.formatted(.number.precision(.fractionLength(0...1)))) s",
      comment: "A duration in seconds. The placeholder is a formatted number")
  }
}

/// `agent_channel.voiceless_close_sec` on fixed stops: Never, 15 s … 10 min.
private struct AgentChannelVoicelessSlider: View {
  @ObservedObject var model: SettingsViewModel
  @State private var gate = SliderCommitGate()

  var body: some View {
    let stops = SettingsViewModel.agentChannelVoicelessStops
    let displayed = gate.displayed(persisted: model.agentChannelVoicelessSlider)
    let index = min(stops.count - 1, max(0, Int(displayed.rounded())))
    let label = Self.label(stops[index])
    let title = String(localized: "Close without voice after")
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      Text(title)
        .font(SettingsScale.settingName)
        .foregroundStyle(Color.primary)
      Text("A session that hears no voice ends on its own. Agent replies do not count.")
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
        .fixedSize(horizontal: false, vertical: true)
      HStack(spacing: CSSpace.md) {
        Slider(
          value: Binding(
            get: { gate.displayed(persisted: model.agentChannelVoicelessSlider) },
            set: { newValue in
              if let direct = gate.valueChanged(newValue) {
                model.agentChannelVoicelessSlider = direct
              }
            }
          ),
          in: 0...Double(stops.count - 1), step: 1,
          onEditingChanged: { editing in
            if let committed = gate.editingChanged(editing) {
              model.agentChannelVoicelessSlider = committed
            }
          }
        )
        .tint(CSColor.chromeAccent)
        .frame(maxWidth: SettingsScale.sliderWidth)
        .accessibilityLabel(title)
        .accessibilityValue(label)
        Text(label)
          .font(SettingsScale.monoValue)
          .foregroundStyle(Color.primary)
      }
      .padding(.top, CSSpace.xs)
    }
    .accessibilityIdentifier("agent-channel-voiceless-close")
  }

  static func label(_ seconds: UInt64) -> String {
    if seconds == 0 {
      return String(localized: "Never", comment: "Channel close without voice: never")
    }
    if seconds >= 60, seconds % 60 == 0 {
      return String(
        localized: "\(Int(seconds / 60)) min",
        comment: "A duration in minutes. The placeholder is a whole number")
    }
    return String(
      localized: "\(Int(seconds)) s",
      comment: "A duration in seconds. The placeholder is a whole number")
  }
}
