import SwiftUI

// Individual step bodies for the first-run wizard. B3a implements Welcome,
// Permission (reused for all five scopes), ApiKey, and Done for real; the four
// stubbed steps (mode / language / hotkeyMode / agenticReadiness) share
// `OnboardingPlaceholderStepView`. Navigation (Back / Continue / Skip / Finish)
// lives in the footer in OnboardingView.swift — these bodies only render content
// and step-local actions (open settings, save key).

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

// MARK: - Placeholder (stubbed steps — filled in B3b)

/// Shared body for the four steps B3a intentionally stubs. They keep their fixed
/// slots in the flow (stable resume indices) but only show framing copy plus the
/// footer's Continue. B3b replaces each with the real control (mode radios,
/// language picker, hotkey lane, agentic readiness verdict).
struct OnboardingPlaceholderStepView: View {
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

            HStack(spacing: 8) {
                Text("○")
                    .font(CSFont.mono(11, .medium))
                    .foregroundStyle(CSColor.amber)
                Text("Configured later — press Continue for now.")
                    .font(CSFont.mono(11, .medium))
                    .foregroundStyle(CSColor.textFaint)
            }
            .padding(.top, 6)
        }
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
            Image(systemName: granted ? "checkmark.circle.fill" : "circle")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(granted ? CSColor.oliveLight : CSColor.textFaint)
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
