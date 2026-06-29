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

            SettingsSectionLabel("Providers")
                .padding(.top, 22)
            VStack(spacing: 8) {
                ForEach(model.keyAccounts, id: \.self) { account in
                    KeyRow(
                        account: account,
                        label: SettingsViewModel.keyLabel(for: account),
                        isSet: model.keyStatus.isSet(account: account),
                        onSave: { model.saveKey(account: account, secret: $0) },
                        onClear: { model.clearKey(account: account) }
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

// MARK: - Key row

private struct KeyRow: View {
    let account: String
    let label: String
    let isSet: Bool
    let onSave: (String) -> Void
    let onClear: () -> Void

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

                Button(action: onClear) {
                    Image(systemName: "trash")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(isSet ? CSColor.terracottaLight : CSColor.textFaint)
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

#Preview("Keys panel") {
    ScrollView { KeysPanel(model: .preview(.keys)) }
        .frame(width: 720, height: 620)
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
