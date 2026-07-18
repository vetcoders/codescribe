import SwiftUI

// Individual step bodies for the first-run wizard. Welcome, Permission (reused
// for all five scopes), ApiKey, and Done landed in B3a; B3b fills the four
// choice steps (Mode / Language / HotkeyMode / AgenticReadiness) with real
// controls backed by the shared config / hotkeys / agent-status seams.
// Navigation (Back / Continue / Skip / Finish) lives in the footer in
// OnboardingView.swift — these bodies only render content and step-local actions.

// MARK: - Welcome

struct WelcomeStepView: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            EyebrowLabel(text: "Welcome")
            Text("Codescribe turns your voice into text — anywhere.")
                .font(CSFont.ui(28, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
                .fixedSize(horizontal: false, vertical: true)
            Text(
                "This quick setup grants the macOS permissions Codescribe needs, "
                    + "picks your language and hotkeys, and optionally wires up an AI "
                    + "provider. You can change everything later in Settings."
            )
            .font(CSFont.ui(14))
            .lineSpacing(3)
            .foregroundStyle(CSColor.textMutedAlt)
            .fixedSize(horizontal: false, vertical: true)
        }
    }
}

// MARK: - Step scaffold + selectable choice card (shared by Mode / Language / Hotkey)

/// Shared heading (eyebrow + title + blurb) for the choice steps, matching the
/// Welcome/Permission typography.
private struct OnboardingStepHeader: View {
    let eyebrow: String
    let title: String
    let blurb: String

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            EyebrowLabel(text: eyebrow)
            Text(title)
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
                .fixedSize(horizontal: false, vertical: true)
            Text(blurb)
                .font(CSFont.ui(14))
                .lineSpacing(3)
                .foregroundStyle(CSColor.textMutedAlt)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// A single radio-style selectable card: title + optional subtitle, with a
/// terracotta ring + filled dot when selected. Reused by the three choice steps.
struct OnboardingChoiceCard: View {
    let title: String
    let subtitle: String?
    let isSelected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(alignment: .top, spacing: 12) {
                ZStack {
                    Circle()
                        .strokeBorder(
                            isSelected ? CSColor.terracotta.opacity(0.9) : CSColor.hairline(0.18),
                            lineWidth: 1.5
                        )
                        .frame(width: 16, height: 16)
                    if isSelected {
                        Circle().fill(CSColor.terracotta).frame(width: 8, height: 8)
                    }
                }
                .padding(.top, 1)
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(CSFont.ui(13.5, .semibold))
                        .foregroundStyle(CSColor.textHigh)
                    if let subtitle {
                        Text(subtitle)
                            .font(CSFont.ui(12))
                            .lineSpacing(2)
                            .foregroundStyle(CSColor.textMutedAlt)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 15)
            .padding(.vertical, 13)
            .background(
                RoundedRectangle(cornerRadius: 11, style: .continuous)
                    .fill(CSColor.terracotta.opacity(isSelected ? 0.07 : 0))
                    .background(
                        RoundedRectangle(cornerRadius: 11, style: .continuous)
                            .fill(CSColor.surfaceRaised(isSelected ? 0 : 0.03))
                    )
            )
            .overlay(
                RoundedRectangle(cornerRadius: 11, style: .continuous)
                    .strokeBorder(
                        isSelected ? CSColor.terracotta.opacity(0.28) : CSColor.hairline(0.08),
                        lineWidth: 1
                    )
            )
        }
        .buttonStyle(.plain)
    }
}

/// A shared note line ("full editing lives in Settings") under a choice step.
private struct OnboardingStepNote: View {
    let text: String

    var body: some View {
        Text(text)
            .font(CSFont.mono(11, .medium))
            .foregroundStyle(CSColor.textFaint)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.top, 4)
    }
}

// MARK: - Mode (Basic vs Agentic operating lane)

struct ModeStepView: View {
    @ObservedObject var model: OnboardingViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            OnboardingStepHeader(
                eyebrow: "Operating lane",
                title: "Basic or Agentic.",
                blurb: "Choose how codescribe works. You can switch lanes later in Settings.")

            VStack(spacing: 10) {
                OnboardingChoiceCard(
                    title: "Basic — dictation only",
                    subtitle: "Voice-to-text anywhere. The simplest, fastest setup.",
                    isSelected: model.onboardingMode == .basic
                ) { model.selectMode(.basic) }

                OnboardingChoiceCard(
                    title: "Agentic — dictation + AI agent",
                    subtitle: "Unlocks the agent chat and MCP tool substrate, "
                        + "so your voice can drive an AI assistant, not just type.",
                    isSelected: model.onboardingMode == .agentic
                ) { model.selectMode(.agentic) }
            }
            .padding(.top, 4)

            OnboardingStepNote(
                text: "Agentic adds one more setup step (readiness check). Basic skips it.")
        }
    }
}

// MARK: - Language (dictation language)

struct LanguageStepView: View {
    @ObservedObject var model: OnboardingViewModel

    private let choices: [CsLanguage] = [.auto, .english, .polish]

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            OnboardingStepHeader(
                eyebrow: "Language",
                title: "Pick your dictation language.",
                blurb: "Sets the transcription language. Auto-detect handles mixed or "
                    + "multilingual speech. Change it any time in Settings.")

            VStack(spacing: 10) {
                ForEach(choices, id: \.self) { language in
                    OnboardingChoiceCard(
                        title: languageTitle(language),
                        subtitle: languageSubtitle(language),
                        isSelected: model.selectedLanguage == language
                    ) { model.selectLanguage(language) }
                }
            }
            .padding(.top, 4)
        }
    }

    private func languageTitle(_ language: CsLanguage) -> String {
        switch language {
        case .auto: return "Auto-detect"
        case .english: return "English"
        case .polish: return "Polish"
        }
    }

    private func languageSubtitle(_ language: CsLanguage) -> String? {
        switch language {
        case .auto: return "Multilingual — detects the language as you speak."
        default: return nil
        }
    }
}

// MARK: - Hotkey mode (recording-trigger preset)

struct HotkeyModeStepView: View {
    @ObservedObject var model: OnboardingViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            OnboardingStepHeader(
                eyebrow: "Hotkeys",
                title: "How do you trigger recording?",
                blurb: "Pick a starting preset. This sets the Dictation, Formatting, "
                    + "and Assistive shortcuts for you.")

            VStack(spacing: 10) {
                ForEach(HotkeyModeChoice.allCases, id: \.self) { mode in
                    OnboardingChoiceCard(
                        title: mode.label,
                        subtitle: mode.summary,
                        isSelected: model.hotkeyMode == mode
                    ) { model.selectHotkeyMode(mode) }
                }
            }
            .padding(.top, 4)

            OnboardingStepNote(
                text: "Fine-tune the exact keys later in Settings › Shortcuts.")
        }
    }
}

// MARK: - Agentic readiness (agentic lane only — informational)

struct AgenticReadinessStepView: View {
    @ObservedObject var model: OnboardingViewModel
    // macOS 14+ action to open the app's Settings scene. This accessory /
    // LSUIElement app has no responder for the private `showSettingsWindow:`
    // selector, so the SwiftUI environment action is the only reliable open path
    // (matching TrayMenuView / AgentChatView). The Settings scene activates the
    // app and orders its window front, above the wizard.
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                OnboardingStepHeader(
                    eyebrow: "Agentic readiness",
                    title: "Your agentic substrate.",
                    blurb: "A read-only check of what the agent lane needs: an AI "
                        + "provider + key, native tools, and any MCP servers you've wired.")
                Spacer(minLength: 0)
            }

            if let readiness = model.readiness {
                readinessPill(ready: readiness.ready)
                statusCard(rows: readiness.rows)
            }

            if let mcp = model.mcpStatus, mcp.configured {
                Text("MCP servers")
                    .font(CSFont.mono(10, .semibold))
                    .tracking(0.4)
                    .foregroundStyle(CSColor.textFaint)
                    .padding(.top, 4)
                statusCard(rows: mcp.rows)
            } else if !model.mcpSetupDismissed {
                mcpSetupPrompt
                    .padding(.top, 4)
            }

            OnboardingButton(title: "Refresh", kind: .secondary) {
                model.refreshReadiness()
            }
            .padding(.top, 2)

            OnboardingStepNote(
                text: "Informational — press Continue whether or not everything is green.")
        }
    }

    /// Shown on the readiness step when no MCP server is configured yet: a short,
    /// human explainer plus a route into the real setup surface and a no-guilt skip.
    /// Replaces the old dead end where a missing `mcp.json` showed nothing at all.
    private var mcpSetupPrompt: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("MCP servers (optional)")
                .font(CSFont.mono(10, .semibold))
                .tracking(0.4)
                .foregroundStyle(CSColor.textFaint)
            Text("MCP servers give the agent extra tools — things like code search, "
                + "PR review, or web search. It's entirely optional: skip it now and "
                + "wire servers any time from Settings › Engine.")
                .font(CSFont.ui(13))
                .lineSpacing(3)
                .foregroundStyle(CSColor.textMutedAlt)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 10) {
                OnboardingButton(title: "Set up MCP servers", kind: .primary) {
                    model.prepareMcpSettingsDeepLink()
                    openSettings()
                }
                OnboardingButton(title: "Skip for now", kind: .secondary) {
                    model.dismissMcpSetupPrompt()
                }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(CSColor.surfaceRaised(0.02)))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1))
    }

    private func readinessPill(ready: Bool) -> some View {
        let accent = ready ? CSColor.olive : CSColor.terracotta
        let accentLight = ready ? CSColor.oliveLight : CSColor.terracottaLight
        return Text(ready ? "READY" : "NOT READY")
            .font(CSFont.mono(9, .semibold))
            .tracking(0.4)
            .foregroundStyle(accentLight)
            .padding(.horizontal, 8)
            .padding(.vertical, 2)
            .background(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(accent.opacity(0.12)))
            .overlay(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .strokeBorder(accent.opacity(0.24), lineWidth: 1))
    }

    @ViewBuilder
    private func statusCard(rows: [CsMcpStatusRow]) -> some View {
        VStack(spacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
                if index > 0 {
                    Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
                }
                HStack(spacing: 12) {
                    Text(row.label)
                        .font(CSFont.mono(11.5, .medium))
                        .foregroundStyle(CSColor.textMutedAlt)
                        .frame(width: 150, alignment: .leading)
                    Text(row.value)
                        .font(CSFont.ui(12, .semibold))
                        .foregroundStyle(CSColor.textHigh)
                        .lineLimit(2)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Circle().fill(row.tone.dotColor).frame(width: 7, height: 7)
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 11)
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1))
    }
}

// MARK: - Permission (all five scopes)

struct PermissionStepView: View {
    let kind: PermissionKind
    @ObservedObject var model: OnboardingViewModel

    private var state: PermissionState { model.permissions.state(kind) }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            EyebrowLabel(text: "Permission · \(kind.rawValue)")
            Text(kind.onboardingTitle)
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
                .fixedSize(horizontal: false, vertical: true)
            Text(kind.onboardingReason)
                .font(CSFont.ui(14))
                .lineSpacing(3)
                .foregroundStyle(CSColor.textMutedAlt)
                .fixedSize(horizontal: false, vertical: true)

            statusRow
                .padding(.top, 4)

            HStack(spacing: 10) {
                OnboardingButton(title: "Open System Settings", kind: .primary) {
                    model.openSystemSettings(for: kind)
                }
                OnboardingButton(title: "Refresh status", kind: .secondary) {
                    model.refreshPermissions()
                }
            }
            .padding(.top, 4)

            if kind == .fullDiskAccess {
                Text("Optional — skip it to limit file-aware features only.")
                    .font(CSFont.mono(11, .medium))
                    .foregroundStyle(CSColor.textFaint)
            } else {
                Text("You can continue without granting this, but the matching feature stays off until you do.")
                    .font(CSFont.mono(11, .medium))
                    .foregroundStyle(CSColor.textFaint)
            }
        }
    }

    private var statusRow: some View {
        HStack(spacing: 10) {
            Circle().fill(statusColor.opacity(0.9)).frame(width: 8, height: 8)
            Text(state.label)
                .font(CSFont.mono(12, .semibold))
                .foregroundStyle(statusColor)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .fill(CSColor.surfaceRaised(0.03))
        )
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
        )
    }

    private var statusColor: Color {
        switch state {
        case .granted: return CSColor.oliveLight
        case .denied: return CSColor.terracottaLight
        case .notDetermined: return CSColor.textMutedAlt
        }
    }
}

// MARK: - API key

struct ApiKeyStepView: View {
    @ObservedObject var model: OnboardingViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            EyebrowLabel(text: "AI provider")
            Text("Connect an AI provider.")
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
            Text(
                "Powers AI formatting and the agent lane. Stored in the macOS "
                    + "Keychain — write-only, never shown back. Optional: skip and add "
                    + "it later in Settings › Keys."
            )
            .font(CSFont.ui(14))
            .lineSpacing(3)
            .foregroundStyle(CSColor.textMutedAlt)
            .fixedSize(horizontal: false, vertical: true)

            providerPicker
                .padding(.top, 4)

            keyField
        }
    }

    private var providerPicker: some View {
        HStack(spacing: 12) {
            Text("Provider")
                .font(CSFont.mono(12, .medium))
                .foregroundStyle(CSColor.textMutedAlt)
                .frame(width: 72, alignment: .leading)
            Menu {
                ForEach(model.providers, id: \.id) { provider in
                    Button {
                        model.selectProvider(provider.id)
                    } label: {
                        if provider.id == model.selectedProviderId {
                            Label(provider.displayName, systemImage: "checkmark")
                        } else {
                            Text(provider.displayName)
                        }
                    }
                }
            } label: {
                Text(model.selectedProvider?.displayName ?? model.selectedProviderId)
                    .font(CSFont.ui(12.5, .semibold))
                    .foregroundStyle(CSColor.textHigh)
            }
            .menuStyle(.borderlessButton)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 15)
        .padding(.vertical, 12)
        .background(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill(CSColor.surfaceRaised(0.03))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
        )
    }

    private var keyField: some View {
        let account = model.selectedProvider?.apiKeyAccount ?? "LLM_ASSISTIVE_API_KEY"
        let isSet = model.selectedProviderKeySet
        return VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Circle()
                    .fill((isSet ? CSColor.olive : CSColor.terracotta).opacity(0.85))
                    .frame(width: 7, height: 7)
                Text(SettingsViewModel.keyLabel(for: account))
                    .font(CSFont.ui(13.5, .semibold))
                    .foregroundStyle(CSColor.textBody)
                Text(account)
                    .font(CSFont.mono(10, .medium))
                    .foregroundStyle(CSColor.textFaint)
                Spacer(minLength: 0)
                Text(isSet ? "set" : "not set")
                    .font(CSFont.mono(10, .semibold))
                    .foregroundStyle(isSet ? CSColor.oliveLight : CSColor.terracottaLight)
            }
            HStack(spacing: 8) {
                SecureField(isSet ? "Replace key…" : "Paste key…", text: $model.apiKeyDraft)
                    .textFieldStyle(.plain)
                    .font(CSFont.mono(12))
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
                    .onSubmit { model.saveApiKey() }
                OnboardingButton(title: "Save key", kind: .primary) { model.saveApiKey() }
            }
        }
        .padding(.horizontal, 15)
        .padding(.vertical, 13)
        .background(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill((isSet ? CSColor.olive : CSColor.terracotta).opacity(0.06))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .strokeBorder((isSet ? CSColor.olive : CSColor.terracotta).opacity(0.18), lineWidth: 1)
        )
    }
}

// MARK: - Done

struct DoneStepView: View {
    @ObservedObject var model: OnboardingViewModel

    private let summaryOrder: [PermissionKind] = [
        .microphone, .accessibility, .inputMonitoring, .screenRecording, .fullDiskAccess,
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            EyebrowLabel(text: "All set")
            Text("You're ready to talk.")
                .font(CSFont.ui(28, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
            Text("Press Finish to close setup and start using Codescribe. Anything you skipped is available in Settings.")
                .font(CSFont.ui(14))
                .lineSpacing(3)
                .foregroundStyle(CSColor.textMutedAlt)
                .fixedSize(horizontal: false, vertical: true)

            VStack(alignment: .leading, spacing: 8) {
                ForEach(summaryOrder) { kind in
                    summaryRow(kind.rawValue, granted: model.permissions.state(kind).isGranted)
                }
                summaryRow("AI provider key", granted: model.selectedProviderKeySet)
            }
            .padding(.top, 6)
        }
    }

    private func summaryRow(_ label: String, granted: Bool) -> some View {
        HStack(spacing: 10) {
            CSIconView(
                icon: granted ? .checkCircleFill : .circleEmpty,
                size: 12,
                weight: .semibold,
                color: granted ? CSColor.oliveLight : CSColor.textFaint
            )
            Text(label)
                .font(CSFont.ui(13))
                .foregroundStyle(CSColor.textBody)
            Spacer(minLength: 0)
            Text(granted ? "granted" : "optional")
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(granted ? CSColor.oliveLight : CSColor.textFaint)
        }
    }
}

// MARK: - Permission onboarding copy (ported from app/ui/onboarding/steps.rs)

extension PermissionKind {
    /// Wizard heading, mirroring the excised AppKit `PermissionKind::title`.
    var onboardingTitle: String {
        switch self {
        case .microphone: return "Microphone Access"
        case .accessibility: return "Accessibility Access"
        case .inputMonitoring: return "Input Monitoring Access"
        case .screenRecording: return "Screen Recording Access"
        case .fullDiskAccess: return "Full Disk Access"
        }
    }

    /// Why codescribe needs the scope, mirroring `PermissionKind::reason`.
    var onboardingReason: String {
        switch self {
        case .microphone:
            return "Transcribe your voice into text. Audio is processed locally on your Mac."
        case .accessibility:
            return "Type transcribed text into any application and control text insertion."
        case .inputMonitoring:
            return "Detect keyboard shortcuts to start and stop voice recording."
        case .screenRecording:
            return "Capture screen context to give the AI assistant visual awareness of what you're working on."
        case .fullDiskAccess:
            return "Read project files for AI context. Optional — limits file-aware features if skipped."
        }
    }
}
