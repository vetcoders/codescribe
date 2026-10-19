import SwiftUI

/// Agent › AI models: the lane editors, the two Codescribe Bus timers, the
/// built-in Agent's auto-send switch, then the resolved runtime truth — the
/// answer to "which provider am I actually talking to" — folded away as
/// read-only proof of what resolved.
///
/// Two timers sit on this page and they act on different things, so each one
/// names its target (Founder review, 2026-10-18). The pause and the hold both
/// belong to Codescribe Bus channel messages and are grouped under one header:
/// the pause is the silence that ends a spoken message, the hold is the window
/// in which a sent channel message can still be revised or withdrawn
/// (`docs/TRANSCRIPT_BUS.md`, "Held delivery"). Auto-send is unrelated to the
/// bus: it is the built-in Agent's own fixed 5 s delay
/// (`OverlayState.autoHideDelaySeconds`) and keeps a card of its own.
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
        String(localized: "Codescribe Bus", comment: "AI models section: the two bus timers"))
        .padding(.top, SettingsScale.sectionGap)

      VStack(alignment: .leading, spacing: SettingsScale.rowGap) {
        SettingsSecondsSlider(
          title: String(localized: "Pause before sending to Codescribe Bus"),
          detail: String(
            localized:
              "Silence after you stop speaking before the message is sent; also the hands-free dictation limit. Applies from the next channel or recording."
          ),
          seconds: model.settings.toggleSilenceSec,
          value: $model.toggleSilenceSlider,
          range: 0.5...30,
          step: 0.5
        )
        .settingsGroupedInset()
        .accessibilityIdentifier("agent-bus-speech-pause")

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
      }
      .padding(.top, SettingsScale.headerGap)

      // Mirrors `OverlayState`: armed in Agent mode only, fires after
      // `autoHideDelaySeconds`, cancelled the moment the transcript is edited.
      // One label, not a section heading repeating the switch name (Founder
      // brief, round 8, 2026-10-10). The title names the built-in Agent so this
      // fixed 5 s delay is not read as a bus setting.
      SettingsControlRow(
        title: String(localized: "Auto-send to the built-in Agent"),
        subtitle: String(
          localized: "Sends the transcript after 5 seconds unless you start editing it."
        )
      ) {
        Toggle(
          "",
          isOn: Binding(
            get: { model.settings.agentAutoSend },
            set: { model.setAgentAutoSend($0) }
          )
        )
        .toggleStyle(.switch)
        .labelsHidden()
        .tint(CSColor.chromeAccent)
      }
      .padding(.top, SettingsScale.sectionGap)

      // Collapsed by default: the editors above answer the everyday question;
      // this is where the endpoints and the settings keys live.
      DisclosureGroup(isExpanded: $detailsExpanded) {
        VStack(spacing: 0) {
          RuntimeRow(
            key: String(localized: "AI formatting"),
            value: model.formattingDescription,
            tint: true,
            trailing: .none,
            keyWidth: Self.keyWidth
          )
          ForEach(LLMLane.allCases) { lane in
            let laneModel = model.llmLane(lane)
            divider
            RuntimeRow(
              key: String(localized: "\(lane.title) provider"),
              value: laneModel.providerDisplayName,
              tint: false,
              trailing: .dot(laneModel.availabilityTint),
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
            .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
        }
        .padding(.top, SettingsScale.headerGap)
      } label: {
        SettingsSectionLabel(String(localized: "Active configuration details"))
      }
      .padding(.top, SettingsScale.sectionGap)
      .accessibilityIdentifier("agent-lanes-details")
    }
  }

  private var divider: some View {
    Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
  }
}
