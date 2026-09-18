import Foundation
import SwiftUI

// Row-level building blocks for credential and URL editing. Secrets go to the
// Keychain via `setApiKey` and are NEVER read back across the FFI; presence
// renders from `apiKeySet` / `CsKeyStatus` booleans. Composed by
// `ProvidersPanel` (vendor + custom + speech-to-text lanes + service keys).

// MARK: - Shared chrome

/// Input chrome shared by every editable settings row (text and secure fields).
private struct SettingsInputChrome: ViewModifier {
  func body(content: Content) -> some View {
    content
      .textFieldStyle(.plain)
      .font(CSFont.mono(12, .regular))
      .foregroundStyle(CSColor.textBody)
      .autocorrectionDisabled()
      .padding(.horizontal, 11)
      .padding(.vertical, 8)
      .background(
        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
          .fill(CSColor.surfaceRaised(0.03))
      )
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
          .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
      )
  }
}

extension View {
  func settingsInputChrome() -> some View { modifier(SettingsInputChrome()) }
}

/// Accent "Save": dimmed until there is something to save.
struct SettingsSaveButton: View {
  var title: String = "Save"
  let enabled: Bool
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Text(title)
        .font(CSFont.ui(12, .semibold))
        .foregroundStyle(enabled ? CSColor.chromeAccent : CSColor.textFaint)
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

/// Neutral chip button (Test / Remove / Sign out / Edit): surface fill, hairline.
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
            .fill(CSColor.surfaceRaised(0.03))
        )
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
        )
    }
    .csFocusRing()
    .disabled(!enabled)
  }
}

extension SettingsChipButton where Label == Text {
  /// Text-only chip.
  init(_ title: String, tint: Color, enabled: Bool = true, action: @escaping () -> Void) {
    self.init(enabled: enabled, action: action) {
      Text(title)
        .font(CSFont.ui(11.5, .semibold))
        .foregroundStyle(enabled ? tint : CSColor.textFaint)
    }
  }
}

// MARK: - URL row (non-secret)

/// Non-secret URL field: the speech-to-text lane endpoints
/// (`STT_FILE_ENDPOINT` / `STT_LIVE_ENDPOINT`) and the Cloud session-mint URL,
/// all on Providers › Speech-to-text. Provider endpoints are NOT edited here —
/// vendors are factory-pinned and custom hosts edit theirs in `CustomProviderForm`.
struct SettingsUrlRow: View {
  let title: String
  let keyLabel: String
  let current: String
  let placeholder: String
  let help: String
  var unsetLabel: String = "unset"
  let onSave: (String) -> Void

  @State private var draft: String = ""
  @State private var loadedInitial = false

  private var isSet: Bool { !current.isEmpty }
  private var accent: Color { isSet ? CSColor.olive : CSColor.textFaint }

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack(spacing: 10) {
        Circle().fill(accent.opacity(0.85)).frame(width: 7, height: 7)
        Text(title)
          .font(CSFont.ui(13.5, .semibold))
          .foregroundStyle(CSColor.textBody)
        Text(keyLabel)
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textFaint)
        Spacer(minLength: 0)
        Text(isSet ? "set" : unsetLabel)
          .font(CSFont.mono(10, .semibold))
          .foregroundStyle(isSet ? CSColor.oliveLight : CSColor.textFaint)
      }

      HStack(spacing: 8) {
        TextField(placeholder, text: $draft)
          .settingsInputChrome()
          .onSubmit { onSave(draft) }
          .accessibilityLabel(title)
        SettingsSaveButton(enabled: draft != current) { onSave(draft) }
          .accessibilityLabel("Save \(title)")
      }

      Text(help)
        .font(CSFont.ui(11.5))
        .lineSpacing(2)
        .foregroundStyle(CSColor.textMutedAlt)
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
}

// MARK: - Key row (secret, write-only)

/// One Keychain account: presence dot, paste-to-replace secure field, Test,
/// Clear. `optional` marks key-optional hosts (custom providers): an absent key
/// is a neutral state there, not a red one.
struct KeyRow: View {
  let account: String
  let label: String
  let isSet: Bool
  var optional: Bool = false
  let probeResult: CsApiKeyProbeResult?
  let probePending: Bool
  let onSave: (String) -> Void
  let onClear: () -> Void
  let onTest: () -> Void

  @State private var draft: String = ""

  private var accent: Color {
    isSet ? CSColor.olive : (optional ? CSColor.textFaint : CSColor.terracotta)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack(spacing: 10) {
        Circle().fill(accent.opacity(0.85)).frame(width: 7, height: 7)
        Text(label)
          .font(CSFont.ui(13.5, .semibold))
          .foregroundStyle(CSColor.textBody)
        Text(account)
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textFaint)
        Spacer(minLength: 0)
        if let probeResult {
          KeyProbeChip(result: probeResult)
        }
        Text(isSet ? "set" : (optional ? "optional" : "not set"))
          .font(CSFont.mono(10, .semibold))
          .foregroundStyle(isSet ? CSColor.oliveLight : accent)
      }

      HStack(spacing: 8) {
        SecureField(isSet ? "Replace key…" : "Paste key…", text: $draft)
          .settingsInputChrome()
          .onSubmit(save)
          .accessibilityLabel("\(label) secret")

        SettingsSaveButton(enabled: !draft.isEmpty, action: save)
          .accessibilityLabel("Save \(label)")

        SettingsChipButton(enabled: isSet && !probePending, action: onTest) {
          Group {
            if probePending {
              ProgressView().controlSize(.small).scaleEffect(0.62).frame(width: 20, height: 14)
            } else {
              Text("Test").font(CSFont.ui(12, .semibold))
            }
          }
          .frame(width: 26, height: 18)
          .foregroundStyle(isSet ? CSColor.textMutedAlt : CSColor.textFaint)
        }
        .help(isSet ? "Test this key" : "Save a key first to test it")
        .accessibilityLabel("Test \(label)")

        SettingsChipButton(enabled: isSet, action: onClear) {
          CSIconView(
            icon: .delete, size: 12, weight: .semibold,
            color: isSet ? CSColor.terracottaLight : CSColor.textFaint
          )
          .frame(width: 10, height: 18)
        }
        .help("Remove this key from the Keychain")
        .accessibilityLabel("Clear \(label)")
      }
    }
    // Presence-tinted card: green when set, red (required) / grey (optional) when not.
    .padding(.horizontal, 15)
    .padding(.vertical, 13)
    .background(RoundedRectangle(cornerRadius: 11, style: .continuous).fill(accent.opacity(0.06)))
    .overlay(
      RoundedRectangle(cornerRadius: 11, style: .continuous)
        .strokeBorder(accent.opacity(0.18), lineWidth: 1)
    )
  }

  private func save() {
    guard !draft.isEmpty else { return }
    onSave(draft)
    draft = ""
  }
}

extension KeyRow {
  /// Row wired to the view-model's save / clear / test for one account.
  init(
    model: SettingsViewModel, account: String, label: String, isSet: Bool, optional: Bool = false
  ) {
    self.init(
      account: account, label: label, isSet: isSet, optional: optional,
      probeResult: model.keyProbeResults[account],
      probePending: model.keyProbePending.contains(account),
      onSave: { model.saveKey(account: account, secret: $0) },
      onClear: { model.clearKey(account: account) },
      onTest: { model.testKey(account: account) })
  }
}

struct KeyProbeChip: View {
  let result: CsApiKeyProbeResult

  private var label: String {
    let verdict: String
    switch result.status {
    case .ok: verdict = "Key OK"
    case .invalid: verdict = "Invalid key"
    case .noQuota: verdict = "No credits (check billing)"
    case .network: verdict = "Network error"
    case .missing: verdict = "Not set"
    // "Unsupported" read as "bad key" — it only means this provider ships no
    // cheap liveness probe. The key itself is stored and used normally.
    case .unsupported: verdict = "Saved — no test for this key"
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
    case .invalid, .noQuota: return CSColor.terracottaLight
    case .network: return CSColor.amber
    case .missing, .unsupported: return CSColor.textFaint
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
          "\(result.message)\nEndpoint: \($0)"
        } ?? result.message
      )
  }
}

// MARK: - Vendor account (OAuth) row

/// "Sign in with <brand>" for vendors that ship an OAuth flow. The signed-in
/// account wins over a stored API key on the assistive lane (loader predicate
/// `account_auth`), so the row says which credential will actually be sent.
struct AccountLoginRow: View {
  let provider: CsProviderOption
  let loginPending: Bool
  let loginNotice: String?
  let onStart: () -> Void
  let onSignOut: () -> Void
  let onSaveClientId: (String) -> Void

  @State private var clientIdDraft: String = ""
  @State private var showAdvancedClientId = false

  private var signedIn: Bool { provider.accountSignedIn }
  private var accent: Color { signedIn ? CSColor.olive : CSColor.textFaint }

  /// Short brand for the account row — OpenCode-style, not a client-id dump.
  private var accountBrand: String {
    switch provider.id {
    case "openai-responses": return "ChatGPT"
    case "xai-responses": return "xAI"
    case "anthropic-messages": return "Claude"
    default: return provider.displayName
    }
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      HStack(spacing: 10) {
        Circle().fill(accent.opacity(0.85)).frame(width: 7, height: 7)
        Text("\(accountBrand) account")
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(CSColor.textBody)
        // "signed in as <email>" / "not signed in" / "awaiting app registration".
        Text(provider.accountStatusMessage)
          .font(CSFont.mono(10, .semibold))
          .foregroundStyle(accent)
          .lineLimit(1)
        if let loginNotice, !loginNotice.isEmpty {
          Text(loginNotice)
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(CSColor.terracottaLight)
            .lineLimit(1)
            .help(loginNotice)
        }
        Spacer(minLength: 0)
        if signedIn {
          SettingsChipButton(
            "Sign out", tint: CSColor.terracottaLight, enabled: !loginPending, action: onSignOut
          )
          .help("Remove the stored \(accountBrand) account tokens")
          .accessibilityLabel("Sign out of \(accountBrand)")
        }
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
            provider.accountLoginEnabled && !loginPending ? CSColor.oliveLight : CSColor.textFaint
          )
        }
        .help(provider.accountStatusMessage)
        .accessibilityLabel("Sign in with \(accountBrand)")
      }

      // Client id is a non-secret public app identity. OpenAI + xAI ship
      // defaults (NOTICE); operators almost never need to paste one, so the
      // override stays under Advanced.
      DisclosureGroup(isExpanded: $showAdvancedClientId) {
        HStack(spacing: 8) {
          Text("client id")
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(CSColor.textFaint)
          TextField(
            provider.oauthClientId ?? "Override OAuth client id…",
            text: $clientIdDraft
          )
          .settingsInputChrome()
          .onSubmit { onSaveClientId(clientIdDraft) }
          .accessibilityLabel("\(accountBrand) OAuth client id")
          SettingsChipButton(
            "Save", tint: CSColor.oliveLight,
            enabled: clientIdDraft != (provider.oauthClientId ?? ""),
            action: { onSaveClientId(clientIdDraft) }
          )
          .help("Optional override (settings.json) — empty restores the shipped default")
        }
        .padding(.top, 4)
      } label: {
        Text("Advanced · OAuth client id")
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textFaint)
      }
    }
    .onAppear { clientIdDraft = provider.oauthClientId ?? "" }
    .onChange(of: provider.oauthClientId) { _, updated in
      clientIdDraft = updated ?? ""
    }
  }
}
