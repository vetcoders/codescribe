import SwiftUI

// Local-first identity surface. Codescribe has no account model, so this panel
// reports the running build and local data truth instead of inventing a profile.
struct UserPanel: View {
    @ObservedObject var model: SettingsViewModel

    private static let docsURL = URL(string: "https://github.com/VetCoders/CodeScribe/tree/develop/docs")!

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            EyebrowLabel(text: "Settings · User")
            Text("Local by design.")
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
                .padding(.top, 6)
            Text("No account is required. Your configuration and transcript history stay on this Mac.")
                .font(CSFont.ui(12.5))
                .lineSpacing(2)
                .foregroundStyle(CSColor.textMutedAlt)
                .padding(.top, 8)

            SettingsSectionLabel("Running build")
                .padding(.top, 24)
            VStack(spacing: 0) {
                infoRow("Version", "\(model.buildInfo.version) (\(model.buildInfo.build))")
                divider
                infoRow("Commit", model.buildInfo.commit)
                divider
                infoRow("Built", model.buildInfo.builtAt)
            }
            .padding(.top, 11)
            .background(card)
            .overlay(cardBorder)

            SettingsSectionLabel("Local data")
                .padding(.top, 24)
            VStack(spacing: 0) {
                pathRow("Config, logs & runtime data", model.configDir)
                divider
                pathRow("Transcripts", model.transcriptsPath)
            }
            .padding(.top, 11)
            .background(card)
            .overlay(cardBorder)

            SettingsSectionLabel("Agent transcript tagging")
                .padding(.top, 24)
            SettingsControlRow(
                title: "Tag transcripts for AI agents",
                subtitle: "Wrap delivered dictation in an explicit source tag"
            ) {
                Toggle("", isOn: taggingBinding)
                    .toggleStyle(.switch)
                    .labelsHidden()
                    .tint(CSColor.terracotta)
                    .accessibilityLabel("Tag transcripts for AI agents")
                    .accessibilityValue(model.settings.transcriptTaggingEnabled ? "On" : "Off")
            }
            .padding(.top, 11)

            Text("Template preview")
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(CSColor.textFaint)
                .padding(.top, 12)
            Text(model.transcriptTagPreview)
                .font(CSFont.mono(11.5, .regular))
                .foregroundStyle(CSColor.textBodyAlt)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
                .background(card)
                .overlay(cardBorder)
                .accessibilityLabel("Transcript tag template preview")
                .accessibilityValue(model.transcriptTagPreview)

            Link(destination: Self.docsURL) {
                HStack(spacing: 6) {
                    Text("Open Codescribe documentation")
                    Text("↗")
                }
                .font(CSFont.mono(11, .semibold))
                .foregroundStyle(CSColor.terracottaLight)
            }
            .padding(.top, 18)
            .accessibilityLabel("Open Codescribe documentation")

            ResetAppDataSection(model: model)
                .padding(.top, 30)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
    }

    private var taggingBinding: Binding<Bool> {
        Binding(
            get: { model.settings.transcriptTaggingEnabled },
            set: { model.setTranscriptTaggingEnabled($0) }
        )
    }

    private func infoRow(_ label: String, _ value: String) -> some View {
        HStack(spacing: 14) {
            Text(label)
                .font(CSFont.ui(12.5, .medium))
                .foregroundStyle(CSColor.textMutedAlt)
                .frame(width: 90, alignment: .leading)
            Text(value)
                .font(CSFont.mono(11.5, .medium))
                .foregroundStyle(CSColor.textBody)
                .textSelection(.enabled)
                .accessibilityLabel(label)
                .accessibilityValue(value)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 15)
        .padding(.vertical, 12)
    }

    private func pathRow(_ label: String, _ path: String) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(label)
                .font(CSFont.ui(12.5, .semibold))
                .foregroundStyle(CSColor.textBody)
            Text(path.isEmpty ? "not loaded yet" : path)
                .font(CSFont.mono(10.5, .regular))
                .foregroundStyle(CSColor.textMutedAlt)
                .textSelection(.enabled)
                .lineLimit(2)
                .truncationMode(.middle)
                .accessibilityLabel(label)
                .accessibilityValue(path.isEmpty ? "not loaded yet" : path)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 15)
        .padding(.vertical, 12)
    }

    private var divider: some View {
        Rectangle().fill(CSColor.hairline(0.06)).frame(height: 1)
    }

    private var card: some View {
        RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
            .fill(CSColor.surfaceRaised(0.025))
    }

    private var cardBorder: some View {
        RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
            .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
    }
}

// MARK: - Danger zone

/// The full-data reset lives only at the foot of User settings, away from MCP
/// editing. Data is recoverable from Trash; Keychain deletion remains opt-in.
private struct ResetAppDataSection: View {
    @ObservedObject var model: SettingsViewModel
    @State private var includeKeys = false
    @State private var confirming = false
    @State private var confirmationText = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSectionLabel("Danger zone")
                .foregroundStyle(CSColor.dangerLight)

            Text("Moves recordings, transcript history, conversations, logs, preferences, "
                + "and local configuration to Trash so they can be recovered.")
                .font(CSFont.mono(11, .medium))
                .foregroundStyle(CSColor.textMutedAlt)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)

            Toggle(isOn: $includeKeys) {
                Text("Also remove API keys from Keychain")
                    .font(CSFont.ui(12.5, .medium))
                    .foregroundStyle(CSColor.textBody)
            }
            .toggleStyle(.checkbox)
            .padding(.top, 13)

            Button(role: .destructive) {
                model.refreshResetPreview()
                confirmationText = ""
                confirming = true
            } label: {
                Text("Move app data to Trash…")
                    .font(CSFont.ui(12, .semibold))
                    .foregroundStyle(CSColor.dangerLight)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)
                    .background(
                        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                            .fill(CSColor.danger.opacity(0.14))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                            .strokeBorder(CSColor.danger.opacity(0.42), lineWidth: 1)
                    )
            }
            .buttonStyle(.plain)
            .padding(.top, 13)
            .accessibilityLabel("Reset app data. Destructive action.")
            .accessibilityHint("Shows the live impact and requires typing RESET before data moves to Trash.")
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.vertical, 16)
        .background(
            RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
                .fill(CSColor.danger.opacity(0.055))
        )
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
                .strokeBorder(CSColor.danger.opacity(0.55), lineWidth: 1)
        )
        .alert("Move app data to Trash?", isPresented: $confirming) {
            TextField("Type RESET to continue", text: $confirmationText)
            Button("Cancel", role: .cancel) {
                confirmationText = ""
            }
            Button("Move to Trash & Relaunch", role: .destructive) {
                model.resetAppData(includeKeys: includeKeys)
            }
            .disabled(!resetConfirmationMatches(confirmationText))
        } message: {
            Text(model.resetImpactDescription(includeKeys: includeKeys))
        }
    }
}

#if DEBUG
#Preview("User panel") {
    ScrollView { UserPanel(model: .preview(.user)) }
        .frame(width: 720, height: 720)
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
#endif
