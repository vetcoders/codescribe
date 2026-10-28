import Foundation
import SwiftUI

// Row-level building blocks for credential and URL editing. Secrets go to the
// Keychain via `setApiKey` and are NEVER read back across the FFI; presence
// renders from `apiKeySet` / `CsKeyStatus` booleans. Composed by
// `ProvidersPanel` (vendor + custom + cloud transcription lanes + service
// keys). A row says what the user needs and no more: label and state on one
// line, the editor behind `Change`, Keychain account names and wire keys only
// under a card's Advanced disclosure.

// MARK: - Shared chrome

/// Input chrome shared by every editable settings row (text and secure fields).
/// A field sits on the control rung, the chip's insets, so it is as tall as the
/// Save and Test actions beside it (Founder brief, round 16, 2026-10-18: lower
/// fields in the MCP form; one owner, so every field follows).
private struct SettingsInputChrome: ViewModifier {
  let isFocused: Bool
  /// A sentence to format or a display name reads in the UI face; keys,
  /// tokens and addresses keep the mono face (Founder smoke, 2026-10-26, A7).
  var prose = false

  func body(content: Content) -> some View {
    content
      .textFieldStyle(.plain)
      .font(prose ? CSFont.ui(13) : CSFont.mono(12, .regular))
      .foregroundStyle(Color.primary)
      .autocorrectionDisabled()
      .padding(.horizontal, SettingsScale.controlInsetH)
      .padding(.vertical, SettingsScale.controlInsetV)
      .background(
        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
          .fill(Color.primary.opacity(CSAlpha.fill))
      )
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
          .strokeBorder(Color.primary.opacity(CSAlpha.stroke), lineWidth: 1)
      )
      .overlay {
        CSFocusOutline(isFocused: isFocused, cornerRadius: CSRadius.input)
      }
  }
}

extension View {
  func settingsInputChrome(isFocused: Bool, prose: Bool = false) -> some View {
    modifier(SettingsInputChrome(isFocused: isFocused, prose: prose))
  }
}

/// Accent "Save": dimmed until there is something to save. On the control
/// rung, like the field it usually stands next to.
struct SettingsSaveButton: View {
  var title: LocalizedStringKey = "Save"
  let enabled: Bool
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Text(title)
        .font(SettingsScale.controlLabel)
        .foregroundStyle(enabled ? CSColor.chromeAccent : CSColor.secondaryInk)
        .padding(.horizontal, SettingsScale.controlInsetH)
        .padding(.vertical, SettingsScale.controlInsetV)
        .background(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .fill(CSColor.chromeAccent.opacity(enabled ? 0.14 : 0.06))
        )
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .strokeBorder(CSColor.chromeAccent.opacity(enabled ? 0.28 : 0.1), lineWidth: 1)
        )
    }
    .csFocusRing()
    .disabled(!enabled)
  }
}

/// Neutral chip button (Test / Remove / Sign out / Edit).
struct SettingsChipButton<Label: View>: View {
  var enabled: Bool = true
  let action: () -> Void
  let label: () -> Label

  var body: some View {
    Button(action: action) {
      label()
        .padding(.horizontal, SettingsScale.controlInsetH)
        .padding(.vertical, SettingsScale.controlInsetV)
        .background(
          RoundedRectangle(cornerRadius: SettingsScale.controlRadius, style: .continuous)
            .fill(Color.primary.opacity(CSAlpha.fill))
        )
        .overlay(
          RoundedRectangle(cornerRadius: SettingsScale.controlRadius, style: .continuous)
            .strokeBorder(Color.primary.opacity(CSAlpha.stroke), lineWidth: 1)
        )
    }
    .csFocusRing(cornerRadius: SettingsScale.controlRadius)
    .disabled(!enabled)
  }
}

// MARK: - Refresh button (the one Settings-wide standard)

/// The one refresh control in Settings (Founder brief, round 8, 2026-10-10):
/// a small secondary chip — refresh glyph, short "Refresh" label — with the
/// same font, padding, colours and states everywhere. `busy` swaps the glyph
/// for a spinner and disables the chip; a section with its own progress line
/// passes `enabled` instead. The accessibility label says what exactly is
/// refreshed, because the visible label deliberately does not.
struct SettingsRefreshButton: View {
  var title: LocalizedStringKey = "Refresh"
  var busy = false
  var enabled = true
  /// `LocalizedStringKey`, not `String`: these reach VoiceOver and must keep
  /// their catalog entries.
  let axLabel: LocalizedStringKey
  var axHint: LocalizedStringKey?
  let action: () -> Void

  var body: some View {
    SettingsChipButton(enabled: enabled && !busy, action: action) {
      HStack(spacing: 5) {
        if busy {
          ProgressView().controlSize(.small).scaleEffect(0.62).frame(width: 12, height: 12)
        } else {
          CSIconView(icon: .refresh, size: 10, weight: .semibold)
        }
        Text(title).font(SettingsScale.controlLabel)
      }
      .foregroundStyle(enabled && !busy ? CSColor.secondaryInk : Color.secondary.opacity(0.6))
      .frame(height: 14)
    }
    .accessibilityLabel(Text(axLabel))
    .accessibilityHint(axHint.map { Text($0) } ?? Text(verbatim: ""))
  }
}

extension SettingsChipButton where Label == Text {
  /// Text-only chip.
  init(
    _ title: LocalizedStringKey, tint: Color, enabled: Bool = true,
    action: @escaping () -> Void
  ) {
    self.init(enabled: enabled, action: action) {
      Text(title)
        .font(SettingsScale.controlLabel)
        .foregroundStyle(enabled ? tint : CSColor.secondaryInk)
    }
  }
}

// MARK: - URL row (non-secret)

/// Non-secret URL field: the cloud transcription lane endpoints
/// (`STT_FILE_ENDPOINT` / `STT_LIVE_ENDPOINT`) and the Cloud session-mint URL,
/// all on Providers › Cloud transcription. Provider endpoints are NOT edited
/// here — vendors are factory-pinned and custom hosts edit theirs in
/// `CustomProviderForm`. Title, field, Save: a rejected save puts its one
/// sentence under the field, and nothing warns about the validator ahead of it.
struct SettingsUrlRow: View {
  let title: String
  let current: String
  let placeholder: String
  /// One line under the field, e.g. "Optional. Used for live transcription."
  var caption: String?
  /// Persists the draft; returns the sentence to show when the save was rejected.
  let onSave: (String) -> String?

  @State private var draft: String = ""
  @State private var error: String?
  @FocusState private var isFocused: Bool
  @State private var loadedInitial = false

  var body: some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      Text(title)
        .font(SettingsScale.settingName)
        .foregroundStyle(Color.primary)

      HStack(spacing: CSSpace.sm) {
        TextField(placeholder, text: $draft)
          .settingsInputChrome(isFocused: isFocused)
          .focused($isFocused)
          .onSubmit(save)
          .onChange(of: draft) { _, _ in error = nil }
          .accessibilityLabel(title)
          .accessibilityHint(error ?? "")
        SettingsSaveButton(enabled: draft != current, action: save)
          .accessibilityLabel("Save \(title)")
      }

      if let error {
        Text(error)
          .font(CSFont.ui(11, .medium))
          .foregroundStyle(CSColor.dangerLight)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityIdentifier("settings-url-row-error")
      }
      if let caption {
        Text(caption)
          .font(SettingsScale.helper)
          .lineSpacing(2)
          .foregroundStyle(CSColor.secondaryInk)
      }
    }
    .onAppear {
      if !loadedInitial {
        draft = current
        loadedInitial = true
      }
    }
    .onChange(of: current) { _, newValue in
      draft = newValue
    }
  }

  private func save() {
    error = onSave(draft)
  }
}

// MARK: - Detail row (Advanced disclosure)

/// One read-only fact for a card's Advanced disclosure: a vendor's factory
/// endpoint, a Keychain account name, a lane's wire keys. Selectable, never edited.
struct SettingsDetailRow: View {
  let title: String
  let value: String

  var body: some View {
    HStack(spacing: 8) {
      Text(title)
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
      Text(value)
        .font(SettingsScale.metaKey)
        .foregroundStyle(Color.primary)
        .lineLimit(1)
        .truncationMode(.middle)
        .textSelection(.enabled)
      Spacer(minLength: 0)
    }
    .accessibilityElement(children: .combine)
  }
}

// MARK: - Key row (secret, write-only)

enum KeyRowPresentation {
  case standard
  case customProvider(endpoint: String)
}

/// One Keychain account on one line: presence dot, label, state (`Set` /
/// `Not set` / `Optional`) and a `Change` / `Add` chip that opens the editor —
/// paste-to-replace secure field, Save, Test, Clear. The account name is not
/// on the row; the card's Advanced disclosure shows it. `optional` marks a key
/// nothing is waiting for — a key-optional host, or a provider already covered
/// by a connected account — and only chooses the wording of the state.
struct KeyRow: View {
  let account: String
  let label: String
  let isSet: Bool
  var optional: Bool = false
  /// Inside a provider or lane card the row drops its own tinted card — the
  /// presence dot and state text carry the colour — so the host card stays one
  /// card, not a card in a card (Founder brief, round 7, 2026-10-10).
  var embedded: Bool = false
  /// Custom providers place their visible endpoint beside this row's existing
  /// presence summary. The editor and all mutations remain owned here.
  var presentation: KeyRowPresentation = .standard
  let probeResult: CsApiKeyProbeResult?
  let probePending: Bool
  var mutationPending = false
  let onSave: (String) async throws -> Void
  let onClear: () async throws -> Void
  let onTest: () -> Void

  @State private var draft: String = ""
  @State private var editing = false
  @State private var operationPending = false
  @State private var operationError: String?
  @FocusState private var isFocused: Bool

  private var isUpdating: Bool { mutationPending || operationPending }

  /// Stored credentials are presence, not service acceptance. Only the explicit
  /// probe chip carries a successful request's green verdict.
  private var accent: Color { CSColor.secondaryInk }

  /// A stored key is "saved, unverified" until a probe answers; only the
  /// probe's verdict may turn the glyph into a check, warning or error.
  private var statusTone: CsMcpRowTone { probeResult?.status.tone ?? .neutral }

  init(
    account: String, label: String, isSet: Bool, optional: Bool = false,
    embedded: Bool = false, presentation: KeyRowPresentation = .standard,
    probeResult: CsApiKeyProbeResult?, probePending: Bool,
    mutationPending: Bool = false, initiallyEditing: Bool = false,
    onSave: @escaping (String) async throws -> Void,
    onClear: @escaping () async throws -> Void, onTest: @escaping () -> Void
  ) {
    self.account = account
    self.label = label
    self.isSet = isSet
    self.optional = optional
    self.embedded = embedded
    self.presentation = presentation
    self.probeResult = probeResult
    self.probePending = probePending
    self.mutationPending = mutationPending
    self.onSave = onSave
    self.onClear = onClear
    self.onTest = onTest
    _editing = State(initialValue: initiallyEditing)
  }

  var body: some View {
    Group {
      switch presentation {
      case .standard:
        standardContent
      case .customProvider(let endpoint):
        customProviderContent(endpoint: endpoint)
      }
    }
    // Presence-tinted card: green when set, grey when not.
    // Embedded rows keep the tint on the glyph and state text only.
    .padding(.horizontal, embedded ? 0 : SettingsScale.cardPadding)
    .padding(.vertical, embedded ? 2 : SettingsScale.cardPadding)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(accent.opacity(embedded ? 0 : 0.06))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(accent.opacity(embedded ? 0 : 0.18), lineWidth: 1)
    )
  }

  private var standardContent: some View {
    VStack(alignment: .leading, spacing: CSSpace.sm) {
      standardSummary

      editingAndStatus
    }
  }

  private func customProviderContent(endpoint: String) -> some View {
    VStack(alignment: .leading, spacing: CSSpace.sm) {
      ViewThatFits(in: .horizontal) {
        HStack(alignment: .center, spacing: CSSpace.md) {
          customProviderEndpoint(endpoint)
            .frame(minWidth: 240, maxWidth: .infinity, alignment: .leading)
          compactSummary
            .fixedSize(horizontal: true, vertical: false)
        }

        VStack(alignment: .leading, spacing: CSSpace.xs) {
          customProviderEndpoint(endpoint)
          compactSummary
            .frame(maxWidth: .infinity, alignment: .trailing)
        }
      }

      editingAndStatus
    }
  }

  private var standardSummary: some View {
    HStack(spacing: CSSpace.sm) {
      StatusToneGlyph(tone: statusTone)
      Text(label)
        .font(SettingsScale.settingName)
        .foregroundStyle(Color.primary)
      Spacer(minLength: 0)
      if let probeResult {
        KeyProbeChip(result: probeResult)
      }
      presenceState
      editButton
    }
  }

  private var compactSummary: some View {
    HStack(spacing: CSSpace.sm) {
      StatusToneGlyph(tone: statusTone)
      Text(label)
        .font(SettingsScale.controlLabel)
        .foregroundStyle(Color.primary)
      if let probeResult {
        KeyProbeChip(result: probeResult)
      }
      presenceState
      editButton
    }
  }

  @ViewBuilder
  private var presenceState: some View {
    if isSet {
      CSStatusBadge(text: String(localized: "Saved"), mark: .saved)
    } else {
      Text(optional ? "Optional" : "Not set")
        .font(SettingsScale.statusBadge)
        .foregroundStyle(accent)
    }
  }

  private var editButton: some View {
    SettingsChipButton(
      isSet ? "Change" : "Add", tint: CSColor.secondaryInk, enabled: !isUpdating
    ) {
      editing.toggle()
      isFocused = editing
    }
    .accessibilityIdentifier("key-row-edit")
  }

  private func customProviderEndpoint(_ endpoint: String) -> some View {
    VStack(alignment: .leading, spacing: 3) {
      Text("API address")
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
      Text(verbatim: endpoint)
        .font(SettingsScale.metaKey)
        .foregroundStyle(Color.primary)
        .textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
    }
    .layoutPriority(1)
  }

  @ViewBuilder private var editingAndStatus: some View {
    if editing {
      HStack(spacing: CSSpace.sm) {
        SecureField(isSet ? "Replace key…" : "Paste key…", text: $draft)
          .settingsInputChrome(isFocused: isFocused)
          .focused($isFocused)
          .onSubmit(save)
          .accessibilityLabel("\(label) secret")

        SettingsSaveButton(
          enabled: !isUpdating && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
          action: save
        )
        .fixedSize(horizontal: true, vertical: false)
        .accessibilityLabel("Save \(label)")

        SettingsChipButton(enabled: isSet && !probePending && !isUpdating, action: onTest) {
          Group {
            if probePending {
              ProgressView().controlSize(.small).scaleEffect(0.62).frame(width: 20, height: 14)
            } else {
              Text("Test", comment: "Button label: run a connection test")
                .font(SettingsScale.controlLabel)
            }
          }
          .frame(minWidth: 26, minHeight: 18)
          .fixedSize(horizontal: true, vertical: false)
          .foregroundStyle(CSColor.secondaryInk)
        }
        .fixedSize(horizontal: true, vertical: false)
        .help(isSet ? "Test this key" : "Save a key first to test it")
        .accessibilityLabel("Test \(label)")

        SettingsChipButton(enabled: isSet && !isUpdating, action: clear) {
          CSIconView(
            icon: .delete, size: 12, weight: .semibold,
            color: isSet ? CSColor.dangerLight : CSColor.secondaryInk
          )
          .frame(width: 10, height: 18)
        }
        .help("Remove this key from the Keychain")
        .accessibilityLabel("Clear \(label)")
      }
    }
    if operationPending {
      HStack {
        ProgressView().controlSize(.small)
        Text("Updating provider access…").font(SettingsScale.helper)
      }
    }
    if let operationError {
      Text(operationError)
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.dangerLight)
        .textSelection(.enabled)
    }
  }

  private func save() {
    guard !isUpdating, !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
      return
    }
    let submitted = draft
    operationPending = true
    operationError = nil
    Task { @MainActor in
      defer { operationPending = false }
      do {
        try await onSave(submitted)
        if draft == submitted { draft = "" }
        editing = false
      } catch { operationError = error.userFacingMessage }
    }
  }

  private func clear() {
    guard !isUpdating else { return }
    operationPending = true
    operationError = nil
    Task { @MainActor in
      defer { operationPending = false }
      do {
        try await onClear()
        editing = false
      } catch { operationError = error.userFacingMessage }
    }
  }
}

extension KeyRow {
  /// Row wired to the view-model's save / clear / test for one account.
  init(
    model: SettingsViewModel, account: String, label: String, isSet: Bool,
    optional: Bool = false, embedded: Bool = false,
    presentation: KeyRowPresentation = .standard, initiallyEditing: Bool = false
  ) {
    self.init(
      account: account, label: label, isSet: isSet, optional: optional, embedded: embedded,
      presentation: presentation,
      probeResult: model.keyProbeResults[account],
      probePending: model.keyProbePending.contains(account),
      mutationPending: model.providerMutationPending,
      initiallyEditing: initiallyEditing,
      onSave: { try await model.saveKey(account: account, secret: $0) },
      onClear: { try await model.clearKey(account: account) },
      onTest: { model.testKey(account: account) })
  }
}

extension CsApiKeyProbeStatus {
  /// One verdict → tone map for the probe chip and the key row's glyph.
  var tone: CsMcpRowTone {
    switch self {
    case .ok: return .good
    case .invalid, .noQuota: return .bad
    case .network, .rejected: return .warn
    case .missing, .unsupported: return .neutral
    }
  }
}

struct KeyProbeChip: View {
  let result: CsApiKeyProbeResult

  private var label: String {
    let verdict: String
    switch result.status {
    case .ok: verdict = String(localized: "Key OK")
    case .invalid: verdict = String(localized: "Invalid key")
    case .noQuota: verdict = String(localized: "Usage limit (check billing and rate limits)")
    case .rejected: verdict = String(localized: "Request rejected; key not verified")
    case .network: verdict = String(localized: "Network error")
    case .missing: verdict = String(localized: "Not set")
    // "Unsupported" read as "bad key" — it only means this provider ships no
    // cheap liveness probe. The key itself is stored and used normally.
    case .unsupported: verdict = String(localized: "Saved — no test for this key")
    }
    guard let endpoint = result.probedEndpoint,
      let host = URL(string: endpoint)?.host,
      !host.isEmpty
    else { return verdict }
    return "\(verdict) @ \(host)"
  }

  private var tint: Color { result.status.tone.dotColor }

  var body: some View {
    Text(label)
      .font(SettingsScale.statusBadge)
      .lineLimit(1)
      .foregroundStyle(tint)
      .padding(.horizontal, 8)
      .padding(.vertical, 4)
      .background(Capsule().fill(tint.opacity(0.11)))
      .overlay(Capsule().strokeBorder(tint.opacity(0.24), lineWidth: 1))
      .help(
        result.probedEndpoint.map {
          String(
            localized: "\(result.message)\nEndpoint: \($0)",
            comment: "Tooltip: probe message, then the endpoint that was probed")
        } ?? result.message
      )
  }
}

// MARK: - Vendor account (OAuth) row

/// One line per vendor that ships an OAuth flow: "<brand> account", its state,
/// and ONE action — `Sign out` while connected, `Sign in with <brand>`
/// otherwise. Never both. The signed-in account wins over a stored API key on
/// the assistive lane (loader predicate `account_auth`), so the row says
/// which credential will actually be sent. The Rust one-liner stays as the
/// tooltip; the client-id override lives in the card's Advanced disclosure.
struct AccountLoginRow: View {
  let provider: CsProviderOption
  let loginPending: Bool
  let loginNotice: String?
  let onStart: () -> Void
  let onSignOut: () -> Void

  private var signedIn: Bool { provider.accountSignedIn }
  private var accountBrand: String { Self.brand(for: provider) }

  /// Short brand for the account row — OpenCode-style, not a client-id dump.
  static func brand(for provider: CsProviderOption) -> String {
    switch provider.id {
    case "openai-responses": return "ChatGPT"
    case "xai-responses": return "xAI"
    case "anthropic-messages": return "Claude"
    default: return provider.displayName
    }
  }

  /// A saved account is identity storage, not proof that the selected service
  /// currently accepts requests. Capability is measured by the lane's Test.
  static func status(for provider: CsProviderOption) -> String {
    guard provider.accountSignedIn else { return String(localized: "Not connected") }
    return String(localized: "Saved")
  }

  /// The signed-in identity, shown as a line under the row name rather than
  /// as part of the state word; nil when there is nothing to name.
  static func identityLine(for provider: CsProviderOption) -> String? {
    guard provider.accountSignedIn, let identity = provider.accountIdentity, !identity.isEmpty
    else { return nil }
    return String(
      localized: "Saved account: \(identity)",
      comment: "Provider account row; the placeholder is the signed-in email")
  }

  var body: some View {
    HStack(spacing: CSSpace.sm) {
      if !signedIn {
        StatusToneGlyph(tone: .neutral)
      }
      VStack(alignment: .leading, spacing: 3) {
        Text("\(accountBrand) account", comment: "The placeholder is a vendor brand, e.g. ChatGPT")
          .font(SettingsScale.settingName)
          .foregroundStyle(Color.primary)
        if let identityLine = Self.identityLine(for: provider) {
          Text(identityLine)
            .font(SettingsScale.helper)
            .foregroundStyle(CSColor.secondaryInk)
            .lineLimit(1)
            .truncationMode(.middle)
            .help(provider.accountIdentity ?? "")
        }
      }
      Group {
        if signedIn {
          CSStatusBadge(text: Self.status(for: provider), mark: .saved)
        } else {
          Text(Self.status(for: provider))
        }
      }
      .font(SettingsScale.statusBadge)
      .foregroundStyle(CSColor.secondaryInk)
      .lineLimit(1)
      .truncationMode(.middle)
      .help(
        signedIn
          ? String(
            localized: "Saved. Use Test in Formatting to check the selected provider and model.")
          : provider.accountStatusMessage)
      if let loginNotice, !loginNotice.isEmpty {
        Text(loginNotice)
          .font(SettingsScale.statusBadge)
          .foregroundStyle(CSColor.dangerLight)
          .lineLimit(1)
          .help(loginNotice)
      }
      Spacer(minLength: 0)
      // Sign out is a neutral chip like every other row action: it is an
      // ordinary choice, and destructive styling belongs to a confirmation.
      if signedIn {
        SettingsChipButton(
          "Sign out", tint: CSColor.secondaryInk, enabled: !loginPending, action: onSignOut
        )
        .help("Remove the stored \(accountBrand) account tokens")
        .accessibilityLabel("Sign out of \(accountBrand)")
      } else {
        SettingsChipButton(
          enabled: provider.accountLoginEnabled && !loginPending, action: onStart
        ) {
          HStack(spacing: CSSpace.xs) {
            if loginPending {
              ProgressView().controlSize(.small).scaleEffect(0.62).frame(width: 14, height: 12)
            } else {
              CSIconView(icon: .accountVerified, size: 12, weight: .semibold)
            }
            Text(
              loginPending
                ? (provider.id == "xai-responses" ? "Approve in browser…" : "Waiting for browser…")
                : "Sign in with \(accountBrand)"
            )
            .font(SettingsScale.controlLabel)
          }
          .foregroundStyle(
            provider.accountLoginEnabled && !loginPending
              ? CSColor.oliveLight : CSColor.secondaryInk
          )
        }
        .help(provider.accountStatusMessage)
        .accessibilityLabel("Sign in with \(accountBrand)")
      }
    }
  }
}

/// `OAuth client id…` for a vendor card's Advanced disclosure. The client id
/// is a non-secret public app identity; OpenAI + xAI ship defaults (NOTICE),
/// so the override opens in a popover instead of taking a row.
struct OAuthClientIdButton: View {
  let provider: CsProviderOption
  let onSave: (String) -> Void

  @State private var draft: String = ""
  @State private var editing = false

  private var accountBrand: String { AccountLoginRow.brand(for: provider) }

  var body: some View {
    Button("OAuth client id…") { editing = true }
      .buttonStyle(.plain)
      .font(SettingsScale.helper)
      .foregroundStyle(CSColor.secondaryInk)
      .csFocusRing()
      .popover(isPresented: $editing, arrowEdge: .bottom) {
        OAuthClientIdEditor(
          accountBrand: accountBrand,
          placeholder: provider.oauthClientId ?? String(localized: "Override OAuth client id…"),
          savedClientId: provider.oauthClientId ?? "",
          draft: $draft,
          onSave: {
            onSave(draft)
            editing = false
          }
        )
      }
      .onAppear { draft = provider.oauthClientId ?? "" }
      .onChange(of: provider.oauthClientId) { _, updated in
        draft = updated ?? ""
      }
  }
}

/// Absorbs a slider gesture so the persisted binding is written once, at the
/// end of the drag, instead of on every tick. Writes that arrive outside an
/// editing envelope (keyboard, accessibility) pass through unchanged.
struct SliderCommitGate {
  private(set) var draft: Double?
  private(set) var isEditing = false

  /// The value the control and its label should show right now.
  func displayed(persisted: Double) -> Double { draft ?? persisted }

  /// A new control value: held as draft while a gesture is in flight,
  /// returned for immediate persistence otherwise.
  mutating func valueChanged(_ newValue: Double) -> Double? {
    guard isEditing else { return newValue }
    draft = newValue
    return nil
  }

  /// The gesture envelope: returns the one value to persist when it closes.
  mutating func editingChanged(_ editing: Bool) -> Double? {
    isEditing = editing
    guard !editing, let committed = draft else { return nil }
    draft = nil
    return committed
  }
}

/// One labelled seconds slider: title, the one line that says what it changes,
/// and the live value. Shared by Agent voice timing and Lab recognition parameters.
/// A drag holds its ticks in `SliderCommitGate` and persists once on release —
/// every tick used to write settings.json and rebuild the Whisper catalog on
/// the main thread through each live SettingsViewModel.
struct SettingsSecondsSlider: View {
  let title: String
  let detail: String
  /// The persisted value, so the label shows what the core accepted.
  let seconds: Float
  @Binding var value: Double
  let range: ClosedRange<Double>
  let step: Double
  @State private var gate = SliderCommitGate()

  var body: some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      Text(title)
        .font(SettingsScale.settingName)
        .foregroundStyle(Color.primary)
      Text(detail)
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
        .fixedSize(horizontal: false, vertical: true)
      // The track stops at `sliderWidth`; the value reads at its end.
      HStack(spacing: CSSpace.md) {
        Slider(
          value: Binding(
            get: { gate.displayed(persisted: value) },
            set: { newValue in
              if let direct = gate.valueChanged(newValue) { value = direct }
            }
          ),
          in: range, step: step,
          onEditingChanged: { editing in
            if let committed = gate.editingChanged(editing) { value = committed }
          }
        )
        .tint(CSColor.chromeAccent)
        .frame(maxWidth: SettingsScale.sliderWidth)
        .accessibilityLabel(title)
        .accessibilityValue(valueLabel)
        Text(valueLabel)
          .font(SettingsScale.monoValue)
          .foregroundStyle(Color.primary)
      }
      .padding(.top, CSSpace.xs)
    }
  }

  /// Seconds without a trailing zero: `5 s`, `1.5 s` — never `5.0 s`.
  /// Mid-drag the label follows the draft, not the persisted value.
  private var valueLabel: String {
    String(
      localized: "\(Float(gate.displayed(persisted: Double(seconds))), format: Self.seconds) s",
      comment: "Slider value in seconds. The placeholder is the number; keep the unit abbreviated")
  }

  private static let seconds = FloatingPointFormatStyle<Float>.number
    .precision(.fractionLength(0...1))
}
