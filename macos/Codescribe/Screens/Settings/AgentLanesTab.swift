import SwiftUI

/// Agent › LLM lanes: the lane editors, then the resolved runtime truth — the
/// answer to "which provider am I actually talking to", placed after the
/// editors as read-only proof of what resolved.
struct AgentLanesTab: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      LLMLanesSection(model: model)

      SettingsSectionLabel("Transcript delivery")
        .padding(.top, CSSpace.section)
      SettingsControlRow(
        title: "Auto-send to Agent",
        subtitle: "Send an untouched transcript 5 seconds after the take ends."
      ) {
        Toggle("", isOn: Binding(
          get: { model.settings.agentAutoSend },
          set: { model.setAgentAutoSend($0) }
        ))
        .toggleStyle(.switch)
        .labelsHidden()
        .tint(CSColor.chromeAccent)
      }

      SettingsSectionLabel("Resolved runtime truth")
        .padding(.top, CSSpace.section)

      VStack(spacing: 0) {
        RuntimeRow(
          key: "AI formatting",
          value: model.formattingDescription,
          tint: true,
          trailing: .none
        )
        ForEach(LLMLane.allCases) { lane in
          let laneModel = model.llmLane(lane)
          divider
          RuntimeRow(
            key: "\(lane.title) provider",
            value: laneModel.providerDisplayName,
            tint: false,
            trailing: .dot(laneModel.availabilityTint)
          )
          divider
          RuntimeRow(
            key: "\(lane.title) endpoint",
            value: laneModel.resolvedEndpoint,
            tint: false,
            mono: true,
            trailing: .none
          )
          divider
          RuntimeRow(
            key: "\(lane.title) model",
            value: laneModel.resolvedModel,
            tint: true,
            mono: true,
            trailing: .text(laneModel.availabilityDescription, laneModel.availabilityTint)
          )
        }
      }
      .clipShape(.rect(cornerRadius: CSRadius.composer))
      .overlay {
        RoundedRectangle(cornerRadius: CSRadius.composer)
          .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
      }
      .padding(.top, CSSpace.control)
    }
  }

  private var divider: some View {
    Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
  }
}
