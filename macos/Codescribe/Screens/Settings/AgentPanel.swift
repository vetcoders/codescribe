import SwiftUI

// Agent panel: the one owner of request lanes and local agent substrate.
// Lanes pick a PROVIDER (from Settings › Providers) and a MODEL; there is no
// endpoint field here — the endpoint belongs to the provider.
struct AgentPanel: View {
  static let ownedCapabilities: Set<SettingsPanelCapability> = [
    .llmLanes,
    .workspaceRoots,
    .agentStatus,
    .mcpServers,
    .toolPermissions,
  ]

  @ObservedObject var model: SettingsViewModel

  /// One page at a time, and the page IS the content. Five independent
  /// subsystems (lanes, roots, capabilities, tool permissions, MCP servers —
  /// the last two alone are ~800 lines) used to stack into a single scroll.
  /// Each page owns its headline and body; the resolved-truth table lives only
  /// where it is the subject (Request lanes), below the editors.
  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      EyebrowLabel(text: "Settings · \(SettingsSection.agent.title) · \(current.title)")
      Text(headline)
        .font(CSFont.ui(26, .bold))
        .tracking(-0.5)
        .foregroundStyle(CSColor.textHigh)
        .padding(.top, 6)

      Text(blurb)
        .font(CSFont.ui(12.5))
        .lineSpacing(2)
        .foregroundStyle(CSColor.textMutedAlt)
        .padding(.top, 8)

      page
        .padding(.top, CSSpace.lg)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
  }

  /// The nil route (a deep link that named no page) lands on lanes,
  /// mirroring the `page` switch below. Pages of other sections cannot
  /// reach this panel, so they clamp to lanes too.
  private var current: SettingsPage {
    switch model.page {
    case .agentWorkspace, .agentStatus, .agentTools, .agentMcp:
      return model.page ?? .agentLanes
    default:
      return .agentLanes
    }
  }

  private var headline: String {
    switch current {
    case .agentWorkspace: return "Workspace roots."
    case .agentStatus: return "Capabilities."
    case .agentTools: return "Tool permissions."
    case .agentMcp: return "MCP servers."
    default: return "Request lanes."
    }
  }

  private var blurb: String {
    switch current {
    case .agentWorkspace:
      return "Directories the agent may read and write. Everything outside them is out of reach."
    case .agentStatus:
      return "What the local agent substrate can currently do, and why."
    case .agentTools:
      return "Allow, ask, or deny — per tool. Deny wins over everything."
    case .agentMcp:
      return "External MCP servers the agent can call, and their transports."
    default:
      return
        "Provider and model per request path. Endpoints and keys live on Providers; the resolved runtime truth is below the editors."
    }
  }

  @ViewBuilder
  private var page: some View {
    switch model.page {
    case .agentWorkspace:
      WorkspaceRootsSection(model: model)
    case .agentStatus:
      AgentStatusSection(model: model)
    case .agentTools:
      ToolPermissionsSection(model: model)
    case .agentMcp:
      MCPServersSection(model: model)
    default:
      // `.agentLanes` and the nil route (deep link that named no page).
      VStack(alignment: .leading, spacing: 0) {
        LLMLanesSection(model: model)

        // The answer to "which provider am I actually talking to",
        // placed after the editors as read-only proof of what resolved.
        SettingsSectionLabel("Resolved runtime truth")
          .padding(.top, CSSpace.section)
        runtimeRows
          .padding(.top, CSSpace.control)
      }
    }
  }

  // MARK: - Resolved LLM truth (read-only)

  private var runtimeRows: some View {
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
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
        .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
    )
  }

  private var divider: some View {
    Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
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
