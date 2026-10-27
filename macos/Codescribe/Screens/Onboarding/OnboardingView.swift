import SwiftUI

// Root of the first-run wizard: a fixed chrome (wordmark, step counter and
// progress header + navigation footer) around one scrollable reading column that
// holds every step's heading, category label and body. The window host
// lives in OnboardingWindow.swift; individual step bodies live in
// OnboardingSteps.swift.

struct OnboardingView: View {
  @ObservedObject var model: OnboardingViewModel

  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.colorScheme) private var colorScheme

  @State private var hostWindow: NSWindow?

  var body: some View {
    content
      .frame(minWidth: 680, minHeight: 560)
      .background {
        OverlayCanvasBackdrop(
          palette: OverlayAppearancePalette.resolve(colorScheme),
          reduceTransparency: reduceTransparency
        )
        .overlay {
          // Slightly soften desktop bleed-through in setup, without tinting shared glass.
          CSColor.windowCanvas.opacity(reduceTransparency ? 0 : 0.08)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
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
        model.refreshForCurrentStep()
      }
  }

  /// One reading column for every step: the heading, its category label, the
  /// step's content and its footnotes share this width and one leading edge.
  /// 560 pt keeps the widest real row — a permission with its longest Polish
  /// name, the Optional badge and the "Open System Settings" button — on one
  /// line, and stays under the 624 pt the minimum window width leaves, so no
  /// step is squeezed. The window chrome (wordmark, counter, progress, footer)
  /// keeps the full width.
  static let columnMaxWidth: CGFloat = 560

  private var content: some View {
    VStack(spacing: 0) {
      header
      ScrollView {
        VStack(alignment: .leading, spacing: 20) {
          stepHeading
          stepBody
        }
        .frame(maxWidth: Self.columnMaxWidth, alignment: .leading)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(28)
        .id(model.stepIndex)
        .csAutohidingScrollers()
      }
      .animation(
        reduceMotion ? nil : .easeInOut(duration: CSMotion.Tempo.enter), value: model.stepIndex)
      footer
    }
  }

  /// Each step's single task message (a question or an instruction), its icon
  /// and its short category label. The step bodies never repeat it.
  private var heading: (symbol: String, title: String, label: String) {
    let locale = model.interfaceLocale
    switch model.step {
    case .interfaceLanguage:
      return (
        "globe",
        String(
          localized: LocalizedStringResource(
            "What language should Codescribe use?", locale: locale,
            comment: "First setup step heading")),
        String(
          localized: LocalizedStringResource(
            "Interface language", locale: locale, comment: "First setup step eyebrow"))
      )
    case .mode:
      return (
        "waveform",
        String(
          localized: LocalizedStringResource(
            "How do you want to work?", locale: locale, comment: "Setup step heading")),
        String(
          localized: LocalizedStringResource(
            "Operating lane", locale: locale, comment: "Setup step eyebrow"))
      )
    case .permissions:
      return (
        "hand.raised",
        String(
          localized: LocalizedStringResource(
            "Choose which features to allow", locale: locale,
            comment: "Heading above the unified onboarding permission checklist")),
        String(
          localized: LocalizedStringResource(
            "Permissions", locale: locale, comment: "Setup step eyebrow"))
      )
    case .language:
      return (
        "character.bubble",
        String(
          localized: LocalizedStringResource(
            "Pick your dictation language.", locale: locale, comment: "Setup step heading")),
        String(
          localized: LocalizedStringResource(
            "Dictation language", locale: locale,
            comment: "Setup step eyebrow: the language you dictate in, not the app language"))
      )
    case .localModel:
      return (
        "arrow.down.circle",
        String(
          localized: LocalizedStringResource(
            "Do you want to transcribe on your Mac?", locale: locale,
            comment: "Setup step heading for the optional local Whisper model")),
        String(
          localized: LocalizedStringResource(
            "Local transcription", locale: locale,
            comment: "Setup step eyebrow: transcription that runs on this computer"))
      )
    case .apiKey:
      return (
        "key",
        String(
          localized: LocalizedStringResource(
            "Connect an AI provider", locale: locale, comment: "Setup step heading")),
        String(
          localized: LocalizedStringResource(
            "AI provider", locale: locale, comment: "Setup step eyebrow"))
      )
    case .hotkeyMode:
      return (
        "keyboard",
        String(
          localized: LocalizedStringResource(
            "How do you trigger recording?", locale: locale, comment: "Setup step heading")),
        String(
          localized: LocalizedStringResource(
            "Hotkeys", locale: locale, comment: "Setup step eyebrow"))
      )
    case .agenticReadiness:
      return (
        "sparkles",
        String(
          localized: LocalizedStringResource(
            "Choose the agents you want to work with by voice.", locale: locale,
            comment: "Setup step heading for the coding-agent skill choice")),
        String(
          localized: LocalizedStringResource(
            "Agents", locale: locale, comment: "Setup step eyebrow: coding-agent clients"))
      )
    case .done:
      let summary = String(
        localized: LocalizedStringResource("Summary", locale: locale, comment: "Setup step eyebrow"))
      // The heading states the verdict; it never celebrates ahead of it.
      if model.dictationReadiness.isReady {
        // Finish waits for dictation only. In the Agent lane the whole app is
        // ready only once the Agent step's own verdict is too.
        if model.onboardingMode == .agentic, !model.agentBridgeReadyToGo {
          return (
            "checkmark",
            String(
              localized: LocalizedStringResource(
                "Dictation is ready.", locale: locale,
                comment:
                  "Setup final heading: dictation passed its checks, the Agent lane is not ready yet")),
            summary
          )
        }
        return (
          "checkmark",
          String(
            localized: LocalizedStringResource(
              "Codescribe is ready.", locale: locale, comment: "Setup chapter heading")),
          summary
        )
      }
      if model.dictationReadinessChecking {
        return (
          "hourglass",
          String(
            localized: LocalizedStringResource(
              "Checking your setup…", locale: locale,
              comment: "Setup chapter heading while the recording readiness verdict loads")),
          summary
        )
      }
      return (
        "exclamationmark.circle",
        String(
          localized: LocalizedStringResource(
            "Codescribe is not ready yet.", locale: locale,
            comment: "Setup chapter heading when something required for transcription is missing")),
        summary
      )
    }
  }

  /// Fixed window chrome: wordmark, step counter and progress. It stays put on
  /// every step and owns window dragging.
  private var header: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack {
        Wordmark(size: 16)
        Spacer()
        Text(model.progressLabel)
          .font(CSFont.ui(12, .regular))
          .foregroundStyle(CSColor.textMuted)
      }
      // Progress is state, so it wears the brand hue rather than the macOS
      // accent, which turns grey under Graphite (Founder, 2026-10-26).
      CSProgressBar(value: model.progressFraction)
        .accessibilityLabel("Setup progress")
    }
    .padding(.horizontal, 28)
    .padding(.top, 24)
    .padding(.bottom, 12)
    .background(OnboardingDragRegion())
  }

  /// Top of the reading column: the step icon with its one task message, then
  /// the quieter category label.
  private var stepHeading: some View {
    let current = heading
    return VStack(alignment: .leading, spacing: 10) {
      HStack(alignment: .firstTextBaseline, spacing: 10) {
        Image(systemName: current.symbol)
          .font(.title2.weight(.regular))
          .foregroundStyle(CSColor.brandAccent)
          .accessibilityHidden(true)
        Text(current.title)
          .font(.title2.weight(.semibold))
          .foregroundStyle(.primary)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityAddTraits(.isHeader)
          .accessibilityIdentifier("onboarding-step-heading")
      }
      // The wizard speaks in one family: the section marker keeps its
      // uppercase and tracking in the system face, and reads in the muted
      // grey so the step's colour belongs to its icon (Founder, 2026-10-26).
      EyebrowLabel(text: current.label, color: CSColor.textMuted, font: CSFont.ui(11, .semibold))
    }
  }

  // MARK: - Step dispatch

  @ViewBuilder private var stepBody: some View {
    switch model.step {
    case .interfaceLanguage:
      InterfaceLanguageStepView(model: model)
    case .mode:
      ModeStepView(model: model)
    case .permissions:
      PermissionsStepView(model: model)
    case .language:
      LanguageStepView(model: model)
    case .localModel:
      LocalModelStepView(model: model, downloads: model.whisperDownloadStore)
    case .apiKey:
      ApiKeyStepView(model: model)
    case .hotkeyMode:
      HotkeyModeStepView(model: model)
    case .agenticReadiness:
      AgenticReadinessStepView(model: model)
    case .done:
      DoneStepView(model: model, downloads: model.whisperDownloadStore)
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
      }
      .csAction(prominent: true)
      // Finish writes `setup_done`; it waits for a ready verdict.
      .disabled(model.isDone && !model.canFinish)
      .accessibilityIdentifier("onboarding-primary")
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
        engine: MockOnboardingEngine(progress: 3),
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
        engine: MockOnboardingEngine(progress: 5),
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
        engine: MockOnboardingEngine(progress: 6),
        hotkeys: MockHotkeysEngine(),
        agentStatus: MockAgentStatusEngine(),
        probe: MockPermissionProbe(.allGranted))
    )
    .frame(width: 720, height: 620)
    .preferredColorScheme(.dark)
  }

  #Preview("Onboarding — Agentic readiness") {
    let engine = MockOnboardingEngine(progress: 7)
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
