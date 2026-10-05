import SwiftUI

// Root of the first-run wizard: a fixed chrome (progress header + scrollable step
// body + navigation footer) that dispatches on the current step. The window host
// lives in OnboardingWindow.swift; individual step bodies live in
// OnboardingSteps.swift.

struct OnboardingView: View {
  @ObservedObject var model: OnboardingViewModel

  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

  @State private var hostWindow: NSWindow?

  var body: some View {
    content
      .frame(minWidth: 680, minHeight: 560)
      .background {
        Group {
          if reduceTransparency {
            Color(nsColor: .windowBackgroundColor)
          } else if #available(macOS 26, *) {
            Color.clear.glassEffect(.regular, in: .rect(cornerRadius: 0))
          } else {
            Rectangle().fill(.regularMaterial)
          }
        }
        .ignoresSafeArea()
      }
      .csFocusPolicy()
      .controlSize(.regular)
      .environment(\.locale, model.interfaceLocale)
      .background(OnboardingWindowReader { hostWindow = $0 })
      .onAppear { model.refreshForCurrentStep() }
      .onChange(of: model.interfaceLanguage) { _, _ in
        hostWindow?.title = model.windowTitle
      }
      .onReceive(NotificationCenter.default.publisher(for: NSWindow.didBecomeKeyNotification)) {
        notification in
        guard let window = notification.object as? NSWindow, window === hostWindow else { return }
        model.refreshProviderAccess()
      }
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

  private var chapter: (title: String, symbol: String, purpose: String?) {
    switch model.step {
    case .interfaceLanguage:
      return (
        String(
          localized: LocalizedStringResource(
            "Choose your language", locale: model.interfaceLocale,
            comment: "First setup chapter heading")),
        "globe",
        nil
      )
    case .mode:
      return (
        String(
          localized: LocalizedStringResource(
            "Choose how you want to use Codescribe.", locale: model.interfaceLocale,
            comment: "Setup chapter heading")),
        "waveform",
        nil
      )
    case .permission:
      return (
        String(
          localized: LocalizedStringResource(
            "Make the connection", locale: model.interfaceLocale, comment: "Setup chapter heading")),
        "hand.raised",
        String(
          localized: LocalizedStringResource(
            "You decide what Codescribe can access.", locale: model.interfaceLocale,
            comment: "Setup chapter subtitle; Codescribe is the product name"))
      )
    case .language, .apiKey, .hotkeyMode:
      return (
        String(
          localized: LocalizedStringResource(
            "Make it yours", locale: model.interfaceLocale, comment: "Setup chapter heading")),
        "slider.horizontal.3",
        String(
          localized: LocalizedStringResource(
            "Your language. Your shortcuts. Your way of working.", locale: model.interfaceLocale,
            comment: "Setup chapter subtitle"))
      )
    case .agenticReadiness:
      return (
        String(
          localized: LocalizedStringResource(
            "Give your voice tools", locale: model.interfaceLocale, comment: "Setup chapter heading"
          )),
        "sparkles",
        String(
          localized: LocalizedStringResource(
            "Connect the assistants you want to work with.", locale: model.interfaceLocale,
            comment: "Setup chapter subtitle"))
      )
    case .done:
      return (
        String(
          localized: LocalizedStringResource(
            "Your next thought starts here", locale: model.interfaceLocale,
            comment: "Setup chapter heading")),
        "checkmark",
        String(
          localized: LocalizedStringResource(
            "Setup is complete. Your voice takes it from here.", locale: model.interfaceLocale,
            comment: "Setup chapter subtitle"))
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
          if let purpose = chapter.purpose {
            Text(purpose).font(.subheadline).foregroundStyle(.secondary)
          }
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
    .background(OnboardingDragRegion())
  }

  // MARK: - Step dispatch

  @ViewBuilder private var stepBody: some View {
    switch model.step {
    case .interfaceLanguage:
      InterfaceLanguageStepView(model: model)
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
    .disabled(model.providerMutationPending || model.applyingInterfaceLanguage)
    .padding(.horizontal, CSSpace.page)
    .padding(.vertical, 18)
  }
}

#if DEBUG
  #Preview("Onboarding — Interface language") {
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

/// Bind focus notifications to this wizard's actual native window.
private struct OnboardingWindowReader: NSViewRepresentable {
  let onWindow: (NSWindow?) -> Void

  func makeNSView(context: Context) -> WindowView {
    let view = WindowView()
    view.onWindow = onWindow
    return view
  }

  func updateNSView(_ view: WindowView, context: Context) { view.onWindow = onWindow }

  final class WindowView: NSView {
    var onWindow: ((NSWindow?) -> Void)?
    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      DispatchQueue.main.async { [weak self] in
        guard let self else { return }
        self.onWindow?(self.window)
      }
    }
  }
}
