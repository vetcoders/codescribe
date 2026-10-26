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
  @State private var probeSample = ""

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
    case "cached": return CSColor.amberLight
    case "no_key", "loading": return CSColor.secondaryInk
    default: return CSColor.dangerLight
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
          .foregroundStyle(CSColor.secondaryInk)
        Text(model.laneUsageDescription(lane))
          .font(SettingsScale.helper)
          .foregroundStyle(laneModel.availabilityTint)
          .fixedSize(horizontal: false, vertical: true)
      }

      // The settings keys behind these rows live under "Active configuration
      // details" on the tab; the card itself stays free of identifiers.
      if lane == .formatting {
        // One short row (audit of this page, 2026-10-21): where Correction and
        // Smart run. Routing details and the .env override live in the guide.
        SettingsControlRow(
          title: String(
            localized: "Formatting engine",
            comment: "Settings row: Apple on this Mac or the cloud provider formats the text"),
          subtitle: String(
            localized: "Apple on your Mac first; this provider steps in when Apple fails.",
            comment: "Formatting engine row explanation"),
          inset: 10
        ) {
          Picker(
            "",
            selection: Binding(
              get: { model.settings.formatOnDevice },
              set: { model.setFormatOnDevice($0) }
            )
          ) {
            Text("Cloud provider", comment: "Formatting engine choice").tag(false)
            Text("Apple on this Mac (experimental)", comment: "Formatting engine choice").tag(true)
          }
          .labelsHidden()
          .fixedSize()
          .accessibilityLabel(
            String(
              localized: "Formatting engine",
              comment: "Settings row: Apple on this Mac or the cloud provider formats the text"))
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
      SettingsControlRow(title: String(localized: "Model"), subtitle: modelSourceCaption, inset: 10)
      {
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
            .foregroundStyle(CSColor.secondaryInk)
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

      reasoningRow
      if lane == .formatting {
        formattingTestRow
      }
    }
  }

  /// How long the lane's model may think, in oh-my-pi's levels. Formatting
  /// transforms text it already has, so it asks for none unless the user opts
  /// in (Maciej, 2026-10-22). The Agent keeps the model's own reasoning until a
  /// level is chosen. A provider that refuses a level is asked again lower.
  private var reasoningRow: some View {
    let isFormatting = lane == .formatting
    return SettingsControlRow(
      title: String(
        localized: "Reasoning",
        comment: "Settings row: how long the lane's model may think before answering"),
      subtitle: isFormatting
        ? String(
          localized:
            "None answers fastest. Higher levels are slower; choose one only if a model needs it.",
          comment: "Formatting reasoning row explanation")
        : String(
          localized:
            "Model default keeps what the model does on its own. In the chat: /effort.",
          comment: "Agent reasoning row explanation"),
      inset: 10
    ) {
      Picker(
        "",
        selection: Binding(
          get: {
            isFormatting
              ? model.settings.llmFormattingReasoning : model.settings.llmAssistiveReasoning
          },
          set: { level in
            if isFormatting {
              model.setFormattingReasoning(level)
            } else {
              model.setAgentReasoning(level)
            }
          }
        )
      ) {
        if !isFormatting {
          Text("Model default", comment: "Agent reasoning level: the model's own").tag("default")
        }
        ForEach(ReasoningLevel.all) { level in
          Text(level.title).tag(level.id)
        }
      }
      .labelsHidden()
      .fixedSize()
      .accessibilityLabel("\(lane.title) reasoning")
    }
  }

  /// One request through the live formatter path, so a provider is proven
  /// before a take depends on it (Maciej, 2026-10-22). Empty text sends a
  /// built-in raw transcript.
  private var formattingTestRow: some View {
    SettingsControlRow(
      title: String(localized: "Test", comment: "Settings row: send one formatting request now"),
      subtitle: String(
        localized: "Formats a raw transcript, or your text, the way dictation does.",
        comment: "Formatting test row explanation"),
      inset: 10
    ) {
      VStack(alignment: .leading, spacing: CSSpace.sm) {
        HStack(spacing: CSSpace.sm) {
          TextField(
            String(
              localized: "tak jeśli nie stosuje się zaleceń dokumentacji…",
              comment: "Formatting test placeholder: a raw unpunctuated transcript"),
            text: $probeSample, axis: .vertical
          )
          .lineLimit(1...4)
          .settingsInputChrome(isFocused: false)
          .accessibilityLabel("Formatting test text")
          Button {
            model.testFormatting(sample: probeSample)
          } label: {
            if model.formattingProbePending {
              ProgressView().controlSize(.small)
            } else {
              Text("Test", comment: "Button: send one formatting request")
            }
          }
          .disabled(model.formattingProbePending)
          .accessibilityLabel("Test formatting")
        }
        if let probe = model.formattingProbe {
          formattingProbeResult(probe)
        }
      }
      .frame(width: 440)
    }
  }

  private func formattingProbeResult(_ probe: CsFormattingProbe) -> some View {
    let seconds = { (ms: UInt64) in String(format: "%.1f s", Double(ms) / 1000) }
    let verdict: String
    let tone: CsMcpRowTone
    switch probe.status {
    case "applied":
      verdict = String(localized: "Formatted", comment: "Formatting test verdict")
      tone = .good
    case "unchanged":
      verdict = String(localized: "Answered, text unchanged", comment: "Formatting test verdict")
      tone = .good
    case "skipped":
      verdict = String(
        localized: "Not sent: formatting is off or runs on the Agent",
        comment: "Formatting test verdict")
      tone = .neutral
    case "timed_out":
      verdict = String(localized: "No answer in time", comment: "Formatting test verdict")
      tone = .warn
    default:
      verdict = String(localized: "Formatting failed", comment: "Formatting test verdict")
      tone = .bad
    }
    let level = probe.reasoning ?? String(localized: "not sent", comment: "Reasoning field absent")
    return VStack(alignment: .leading, spacing: CSSpace.xs) {
      HStack(spacing: CSSpace.sm) {
        StatusToneGlyph(tone: tone)
        Text(verdict)
        Text(
          "\(seconds(probe.elapsedMs)) / \(seconds(probe.deadlineMs)) · \(probe.model) · reasoning \(level)"
        )
        .foregroundStyle(CSColor.secondaryInk)
      }
      .font(SettingsScale.helper)
      if let message = Self.formattingFailureMessage(probe) {
        Text(message)
          .font(SettingsScale.helper)
          .foregroundStyle(CSColor.dangerLight)
          .fixedSize(horizontal: false, vertical: true)
      }
      let checked = Date(timeIntervalSince1970: Double(probe.checkedAtUnixMs) / 1000)
      Text("Tested: \(checked.formatted(date: .abbreviated, time: .standard))")
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
      if let host = probe.endpointHost {
        let credential = probe.credentialSource == "account"
          ? String(localized: "saved account") : String(localized: "API key or key-free endpoint")
        Text("Configured cloud route: \(probe.provider) · \(host) · \(credential)")
          .font(SettingsScale.helper)
          .foregroundStyle(CSColor.secondaryInk)
          .textSelection(.enabled)
      }
      if !probe.output.isEmpty, probe.status == "applied" || probe.status == "unchanged" {
        Text(probe.output)
          .font(SettingsScale.helper)
          .textSelection(.enabled)
          .fixedSize(horizontal: false, vertical: true)
      }
    }
    .accessibilityElement(children: .combine)
    .accessibilityIdentifier("formatting-test-result")
  }

  /// Only fixed diagnostic buckets cross the bridge. Never render a raw
  /// provider error: it can contain a bearer, prompt or transcript.
  static func formattingFailureMessage(_ probe: CsFormattingProbe) -> String? {
    guard probe.status == "failed" || probe.status == "timed_out" else { return nil }
    switch probe.failureCode {
    case "unavailable":
      return String(localized: "Formatting is unavailable. Check the provider, model and account settings.")
    case "account_authentication":
      return String(localized: "Account access failed. Check your connection and account settings, then repeat the test.")
    case "timeout":
      return String(localized: "The request exceeded the formatting deadline. Check the connection or choose a faster model.")
    case "transport":
      return String(localized: "The provider could not be reached. Check the connection and provider address.")
    case "http":
      switch probe.httpStatus {
      case 401:
        return String(localized: "Authentication rejected. Sign in again or replace the API key.")
      case 403:
        return String(localized: "Access denied. Check account permissions and model access.")
      case 402, 429:
        return String(localized: "Usage limit reached. Check billing and rate limits.")
      default:
        if let status = probe.httpStatus {
          return String(localized: "Provider request failed (HTTP \(Int(status))). Check the model and provider settings.")
        }
        return String(localized: "The provider rejected the request. Check the model and provider settings.")
      }
    default:
      return String(localized: "The provider returned no usable formatting result. Check the model and repeat the test.")
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
            .foregroundStyle(CSColor.secondaryInk)
            .fixedSize(horizontal: false, vertical: true)
        } else {
          Circle()
            .fill(discoveryDotColor.opacity(0.85))
            .frame(width: 7, height: 7)
          Text(laneModel.discoveryDescription)
            .font(SettingsScale.helper)
            .foregroundStyle(CSColor.secondaryInk)
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
            .foregroundStyle(CSColor.secondaryInk)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.top, 4)
        }
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
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
