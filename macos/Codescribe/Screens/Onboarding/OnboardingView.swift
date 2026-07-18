import SwiftUI

// Root of the first-run wizard: a fixed chrome (progress header + scrollable step
// body + navigation footer) that dispatches on the current step. The window host
// lives in OnboardingWindow.swift; individual step bodies live in
// OnboardingSteps.swift.

struct OnboardingView: View {
    @ObservedObject var model: OnboardingViewModel

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(CSColor.hairline(0.08))
            ScrollView {
                stepBody
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 32)
                    .padding(.vertical, 26)
            }
            Divider().overlay(CSColor.hairline(0.08))
            footer
        }
        .frame(minWidth: 680, minHeight: 560)
        .background(SettingsView.windowGradient.ignoresSafeArea())
        .onAppear { model.refreshForCurrentStep() }
    }

    // MARK: - Header (brand + progress)

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                EyebrowLabel(text: "codescribe · setup")
                Spacer()
                Text(model.progressLabel)
                    .font(CSFont.mono(11, .medium))
                    .foregroundStyle(CSColor.textFaint)
            }
            OnboardingProgressBar(current: model.stepIndex, total: model.totalSteps)
        }
        .padding(.horizontal, 32)
        .padding(.top, 22)
        .padding(.bottom, 16)
    }

    // MARK: - Step dispatch

    @ViewBuilder private var stepBody: some View {
        switch model.step {
        case .welcome:
            WelcomeStepView()
        case .mode:
            ModeStepView(model: model)
        case .permission(let kind):
            PermissionStepView(kind: kind, model: model)
        case .language:
            LanguageStepView(model: model)
        case .apiKey:
            ApiKeyStepView(model: model)
        case .hotkeyMode:
            HotkeyModeStepView(model: model)
        case .agenticReadiness:
            AgenticReadinessStepView(model: model)
        case .done:
            DoneStepView(model: model)
        }
    }

    // MARK: - Footer (navigation)

    private var footer: some View {
        HStack(spacing: 10) {
            if model.canGoBack {
                OnboardingButton(title: "Back", kind: .secondary) { model.back() }
            }
            Spacer(minLength: 0)
            // The API-key step is skippable — a key can be added later in Settings.
            if case .apiKey = model.step {
                OnboardingButton(title: "Skip", kind: .secondary) { model.advance() }
            }
            OnboardingButton(title: model.primaryLabel, kind: .primary) {
                model.primaryAction()
            }
        }
        .padding(.horizontal, 32)
        .padding(.vertical, 16)
    }
}

// MARK: - Progress bar

struct OnboardingProgressBar: View {
    let current: Int
    let total: Int

    private var fraction: CGFloat {
        guard total > 1 else { return 1 }
        return CGFloat(current) / CGFloat(total - 1)
    }

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(CSColor.surfaceRaised(0.05))
                Capsule()
                    .fill(CSColor.terracotta.opacity(0.85))
                    .frame(width: max(6, geo.size.width * fraction))
            }
        }
        .frame(height: 4)
    }
}

// MARK: - Buttons

/// Wizard navigation button, matching the Keys panel's accent-on-surface style.
struct OnboardingButton: View {
    enum Kind { case primary, secondary }
    let title: String
    var kind: Kind = .primary
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(CSFont.ui(13, .semibold))
                .foregroundStyle(foreground)
                .padding(.horizontal, 18)
                .padding(.vertical, 9)
                .background(
                    RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                        .fill(fill)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                        .strokeBorder(border, lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
    }

    private var foreground: Color {
        kind == .primary ? CSColor.terracottaLight : CSColor.textMutedAlt
    }
    private var fill: Color {
        kind == .primary ? CSColor.terracotta.opacity(0.16) : CSColor.surfaceRaised(0.03)
    }
    private var border: Color {
        kind == .primary ? CSColor.terracotta.opacity(0.30) : CSColor.hairline(0.08)
    }
}

#if DEBUG
#Preview("Onboarding — Welcome") {
    OnboardingView(model: OnboardingViewModel(
        engine: MockOnboardingEngine(progress: 0),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted)))
        .frame(width: 720, height: 620)
        .preferredColorScheme(.dark)
}

#Preview("Onboarding — Mode") {
    OnboardingView(model: OnboardingViewModel(
        engine: MockOnboardingEngine(progress: 1),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted)))
        .frame(width: 720, height: 620)
        .preferredColorScheme(.dark)
}

#Preview("Onboarding — Permission") {
    OnboardingView(model: OnboardingViewModel(
        engine: MockOnboardingEngine(progress: 2),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(PermissionSnapshot(
            microphone: .denied, accessibility: .granted,
            inputMonitoring: .notDetermined, screenRecording: .denied,
            fullDiskAccess: .notDetermined))))
        .frame(width: 720, height: 620)
        .preferredColorScheme(.dark)
}

#Preview("Onboarding — Language") {
    OnboardingView(model: OnboardingViewModel(
        engine: MockOnboardingEngine(progress: 7),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted)))
        .frame(width: 720, height: 620)
        .preferredColorScheme(.dark)
}

#Preview("Onboarding — API key") {
    OnboardingView(model: OnboardingViewModel(
        engine: MockOnboardingEngine(progress: 8),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted)))
        .frame(width: 720, height: 620)
        .preferredColorScheme(.dark)
}

#Preview("Onboarding — Hotkeys") {
    OnboardingView(model: OnboardingViewModel(
        engine: MockOnboardingEngine(progress: 9),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted)))
        .frame(width: 720, height: 620)
        .preferredColorScheme(.dark)
}

#Preview("Onboarding — Agentic readiness") {
    let engine = MockOnboardingEngine(progress: 10)
    engine.mode = "agentic"
    return OnboardingView(model: OnboardingViewModel(
        engine: engine,
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted)))
        .frame(width: 720, height: 620)
        .preferredColorScheme(.dark)
}
#endif
