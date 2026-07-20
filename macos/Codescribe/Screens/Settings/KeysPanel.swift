import Foundation
import SwiftUI

// Providers panel: write-only API-key management. Secrets go to the Keychain
// via `setApiKey` and are NEVER read back across the FFI; presence renders from
// `CsKeyStatus` booleans. Agent configuration lives in `AgentPanel`.

struct KeysPanel: View {
    static let ownedCapabilities: Set<SettingsPanelCapability> = [.apiKeys]

    @ObservedObject var model: SettingsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            EyebrowLabel(text: "Settings · \(SettingsSection.keys.title)")
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

            SettingsSectionLabel("API keys")
                .padding(.top, 22)
            VStack(spacing: 8) {
                ForEach(model.keyAccounts, id: \.self) { account in
                    let provider = model.providerForKeyAccount(account)
                    KeyRow(
                        account: account,
                        label: SettingsViewModel.keyLabel(for: account),
                        isSet: model.keyStatus.isSet(account: account),
                        probeResult: model.keyProbeResults[account],
                        probePending: model.keyProbePending.contains(account),
                        accountProvider: provider,
                        accountLoginPending: provider.map {
                            model.accountLoginPending.contains($0.id)
                        } ?? false,
                        accountLoginNotice: provider.flatMap { model.accountLoginNotices[$0.id] },
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

// MARK: - Editable LLM lanes (the one provider/model edit grammar)

/// Three request lanes sharing one visual grammar while preserving their distinct
/// promoted config keys. AgentPanel's runtime rows remain the effective
/// read-only truth.
struct LLMLanesSection: View {
    @ObservedObject var model: SettingsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Set provider, endpoint, and model per request path. Leave an override empty to use the resolved fallback.")
                .font(CSFont.ui(11.5))
                .lineSpacing(2)
                .foregroundStyle(CSColor.textMutedAlt)

            ForEach(LLMLane.allCases) { lane in
                LLMLaneEditor(model: model, lane: lane)
                if lane != LLMLane.allCases.last {
                    Rectangle()
                        .fill(CSColor.hairline(0.05))
                        .frame(height: 1)
                }
            }
        }
    }
}

private struct LLMLaneEditor: View {
    @ObservedObject var model: SettingsViewModel
    let lane: LLMLane

    @State private var endpointDraft = ""
    @State private var modelDraft = ""

    private var laneModel: LLMLaneModel { model.llmLane(lane) }

    private var providerLabel: String {
        laneModel.provider?.displayName ?? laneModel.providerId
    }

    private var currentModelLabel: String {
        laneModel.modelOptions.first { $0.id == laneModel.resolvedModel }?.displayName
            ?? laneModel.resolvedModel
    }

    private var discoveryDotColor: Color {
        if laneModel.manualModelReason != nil { return CSColor.textFaint }
        switch laneModel.discovery.status {
        case "fresh": return CSColor.olive
        case "cached": return CSColor.amber
        case "no_key", "loading": return CSColor.textFaint
        default: return CSColor.terracotta
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(lane.title)
                    .font(CSFont.ui(14.5, .bold))
                    .foregroundStyle(CSColor.textHigh)
                Text(lane.subtitle)
                    .font(CSFont.ui(11.5))
                    .foregroundStyle(CSColor.textMutedAlt)
            }

            if lane == .assistive {
                SettingsControlRow(title: "Provider", subtitle: "Assistive requests only") {
                    Menu {
                        ForEach(model.providers, id: \.id) { provider in
                            Button {
                                model.setAssistiveProvider(provider.id)
                            } label: {
                                if provider.id == laneModel.providerId {
                                    Label(provider.displayName, systemImage: "checkmark")
                                } else {
                                    Text(provider.displayName)
                                }
                            }
                        }
                    } label: {
                        SettingsMenuLabel(text: providerLabel)
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .fixedSize()
                    .accessibilityLabel("Assistive provider")
                    .accessibilityValue(providerLabel)
                }
            }

            SettingsControlRow(title: "Endpoint", subtitle: lane.endpointKey) {
                HStack(spacing: 8) {
                    overrideTextField(
                        placeholder: laneModel.resolvedEndpoint,
                        text: $endpointDraft,
                        accessibilityLabel: "\(lane.title) LLM endpoint",
                        onSubmit: saveEndpoint
                    )

                    saveOverrideButton(
                        draft: endpointDraft,
                        accessibilityLabel: "Save \(lane.title) endpoint",
                        action: saveEndpoint
                    )

                    resetOverrideButton(
                        help: "Clear this endpoint override",
                        accessibilityLabel: "Reset \(lane.title) endpoint"
                    ) {
                        endpointDraft = ""
                        model.setLLMEndpoint("", for: lane)
                    }
                }
                .frame(width: 380)
            }

            SettingsControlRow(title: "Model", subtitle: lane.modelKey) {
                HStack(spacing: 8) {
                    if laneModel.discovery.status == "loading"
                        && laneModel.manualModelReason == nil
                    {
                        HStack(spacing: 7) {
                            ProgressView()
                                .controlSize(.small)
                            Text("Discovering models…")
                                .font(CSFont.mono(10.5, .medium))
                                .foregroundStyle(CSColor.textFaint)
                        }
                        .frame(maxWidth: .infinity, alignment: .trailing)
                        .accessibilityLabel("Discovering \(lane.title) models")
                    } else if laneModel.usesDiscoveredPicker {
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
                        } label: { SettingsMenuLabel(text: currentModelLabel) }
                        .menuStyle(.borderlessButton)
                        .menuIndicator(.hidden)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                        .accessibilityLabel("\(lane.title) model")
                        .accessibilityValue(currentModelLabel)
                    } else {
                        overrideTextField(
                            placeholder: laneModel.resolvedModel,
                            text: $modelDraft,
                            accessibilityLabel: "\(lane.title) model ID",
                            onSubmit: saveModel
                        )

                        saveOverrideButton(
                            draft: modelDraft,
                            accessibilityLabel: "Save \(lane.title) model",
                            action: saveModel
                        )
                    }

                    resetOverrideButton(
                        help: "Clear this model override",
                        accessibilityLabel: "Reset \(lane.title) model"
                    ) {
                        modelDraft = ""
                        model.setLLMModel("", for: lane)
                    }
                }
                .frame(width: 380)
            }

            HStack(spacing: 8) {
                Circle()
                    .fill(discoveryDotColor.opacity(0.85))
                    .frame(width: 7, height: 7)
                Text(laneModel.discoveryDescription)
                    .font(CSFont.mono(10.5, .medium))
                    .foregroundStyle(CSColor.textFaint)
                    .lineLimit(2)
            }
            .padding(.leading, 2)
        }
    }

    private func saveEndpoint() {
        model.setLLMEndpoint(endpointDraft, for: lane)
        endpointDraft = ""
    }

    private func saveModel() {
        model.setLLMModel(modelDraft, for: lane)
        modelDraft = ""
    }

    private func overrideTextField(
        placeholder: String,
        text: Binding<String>,
        accessibilityLabel: String,
        onSubmit: @escaping () -> Void
    ) -> some View {
        TextField(placeholder, text: text)
            .textFieldStyle(.plain)
            .font(CSFont.mono(11.5, .regular))
            .foregroundStyle(CSColor.textBody)
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(
                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .fill(CSColor.surfaceRaised(0.03))
            )
            .overlay(
                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
            )
            .onSubmit(onSubmit)
            .accessibilityLabel(accessibilityLabel)
    }

    private func saveOverrideButton(
        draft: String,
        accessibilityLabel: String,
        action: @escaping () -> Void
    ) -> some View {
        Button("Save", action: action)
            .font(CSFont.ui(11.5, .semibold))
            .foregroundStyle(draft.isEmpty ? CSColor.textFaint : CSColor.terracottaLight)
            .buttonStyle(.plain)
            .disabled(draft.isEmpty)
            .accessibilityLabel(accessibilityLabel)
    }

    private func resetOverrideButton(
        help: String,
        accessibilityLabel: String,
        action: @escaping () -> Void
    ) -> some View {
        Button("Reset", action: action)
            .font(CSFont.ui(11.5, .semibold))
            .foregroundStyle(CSColor.textMutedAlt)
            .buttonStyle(.plain)
            .help(help)
            .accessibilityLabel(accessibilityLabel)
    }
}

// MARK: - Agent workspace roots editor

/// Editable list of workspace roots the agent's `list_projects` tool scans to
/// resolve project names to absolute paths. Rows are edited locally and committed
/// through `SettingsViewModel.setAgentWorkspaceRoots` (colon-joined ->
/// `AGENT_WORKSPACE_ROOTS`). Each row shows a live "directory exists" indicator.
struct WorkspaceRootsSection: View {
    @ObservedObject var model: SettingsViewModel

    @State private var rows: [String] = []
    @State private var loaded = false

    private var isDirty: Bool {
        cleaned(rows) != cleaned(model.agentWorkspaceRoots)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSectionLabel("Agent workspace roots")

            Text("Directories the assistant scans to resolve a project name to a path (list_projects). One level deep; git checkouts only.")
                .font(CSFont.ui(11.5))
                .lineSpacing(2)
                .foregroundStyle(CSColor.textMutedAlt)
                .padding(.top, 8)

            VStack(spacing: 8) {
                ForEach(rows.indices, id: \.self) { index in
                    rootRow(index: index)
                }
            }
            .padding(.top, 12)

            HStack(spacing: 10) {
                Button {
                    rows.append("")
                } label: {
                    Label("Add root", systemImage: "plus")
                        .font(CSFont.ui(12, .semibold))
                }
                .buttonStyle(.plain)
                .foregroundStyle(CSColor.textBody)

                Spacer()

                Button {
                    model.setAgentWorkspaceRoots(rows)
                    syncFromModel()
                } label: {
                    Text("Save roots")
                        .font(CSFont.ui(12, .semibold))
                        .foregroundStyle(isDirty ? CSColor.textHigh : CSColor.textFaint)
                }
                .buttonStyle(.plain)
                .disabled(!isDirty)
            }
            .padding(.top, 12)
        }
        .onAppear {
            guard !loaded else { return }
            loaded = true
            syncFromModel()
        }
    }

    private func rootRow(index: Int) -> some View {
        HStack(spacing: 10) {
            existsDot(for: rows[index])
            TextField("~/Git", text: Binding(
                get: { index < rows.count ? rows[index] : "" },
                set: { if index < rows.count { rows[index] = $0 } }
            ))
            .textFieldStyle(.plain)
            .font(CSFont.mono(12, .regular))
            .foregroundStyle(CSColor.textBody)
            .frame(maxWidth: .infinity, alignment: .leading)

            Button {
                rows.remove(at: index)
            } label: {
                CSIconView(icon: .remove, size: 13, weight: .semibold, color: CSColor.textFaint)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 11)
        .padding(.vertical, 9)
        .background(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .fill(CSColor.surfaceRaised(0.03))
        )
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
        )
    }

    /// Green when the (tilde-expanded) path is an existing directory, amber
    /// otherwise — the tool will silently skip a root that does not resolve.
    private func existsDot(for path: String) -> some View {
        let trimmed = path.trimmingCharacters(in: .whitespaces)
        let valid = Self.directoryExists(trimmed)
        return Circle()
            .fill(valid ? CSColor.oliveLight : CSColor.amber)
            .frame(width: 7, height: 7)
    }

    private func syncFromModel() {
        rows = model.agentWorkspaceRoots
        if rows.isEmpty { rows = ["~/Git"] }
    }

    private func cleaned(_ input: [String]) -> [String] {
        input
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    private static func directoryExists(_ path: String) -> Bool {
        guard !path.isEmpty else { return false }
        let expanded = (path as NSString).expandingTildeInPath
        var isDir: ObjCBool = false
        let exists = FileManager.default.fileExists(atPath: expanded, isDirectory: &isDir)
        return exists && isDir.boolValue
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
#Preview("Providers panel") {
    ScrollView { KeysPanel(model: .preview(.keys)) }
        .frame(width: 720, height: 620)
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
#endif
