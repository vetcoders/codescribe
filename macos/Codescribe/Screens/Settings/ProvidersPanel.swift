import OSLog
import SwiftUI

// Settings › Providers. "URLs live where the API keys are; models live where
// the Agent is." Vendors (OpenAI, xAI, Anthropic) are factory-pinned: the
// endpoint is never edited and sits under the card's Advanced disclosure. A
// custom provider is any host speaking the Responses or Messages wire; its
// endpoint shows next to its (optional) key. Cloud transcription is two atomic
// lanes (File, Live), each an endpoint + key row. Models are chosen per
// request lane under Agent › LLM lanes.
//
// The first level shows what a user acts on — account, key, address — and
// nothing that explains the architecture: Keychain account names, wire keys,
// vendor endpoints and the OAuth client-id override live under Advanced.

struct ProvidersPanel: View {
  static let ownedCapabilities: Set<SettingsPanelCapability> = [.providers]

  @ObservedObject var model: SettingsViewModel
  @State private var formTarget: CustomProviderFormTarget?

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsPageHeader(
        String(localized: "Providers"),
        blurb: String(
          localized: "Connect accounts or add API keys. Models are chosen under Agent › AI models.",
          comment: "Providers panel blurb; `AI models` is the Agent tab title")
      )

      if let notice = model.laneResetNotice {
        LaneResetNotice(text: notice)
          .padding(.top, 12)
      }

      if let error = model.providerAccessError {
        Text(model.providerAccessResolved
          ? String(localized: "Provider access is unavailable. The last checked values are shown below.")
          : String(localized: "Provider access is unavailable. No credential status has been confirmed."))
          .font(CSFont.ui(11.5))
          .padding(.top, 12)
        Text(error).font(CSFont.mono(10.5)).textSelection(.enabled)
      }
      HStack(spacing: CSSpace.md) {
        Button("Refresh status") { model.refreshProviderAccess() }
          .disabled(model.providerAccessPending || model.providerMutationPending)
        ProviderAccessStatusSlot(
          accessPending: model.providerAccessPending,
          mutationPending: model.providerMutationPending,
          checkedAt: model.providerAccessCheckedAt)
      }
      .padding(.top, 12)
      if model.providerAccessResolved {
        VStack(spacing: 8) {
          ForEach(model.vendorProviders, id: \.id) { provider in
            ProviderCard(model: model, provider: provider)
          }
        }
        .padding(.top, CSSpace.section)
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
          .disabled(model.providerMutationPending)
          .id(SettingsAnchor.providersCloudTranscription)

        ServiceKeysSection(model: model)
          .padding(.top, CSSpace.section)
          .disabled(model.providerMutationPending)

      }

      Text("Keys are stored securely in the macOS Keychain.")
        .font(CSFont.ui(11.5))
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.top, CSSpace.section)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
    .sheet(item: $formTarget) { target in
      CustomProviderForm(model: model, target: target)
    }
  }
}

/// Refresh feedback that reserves its line. The spinner and its label fade in
/// and out next to `Refresh status` instead of being inserted above it, so a
/// refresh never reflows the cards below (the one-frame tear seen on build 1986
/// came from that insertion). Width and height are the same in every state.
struct ProviderAccessStatusSlot: View {
  let accessPending: Bool
  let mutationPending: Bool
  /// Set once a snapshot has landed; the slot then reads "Checked at HH:MM:SS"
  /// so a refresh that finishes in a blink still leaves a visible receipt.
  let checkedAt: Date?

  var busy: Bool { accessPending || mutationPending }
  var showsReceipt: Bool { !busy && checkedAt != nil }

  var body: some View {
    // Both layers stay mounted and only trade opacity: swapping views here
    // would crossfade them, which is the two-layer tear this slot exists to avoid.
    ZStack(alignment: .leading) {
      HStack(spacing: CSSpace.sm) {
        ProgressView()
          .controlSize(.small)
        Text(mutationPending
          ? String(localized: "Updating provider access…")
          : String(localized: "Checking provider access…"))
          .font(CSFont.ui(11.5))
          .lineLimit(1)
      }
      .opacity(busy ? 1 : 0)
      .accessibilityHidden(!busy)

      Text(receipt)
        .font(CSFont.ui(11.5))
        .foregroundStyle(Color.secondary)
        .lineLimit(1)
        .opacity(showsReceipt ? 1 : 0)
        .accessibilityHidden(!showsReceipt)
    }
    .frame(height: ProviderAccessStatusSlot.height, alignment: .leading)
    .animation(nil, value: busy)
    .animation(nil, value: checkedAt)
  }

  private var receipt: String {
    guard let checkedAt else { return "" }
    let time = checkedAt.formatted(date: .omitted, time: .standard)
    return String(localized: "Checked at \(time)")
  }

  /// One fixed line: tall enough for the small spinner on every macOS build.
  static let height: CGFloat = 20
}

/// Providers section header. The sections on this page put one-line helper
/// copy straight under the heading, so the heading reads one step stronger
/// than the shared `SettingsSectionLabel` — a local fix, not a global header
/// restyle (Founder brief, round 7, 2026-10-10).
struct ProvidersSectionHeader<Action: View>: View {
  let text: String
  @ViewBuilder var action: () -> Action

  init(_ text: String, @ViewBuilder action: @escaping () -> Action) {
    self.text = text
    self.action = action
  }

  var body: some View {
    HStack(alignment: .firstTextBaseline, spacing: 12) {
      Text(text)
        .font(CSFont.ui(13, .semibold))
        .foregroundStyle(Color.primary)
        .accessibilityAddTraits(.isHeader)
        .frame(maxWidth: .infinity, alignment: .leading)
      action()
        .controlSize(.small)
    }
  }
}

extension ProvidersSectionHeader where Action == EmptyView {
  init(_ text: String) {
    self.init(text, action: { EmptyView() })
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

/// One registry row: name, wire, key row; a vendor adds its OAuth account row,
/// a custom row shows its endpoint and adds Edit / Remove — the endpoint moves
/// only through `CustomProviderForm`. Everything else (a vendor's factory
/// endpoint, the Keychain account name, the OAuth client-id override) sits
/// under Advanced. There is no endpoint setter on the view-model for a vendor
/// — by design.
struct ProviderCard: View {
  @ObservedObject var model: SettingsViewModel
  let provider: CsProviderOption
  var onEdit: (() -> Void)?

  @State private var confirmRemove = false

  private var isCustom: Bool { provider.kind == "custom" }
  /// Vendors that ship an OAuth flow, or still hold tokens from one.
  private var hasAccount: Bool {
    !isCustom && (provider.accountLoginEnabled || provider.accountSignedIn)
  }
  /// "Responses" / "Messages" — the one thing every provider declares.
  private var wireLabel: String {
    ["responses": "Responses", "messages": "Messages"][provider.wire] ?? provider.wire
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 7) {
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
      if isCustom {
        endpointRow
      }
      // Custom hosts are key-optional: an absent key there is neutral, not an error.
      KeyRow(
        model: model, account: provider.apiKeyAccount, label: String(localized: "API key"),
        isSet: provider.apiKeySet, optional: !provider.keyRequired, embedded: true)
      if let error = model.providerAccountErrors[provider.id] {
        Text("Account access unavailable")
          .font(CSFont.ui(12, .semibold))
        Text(error).font(CSFont.ui(11.5)).textSelection(.enabled)
        SettingsChipButton("Sign out", tint: CSColor.terracotta) {
          model.signOutAccount(providerId: provider.id)
        }
        .accessibilityLabel("Sign out \(provider.displayName)")
      } else if hasAccount {
        AccountLoginRow(
          provider: provider,
          loginPending: model.accountLoginPending.contains(provider.id),
          loginNotice: model.accountLoginNotices[provider.id],
          onStart: { model.startAccountLogin(providerId: provider.id) },
          onSignOut: { model.signOutAccount(providerId: provider.id) }
        )
      }
      DisclosureGroup("Advanced") {
        VStack(alignment: .leading, spacing: 8) {
          if !isCustom {
            endpointRow
          }
          SettingsDetailRow(
            title: String(localized: "Keychain account"), value: provider.apiKeyAccount)
          if hasAccount {
            OAuthClientIdButton(provider: provider) {
              model.saveOauthClientId(providerId: provider.id, value: $0)
            }
          }
        }
        .padding(.top, 6)
      }
      .font(CSFont.ui(11.5))
      .foregroundStyle(Color.secondary)
      .accessibilityIdentifier("provider-advanced")
    }
    .settingsGroupedInset(padding: 12)
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

  private var endpointRow: some View {
    SettingsDetailRow(title: String(localized: "Endpoint"), value: provider.endpoint)
      .accessibilityLabel("endpoint \(provider.endpoint)")
  }
}

// MARK: - Custom providers

struct CustomProvidersSection: View {
  @ObservedObject var model: SettingsViewModel
  let onAdd: () -> Void
  let onEdit: (CsProviderOption) -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      // The heading, the wire pills on the cards and the add sheet already say
      // what belongs here; the old explainer sentence is gone (Founder brief,
      // round 7, 2026-10-10).
      ProvidersSectionHeader(String(localized: "Custom providers")) {
        Button(action: onAdd) {
          Label("Add provider", systemImage: "plus")
            .font(CSFont.ui(12, .semibold))
        }
        .csFocusRing()
        .foregroundStyle(Color.primary)
        .accessibilityIdentifier("providers-add-custom")
      }

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

  /// The example host is a proper name, but the abbreviation in front of it is
  /// copy, so the placeholder is one localized string (PL-041).
  private static var namePlaceholder: String {
    String(
      localized: "e.g. Libraxis",
      comment: "Name field placeholder; Libraxis is a company name used as the example")
  }
  /// A URL, not copy: it reads the same in every language.
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

// MARK: - Cloud transcription (stt-lanes-v1 §D)

/// Two lanes, File then Live, in `sttLanes()` order. Each lane is one atomic endpoint +
/// key row (ADR Tier 3): the key is never shown apart from the address it authenticates.
struct SpeechToTextSection: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      ProvidersSectionHeader(
        String(localized: "Cloud transcription", comment: "Providers section: cloud STT lanes"))
      Text("Connections for cloud transcription. The mode is chosen under Dictation.")
        .font(CSFont.ui(11.5))
        .lineSpacing(2)
        .foregroundStyle(Color.secondary)
        .padding(.top, 6)
      // Conditional on purpose: a configured endpoint and key do not mean audio
      // is being sent — the real egress paths are Cloud mode and an explicit
      // re-transcription (same truth as Dictation › Cloud & privacy).
      Text(
        "Recordings leave this computer only in Cloud mode, or when you start a cloud re-transcription yourself."
      )
      .font(CSFont.ui(11.5, .medium))
      .foregroundStyle(CSColor.amber)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, 2)
      VStack(spacing: 8) {
        ForEach(model.sttLanes, id: \.id) { lane in
          SttLaneCard(model: model, lane: lane)
        }
      }
      .padding(.top, 12)
    }
  }
}

/// One lane card: title, endpoint row, key row (with Test behind `Change`).
/// The Live card also hosts the session-mint URL, moved here from Dictation
/// because URLs live where the keys are. What the lane accepts on the wire and
/// its settings keys sit under Advanced; the wire line only on a developer build.
struct SttLaneCard: View {
  @ObservedObject var model: SettingsViewModel
  let lane: CsSttLane

  /// The bridge titles lanes in English for the CLI; the panel names them by id.
  static func title(for lane: CsSttLane) -> String {
    switch lane.id {
    case "file":
      return String(localized: "File transcription", comment: "Cloud transcription lane: recorded files")
    case "live":
      return String(localized: "Live transcription", comment: "Cloud transcription lane: live socket")
    default:
      return lane.title
    }
  }

  /// What the lane accepts on the wire. The bridge sends the arguments only —
  /// API paths, encodings and protocol ids — and the sentence around them is
  /// written here, so a translation can reach it (PL-038).
  static func accepts(for lane: CsSttLane) -> String {
    switch lane.id {
    case "file":
      return String(
        localized: "HTTP(S): \(lane.accepts)",
        comment:
          "Cloud transcription transport; plain HTTP is allowed on loopback only; the placeholder lists API paths and encodings"
      )
    case "live":
      return String(
        localized: "Live WebSocket connection (ws(s); \(lane.accepts))",
        comment:
          "Cloud transcription transport; plain ws is allowed on loopback only; the placeholder lists protocol ids"
      )
    default:
      return lane.accepts
    }
  }

  /// One sentence under the field when a save was rejected. The bridge names
  /// the transport the lane requires; anything else is shown as it came.
  static func saveMessage(for error: Error) -> String {
    let message = error.userFacingMessage
    if message.contains("endpoint requires http(s)") {
      return String(
        localized: "This address needs http:// or https://.",
        comment: "File transcription endpoint rejected for its scheme")
    }
    if message.contains("endpoint requires ws(s)") {
      return String(
        localized: "This address needs ws:// or wss://.",
        comment: "Live transcription endpoint rejected for its scheme")
    }
    return message
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 7) {
      Text(Self.title(for: lane))
        .font(CSFont.ui(14.5, .bold))
        .foregroundStyle(Color.primary)
      SettingsUrlRow(
        title: String(localized: "Endpoint"),
        current: lane.endpoint ?? "",
        placeholder: lane.placeholder,
        onSave: { value in
          do {
            try model.setSttLaneEndpoint(lane.id, value)
            return nil
          } catch {
            return Self.saveMessage(for: error)
          }
        }
      )
      KeyRow(
        model: model, account: lane.keyAccount, label: String(localized: "API key"),
        isSet: lane.apiKeySet, embedded: true)
      if lane.id == "live" {
        SettingsUrlRow(
          title: String(localized: "Gateway session URL"),
          current: model.asrGatewayUrl,
          placeholder: "https://…/session",
          caption: String(localized: "Optional. Used for live transcription."),
          onSave: { value in
            do {
              try model.setAsrGatewayUrl(value)
              return nil
            } catch {
              return error.userFacingMessage
            }
          }
        )
      }
      DisclosureGroup("Advanced") {
        VStack(alignment: .leading, spacing: 8) {
          if DeveloperSurface.isEnabled() {
            Text(Self.accepts(for: lane))
              .font(CSFont.mono(10.5, .medium))
              .fixedSize(horizontal: false, vertical: true)
          }
          SettingsDetailRow(
            title: String(localized: "Settings keys"),
            value: ([lane.endpointWireKey, lane.keyAccount]
              + (lane.id == "live" ? ["CODESCRIBE_ASR_GATEWAY_URL"] : []))
              .joined(separator: " · "))
        }
        .padding(.top, 6)
      }
      .font(CSFont.ui(11.5))
      .foregroundStyle(Color.secondary)
      .accessibilityIdentifier("stt-lane-advanced")
    }
    .settingsGroupedInset(padding: 12)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("\(Self.title(for: lane)) lane")
  }
}

// MARK: - Service keys

/// Keys that are not LLM providers and not cloud transcription: GitHub. Only
/// the secret is edited here; the row label says what the key is for.
struct ServiceKeysSection: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      ProvidersSectionHeader(String(localized: "Service keys"))
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
