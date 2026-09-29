import SwiftUI

// Root of the first-run wizard: a fixed chrome (progress header + scrollable step
// body + navigation footer) that dispatches on the current step. The window host
// lives in OnboardingWindow.swift; individual step bodies live in
// OnboardingSteps.swift.

struct OnboardingView: View {
  @ObservedObject var model: OnboardingViewModel

  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

  var body: some View {
    content
    .frame(minWidth: 680, minHeight: 560)
    .background {
      if reduceTransparency {
        Color(nsColor: .windowBackgroundColor)
      } else if #available(macOS 26, *) {
        Color.clear.glassEffect(.regular, in: .rect(cornerRadius: 0))
      } else {
        Rectangle().fill(.regularMaterial)
      }
    }
    .csFocusPolicy()
    .controlSize(.regular)
    .onAppear { model.refreshForCurrentStep() }
  }

  private var content: some View {
    VStack(spacing: 0) {
      header
      ScrollView {
        stepBody
          .frame(maxWidth: .infinity, alignment: .leading)
          .padding(28)
          .id(model.stepIndex)
      }
      .animation(reduceMotion ? nil : .easeInOut(duration: 0.2), value: model.stepIndex)
      footer
    }
  }

  private var chapter: (title: String, symbol: String, purpose: String) {
    switch model.step {
    case .welcome, .mode:
      return ("Your voice, a new possibility", "waveform", "First, choose what you want to do.")
    case .permission:
      return ("Make the connection", "hand.raised", "You decide what Codescribe can access.")
    case .language, .apiKey, .hotkeyMode:
      return (
        "Make it yours", "slider.horizontal.3",
        "Your language. Your shortcuts. Your way of working."
      )
    case .agenticReadiness:
      return ("Give your voice tools", "sparkles", "Connect the assistants you want to work with.")
    case .done:
      return (
        "Your next thought starts here", "checkmark",
        "Setup is complete. Your voice takes it from here."
      )
    }
  }

  private var header: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack {
        Wordmark(size: 16)
        Spacer()
        Text(model.progressLabel).font(.callout).foregroundStyle(.secondary)
      }
      HStack(spacing: 12) {
        Image(systemName: chapter.symbol)
          .font(.system(size: 17, weight: .medium))
          .frame(width: 34, height: 34)
          .accessibilityHidden(true)
        VStack(alignment: .leading, spacing: 5) {
          Text(chapter.title).font(.headline)
          Text(chapter.purpose).font(.subheadline).foregroundStyle(.secondary)
        }
        Spacer(minLength: 0)
      }
      ProgressView(value: Double(model.stepIndex), total: Double(max(1, model.totalSteps - 1)))
        .controlSize(.small)
        .accessibilityLabel("Setup progress")
    }
    .padding(.horizontal, 28)
    .padding(.top, 24)
    .padding(.bottom, 12)
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
        Button("Back") { model.back() }.csAction()
      }
      Spacer(minLength: 0)
      // The API-key step is skippable — a key can be added later in Settings.
      if case .apiKey = model.step {
        Button("Skip") { model.advance() }.csAction()
      }
      Button(model.primaryLabel) {
        model.primaryAction()
      }.csAction(prominent: true)
    }
    .padding(.horizontal, CSSpace.page)
    .padding(.vertical, 18)
  }
}

#if DEBUG
  #Preview("Onboarding — Welcome") {
    OnboardingView(
      model: OnboardingViewModel(
        engine: MockOnboardingEngine(progress: 0),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted))
    )
    .frame(width: 720, height: 620)
    .preferredColorScheme(.dark)
  }

  #Preview("Onboarding — Mode") {
    OnboardingView(
      model: OnboardingViewModel(
        engine: MockOnboardingEngine(progress: 1),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted))
    )
    .frame(width: 720, height: 620)
    .preferredColorScheme(.dark)
  }

  #Preview("Onboarding — Permission") {
    OnboardingView(
      model: OnboardingViewModel(
        engine: MockOnboardingEngine(progress: 2),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(
          PermissionSnapshot(
            microphone: .denied, accessibility: .granted,
            inputMonitoring: .notDetermined, screenRecording: .denied,
            fullDiskAccess: .notDetermined)))
    )
    .frame(width: 720, height: 620)
    .preferredColorScheme(.dark)
  }

  #Preview("Onboarding — Language") {
    OnboardingView(
      model: OnboardingViewModel(
        engine: MockOnboardingEngine(progress: 8),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted))
    )
    .frame(width: 720, height: 620)
    .preferredColorScheme(.dark)
  }

  #Preview("Onboarding — API key") {
    OnboardingView(
      model: OnboardingViewModel(
        engine: MockOnboardingEngine(progress: 9),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted))
    )
    .frame(width: 720, height: 620)
    .preferredColorScheme(.dark)
  }

  #Preview("Onboarding — Hotkeys") {
    OnboardingView(
      model: OnboardingViewModel(
        engine: MockOnboardingEngine(progress: 10),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted))
    )
    .frame(width: 720, height: 620)
    .preferredColorScheme(.dark)
  }

  #Preview("Onboarding — Agentic readiness") {
    let engine = MockOnboardingEngine(progress: 11)
    engine.mode = "agentic"
    return OnboardingView(
      model: OnboardingViewModel(
        engine: engine,
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted))
    )
    .frame(width: 720, height: 620)
    .preferredColorScheme(.dark)
  }
#endif
