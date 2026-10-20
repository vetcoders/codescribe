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
///
/// Lanes only. The send timers moved up to `AgentLanesTab`, where the two
/// Codescribe Bus ones stand together: a bus hold had no business sitting on
/// top of the provider/model grammar (Founder review, 2026-10-18).
struct LLMLanesSection: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: CSSpace.md) {
      if let notice = model.laneResetNotice {
        LaneResetNotice(text: notice)
      }

      ForEach(LLMLane.allCases) { lane in
        LLMLaneEditor(model: model, lane: lane)
        if lane != LLMLane.allCases.last {
          Rectangle()
            .fill(Color.primary.opacity(0.12))
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
  @FocusState private var modelFocused: Bool

  private var laneModel: LLMLaneModel { model.llmLane(lane) }

  private var trimmedDraft: String {
    modelDraft.trimmingCharacters(in: .whitespacesAndNewlines)
  }

  /// The field shows the stored override; Save means "store something new".
  private var canSave: Bool {
    !trimmedDraft.isEmpty && trimmedDraft != laneModel.configuredModel
  }

  private var hasOverride: Bool { !laneModel.configuredModel.isEmpty }

  /// Formatting on the Agent's provider shares its discovery record, so the
  /// same failure would print twice; the footer defers to the line above.
  private var failureShownOnAgent: Bool {
    laneModel.repeatsDiscoveryFailure(of: model.llmLane(.assistive))
  }

  /// Where the model name comes from: a stored override or the provider default.
  private var modelSourceCaption: String {
    hasOverride
      ? String(localized: "Set manually", comment: "Model field caption: an override is stored")
      : String(
        localized: "Provider default model",
        comment: "Model field caption: no override, the provider default resolves")
  }

  private var discoveryDotColor: Color {
    switch laneModel.discovery.status {
    case "fresh": return CSColor.olive
    case "cached": return CSColor.amber
    case "no_key", "loading": return Color.secondary
    default: return CSColor.terracotta
    }
  }

  var body: some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      VStack(alignment: .leading, spacing: 2) {
        Text(lane.title)
          .font(SettingsScale.settingName)
          .foregroundStyle(Color.primary)
        Text(lane.subtitle)
          .font(SettingsScale.helper)
          .foregroundStyle(Color.secondary)
        Text(model.laneUsageDescription(lane))
          .font(SettingsScale.helper)
          .foregroundStyle(laneModel.availabilityTint)
          .fixedSize(horizontal: false, vertical: true)
      }

      // The settings keys behind these rows live under "Active configuration
      // details" on the tab; the card itself stays free of identifiers.
      if lane == .formatting {
        SettingsControlRow(
          title: String(localized: "Text formatting"),
          subtitle: String(
            localized:
              "Applies to Smart and Corrections from the next formatting pass. The on-device model runs first without a cloud credential; on failure the formatting provider is used when one is configured. Max always uses the Agent model. A .env value overrides this toggle."
          ),
          inset: 10
        ) {
          Picker(
            "",
            selection: Binding(
              get: { model.settings.formatOnDevice },
              set: { model.setFormatOnDevice($0) }
            )
          ) {
            Text("Cloud provider").tag(false)
            Text("Apple on-device (experimental)").tag(true)
          }
          .labelsHidden()
          .fixedSize()
          .accessibilityLabel(String(localized: "Text formatting"))
        }
      }

      SettingsControlRow(title: String(localized: "Provider"), inset: 10) {
        Menu {
          ForEach(model.providers, id: \.id) { provider in
            Button {
              model.setLaneProvider(provider.id, for: lane)
            } label: {
              // Account credentials are available to Assistive only.
              Label {
                Text(providerMenuLabel(provider))
              } icon: {
                Image(
                  systemName: provider.id == laneModel.providerId
                    ? "checkmark.circle.fill" : "circle.fill"
                )
                .foregroundStyle(SettingsViewModel.availabilityTint(for: provider, lane: lane))
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

      // Above the Model row on purpose: this line reports the provider's model
      // list, and when there is none it tells you to type an ID — which has to
      // point at a field the eye reaches next, not one it already passed
      // (Founder review, 2026-10-18).
      discoveryFooter

      // The source caption sits under the row title: a third line under the
      // field made the card tall for one short word.
      SettingsControlRow(title: String(localized: "Model"), subtitle: modelSourceCaption, inset: 10) {
        VStack(alignment: .trailing, spacing: CSSpace.xs) {
          // Discovery state ("discovering…", cached, failed) is the line above.
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
          // The field holds the stored override; with none, the placeholder
          // is the provider default that actually resolved.
          HStack(spacing: CSSpace.sm) {
            TextField(laneModel.resolvedModel, text: $modelDraft)
              .settingsInputChrome(isFocused: modelFocused)
              .focused($modelFocused)
              .onSubmit(saveModel)
              .accessibilityLabel("\(lane.title) model ID")
            SettingsSaveButton(enabled: canSave, action: saveModel)
              .accessibilityLabel("Save \(lane.title) model")
            Button(String(localized: "Reset model", comment: "Clears the lane's model override")) {
              modelDraft = ""
              model.setLLMModel("", for: lane)
            }
            .font(SettingsScale.controlLabel)
            .foregroundStyle(Color.secondary)
            .csFocusRing()
            .disabled(!hasOverride)
            .help("Clear this model override")
            .accessibilityLabel("Reset \(lane.title) model")
          }
        }
        .frame(width: 440)
        .onAppear { modelDraft = laneModel.configuredModel }
        .onChange(of: laneModel.configuredModel) { _, stored in modelDraft = stored }
      }
    }
  }

  /// The model-list state for this lane: where the list came from, a refresh,
  /// and the provider's own error text on request.
  private var discoveryFooter: some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      HStack(spacing: CSSpace.sm) {
        if failureShownOnAgent {
          Text("Same provider as the Agent; the model list error is shown above.")
            .font(SettingsScale.helper)
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
        } else {
          Circle()
            .fill(discoveryDotColor.opacity(0.85))
            .frame(width: 7, height: 7)
          Text(laneModel.discoveryDescription)
            .font(SettingsScale.helper)
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
        Spacer(minLength: 0)
        SettingsRefreshButton(
          busy: laneModel.discovery.status == "loading",
          axLabel: "Refresh \(lane.title) models"
        ) { model.refreshModelDiscovery(providerId: laneModel.providerId) }
      }
      // The provider's own words, on request only; the main line stays plain.
      if !failureShownOnAgent, let details = laneModel.discoveryErrorDetails {
        DisclosureGroup("Error details") {
          Text(details)
            .font(CSFont.mono(10.5))
            .foregroundStyle(Color.secondary)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.top, 4)
        }
        .font(SettingsScale.helper)
        .foregroundStyle(Color.secondary)
        .accessibilityIdentifier("lane-discovery-error-details")
      }
    }
    .padding(.leading, 2)
  }

  /// Provider name, plus a marker for a host the user added by hand. The
  /// marker is copy; the separator around it is not.
  private func providerMenuLabel(_ provider: CsProviderOption) -> String {
    guard provider.kind == "custom" else { return provider.displayName }
    let marker = String(
      localized: "custom", comment: "Menu suffix: a provider the user added by hand")
    return "\(provider.displayName)  ·  \(marker)"
  }

  private func saveModel() {
    guard canSave else { return }
    model.setLLMModel(trimmedDraft, for: lane)
  }
}

#if DEBUG
  #Preview("Agent panel") {
    ScrollView { AgentPanel(model: .preview(.agent)) }
      .frame(width: 720, height: 900)
  }
#endif
