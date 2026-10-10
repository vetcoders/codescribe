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
private struct SettingsInputChrome: ViewModifier {
  let isFocused: Bool

  func body(content: Content) -> some View {
    content
      .textFieldStyle(.plain)
      .font(CSFont.mono(12, .regular))
      .foregroundStyle(Color.primary)
      .autocorrectionDisabled()
      .padding(.horizontal, 11)
      .padding(.vertical, 8)
      .background(
        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
          .fill(Color.primary.opacity(0.06))
      )
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
      )
      .overlay {
        CSFocusOutline(isFocused: isFocused, cornerRadius: CSRadius.input)
      }
  }
}

extension View {
  func settingsInputChrome(isFocused: Bool) -> some View {
    modifier(SettingsInputChrome(isFocused: isFocused))
  }
}

/// Accent "Save": dimmed until there is something to save.
struct SettingsSaveButton: View {
  var title: LocalizedStringKey = "Save"
  let enabled: Bool
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Text(title)
        .font(CSFont.ui(12, .semibold))
        .foregroundStyle(enabled ? CSColor.chromeAccent : Color.secondary)
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
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
        .padding(.horizontal, 11)
        .padding(.vertical, 7)
        .background(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .fill(Color.primary.opacity(0.06))
        )
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
        )
    }
    .csFocusRing()
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
        Text(title).font(CSFont.ui(11.5, .semibold))
      }
      .foregroundStyle(enabled && !busy ? Color.secondary : Color.secondary.opacity(0.6))
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
        .font(CSFont.ui(11.5, .semibold))
        .foregroundStyle(enabled ? tint : Color.secondary)
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
    VStack(alignment: .leading, spacing: 8) {
      Text(title)
        .font(CSFont.ui(13.5, .semibold))
        .foregroundStyle(Color.primary)

      HStack(spacing: 8) {
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
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.terracotta)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityIdentifier("settings-url-row-error")
      }
      if let caption {
        Text(caption)
          .font(CSFont.ui(11.5))
          .lineSpacing(2)
          .foregroundStyle(Color.secondary)
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
        .font(CSFont.ui(11.5))
        .foregroundStyle(Color.secondary)
      Text(value)
        .font(CSFont.mono(11, .medium))
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

/// One Keychain account on one line: presence dot, label, state (`Set` /
/// `Not set` / `Optional`) and a `Change` / `Add` chip that opens the editor —
/// paste-to-replace secure field, Save, Test, Clear. The account name is not
/// on the row; the card's Advanced disclosure shows it. `optional` marks
/// key-optional hosts (custom providers): an absent key is neutral there, not red.
struct KeyRow: View {
  let account: String
  let label: String
  let isSet: Bool
  var optional: Bool = false
  /// Inside a provider or lane card the row drops its own tinted card — the
  /// presence dot and state text carry the colour — so the host card stays one
  /// card, not a card in a card (Founder brief, round 7, 2026-10-10).
  var embedded: Bool = false
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

  private var accent: Color {
    isSet ? CSColor.olive : (optional ? Color.secondary : CSColor.terracotta)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack(spacing: 10) {
        Circle().fill(accent.opacity(0.85)).frame(width: 7, height: 7)
        Text(label)
          .font(CSFont.ui(13.5, .semibold))
          .foregroundStyle(Color.primary)
        Spacer(minLength: 0)
        if let probeResult {
          KeyProbeChip(result: probeResult)
        }
        Text(isSet ? "Set" : (optional ? "Optional" : "Not set"))
          .font(CSFont.mono(10, .semibold))
          .foregroundStyle(isSet ? CSColor.oliveLight : accent)
        SettingsChipButton(
          isSet ? "Change" : "Add", tint: Color.secondary, enabled: !isUpdating
        ) {
          editing.toggle()
          isFocused = editing
        }
        .accessibilityIdentifier("key-row-edit")
      }

      if editing {
        HStack(spacing: 8) {
          SecureField(isSet ? "Replace key…" : "Paste key…", text: $draft)
            .settingsInputChrome(isFocused: isFocused)
            .focused($isFocused)
            .onSubmit(save)
            .accessibilityLabel("\(label) secret")

          SettingsSaveButton(
            enabled: !isUpdating && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            action: save
          )
          .accessibilityLabel("Save \(label)")

          SettingsChipButton(enabled: isSet && !probePending && !isUpdating, action: onTest) {
            Group {
              if probePending {
                ProgressView().controlSize(.small).scaleEffect(0.62).frame(width: 20, height: 14)
              } else {
                Text("Test", comment: "Button label: run a connection test")
                  .font(CSFont.ui(12, .semibold))
              }
            }
            .frame(width: 26, height: 18)
            .foregroundStyle(Color.secondary)
          }
          .help(isSet ? "Test this key" : "Save a key first to test it")
          .accessibilityLabel("Test \(label)")

          SettingsChipButton(enabled: isSet && !isUpdating, action: clear) {
            CSIconView(
              icon: .delete, size: 12, weight: .semibold,
              color: isSet ? CSColor.terracotta : Color.secondary
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
          Text("Updating provider access…").font(CSFont.ui(11.5))
        }
      }
      if let operationError {
        Text(operationError)
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.danger)
          .textSelection(.enabled)
      }
    }
    // Presence-tinted card: green when set, red (required) / grey (optional)
    // when not. Embedded rows keep the tint on the dot and state text only.
    .padding(.horizontal, embedded ? 0 : 15)
    .padding(.vertical, embedded ? 2 : 13)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(accent.opacity(embedded ? 0 : 0.06))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(accent.opacity(embedded ? 0 : 0.18), lineWidth: 1)
    )
  }

  private func save() {
    guard !isUpdating, !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
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
    optional: Bool = false, embedded: Bool = false
  ) {
    self.init(
      account: account, label: label, isSet: isSet, optional: optional, embedded: embedded,
      probeResult: model.keyProbeResults[account],
      probePending: model.keyProbePending.contains(account),
      mutationPending: model.providerMutationPending,
      onSave: { try await model.saveKey(account: account, secret: $0) },
      onClear: { try await model.clearKey(account: account) },
      onTest: { model.testKey(account: account) })
  }
}

struct KeyProbeChip: View {
  let result: CsApiKeyProbeResult

  private var label: String {
    let verdict: String
    switch result.status {
    case .ok: verdict = String(localized: "Key OK")
    case .invalid: verdict = String(localized: "Invalid key")
    case .noQuota: verdict = String(localized: "No credits (check billing)")
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

  private var tint: Color {
    switch result.status {
    case .ok: return CSColor.oliveLight
    case .invalid, .noQuota: return CSColor.terracotta
    case .network: return CSColor.amber
    case .missing, .unsupported: return Color.secondary
    }
  }

  var body: some View {
    Text(label)
      .font(CSFont.mono(10, .semibold))
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
  private var accent: Color { signedIn ? CSColor.olive : Color.secondary }
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

  /// `Connected as <email>` when the id token names the account, `Connected`
  /// when it does not, `Not connected` otherwise.
  static func status(for provider: CsProviderOption) -> String {
    guard provider.accountSignedIn else { return String(localized: "Not connected") }
    if let identity = provider.accountIdentity, !identity.isEmpty {
      return String(
        localized: "Connected as \(identity)",
        comment: "Provider account row; the placeholder is the signed-in email")
    }
    return String(localized: "Connected")
  }

  var body: some View {
    HStack(spacing: 10) {
      Circle().fill(accent.opacity(0.85)).frame(width: 7, height: 7)
      Text("\(accountBrand) account", comment: "The placeholder is a vendor brand, e.g. ChatGPT")
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(Color.primary)
      Text(Self.status(for: provider))
        .font(CSFont.mono(10, .semibold))
        .foregroundStyle(accent)
        .lineLimit(1)
        .truncationMode(.middle)
        .help(provider.accountStatusMessage)
      if let loginNotice, !loginNotice.isEmpty {
        Text(loginNotice)
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.terracotta)
          .lineLimit(1)
          .help(loginNotice)
      }
      Spacer(minLength: 0)
      if signedIn {
        SettingsChipButton(
          "Sign out", tint: CSColor.terracotta, enabled: !loginPending, action: onSignOut
        )
        .help("Remove the stored \(accountBrand) account tokens")
        .accessibilityLabel("Sign out of \(accountBrand)")
      } else {
        SettingsChipButton(
          enabled: provider.accountLoginEnabled && !loginPending, action: onStart
        ) {
          HStack(spacing: 6) {
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
            .font(CSFont.ui(12, .semibold))
          }
          .foregroundStyle(
            provider.accountLoginEnabled && !loginPending ? CSColor.oliveLight : Color.secondary
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
      .font(CSFont.mono(10, .medium))
      .foregroundStyle(Color.secondary)
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
