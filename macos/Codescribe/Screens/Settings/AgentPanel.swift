import SwiftUI

// Agent panel: the one owner of request lanes, prompts and local agent
// substrate. Lanes pick a PROVIDER (from Settings › Providers) and a MODEL;
// there is no endpoint field here — the endpoint belongs to the provider.
struct AgentPanel: View {
  static let ownedCapabilities: Set<SettingsPanelCapability> = [
    .llmLanes,
    .prompts,
    .workspaceRoots,
    .agentStatus,
    .mcpServers,
    .toolPermissions,
  ]

  @ObservedObject var model: SettingsViewModel

  /// One tab at a time, and the tab IS the content. Six independent
  /// subsystems (lanes, prompts, roots, capabilities, tool permissions, MCP
  /// servers) used to stack into one scroll, then into sidebar child rows.
  var body: some View {
    SettingsTabbedPane(model: model, section: .agent) {
      switch model.currentTab {
      case .agentPrompts:
        PromptPanel(model: model)
      case .agentWorkspace:
        WorkspaceRootsSection(model: model)
      case .agentStatus:
        AgentStatusSection(model: model)
      case .agentTools:
        ToolPermissionsSection(model: model)
      case .agentMcp:
        MCPServersSection(model: model)
      default:
        // `.agentLanes`; other sections' tabs cannot reach this panel.
        AgentLanesTab(model: model)
      }
    }
  }
}

// MARK: - Editable request lanes (the one provider/model edit grammar)

/// Two request lanes sharing one visual grammar while preserving their distinct
/// promoted config keys (`LLM_<LANE>_PROVIDER`, `LLM_<LANE>_MODEL`).
struct LLMLanesSection: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      Text(
        "Pick a provider and a model per request path. Discovery lists what the provider serves; the Model ID field always accepts a name the list does not know."
      )
      .font(CSFont.ui(11.5))
      .lineSpacing(2)
      .foregroundStyle(CSColor.textMutedAlt)

      if let notice = model.laneResetNotice {
        LaneResetNotice(text: notice)
      }

      ForEach(LLMLane.allCases) { lane in
        LLMLaneEditor(model: model, lane: lane)
        if lane != LLMLane.allCases.last {
          Rectangle()
            .fill(CSColor.hairline(0.05))
            .frame(height: 1)
        }
      }
    }
  }
}

private struct LLMLaneEditor: View {
  @ObservedObject var model: SettingsViewModel
  let lane: LLMLane

  @State private var modelDraft = ""

  private var laneModel: LLMLaneModel { model.llmLane(lane) }

  private var discoveryDotColor: Color {
    switch laneModel.discovery.status {
    case "fresh": return CSColor.olive
    case "cached": return CSColor.amber
    case "no_key", "loading": return CSColor.textFaint
    default: return CSColor.terracotta
    }
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      VStack(alignment: .leading, spacing: 2) {
        Text(lane.title)
          .font(CSFont.ui(14.5, .bold))
          .foregroundStyle(CSColor.textHigh)
        Text(lane.subtitle)
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.textMutedAlt)
      }

      SettingsControlRow(title: "Provider", subtitle: lane.providerKey) {
        Menu {
          ForEach(model.providers, id: \.id) { provider in
            Button {
              model.setLaneProvider(provider.id, for: lane)
            } label: {
              // Availability dot: key or account present, or key-optional host.
              Label {
                Text(provider.displayName + (provider.kind == "custom" ? "  ·  custom" : ""))
              } icon: {
                Image(
                  systemName: provider.id == laneModel.providerId
                    ? "checkmark.circle.fill" : "circle.fill"
                )
                .foregroundStyle(SettingsViewModel.availabilityTint(for: provider))
              }
            }
          }
        } label: {
          SettingsMenuLabel(text: laneModel.providerDisplayName)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .accessibilityLabel("\(lane.title) provider")
        .accessibilityValue(laneModel.providerDisplayName)
      }

      SettingsControlRow(title: "Model", subtitle: lane.modelKey) {
        VStack(alignment: .trailing, spacing: 8) {
          // Discovery state ("discovering…", cached, failed) is the footer line below.
          if laneModel.usesDiscoveredPicker {
            let current =
              laneModel.modelOptions.first { $0.id == laneModel.resolvedModel }?.displayName
              ?? laneModel.resolvedModel
            Menu {
              ForEach(laneModel.modelOptions, id: \.id) { option in
                Button {
                  model.setLLMModel(option.id, for: lane)
                } label: {
                  if option.id == laneModel.resolvedModel {
                    Label(option.displayName, systemImage: "checkmark")
                  } else {
                    Text(option.displayName)
                  }
                }
              }
            } label: {
              SettingsMenuLabel(text: current)
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .accessibilityLabel("\(lane.title) model")
            .accessibilityValue(current)
          }

          // Always present: custom hosts may publish no list, and a name the
          // list does not know is still a valid model for the lane (D2).
          HStack(spacing: 8) {
            TextField(laneModel.resolvedModel, text: $modelDraft)
              .settingsInputChrome()
              .onSubmit(saveModel)
              .accessibilityLabel("\(lane.title) model ID")
            SettingsSaveButton(enabled: !modelDraft.isEmpty, action: saveModel)
              .accessibilityLabel("Save \(lane.title) model")
            Button("Reset") {
              modelDraft = ""
              model.setLLMModel("", for: lane)
            }
            .font(CSFont.ui(11.5, .semibold))
            .foregroundStyle(CSColor.textMutedAlt)
            .csFocusRing()
            .help("Clear this model override")
            .accessibilityLabel("Reset \(lane.title) model")
          }
        }
        .frame(width: 380)
      }

      HStack(spacing: 8) {
        Circle()
          .fill(discoveryDotColor.opacity(0.85))
          .frame(width: 7, height: 7)
        Text(laneModel.discoveryDescription)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.textFaint)
          .lineLimit(2)
        Spacer(minLength: 0)
        Button("Refresh") {
          model.refreshModelDiscovery(providerId: laneModel.providerId)
        }
        .font(CSFont.ui(11, .semibold))
        .foregroundStyle(CSColor.textMutedAlt)
        .csFocusRing()
        .accessibilityLabel("Refresh \(lane.title) models")
      }
      .padding(.leading, 2)
    }
  }

  private func saveModel() {
    model.setLLMModel(modelDraft, for: lane)
    modelDraft = ""
  }
}

#if DEBUG
  #Preview("Agent panel") {
    ScrollView { AgentPanel(model: .preview(.agent)) }
      .frame(width: 720, height: 900)
      .background(CSColor.windowWash)
      .preferredColorScheme(.dark)
  }
#endif
