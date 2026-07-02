import SwiftUI

// Creator setup panel: live permission checklist + editable voice/formatting
// controls (language, AI formatting, formatting level) written through the core
// router, plus quick-start cards and launchpad chips.
// Permission rows reflect LIVE AVAuthorization / AX / IOHID / CG status.

struct CreatorPanel: View {
    @ObservedObject var model: SettingsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            EyebrowLabel(text: "Settings · Creator")
            Text("Get set up.")
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
                .padding(.top, 6)

            SettingsSectionLabel("Permission checklist")
                .padding(.top, 22)
            VStack(spacing: 8) {
                ForEach([PermissionKind.microphone, .accessibility, .inputMonitoring, .screenRecording]) { kind in
                    PermissionChecklistRow(kind: kind, state: model.permissions.state(kind))
                }
            }
            .padding(.top, 11)

            SettingsSectionLabel("Voice & formatting")
                .padding(.top, 24)
            VStack(spacing: 8) {
                SettingsControlRow(title: "Whisper language",
                                   subtitle: "Language used for speech-to-text") {
                    Picker("", selection: languageBinding) {
                        Text("Auto").tag(CsLanguage.auto)
                        Text("Polish").tag(CsLanguage.polish)
                        Text("English").tag(CsLanguage.english)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: 220)
                }
                SettingsControlRow(title: "AI formatting",
                                   subtitle: "Clean up transcripts with the LLM") {
                    Toggle("", isOn: formattingEnabledBinding)
                        .toggleStyle(.switch)
                        .labelsHidden()
                        .tint(CSColor.terracotta)
                }
                if model.settings.aiFormattingEnabled {
                    SettingsControlRow(title: "Formatting level",
                                       subtitle: "How aggressively the LLM rewrites") {
                        Picker("", selection: formattingLevelBinding) {
                            Text("Raw").tag("raw")
                            Text("Medium").tag("medium")
                            Text("Creative").tag("creative")
                        }
                        .pickerStyle(.segmented)
                        .labelsHidden()
                        .frame(width: 220)
                    }
                }
            }
            .padding(.top, 11)

            SettingsSectionLabel("Quick start")
                .padding(.top, 24)
            HStack(spacing: 10) {
                QuickStartCard(glyph: "🎙", title: "Test mic", subtitle: "Check levels & engine")
                QuickStartCard(glyph: "▦", title: "Open overlay", subtitle: "Summon the agent")
                QuickStartCard(glyph: "⌨", title: "Tune shortcuts", subtitle: "Hotkeys & cadence")
            }
            .padding(.top, 11)

            SettingsSectionLabel("Launchpads")
                .padding(.top, 24)
            LaunchpadChips()
                .padding(.top, 11)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
    }

    // MARK: - Bindings (read VM state, write through the router)

    private var languageBinding: Binding<CsLanguage> {
        Binding(get: { model.settings.whisperLanguage },
                set: { model.setLanguage($0) })
    }

    private var formattingEnabledBinding: Binding<Bool> {
        Binding(get: { model.settings.aiFormattingEnabled },
                set: { model.setFormattingEnabled($0) })
    }

    private var formattingLevelBinding: Binding<String> {
        Binding(get: { model.settings.formattingLevel ?? "medium" },
                set: { model.setFormattingLevel($0) })
    }
}

// MARK: - Labeled control row (shared shape for the editable settings rows)

struct SettingsControlRow<Control: View>: View {
    let title: String
    let subtitle: String
    @ViewBuilder var control: () -> Control

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(CSFont.ui(13.5, .semibold))
                    .foregroundStyle(CSColor.textBody)
                Text(subtitle)
                    .font(CSFont.ui(11.5))
                    .foregroundStyle(CSColor.textMutedAlt)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            control()
        }
        .padding(.horizontal, 15)
        .padding(.vertical, 12)
        .background(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill(CSColor.surfaceRaised(0.025))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
        )
    }
}

// MARK: - Permission checklist row

private struct PermissionChecklistRow: View {
    let kind: PermissionKind
    let state: PermissionState

    private var granted: Bool { state.isGranted }

    var body: some View {
        HStack(spacing: 12) {
            statusBadge
            Text(kind.rawValue)
                .font(CSFont.ui(13.5, .medium))
                .foregroundStyle(CSColor.textBody)
                .frame(maxWidth: .infinity, alignment: .leading)
            if granted {
                Text("granted")
                    .font(CSFont.mono(11, .semibold))
                    .foregroundStyle(CSColor.oliveLight)
            } else {
                Button {
                    kind.openSystemSettings()
                } label: {
                    Text("open System Settings")
                        .font(CSFont.mono(11, .semibold))
                        .foregroundStyle(CSColor.terracottaLight)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 15)
        .padding(.vertical, 13)
        .background(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill((granted ? CSColor.olive : CSColor.terracotta).opacity(0.08))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .strokeBorder((granted ? CSColor.olive : CSColor.terracotta).opacity(0.22), lineWidth: 1)
        )
    }

    @ViewBuilder
    private var statusBadge: some View {
        ZStack {
            Circle().fill((granted ? CSColor.olive : CSColor.terracotta).opacity(0.2))
            Text(granted ? "✓" : "!")
                .font(CSFont.ui(11, .semibold))
                .foregroundStyle(granted ? CSColor.oliveLight : CSColor.terracottaLight)
        }
        .frame(width: 20, height: 20)
    }
}

// MARK: - Quick start card

private struct QuickStartCard: View {
    let glyph: String
    let title: String
    let subtitle: String

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(glyph).font(.system(size: 16))
            Text(title)
                .font(CSFont.ui(13, .semibold))
                .foregroundStyle(CSColor.textHigh)
                .padding(.top, 9)
            Text(subtitle)
                .font(CSFont.ui(11.5))
                .lineSpacing(2)
                .foregroundStyle(CSColor.textMutedAlt)
                .padding(.top, 3)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 14)
        .padding(.vertical, 16)
        .background(
            RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
                .fill(CSColor.surfaceRaised(0.025))
        )
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
        )
    }
}

// MARK: - Launchpad chips

private struct LaunchpadChips: View {
    // "Dictation" is the active launchpad (terracotta); the rest are available.
    var body: some View {
        HStack(spacing: 8) {
            chip("Dictation", active: true)
            chip("Formatting", active: false)
            chip("Agent chat", active: false)
            chip("Quick Notes", active: false)
            Spacer(minLength: 0)
        }
    }

    @ViewBuilder
    private func chip(_ text: String, active: Bool) -> some View {
        Text(text)
            .font(CSFont.ui(12, active ? .semibold : .medium))
            .foregroundStyle(active ? CSColor.terracottaLight : Color(hex: 0xC7CABF))
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            .background(
                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .fill(active ? CSColor.terracotta.opacity(0.12) : CSColor.surfaceRaised(0.03))
            )
            .overlay(
                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .strokeBorder(active ? CSColor.terracotta.opacity(0.26) : CSColor.hairline(0.08), lineWidth: 1)
            )
    }
}

#Preview("Creator panel") {
    ScrollView { CreatorPanel(model: .preview) }
        .frame(width: 720, height: 620)
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
