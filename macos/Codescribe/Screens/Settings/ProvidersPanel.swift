import SwiftUI

// Settings › Providers. "URLs live where the API keys are; models live where
// the Agent is." Vendors (OpenAI, xAI, Anthropic) are factory-pinned: the
// endpoint is shown, never edited. A custom provider is any host speaking the
// Responses or Messages wire; its endpoint sits next to its (optional) key.
// Speech-to-text is two atomic lanes (File, Live), each an endpoint + key row.
// Models are chosen per request lane under Agent › Request lanes.

struct ProvidersPanel: View {
  static let ownedCapabilities: Set<SettingsPanelCapability> = [.providers]

  @ObservedObject var model: SettingsViewModel
  @State private var formTarget: CustomProviderFormTarget?

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      EyebrowLabel(text: "Settings · \(SettingsSection.keys.title)")
      Text("Providers.")
        .font(CSFont.ui(26, .bold))
        .tracking(-0.5)
        .foregroundStyle(CSColor.textHigh)
        .padding(.top, 6)

      Text(
        "Keys and endpoints. Vendors always use their factory endpoint; a custom provider is any host that speaks /v1/responses or /v1/messages. Which model each lane sends lives under Agent › Request lanes."
      )
      .font(CSFont.ui(12.5))
      .lineSpacing(2)
      .foregroundStyle(CSColor.textMutedAlt)
      .padding(.top, 8)

      if let notice = model.laneResetNotice {
        LaneResetNotice(text: notice)
          .padding(.top, 12)
      }

      SettingsSectionLabel("Vendors")
        .padding(.top, CSSpace.section)
      VStack(spacing: 8) {
        ForEach(model.vendorProviders, id: \.id) { provider in
          ProviderCard(model: model, provider: provider)
        }
      }
      .padding(.top, CSSpace.control)

      CustomProvidersSection(
        model: model,
        onAdd: { formTarget = .add },
        onEdit: { formTarget = .edit($0) }
      )
      .padding(.top, CSSpace.section)

      SpeechToTextSection(model: model)
        .padding(.top, CSSpace.section)

      ServiceKeysSection(model: model)
        .padding(.top, CSSpace.section)

      HStack(spacing: 8) {
        Text("●").font(CSFont.mono(11, .medium)).foregroundStyle(CSColor.olive)
        Text("secrets live only in the Keychain — presence shown, value hidden")
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.textFaint)
      }
      .padding(.top, 16)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
    .sheet(item: $formTarget) { target in
      CustomProviderForm(model: model, target: target)
    }
  }
}

/// What the custom-provider sheet is editing. `Identifiable` so `.sheet(item:)`
/// re-seeds the form per target instead of reusing stale drafts.
enum CustomProviderFormTarget: Identifiable {
  case add
  case edit(CsProviderOption)

  var id: String {
    switch self {
    case .add: return "add"
    case .edit(let provider): return provider.id
    }
  }
}

/// A lane lost its provider (custom row removed) and fell back to the default
/// vendor. Rendered on Providers (where the removal happened) and on Request
/// lanes (where the effect is), until the next lane edit.
struct LaneResetNotice: View {
  let text: String

  var body: some View {
    HStack(spacing: 8) {
      Circle().fill(CSColor.amber).frame(width: 7, height: 7)
      Text(text)
        .font(CSFont.mono(11, .medium))
        .foregroundStyle(CSColor.amber)
        .lineLimit(2)
    }
    .accessibilityIdentifier("lane-reset-notice")
  }
}

// MARK: - Provider card (vendor and custom share one shell)

/// One registry row: name, wire, read-only endpoint (selectable, never edited
/// here), key row. A vendor adds its OAuth account row; a custom row adds
/// Edit / Remove — the endpoint moves only through `CustomProviderForm`.
/// There is no endpoint setter on the view-model for a vendor — by design.
struct ProviderCard: View {
  @ObservedObject var model: SettingsViewModel
  let provider: CsProviderOption
  var onEdit: (() -> Void)?

  @State private var confirmRemove = false

  private var isCustom: Bool { provider.kind == "custom" }
  /// "Responses" / "Messages" — the one thing every provider declares.
  private var wireLabel: String {
    ["responses": "Responses", "messages": "Messages"][provider.wire] ?? provider.wire
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack(spacing: 10) {
        Text(provider.displayName)
          .font(CSFont.ui(14.5, .bold))
          .foregroundStyle(CSColor.textHigh)
        Text(wireLabel)
          .font(CSFont.mono(10, .semibold))
          .foregroundStyle(CSColor.textMutedAlt)
          .padding(.horizontal, 7)
          .padding(.vertical, 3)
          .background(Capsule().fill(CSColor.surfaceRaised(0.05)))
          .overlay(Capsule().strokeBorder(CSColor.hairline(0.1), lineWidth: 1))
          .accessibilityLabel("\(wireLabel) wire")
        Spacer(minLength: 0)
        if isCustom {
          SettingsChipButton("Edit", tint: CSColor.textMutedAlt) { onEdit?() }
            .accessibilityLabel("Edit custom provider \(provider.displayName)")
          SettingsChipButton("Remove", tint: CSColor.terracottaLight) { confirmRemove = true }
            .accessibilityLabel("Remove custom provider \(provider.displayName)")
        }
      }
      HStack(spacing: 8) {
        Text(isCustom ? "endpoint" : "factory endpoint")
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textFaint)
        Text(provider.endpoint)
          .font(CSFont.mono(11.5, .medium))
          .foregroundStyle(CSColor.textBodyAlt)
          .lineLimit(1)
          .truncationMode(.middle)
          .textSelection(.enabled)
        Spacer(minLength: 0)
      }
      .accessibilityElement(children: .combine)
      .accessibilityLabel("endpoint \(provider.endpoint)")
      // Custom hosts are key-optional: an absent key there is neutral, not an error.
      KeyRow(
        model: model, account: provider.apiKeyAccount, label: "API key",
        isSet: provider.apiKeySet, optional: !provider.keyRequired)
      if !isCustom, provider.accountLoginEnabled || provider.accountSignedIn {
        AccountLoginRow(
          provider: provider,
          loginPending: model.accountLoginPending.contains(provider.id),
          loginNotice: model.accountLoginNotices[provider.id],
          onStart: { model.startAccountLogin(providerId: provider.id) },
          onSignOut: { model.signOutAccount(providerId: provider.id) },
          onSaveClientId: { model.saveOauthClientId(providerId: provider.id, value: $0) }
        )
      }
    }
    .csSettingsCard()
    .accessibilityElement(children: .contain)
    .accessibilityLabel("\(provider.displayName) \(isCustom ? "custom provider" : "provider")")
    .confirmationDialog(
      "Remove \(provider.displayName)?",
      isPresented: $confirmRemove,
      titleVisibility: .visible
    ) {
      Button("Remove provider and its key", role: .destructive) {
        model.removeCustomProvider(id: provider.id)
      }
      Button("Cancel", role: .cancel) {}
    } message: {
      Text(
        "Deletes the provider row and its API key from the Keychain. Any request lane using it falls back to the default vendor."
      )
    }
  }
}

// MARK: - Custom providers

struct CustomProvidersSection: View {
  @ObservedObject var model: SettingsViewModel
  let onAdd: () -> Void
  let onEdit: (CsProviderOption) -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack {
        SettingsSectionLabel("Custom providers")
        Spacer()
        Button(action: onAdd) {
          Label("Add custom provider", systemImage: "plus")
            .font(CSFont.ui(12, .semibold))
        }
        .csFocusRing()
        .foregroundStyle(CSColor.textBody)
        .accessibilityIdentifier("providers-add-custom")
      }

      Text(
        "Any host speaking the OpenAI Responses or Anthropic Messages wire — a local model server, a gateway, a relay. The key is optional; add as many as you need."
      )
      .font(CSFont.ui(11.5))
      .lineSpacing(2)
      .foregroundStyle(CSColor.textMutedAlt)
      .padding(.top, 8)

      if model.customProviders.isEmpty {
        Text("No custom providers yet.")
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.textFaint)
          .padding(.top, 12)
      } else {
        VStack(spacing: 8) {
          ForEach(model.customProviders, id: \.id) { provider in
            ProviderCard(model: model, provider: provider, onEdit: { onEdit(provider) })
          }
        }
        .padding(.top, 12)
      }
    }
  }
}

/// Add / edit sheet. Validation is the bridge's job (`CsError` → message under
/// the fields); the form only keeps the obviously blank draft unsaveable.
struct CustomProviderForm: View {
  @ObservedObject var model: SettingsViewModel
  let target: CustomProviderFormTarget

  @Environment(\.dismiss) private var dismiss
  @State private var name = ""
  @State private var wire = "responses"
  @State private var endpoint = ""
  @State private var apiKey = ""
  @State private var error: String?
  @FocusState private var focus: Field?

  private enum Field { case name, endpoint, key }

  private var isEdit: Bool {
    if case .edit = target { return true }
    return false
  }

  private var canSave: Bool {
    !name.trimmingCharacters(in: .whitespaces).isEmpty
      && !endpoint.trimmingCharacters(in: .whitespaces).isEmpty
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      Text(isEdit ? "Edit custom provider." : "Add custom provider.")
        .font(CSFont.ui(20, .bold))
        .tracking(-0.3)
        .foregroundStyle(CSColor.textHigh)
      Text("The endpoint is normalized to the wire's canonical path on save.")
        .font(CSFont.ui(11.5))
        .foregroundStyle(CSColor.textMutedAlt)

      field("Name") {
        TextField("e.g. Libraxis", text: $name)
          .settingsInputChrome()
          .focused($focus, equals: .name)
          .onSubmit { focus = .endpoint }
          .accessibilityLabel("Custom provider name")
      }
      field("Wire") {
        Picker("Wire", selection: $wire) {
          Text("Responses (/v1/responses)").tag("responses")
          Text("Messages (/v1/messages)").tag("messages")
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .accessibilityLabel("Custom provider wire")
      }
      field("Endpoint") {
        TextField("https://api.example.com/v1/responses", text: $endpoint)
          .settingsInputChrome()
          .focused($focus, equals: .endpoint)
          .onSubmit { focus = .key }
          .accessibilityLabel("Custom provider endpoint")
      }
      field("API key (optional)") {
        SecureField(isEdit ? "Leave empty to keep the stored key" : "Paste key…", text: $apiKey)
          .settingsInputChrome()
          .focused($focus, equals: .key)
          .onSubmit(save)
          .accessibilityLabel("Custom provider API key")
      }

      if let error {
        Text(error)
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.terracottaLight)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityIdentifier("custom-provider-form-error")
      }

      HStack(spacing: 10) {
        Spacer()
        Button("Cancel") { dismiss() }
          .keyboardShortcut(.cancelAction)
          .csFocusRing()
        SettingsSaveButton(
          title: isEdit ? "Save changes" : "Add provider", enabled: canSave, action: save
        )
        .accessibilityIdentifier("custom-provider-form-save")
      }
      .padding(.top, 4)
    }
    .padding(24)
    .frame(width: 480)
    .background(CSColor.windowWash)
    .onAppear {
      if case .edit(let provider) = target {
        name = provider.displayName
        wire = provider.wire
        endpoint = provider.endpoint
      }
      focus = .name
    }
  }

  private func field<Control: View>(
    _ title: String, @ViewBuilder control: () -> Control
  ) -> some View {
    VStack(alignment: .leading, spacing: 5) {
      Text(title)
        .font(CSFont.mono(10.5, .semibold))
        .foregroundStyle(CSColor.textMuted)
      control()
    }
  }

  private func save() {
    guard canSave else { return }
    let draft = CsCustomProviderDraft(
      name: name.trimmingCharacters(in: .whitespacesAndNewlines),
      wire: wire,
      endpoint: endpoint.trimmingCharacters(in: .whitespacesAndNewlines),
      apiKey: apiKey.isEmpty ? nil : apiKey
    )
    do {
      switch target {
      case .add:
        try model.addCustomProvider(draft)
      case .edit(let provider):
        try model.updateCustomProvider(id: provider.id, draft)
      }
      dismiss()
    } catch {
      self.error = String(describing: error)
    }
  }
}

// MARK: - Speech-to-text Cloud Service (stt-lanes-v1 §D)

/// Two lanes, File then Live, in `sttLanes()` order. Each lane is one atomic endpoint +
/// key row (ADR Tier 3): the key is never shown apart from the address it authenticates.
struct SpeechToTextSection: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel("Speech-to-text Cloud Service")
      Text("Your recordings leave your machine.")
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(CSColor.amber)
        .padding(.top, 8)
      Text("Cloud mode and consent stay on Dictation; endpoints and keys live here.")
        .font(CSFont.ui(11.5))
        .lineSpacing(2)
        .foregroundStyle(CSColor.textMutedAlt)
        .padding(.top, 4)
      VStack(spacing: 8) {
        ForEach(model.sttLanes, id: \.id) { lane in
          SttLaneCard(model: model, lane: lane)
        }
      }
      .padding(.top, 12)
    }
  }
}

/// One lane card: title, what the lane accepts, its endpoint row and its key
/// row (with Test). The Live card also hosts the session-mint URL, moved here
/// from Dictation because URLs live where the keys are.
struct SttLaneCard: View {
  @ObservedObject var model: SettingsViewModel
  let lane: CsSttLane

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      Text(lane.title)
        .font(CSFont.ui(14.5, .bold))
        .foregroundStyle(CSColor.textHigh)
      Text(lane.accepts)
        .font(CSFont.mono(10.5, .medium))
        .foregroundStyle(CSColor.textMutedAlt)
        .fixedSize(horizontal: false, vertical: true)
      SettingsUrlRow(
        title: "Endpoint",
        keyLabel: lane.endpointWireKey,
        current: lane.endpoint ?? "",
        placeholder: lane.placeholder,
        help:
          "Not a secret. Blank clears the lane; the bridge rejects a URL whose scheme does not fit this lane.",
        onSave: { model.setSttLaneEndpoint(lane.id, $0) }
      )
      KeyRow(model: model, account: lane.keyAccount, label: "API key", isSet: lane.apiKeySet)
      if lane.id == "live" {
        SettingsUrlRow(
          title: "Gateway session URL",
          keyLabel: "CODESCRIBE_ASR_GATEWAY_URL",
          current: model.asrGatewayUrl,
          placeholder: "https://…/session",
          help:
            "Session-mint endpoint for live Cloud Layer 1. Not the live socket above. Clearing restores unset.",
          onSave: { model.setAsrGatewayUrl($0) }
        )
      }
    }
    .csSettingsCard()
    .accessibilityElement(children: .contain)
    .accessibilityLabel("\(lane.title) lane")
  }
}

// MARK: - Service keys

/// Keys that are not LLM providers and not speech-to-text: GitHub. Only the
/// secret is edited here.
struct ServiceKeysSection: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel("Service keys")
      Text("GitHub token. Speech-to-text endpoints and keys live in the section above.")
        .font(CSFont.ui(11.5))
        .lineSpacing(2)
        .foregroundStyle(CSColor.textMutedAlt)
        .padding(.top, 8)
      VStack(spacing: 8) {
        ForEach(model.serviceKeyAccounts, id: \.self) { account in
          KeyRow(
            model: model, account: account, label: SettingsViewModel.keyLabel(for: account),
            isSet: model.keyStatus.isSet(account: account))
        }
      }
      .padding(.top, 12)
    }
  }
}

#if DEBUG
  #Preview("Providers panel") {
    ScrollView { ProvidersPanel(model: .preview(.keys)) }
      .frame(width: 720, height: 900)
      .background(CSColor.windowWash)
      .preferredColorScheme(.dark)
  }
#endif
