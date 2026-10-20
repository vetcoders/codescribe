import OSLog
import SwiftUI

// Settings › Providers. "URLs live where the API keys are; models live where
// the Agent is." Vendors (OpenAI, xAI, Anthropic) are factory-pinned: the
// endpoint is never edited and sits under the card's Advanced disclosure. A
// custom provider is any host speaking the Responses or Messages wire; its
// endpoint shows next to its (optional) key. Cloud transcription is two atomic
// lanes (File, Live), each a key row with its address under Advanced. Models
// are chosen per request lane under Agent › AI models.
//
// The first level shows what a user acts on — account, key — and nothing that
// explains the architecture: Keychain account names, wire keys, vendor and
// cloud-transcription endpoints and the OAuth client-id override live under
// Advanced. The three sections of the page are peers and share one header
// strength; inside a card, a provider's name is a setting name, never a rung
// above the section title that introduces it.

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
          .padding(.top, SettingsScale.sectionGap)
      }

      if let error = model.providerAccessError {
        Text(model.providerAccessResolved
          ? String(localized: "Provider access is unavailable. The last checked values are shown below.")
          : String(localized: "Provider access is unavailable. No credential status has been confirmed."))
          .font(SettingsScale.helper)
          .padding(.top, SettingsScale.sectionGap)
        Text(error).font(CSFont.mono(10.5)).textSelection(.enabled)
      }
      HStack(spacing: CSSpace.md) {
        // The status slot beside the chip carries the progress line, so the
        // chip greys out instead of spinning twice.
        SettingsRefreshButton(
          enabled: !(model.providerAccessPending || model.providerMutationPending),
          axLabel: "Refresh provider access status"
        ) { model.refreshProviderAccess() }
        ProviderAccessStatusSlot(
          accessPending: model.providerAccessPending,
          mutationPending: model.providerMutationPending,
          checkedAt: model.providerAccessCheckedAt)
      }
      .padding(.top, SettingsScale.sectionGap)
      if model.providerAccessResolved {
        VStack(spacing: CSSpace.sm) {
          ForEach(model.vendorProviders, id: \.id) { provider in
            ProviderCard(model: model, provider: provider)
          }
        }
        .padding(.top, SettingsScale.sectionGap)
        .disabled(model.providerMutationPending)

        CustomProvidersSection(
          model: model,
          onAdd: { formTarget = .add },
          onEdit: { formTarget = .edit($0) }
        )
        .padding(.top, SettingsScale.sectionGap)
        .disabled(model.providerMutationPending)

        SpeechToTextSection(model: model)
          .padding(.top, SettingsScale.sectionGap)
          .disabled(model.providerMutationPending)
          .id(SettingsAnchor.providersCloudTranscription)

        ServiceKeysSection(model: model)
          .padding(.top, SettingsScale.sectionGap)
          .disabled(model.providerMutationPending)

      }

      Text("Keys are stored securely in the macOS Keychain.")
        .font(SettingsScale.helper)
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.top, SettingsScale.sectionGap)
    }
    .settingsPage()
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
          .font(SettingsScale.helper)
          .lineLimit(1)
      }
      .opacity(busy ? 1 : 0)
      .accessibilityHidden(!busy)

      Text(receipt)
        .font(SettingsScale.helper)
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
    HStack(spacing: CSSpace.sm) {
      Circle().fill(CSColor.amber).frame(width: 7, height: 7)
      Text(text)
        .font(SettingsScale.helper)
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
  @State private var showingAdvanced = false

  init(
    model: SettingsViewModel, provider: CsProviderOption,
    onEdit: (() -> Void)? = nil, initiallyShowingAdvanced: Bool = false
  ) {
    self.model = model
    self.provider = provider
    self.onEdit = onEdit
    _showingAdvanced = State(initialValue: initiallyShowingAdvanced)
  }

  private var isCustom: Bool { provider.kind == "custom" }
  /// Vendors that ship an OAuth flow, or still hold tokens from one.
  private var hasAccount: Bool {
    !isCustom && (provider.accountLoginEnabled || provider.accountSignedIn)
  }
  /// "Responses" / "Messages" — the one thing every provider declares.
  private var wireLabel: String {
    ["responses": "Responses", "messages": "Messages"][provider.wire] ?? provider.wire
  }
  /// The badge is where the wire is named, so the card title must not name it
  /// twice. Core ships `OpenAI (Responses)` and `Anthropic (Messages)`
  /// (`core/llm/vendors/*.rs`, `DISPLAY_NAME`), and only that exact trailing
  /// parenthesis — the one whose content IS this provider's wire — is dropped.
  /// `xAI (Grok)` keeps its parenthesis: Grok is a product, not a protocol.
  private var cardTitle: String { Self.title(displayName: provider.displayName, wire: wireLabel) }

  static func title(displayName: String, wire: String) -> String {
    let suffix = " (\(wire))"
    guard displayName.hasSuffix(suffix), displayName.count > suffix.count else {
      return displayName
    }
    return String(displayName.dropLast(suffix.count))
  }

  var body: some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      if isCustom {
        customProviderHeader
        Divider().padding(.vertical, 2)
      } else {
        vendorProviderHeader
      }
      // Key-optional hosts, and vendors whose account is already connected,
      // read "Optional": nothing is waiting for that key.
      KeyRow(
        model: model, account: provider.apiKeyAccount, label: String(localized: "API key"),
        isSet: provider.apiKeySet,
        optional: !provider.keyRequired || provider.accountSignedIn, embedded: true,
        presentation: isCustom ? .customProvider(endpoint: provider.endpoint) : .standard)
      if let error = model.providerAccountErrors[provider.id] {
        Text("Account access unavailable")
          .font(SettingsScale.settingName)
        Text(error).font(SettingsScale.helper).textSelection(.enabled)
        SettingsChipButton("Sign out", tint: Color.secondary) {
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
      DisclosureGroup(isExpanded: $showingAdvanced) {
        VStack(alignment: .leading, spacing: CSSpace.xs) {
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
        .padding(.top, CSSpace.xs)
      } label: {
        Text("Advanced")
      }
      .font(SettingsScale.helper)
      .foregroundStyle(Color.secondary)
      .accessibilityIdentifier("provider-advanced")
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

  private var vendorProviderHeader: some View {
    HStack(spacing: CSSpace.sm) {
      Text(verbatim: cardTitle)
        .font(SettingsScale.settingName)
        .foregroundStyle(Color.primary)
      wireBadge
      Spacer(minLength: 0)
    }
  }

  private var customProviderHeader: some View {
    HStack(alignment: .top, spacing: CSSpace.sm) {
      Text(verbatim: cardTitle)
        .font(SettingsScale.settingName)
        .foregroundStyle(Color.primary)
        .fixedSize(horizontal: false, vertical: true)
        .layoutPriority(1)
      wireBadge
      Spacer(minLength: CSSpace.sm)
      HStack(spacing: CSSpace.xs) {
        SettingsChipButton("Edit", tint: Color.secondary) { onEdit?() }
          .accessibilityLabel("Edit custom provider \(provider.displayName)")
        SettingsChipButton(action: { confirmRemove = true }) {
          CSIconView(icon: .delete, size: 12, weight: .semibold, color: CSColor.terracotta)
            .frame(width: 10, height: 18)
        }
        .accessibilityLabel("Remove custom provider \(provider.displayName)")
      }
      .fixedSize(horizontal: true, vertical: false)
    }
  }

  private var wireBadge: some View {
    Text(verbatim: wireLabel)
      .font(CSFont.mono(10, .semibold))
      .foregroundStyle(Color.secondary)
      .padding(.horizontal, CSSpace.xs)
      .padding(.vertical, 2)
      .background(Capsule().fill(Color.primary.opacity(0.1)))
      .overlay(Capsule().strokeBorder(Color.primary.opacity(0.12), lineWidth: 1))
      .accessibilityLabel("\(wireLabel) wire")
      .fixedSize(horizontal: true, vertical: false)
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
      // A peer of "Cloud transcription" and "Service keys", not their parent:
      // one header strength across the page (Founder review, 2026-10-18).
      SettingsSectionHeaderRow(String(localized: "Custom providers")) {
        SettingsChipButton(action: onAdd) {
          Label("Add provider", systemImage: "plus")
            .font(SettingsScale.controlLabel)
            .foregroundStyle(Color.primary)
        }
        .accessibilityIdentifier("providers-add-custom")
      }

      if model.customProviders.isEmpty {
        Text("No custom providers yet.")
          .font(SettingsScale.helper)
          .foregroundStyle(Color.secondary)
          .padding(.top, SettingsScale.headerGap)
      } else {
        VStack(spacing: CSSpace.sm) {
          ForEach(model.customProviders, id: \.id) { provider in
            ProviderCard(model: model, provider: provider, onEdit: { onEdit(provider) })
          }
        }
        .padding(.top, SettingsScale.headerGap)
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
        .font(SettingsScale.helper)
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
            .font(CSFont.ui(11, .medium))
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
          .font(CSFont.ui(11, .medium))
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
        .font(CSFont.ui(10.5, .semibold))
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

/// Two lanes, File then Live, in `sttLanes()` order. Each lane is one atomic
/// card (ADR Tier 3): the key is never shown apart from the address it
/// authenticates, and the address sits one disclosure away.
struct SpeechToTextSection: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionHeaderRow(
        String(localized: "Cloud transcription", comment: "Providers section: cloud STT lanes"))
      // Two short sentences, approved verbatim in Polish (Founder review,
      // 2026-10-18): where the mode is chosen, and when audio actually leaves.
      Text("You choose the mode under Dictation.")
        .font(SettingsScale.helper)
        .foregroundStyle(Color.secondary)
        .padding(.top, SettingsScale.headerGap)
      // Conditional on purpose: a configured endpoint and key do not mean audio
      // is being sent — the real egress paths are Cloud mode and an explicit
      // re-transcription (same truth as Dictation › Cloud & privacy).
      Text("We send recordings only in Cloud mode or during a manual cloud re-transcription.")
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.amber)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.top, 2)
      VStack(spacing: CSSpace.sm) {
        ForEach(model.sttLanes, id: \.id) { lane in
          SttLaneCard(model: model, lane: lane)
        }
      }
      .padding(.top, SettingsScale.headerGap)
    }
  }
}

/// One lane card: title and key row (with Test behind `Change`) at the first
/// level; the endpoint, the Live lane's session-mint URL, what the lane accepts
/// on the wire and its settings keys all sit under Advanced (Founder decision,
/// 2026-10-18 — an address is not an everyday choice). The URLs still live on
/// the card that owns the key they authenticate; the wire line only on a
/// developer build.
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
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      Text(Self.title(for: lane))
        .font(SettingsScale.settingName)
        .foregroundStyle(Color.primary)
      // Cloud transcription is an optional lane: an absent key is neutral, not an error.
      KeyRow(
        model: model, account: lane.keyAccount, label: String(localized: "API key"),
        isSet: lane.apiKeySet, optional: true, embedded: true)
      DisclosureGroup("Advanced") {
        VStack(alignment: .leading, spacing: CSSpace.xs) {
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
        .padding(.top, CSSpace.xs)
      }
      .font(SettingsScale.helper)
      .foregroundStyle(Color.secondary)
      .accessibilityIdentifier("stt-lane-advanced")
    }
    .settingsGroupedInset()
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
      SettingsSectionHeaderRow(String(localized: "Service keys"))
      VStack(spacing: CSSpace.sm) {
        ForEach(model.serviceKeyAccounts, id: \.self) { account in
          // Service keys (GitHub, …) are optional extras: an absent key is
          // neutral, not an error.
          KeyRow(
            model: model, account: account, label: SettingsViewModel.keyLabel(for: account),
            isSet: model.keyStatus.isSet(account: account), optional: true)
        }
      }
      .padding(.top, SettingsScale.headerGap)
    }
  }
}

#if DEBUG
  #Preview("Providers panel") {
    ScrollView { ProvidersPanel(model: .preview(.keys)) }
      .frame(width: 720, height: 900)
  }
#endif
