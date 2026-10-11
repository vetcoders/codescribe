import SwiftUI

/// Agent › AI models: the lane editors, the auto-send switch, then the
/// resolved runtime truth — the answer to "which provider am I actually
/// talking to" — folded away as read-only proof of what resolved.
struct AgentLanesTab: View {
  @ObservedObject var model: SettingsViewModel
  @State private var detailsExpanded = false
  /// "Adres API: Formatowanie" and "Klucze: Formatowanie" overflow the
  /// shared 160 pt key column.
  static let keyWidth: CGFloat = 184

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      LLMLanesSection(model: model)

      // Mirrors `OverlayState`: armed in Agent mode only, fires after
      // `autoHideDelaySeconds`, cancelled the moment the transcript is edited.
      // One label, not a section heading repeating the switch name (Founder
      // brief, round 8, 2026-10-10).
      SettingsControlRow(
        title: String(localized: "Auto-send to the Agent"),
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
      .padding(.top, CSSpace.section)

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
        .padding(.top, CSSpace.control)
      } label: {
        SettingsSectionLabel(String(localized: "Active configuration details"))
      }
      .padding(.top, CSSpace.section)
      .accessibilityIdentifier("agent-lanes-details")
    }
  }

  private var divider: some View {
    Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
  }
}
