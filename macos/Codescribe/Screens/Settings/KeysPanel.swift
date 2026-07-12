import Foundation
import SwiftUI

// Keys panel: write-only API-key management. Secrets are entered through a
// SecureField and pushed to the core's Keychain via `setApiKey`; the trash button
// clears via `clearApiKey`. Presence is rendered from `CsKeyStatus` booleans —
// a secret is NEVER read back across the FFI.

struct KeysPanel: View {
    @ObservedObject var model: SettingsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            EyebrowLabel(text: "Settings · Keys")
            Text("API keys.")
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
                .padding(.top, 6)

            Text("Stored in the macOS Keychain. Keys are write-only here — codescribe never displays a stored secret.")
                .font(CSFont.ui(12.5))
                .lineSpacing(2)
                .foregroundStyle(CSColor.textMutedAlt)
                .padding(.top, 8)

            SettingsSectionLabel("Agent provider")
                .padding(.top, 22)
            AgentProviderSelector(model: model)
                .padding(.top, 11)

            SettingsSectionLabel("Providers")
                .padding(.top, 22)
            VStack(spacing: 8) {
                ForEach(model.keyAccounts, id: \.self) { account in
                    KeyRow(
                        account: account,
                        label: SettingsViewModel.keyLabel(for: account),
                        isSet: model.keyStatus.isSet(account: account),
                        probeResult: model.keyProbeResults[account],
                        probePending: model.keyProbePending.contains(account),
                        accountProvider: model.providerForKeyAccount(account),
                        accountLoginPending: model.providerForKeyAccount(account).map {
                            model.accountLoginPending.contains($0.id)
                        } ?? false,
                        accountLoginNotice: model.providerForKeyAccount(account).flatMap {
                            model.accountLoginNotices[$0.id]
                        },
                        onSave: { model.saveKey(account: account, secret: $0) },
                        onClear: { model.clearKey(account: account) },
                        onTest: { model.testKey(account: account) },
                        onStartAccountLogin: { model.startAccountLogin(providerId: $0) },
                        onSignOutAccount: { model.signOutAccount(providerId: $0) },
                        onSaveOauthClientId: { model.saveOauthClientId(providerId: $0, value: $1) }
                    )
                }
            }
            .padding(.top, 11)

            HStack(spacing: 8) {
                Text("●").font(CSFont.mono(11, .medium)).foregroundStyle(CSColor.olive)
                Text("secrets live only in the Keychain — presence shown, value hidden")
                    .font(CSFont.mono(11, .medium))
                    .foregroundStyle(CSColor.textFaint)
            }
            .padding(.top, 16)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
    }
}

// MARK: - Agent provider / model selector

// Picks the assistive/agent-lane provider + model. The chosen provider maps to
// one of the API-key rows below; the dot reflects whether that key is present.
private struct AgentProviderSelector: View {
    @ObservedObject var model: SettingsViewModel

    private var selected: CsProviderOption? { model.selectedProvider }
    private var discoveredModels: [CsModelOption] { model.discoveredModels }

    private var discoveryDotColor: Color {
        switch model.modelDiscoveryStatus {
        case "fresh": return CSColor.olive
        case "cached": return CSColor.amber
        case "no_key": return CSColor.textFaint
        default: return CSColor.terracotta
        }
    }

    private var currentModelLabel: String {
        let id = model.assistiveModel
        if id.isEmpty {
            return discoveredModels.isEmpty ? "No discovered models" : "Choose a model"
        }
        return discoveredModels.first { $0.id == id }?.displayName ?? id
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            SelectorRow(label: "Provider") {
                Menu {
                    ForEach(model.availableProviders, id: \.id) { provider in
                        Button {
                            model.setAssistiveProvider(provider.id)
                        } label: {
                            if provider.id == model.assistiveProviderId {
                                Label(provider.displayName, systemImage: "checkmark")
                            } else {
                                Text(provider.displayName)
                            }
                        }
                    }
                } label: {
                    MenuLabel(text: selected?.displayName ?? model.assistiveProviderId)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
            }

            SelectorRow(label: "Model") {
                Menu {
                    ForEach(discoveredModels, id: \.id) { option in
                        Button {
                            model.setAssistiveModel(option.id)
                        } label: {
                            if option.id == model.assistiveModel {
                                Label(option.displayName, systemImage: "checkmark")
                            } else {
                                Text(option.displayName)
                            }
                        }
                    }
                } label: {
                    MenuLabel(text: currentModelLabel, mono: true)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .disabled(discoveredModels.isEmpty)
            }

            if let selected {
                HStack(spacing: 8) {
                    Circle()
                        .fill((selected.apiKeySet ? CSColor.olive : CSColor.terracotta).opacity(0.85))
                        .frame(width: 7, height: 7)
                    Text("uses \(SettingsViewModel.keyLabel(for: selected.apiKeyAccount)) — \(selected.apiKeySet ? "set" : "not set") below")
                        .font(CSFont.mono(11, .medium))
                        .foregroundStyle(CSColor.textFaint)
                }
            }

            HStack(spacing: 8) {
                Circle()
                    .fill(discoveryDotColor.opacity(0.85))
                    .frame(width: 7, height: 7)
                Text(model.modelDiscoveryDescription)
                    .font(CSFont.mono(11, .medium))
                    .foregroundStyle(CSColor.textFaint)
                    .lineLimit(2)
            }
        }
        .padding(.horizontal, 15)
        .padding(.vertical, 13)
        .background(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill(CSColor.surfaceRaised(0.03))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
        )
    }
}

private struct SelectorRow<Content: View>: View {
    let label: String
    @ViewBuilder let content: () -> Content

    var body: some View {
        HStack(spacing: 12) {
            Text(label)
                .font(CSFont.mono(12, .medium))
                .foregroundStyle(CSColor.textMutedAlt)
                .frame(width: 80, alignment: .leading)
            content()
            Spacer(minLength: 0)
        }
    }
}

private struct MenuLabel: View {
    let text: String
    var mono: Bool = false

    var body: some View {
        HStack(spacing: 6) {
            Text(text)
                .font(mono ? CSFont.mono(12.5, .semibold) : CSFont.ui(12.5, .semibold))
                .foregroundStyle(CSColor.textHigh)
                .lineLimit(1)
            CSIconView(icon: .chevronUpDown, size: 9, weight: .semibold, color: CSColor.textFaint)
        }
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
        .contentShape(Rectangle())
    }
}

// MARK: - Key row

private struct KeyRow: View {
    let account: String
    let label: String
    let isSet: Bool
    let probeResult: CsApiKeyProbeResult?
    let probePending: Bool
    let accountProvider: CsProviderOption?
    let accountLoginPending: Bool
    let accountLoginNotice: String?
    let onSave: (String) -> Void
    let onClear: () -> Void
    let onTest: () -> Void
    let onStartAccountLogin: (String) -> Void
    let onSignOutAccount: (String) -> Void
    let onSaveOauthClientId: (String, String) -> Void

    @State private var draft: String = ""

    private var accent: Color { isSet ? CSColor.olive : CSColor.terracotta }
    private var accentLight: Color { isSet ? CSColor.oliveLight : CSColor.terracottaLight }

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
                Text(isSet ? "set" : "not set")
                    .font(CSFont.mono(10, .semibold))
                    .foregroundStyle(accentLight)
            }

            HStack(spacing: 8) {
                SecureField(isSet ? "Replace key…" : "Paste key…", text: $draft)
                    .textFieldStyle(.plain)
                    .font(CSFont.mono(12, .regular))
                    .foregroundStyle(CSColor.textBody)
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
                    .onSubmit(save)

                Button(action: save) {
                    Text("Save")
                        .font(CSFont.ui(12, .semibold))
                        .foregroundStyle(draft.isEmpty ? CSColor.textFaint : CSColor.terracottaLight)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .background(
                            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                                .fill(CSColor.terracotta.opacity(draft.isEmpty ? 0.06 : 0.14))
                        )
                        .overlay(
                            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                                .strokeBorder(CSColor.terracotta.opacity(draft.isEmpty ? 0.1 : 0.28), lineWidth: 1)
                        )
                }
                .buttonStyle(.plain)
                .disabled(draft.isEmpty)

                Button(action: onTest) {
                    Group {
                        if probePending {
                            ProgressView()
                                .controlSize(.small)
                                .scaleEffect(0.62)
                                .frame(width: 20, height: 14)
                        } else {
                            Text("Test")
                                .font(CSFont.ui(12, .semibold))
                        }
                    }
                    .frame(width: 48, height: 32)
                    .foregroundStyle(isSet ? CSColor.textMutedAlt : CSColor.textFaint)
                    .background(
                        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                            .fill(CSColor.surfaceRaised(0.03))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                            .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                    )
                }
                .buttonStyle(.plain)
                .disabled(probePending || !isSet)
                .help(isSet ? "Test this key" : "Save a key first to test it")

                Button(action: onClear) {
                    CSIconView(
                        icon: .delete,
                        size: 12,
                        weight: .semibold,
                        color: isSet ? CSColor.terracottaLight : CSColor.textFaint
                    )
                    .frame(width: 32, height: 32)
                        .background(
                            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                                .fill(CSColor.surfaceRaised(0.03))
                        )
                        .overlay(
                            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                                .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                        )
                }
                .buttonStyle(.plain)
                .disabled(!isSet)
                .help("Remove this key from the Keychain")
            }

            if let accountProvider {
                AccountLoginRow(
                    provider: accountProvider,
                    loginPending: accountLoginPending,
                    loginNotice: accountLoginNotice,
                    onStart: { onStartAccountLogin(accountProvider.id) },
                    onSignOut: { onSignOutAccount(accountProvider.id) },
                    onSaveClientId: { onSaveOauthClientId(accountProvider.id, $0) }
                )
            }
        }
        .padding(.horizontal, 15)
        .padding(.vertical, 13)
        .background(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill(accent.opacity(0.06))
        )
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

private struct KeyProbeChip: View {
    let result: CsApiKeyProbeResult

    private var label: String {
        let verdict: String
        switch result.status {
        case .ok: verdict = "Key OK"
        case .invalid: verdict = "Invalid key"
        case .noQuota: verdict = "No credits (check billing)"
        case .network: verdict = "Network error"
        case .missing: verdict = "Not set"
        case .unsupported: verdict = "Unsupported"
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
            .background(
                Capsule()
                    .fill(tint.opacity(0.11))
            )
            .overlay(
                Capsule()
                    .strokeBorder(tint.opacity(0.24), lineWidth: 1)
            )
            .help(
                result.probedEndpoint.map {
                    "\(result.message)\nEndpoint: \($0)"
                } ?? result.message
            )
    }
}

private struct AccountLoginRow: View {
    let provider: CsProviderOption
    let loginPending: Bool
    let loginNotice: String?
    let onStart: () -> Void
    let onSignOut: () -> Void
    let onSaveClientId: (String) -> Void

    @State private var clientIdDraft: String = ""

    private var signedIn: Bool { provider.accountSignedIn }
    private var accent: Color { signedIn ? CSColor.olive : CSColor.textFaint }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                Circle()
                    .fill(accent.opacity(0.85))
                    .frame(width: 7, height: 7)
                Text("ChatGPT account")
                    .font(CSFont.ui(12.5, .semibold))
                    .foregroundStyle(CSColor.textBody)
                // Carries "signed in as <email>" / "not signed in" /
                // "awaiting app registration" straight from the core.
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
                    AccountActionButton(
                        title: "Sign out",
                        tint: CSColor.terracottaLight,
                        enabled: !loginPending,
                        action: onSignOut
                    )
                    .help("Remove the stored ChatGPT account tokens")
                }
                Button(action: onStart) {
                    HStack(spacing: 6) {
                        if loginPending {
                            ProgressView()
                                .controlSize(.small)
                                .scaleEffect(0.62)
                                .frame(width: 14, height: 12)
                        } else {
                            CSIconView(icon: .accountVerified, size: 12, weight: .semibold)
                        }
                        Text(loginPending ? "Waiting for browser…" : "Sign in with ChatGPT")
                            .font(CSFont.ui(12, .semibold))
                    }
                    .foregroundStyle(
                        provider.accountLoginEnabled && !loginPending
                            ? CSColor.oliveLight : CSColor.textFaint
                    )
                    .padding(.horizontal, 12)
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
                .buttonStyle(.plain)
                .disabled(!provider.accountLoginEnabled || loginPending)
                .help(provider.accountStatusMessage)
            }

            // OAuth client id — non-secret app identity (NOT a credential), so a
            // plain TextField with the value visible is correct here. Saving an
            // empty field clears back to "awaiting app registration".
            HStack(spacing: 8) {
                Text("client id")
                    .font(CSFont.mono(10, .medium))
                    .foregroundStyle(CSColor.textFaint)
                TextField(
                    provider.oauthClientId == nil ? "Paste OAuth client id…" : "",
                    text: $clientIdDraft
                )
                .textFieldStyle(.plain)
                .font(CSFont.mono(11, .regular))
                .foregroundStyle(CSColor.textBody)
                .padding(.horizontal, 9)
                .padding(.vertical, 6)
                .background(
                    RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                        .fill(CSColor.surfaceRaised(0.03))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                        .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                )
                .onSubmit { onSaveClientId(clientIdDraft) }
                AccountActionButton(
                    title: "Save",
                    tint: CSColor.oliveLight,
                    enabled: clientIdDraft != (provider.oauthClientId ?? ""),
                    action: { onSaveClientId(clientIdDraft) }
                )
                .help("Store the client id (settings.json) — applies without restart")
            }
        }
        .onAppear { clientIdDraft = provider.oauthClientId ?? "" }
        .onChange(of: provider.oauthClientId) { _, updated in
            clientIdDraft = updated ?? ""
        }
    }
}

private struct AccountActionButton: View {
    let title: String
    let tint: Color
    let enabled: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(CSFont.ui(11.5, .semibold))
                .foregroundStyle(enabled ? tint : CSColor.textFaint)
                .padding(.horizontal, 11)
                .padding(.vertical, 6)
                .background(
                    RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                        .fill(CSColor.surfaceRaised(0.03))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                        .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }
}

#if DEBUG
#Preview("Keys panel") {
    ScrollView { KeysPanel(model: .preview(.keys)) }
        .frame(width: 720, height: 620)
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
#endif
