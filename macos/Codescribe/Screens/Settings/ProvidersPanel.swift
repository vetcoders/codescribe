import OSLog
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
      SettingsPageHeader(
        String(localized: "Providers."),
        blurb: String(
          localized:
            "Keys and endpoints. Vendors always use their factory endpoint; a custom provider is any host that speaks /v1/responses or /v1/messages. Which model each lane sends lives under Agent › Request lanes."
        )
      )

      if let notice = model.laneResetNotice {
        LaneResetNotice(text: notice)
          .padding(.top, 12)
      }

      if model.providerAccessPending || model.providerMutationPending {
        ProgressView()
          .controlSize(.small)
          .padding(.top, 12)
        Text(model.providerMutationPending
          ? String(localized: "Updating provider access…")
          : String(localized: "Checking provider access…"))
          .font(CSFont.ui(11.5))
      }
      if let error = model.providerAccessError {
        Text(model.providerAccessResolved
          ? String(localized: "Provider access is unavailable. The last checked values are shown below.")
          : String(localized: "Provider access is unavailable. No credential status has been confirmed."))
          .font(CSFont.ui(11.5))
          .padding(.top, 12)
        Text(error).font(CSFont.mono(10.5)).textSelection(.enabled)
      }
      Button("Refresh provider access") { model.refreshProviderAccess() }
        .disabled(model.providerAccessPending || model.providerMutationPending)
        .padding(.top, 12)
      if model.providerAccessResolved {
        SettingsSectionLabel(String(localized: "Vendors"))
          .padding(.top, CSSpace.section)
        VStack(spacing: 8) {
          ForEach(model.vendorProviders, id: \.id) { provider in
            ProviderCard(model: model, provider: provider)
          }
        }
        .padding(.top, CSSpace.control)
        .disabled(model.providerMutationPending)

        CustomProvidersSection(
          model: model,
          onAdd: { formTarget = .add },
          onEdit: { formTarget = .edit($0) }
        )
        .padding(.top, CSSpace.section)
        .disabled(model.providerMutationPending)

        SpeechToTextSection(model: model)
          .padding(.top, CSSpace.section)

        ServiceKeysSection(model: model)
          .padding(.top, CSSpace.section)
          .disabled(model.providerMutationPending)

      }

      HStack(spacing: 8) {
        Text(verbatim: "●").font(CSFont.mono(11, .medium)).foregroundStyle(CSColor.olive)
        Text("secrets live only in the Keychain — presence shown, value hidden")
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(Color.secondary)
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
          .foregroundStyle(Color.primary)
        Text(wireLabel)
          .font(CSFont.mono(10, .semibold))
          .foregroundStyle(Color.secondary)
          .padding(.horizontal, 7)
          .padding(.vertical, 3)
          .background(Capsule().fill(Color.primary.opacity(0.1)))
          .overlay(Capsule().strokeBorder(Color.primary.opacity(0.12), lineWidth: 1))
          .accessibilityLabel("\(wireLabel) wire")
        Spacer(minLength: 0)
        if isCustom {
          SettingsChipButton("Edit", tint: Color.secondary) { onEdit?() }
            .accessibilityLabel("Edit custom provider \(provider.displayName)")
          SettingsChipButton("Remove", tint: CSColor.terracotta) { confirmRemove = true }
            .accessibilityLabel("Remove custom provider \(provider.displayName)")
        }
      }
      HStack(spacing: 8) {
        Text(isCustom ? "endpoint" : "factory endpoint")
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(Color.secondary)
        Text(provider.endpoint)
          .font(CSFont.mono(11.5, .medium))
          .foregroundStyle(Color.primary)
          .lineLimit(1)
          .truncationMode(.middle)
          .textSelection(.enabled)
        Spacer(minLength: 0)
      }
      .accessibilityElement(children: .combine)
      .accessibilityLabel("endpoint \(provider.endpoint)")
      // Custom hosts are key-optional: an absent key there is neutral, not an error.
      KeyRow(
        model: model, account: provider.apiKeyAccount, label: String(localized: "API key"),
        isSet: provider.apiKeySet, optional: !provider.keyRequired)
      if let error = model.providerAccountErrors[provider.id] {
        Text("Account access unavailable")
          .font(CSFont.ui(12, .semibold))
        Text(error).font(CSFont.ui(11.5)).textSelection(.enabled)
        SettingsChipButton("Sign out", tint: CSColor.terracotta) {
          model.signOutAccount(providerId: provider.id)
        }
        .accessibilityLabel("Sign out \(provider.displayName)")
      } else if !isCustom, provider.accountLoginEnabled || provider.accountSignedIn {
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
    .settingsGroupedInset()
    .accessibilityElement(children: .contain)
    .accessibilityLabel(
      isCustom
        ? Text("\(provider.displayName) custom provider")
        : Text("\(provider.displayName) provider")
    )
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
        SettingsSectionLabel(String(localized: "Custom providers"))
        Spacer()
        Button(action: onAdd) {
          Label("Add custom provider", systemImage: "plus")
            .font(CSFont.ui(12, .semibold))
        }
        .csFocusRing()
        .foregroundStyle(Color.primary)
        .accessibilityIdentifier("providers-add-custom")
      }

      Text(
        "Any host speaking the OpenAI Responses or Anthropic Messages wire — a local model server, a gateway, a relay. The key is optional; add as many as you need."
      )
      .font(CSFont.ui(11.5))
      .lineSpacing(2)
      .foregroundStyle(Color.secondary)
      .padding(.top, 8)

      if model.customProviders.isEmpty {
        Text("No custom providers yet.")
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(Color.secondary)
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

/// Add / edit sheet. The bridge owns validation; the form presents endpoint
/// failures at the field and other failures through the shared error-message seam.
struct CustomProviderForm: View {
  @ObservedObject var model: SettingsViewModel
  let target: CustomProviderFormTarget

  @Environment(\.dismiss) private var dismiss
  @State private var name = ""
  @State private var wire = "responses"
  @State private var endpoint = ""
  @State private var apiKey = ""
  @State private var error: String?
  @State private var endpointError: String?
  @State private var saving = false
  @FocusState private var focus: Field?

  private enum Field { case name, endpoint, key }

  /// Sample values, not copy: they must read the same in every language.
  private static let namePlaceholder = "e.g. Libraxis"
  private static let endpointPlaceholder = "https://api.example.com/v1/responses"
  private static let log = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "custom-provider-form"
  )

  private var isEdit: Bool {
    if case .edit = target { return true }
    return false
  }

  private var canSave: Bool {
    !saving && !model.providerMutationPending
      && !name.trimmingCharacters(in: .whitespaces).isEmpty
      && !endpoint.trimmingCharacters(in: .whitespaces).isEmpty
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      Text(isEdit ? "Edit custom provider." : "Add custom provider.")
        .font(CSFont.ui(20, .bold))
        .tracking(-0.3)
        .foregroundStyle(Color.primary)
      Text("The endpoint is normalized to the wire's canonical path on save.")
        .font(CSFont.ui(11.5))
        .foregroundStyle(Color.secondary)

      field("Name") {
        TextField(Self.namePlaceholder, text: $name)
          .settingsInputChrome(isFocused: focus == .name)
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
        TextField(Self.endpointPlaceholder, text: $endpoint)
          .settingsInputChrome(isFocused: focus == .endpoint)
          .focused($focus, equals: .endpoint)
          .onSubmit { focus = .key }
          .onChange(of: endpoint) { _, _ in endpointError = nil }
          .accessibilityLabel("Custom provider endpoint")
          .accessibilityHint(endpointError ?? "")
        if let endpointError {
          Text(endpointError)
            .font(CSFont.mono(11, .medium))
            .foregroundStyle(CSColor.terracotta)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("custom-provider-endpoint-error")
        }
      }
      field("API key (optional)") {
        SecureField(isEdit ? "Leave empty to keep the stored key" : "Paste key…", text: $apiKey)
          .settingsInputChrome(isFocused: focus == .key)
          .focused($focus, equals: .key)
          .onSubmit(save)
          .accessibilityLabel("Custom provider API key")
      }

      if let error {
        Text(error)
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.terracotta)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityIdentifier("custom-provider-form-error")
      }

      if saving {
        ProgressView("Saving provider…")
          .controlSize(.small)
      }
      HStack(spacing: 10) {
        Spacer()
        Button("Cancel") { dismiss() }
          .disabled(saving)
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
    .interactiveDismissDisabled(saving)
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
    _ title: LocalizedStringKey, @ViewBuilder control: () -> Control
  ) -> some View {
    VStack(alignment: .leading, spacing: 5) {
      Text(title)
        .font(CSFont.mono(10.5, .semibold))
        .foregroundStyle(Color.secondary)
      control()
    }
  }

  private func save() {
    guard canSave else { return }
    error = nil
    endpointError = nil
    let draft = CsCustomProviderDraft(
      name: name.trimmingCharacters(in: .whitespacesAndNewlines),
      wire: wire,
      endpoint: endpoint.trimmingCharacters(in: .whitespacesAndNewlines),
      apiKey: apiKey.isEmpty ? nil : apiKey
    )
    saving = true
    Task { @MainActor in
      defer { saving = false }
      do {
        switch target {
        case .add:
          try await model.addCustomProvider(draft)
        case .edit(let provider):
          try await model.updateCustomProvider(id: provider.id, draft)
        }
        dismiss()
      } catch {
        Self.log.error("Custom provider save failed: \(String(reflecting: error), privacy: .private)")
        // The bridge currently carries ProviderError's Display sentence in Config.
        // Match its endpoint reason, never revalidate the URL with a second parser.
        if let bridgeError = error as? CsError,
          case .Config(let message) = bridgeError,
          message.hasPrefix("endpoint '"),
          message.hasSuffix("' needs an http(s) scheme and a host")
        {
          endpointError = String(
            localized: "Enter an HTTP or HTTPS URL with a host, such as https://api.example.com."
          )
          focus = .endpoint
        } else {
          self.error = error.userFacingMessage
        }
      }
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
      SettingsSectionLabel(String(localized: "Speech-to-text Cloud Service"))
      Text("Your recordings leave your machine.")
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(CSColor.amber)
        .padding(.top, 8)
      Text("Cloud mode and consent stay on Dictation; endpoints and keys live here.")
        .font(CSFont.ui(11.5))
        .lineSpacing(2)
        .foregroundStyle(Color.secondary)
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
        .foregroundStyle(Color.primary)
      Text(lane.accepts)
        .font(CSFont.mono(10.5, .medium))
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)
      SettingsUrlRow(
        title: String(localized: "Endpoint"),
        keyLabel: lane.endpointWireKey,
        current: lane.endpoint ?? "",
        placeholder: lane.placeholder,
        help: String(
          localized:
            "Not a secret. Blank clears the lane; the bridge rejects a URL whose scheme does not fit this lane."
        ),
        onSave: { model.setSttLaneEndpoint(lane.id, $0) }
      )
      KeyRow(
        model: model, account: lane.keyAccount, label: String(localized: "API key"),
        isSet: lane.apiKeySet)
      if lane.id == "live" {
        SettingsUrlRow(
          title: String(localized: "Gateway session URL"),
          keyLabel: "CODESCRIBE_ASR_GATEWAY_URL",
          current: model.asrGatewayUrl,
          placeholder: "https://…/session",
          help: String(
            localized:
              "Session-mint endpoint for live Cloud Layer 1. Not the live socket above. Clearing restores unset."
          ),
          onSave: { model.setAsrGatewayUrl($0) }
        )
      }
    }
    .settingsGroupedInset()
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
      SettingsSectionLabel(String(localized: "Service keys"))
      Text("GitHub token. Speech-to-text endpoints and keys live in the section above.")
        .font(CSFont.ui(11.5))
        .lineSpacing(2)
        .foregroundStyle(Color.secondary)
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
  }
#endif
