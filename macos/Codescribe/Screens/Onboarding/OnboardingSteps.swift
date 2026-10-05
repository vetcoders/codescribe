import Foundation
import SwiftUI

// Individual step bodies for the first-run wizard, backed by the shared
// config, hotkeys, permissions, and agent-status seams.
// Navigation (Back / Continue / Skip / Finish) lives in the footer in
// OnboardingView.swift — these bodies only render content and step-local actions.

// MARK: - Interface language

struct InterfaceLanguageStepView: View {
  @ObservedObject var model: OnboardingViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 20) {
      OnboardingStepHeader(
        eyebrow: String(
          localized: LocalizedStringResource(
            "Interface language", locale: model.interfaceLocale, comment: "First setup step eyebrow"
          )),
        title: String(
          localized: LocalizedStringResource(
            "What language should Codescribe use?", locale: model.interfaceLocale,
            comment: "First setup step heading")),
        blurb: String(
          localized: LocalizedStringResource(
            "This changes the app’s interface. You’ll choose your dictation language later.",
            locale: model.interfaceLocale,
            comment: "Interface language is independent of speech recognition"))
      )
      ForEach(InterfaceLanguage.allCases, id: \.self) { language in
        OnboardingChoiceCard(
          title: language.nativeName,
          subtitle: nil,
          isSelected: model.interfaceLanguage == language
        ) { model.selectInterfaceLanguage(language) }
        .accessibilityIdentifier("onboarding-interface-language-\(language.rawValue)")
      }
      .disabled(model.applyingInterfaceLanguage)
      if model.interfaceLanguageNeedsRestart {
        Text(
          String(
            localized: LocalizedStringResource(
              "Codescribe will restart in this language and resume setup. Your recording must finish first.",
              locale: model.interfaceLocale, comment: "Whole-app language application explanation"))
        )
        .font(.callout)
        .foregroundStyle(.secondary)
      }
      if let error = model.lastError {
        Text(error).font(.callout).foregroundStyle(CSColor.terracotta)
          .accessibilityIdentifier("onboarding-interface-language-error")
      }
    }
  }
}

// MARK: - Step scaffold + selectable choice card (shared by Mode / Language / Hotkey)

/// Shared heading (eyebrow + title + blurb) for the choice steps, matching the
/// permission-step typography.
private struct OnboardingStepHeader: View {
  let eyebrow: String
  let title: String
  let blurb: String

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      EyebrowLabel(text: eyebrow)
      Text(title)
        .font(.title2.weight(.semibold))
        .foregroundStyle(.primary)
        .fixedSize(horizontal: false, vertical: true)
      Text(blurb)
        .font(.body)
        .lineSpacing(3)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
    }
  }
}

/// A single radio-style selectable card: title + optional subtitle, with a
/// Brand-colored checkmark when selected. Reused by the choice steps.
struct OnboardingChoiceCard: View {
  let title: String
  let subtitle: String?
  let isSelected: Bool
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      HStack(alignment: .top, spacing: 12) {
        Image(systemName: isSelected ? "checkmark.circle.fill" : "circle")
          .foregroundStyle(isSelected ? CSColor.terracotta : Color.secondary)
        VStack(alignment: .leading, spacing: 3) {
          Text(title).font(.body.weight(.semibold))
          if let subtitle {
            Text(subtitle)
              .font(.callout)
              .foregroundStyle(.secondary)
              .fixedSize(horizontal: false, vertical: true)
          }
        }
        Spacer(minLength: 0)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(6)
    }
    .csAction()
    .accessibilityAddTraits(isSelected ? .isSelected : [])
  }
}

/// A shared note line ("full editing lives in Settings") under a choice step.
private struct OnboardingStepNote: View {
  let text: String

  var body: some View {
    Text(text)
      .font(.callout)
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
        eyebrow: String(
          localized: LocalizedStringResource(
            "Operating lane", locale: model.interfaceLocale, comment: "Setup step eyebrow")),
        title: String(
          localized: LocalizedStringResource(
            "Where should your words go?", locale: model.interfaceLocale,
            comment: "Setup step heading")),
        blurb: String(
          localized: LocalizedStringResource(
            "Start with dictation, or bring an assistant into the conversation. Change this any time in Settings.",
            locale: model.interfaceLocale,
            comment: "Setup step blurb"))
      )

      VStack(spacing: 10) {
        OnboardingChoiceCard(
          title: String(
            localized: LocalizedStringResource(
              "Basic — dictation only", locale: model.interfaceLocale,
              comment: "Operating lane choice; Basic is the lane name")),
          subtitle: String(
            localized: LocalizedStringResource(
              "Voice-to-text anywhere. The simplest, fastest setup.", locale: model.interfaceLocale,
              comment: "Operating lane choice detail")),
          isSelected: model.onboardingMode == .basic
        ) { model.selectMode(.basic) }

        OnboardingChoiceCard(
          title: String(
            localized: LocalizedStringResource(
              "Agentic — dictation + AI agent", locale: model.interfaceLocale,
              comment: "Operating lane choice; Agentic is the lane name")),
          subtitle: String(
            localized: LocalizedStringResource(
              "Talk with an AI assistant and connect its tools, so your voice can drive an AI assistant, not just type.",
              locale: model.interfaceLocale,
              comment: "Operating lane choice detail")),
          isSelected: model.onboardingMode == .agentic
        ) { model.selectMode(.agentic) }
      }
      .padding(.top, 4)

      OnboardingStepNote(
        text: String(
          localized: LocalizedStringResource(
            "Agentic adds one more setup step (readiness check). Basic skips it.",
            locale: model.interfaceLocale,
            comment: "Setup step footnote; Agentic and Basic are the two lane names")))
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
        eyebrow: String(
          localized: LocalizedStringResource(
            "Language", locale: model.interfaceLocale, comment: "Setup step eyebrow")),
        title: String(
          localized: LocalizedStringResource(
            "Pick your dictation language.", locale: model.interfaceLocale,
            comment: "Setup step heading")),
        blurb: String(
          localized: LocalizedStringResource(
            "Sets the transcription language. Auto-detect handles mixed or multilingual speech. Change it any time in Settings.",
            locale: model.interfaceLocale,
            comment: "Setup step blurb; Auto-detect is the name of the first language choice")))

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
    case .auto:
      return String(
        localized: LocalizedStringResource(
          "Auto-detect", locale: model.interfaceLocale,
          comment: "Dictation language choice: let the engine detect the language"))
    case .english:
      return String(
        localized: LocalizedStringResource(
          "English", locale: model.interfaceLocale, comment: "Dictation language choice"))
    case .polish:
      return String(
        localized: LocalizedStringResource(
          "Polish", locale: model.interfaceLocale, comment: "Dictation language choice"))
    }
  }

  private func languageSubtitle(_ language: CsLanguage) -> String? {
    switch language {
    case .auto:
      return String(
        localized: LocalizedStringResource(
          "Multilingual — detects the language as you speak.", locale: model.interfaceLocale,
          comment: "Detail under the Auto-detect dictation language choice"))
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
        eyebrow: String(
          localized: LocalizedStringResource(
            "Hotkeys", locale: model.interfaceLocale, comment: "Setup step eyebrow")),
        title: String(
          localized: LocalizedStringResource(
            "How do you trigger recording?", locale: model.interfaceLocale,
            comment: "Setup step heading")),
        blurb: String(
          localized: LocalizedStringResource(
            "Pick a starting preset. This sets the Dictation, Formatting, and Assistive shortcuts for you.",
            locale: model.interfaceLocale,
            comment: "Setup step blurb; Dictation, Formatting and Assistive are the three modes")))

      VStack(spacing: 10) {
        ForEach(HotkeyModeChoice.allCases, id: \.self) { mode in
          OnboardingChoiceCard(
            title: mode.label(locale: model.interfaceLocale),
            subtitle: mode.summary(locale: model.interfaceLocale),
            isSelected: model.hotkeyMode == mode
          ) { model.selectHotkeyMode(mode) }
        }
      }
      .padding(.top, 4)

      OnboardingStepNote(
        text: String(
          localized: LocalizedStringResource(
            "Fine-tune the exact keys later in Settings › Shortcuts.",
            locale: model.interfaceLocale,
            comment: "Setup step footnote; Settings › Shortcuts is a navigation path in the app")))
    }
  }
}

// MARK: - Agentic readiness (agentic lane only)

struct AgenticReadinessStepView: View {
  @ObservedObject var model: OnboardingViewModel
  // macOS 14+ action to open the app's Settings scene. This accessory /
  // LSUIElement app has no responder for the private `showSettingsWindow:`
  // selector, so the SwiftUI environment action is the only reliable open path
  // (matching TrayMenuView / AgentChatView). The Settings scene activates the
  // app and orders its window front, above the wizard.
  @Environment(\.openWindow) private var openWindow

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      agentBridgeSetup
      DisclosureGroup("Connection details") {
        VStack(alignment: .leading, spacing: 12) {
          Text(model.agentBridgeExplanation)
            .font(.callout)
            .foregroundStyle(.secondary)
          Text(model.agentBridgeStatus.detail)
            .font(.callout)
            .foregroundStyle(.secondary)
          ForEach(model.agentBridgeStatus.installedPaths, id: \.self) { path in
            Text(path.replacingOccurrences(of: NSHomeDirectory(), with: "~"))
              .font(.caption.monospaced())
              .textSelection(.enabled)
          }
          if model.providerAccessResolved, model.providerAccessError == nil,
            let readiness = model.readiness
          {
            SettingsSectionLabel(
              String(
                localized: LocalizedStringResource("Agent readiness", locale: model.interfaceLocale)
              ))
            readinessPill(ready: readiness.ready)
            Text(
              "Agent readiness covers Assistive access and native tools. Cloud Formatting is configured separately in Settings › Agent › LLM lanes."
            )
            .font(.callout)
            .foregroundStyle(.secondary)
            // Core orders verdict, provider, native tools and workspace roots first.
            // Optional MCP has its own status report below.
            statusCard(rows: Array(readiness.rows.prefix(4)), valueLineLimit: nil)
              .accessibilityIdentifier("onboarding-agent-readiness-core-status")
          }
          Text(model.providerAccessDescription)
            .font(.callout)
            .foregroundStyle(.secondary)
          if let mcpStatus = model.mcpStatus {
            SettingsSectionLabel(
              String(
                localized: LocalizedStringResource("MCP servers", locale: model.interfaceLocale)))
            statusCard(rows: mcpStatus.rows)
              .accessibilityIdentifier("onboarding-mcp-status")
          }
          Button("Refresh") { model.refreshReadiness() }.csAction()
        }.padding(.top, 8)
      }

      Text(
        "MCP connects your assistant to additional tools. You can add servers later in Settings."
      )
      .font(.callout)
      .foregroundStyle(.secondary)
      Button("MCP settings…") {
        model.prepareMcpSettingsDeepLink()
        openWindow(id: SettingsView.windowID)
      }.csAction()
        .accessibilityIdentifier("onboarding-mcp-settings")

      OnboardingStepNote(
        text: String(
          localized: LocalizedStringResource(
            "This connection is optional. You can continue and set it up later.",
            locale: model.interfaceLocale,
            comment: "Setup step footnote on the agent-readiness step")))
    }
  }

  /// Product install for the external named-session bridge. The checkboxes are
  /// deliberately empty on first run; visiting this step performs no writes.
  /// Reopening Setup seeds clients from the managed receipt for an explicit
  /// reinstall/update or a safe deselection.
  private var agentBridgeSetup: some View {
    VStack(alignment: .leading, spacing: 10) {
      Text("Coding assistants", comment: "Setup: eyebrow above the coding-assistant choices")
        .textCase(.uppercase)
        .font(CSFont.mono(10, .semibold))
        .tracking(0.4)
        .foregroundStyle(CSColor.textFaint)
      Text(model.agentBridgeTitle)
        .font(CSFont.ui(15, .bold))
        .foregroundStyle(.primary)
      Text(
        "Choose where to send your dictation. Your assistant can listen as you speak; changes wait until you finish."
      )
      .font(CSFont.ui(12.5))
      .lineSpacing(3)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)

      VStack(spacing: 8) {
        ForEach(AgentBridgeClient.allCases) { client in
          OnboardingChoiceCard(
            title: client.displayName,
            subtitle: String(
              localized: LocalizedStringResource(
                "Connect a live coding session", locale: model.interfaceLocale,
                comment: "Detail under a coding-assistant checkbox")),
            isSelected: model.selectedAgentClients.contains(client)
          ) { model.toggleAgentClient(client) }
        }
      }

      HStack(spacing: 10) {
        Button(model.agentBridgeButtonTitle) {
          model.installAgentBridge()
        }.csAction(prominent: true)
          .disabled(
            model.selectedAgentClients.isEmpty || !model.agentBridgeStatus.payloadAvailable
          )
      }

      if let error = model.agentBridgeError {
        Text(error)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.terracottaLight)
          .fixedSize(horizontal: false, vertical: true)
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  private func readinessPill(ready: Bool) -> some View {
    let accent = ready ? CSColor.olive : CSColor.terracotta
    let accentLight = ready ? CSColor.oliveLight : CSColor.terracottaLight
    return Text(
      ready
        ? String(
          localized: LocalizedStringResource(
            "Agent capabilities ready", locale: model.interfaceLocale))
        : String(
          localized: LocalizedStringResource(
            "Agent capabilities not ready", locale: model.interfaceLocale))
    )
    .textCase(.uppercase)
    .font(CSFont.mono(9, .semibold))
    .tracking(0.4)
    .foregroundStyle(accentLight)
    .padding(.horizontal, 8)
    .padding(.vertical, 2)
    .background(
      RoundedRectangle(cornerRadius: 6, style: .continuous)
        .fill(accent.opacity(0.12))
    )
    .overlay(
      RoundedRectangle(cornerRadius: 6, style: .continuous)
        .strokeBorder(accent.opacity(0.24), lineWidth: 1))
  }

  @ViewBuilder
  private func statusCard(rows: [CsMcpStatusRow], valueLineLimit: Int? = 2) -> some View {
    VStack(spacing: 0) {
      ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
        if index > 0 {
          Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
        }
        HStack(spacing: 12) {
          Text(row.label)
            .font(CSFont.mono(11.5, .medium))
            .foregroundStyle(.secondary)
            .frame(width: 150, alignment: .leading)
          Text(row.value)
            .font(CSFont.ui(12, .semibold))
            .foregroundStyle(.primary)
            .lineLimit(valueLineLimit)
            .fixedSize(horizontal: false, vertical: valueLineLimit == nil)
            .frame(maxWidth: .infinity, alignment: .leading)
          Circle().fill(row.tone.dotColor).frame(width: 7, height: 7)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
      }
    }
  }
}

// MARK: - Permission (mic → … → speech → full-disk)

struct PermissionStepView: View {
  let kind: PermissionKind
  @ObservedObject var model: OnboardingViewModel

  private var state: PermissionState { model.permissions.state(kind) }

  /// Primary CTA mirrors Settings matrix: in-app request while undetermined
  /// (when the scope supports it), System Settings deep-link once determined.
  private var primaryTitle: String {
    if state == .notDetermined, kind.supportsInAppPermissionRequest {
      return String(
        localized: LocalizedStringResource(
          "Allow \(kind.displayName(locale: model.interfaceLocale))", locale: model.interfaceLocale,
          comment: "Button on a permission step; %@ is a privacy scope such as Microphone"))
    }
    return String(
      localized: LocalizedStringResource(
        "Open System Settings", locale: model.interfaceLocale,
        comment: "Button that deep-links into the macOS System Settings privacy pane"))
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      EyebrowLabel(
        text: String(
          localized: LocalizedStringResource(
            "Permission · \(kind.displayName(locale: model.interfaceLocale))",
            locale: model.interfaceLocale,
            comment: "Eyebrow on a permission step; %@ is a privacy scope such as Microphone")))
      Text(kind.onboardingTitle(locale: model.interfaceLocale))
        .font(.title2.weight(.semibold))
        .foregroundStyle(.primary)
        .fixedSize(horizontal: false, vertical: true)
      Text(kind.onboardingReason(locale: model.interfaceLocale))
        .font(.body)
        .lineSpacing(3)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)

      HStack(spacing: 16) {
        statusRow
        Button("Refresh status") { model.refreshPermissions() }
          .csAction()
      }
      .padding(.top, 4)

      if !state.isGranted {
        Button(primaryTitle) { model.grantPermission(for: kind) }
          .csAction(prominent: true)
      }

      if !state.isGranted {
        if kind == .fullDiskAccess {
          Text("Optional — skip it to limit file-aware features only.")
            .font(.callout)
            .foregroundStyle(CSColor.textFaint)
        } else if kind == .speechRecognition {
          Text(
            "Required for Apple live dictation. Without it Codescribe cannot run on-device Speech."
          )
          .font(.callout)
          .foregroundStyle(CSColor.textFaint)
        } else {
          Text(
            "You can continue without granting this, but the matching feature stays off until you do."
          )
          .font(.callout)
          .foregroundStyle(CSColor.textFaint)
        }
      }
    }
  }

  private var statusRow: some View {
    HStack(spacing: 10) {
      Circle().fill(statusColor.opacity(0.9)).frame(width: 8, height: 8)
      Text(state.label(locale: model.interfaceLocale))
        .font(CSFont.mono(12, .semibold))
        .foregroundStyle(statusColor)
    }

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
  @FocusState private var keyFocused: Bool
  @Environment(\.openWindow) private var openWindow

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      EyebrowLabel(
        text: String(
          localized: LocalizedStringResource(
            "AI provider", locale: model.interfaceLocale, comment: "Setup step eyebrow")))
      Text("Connect an AI provider.")
        .font(.title2.weight(.semibold))
        .foregroundStyle(.primary)
      Text(
        "Account sign-in and API keys are separate ways to connect. Account sign-in supports Assistive; cloud Formatting and model discovery use an API key. Keys are stored in the macOS Keychain and never shown back. You can skip this step and configure access later in Settings › Providers."
      )
      .font(.body)
      .lineSpacing(3)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)

      providerPicker
        .padding(.top, 4)

      if model.providerAccessError != nil {
        Button("Retry provider access") { model.refreshProviderAccess() }
          .disabled(model.providerAccessPending || model.providerMutationPending)
      }
      Text(model.providerAccessDescription)
        .font(.callout)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
      if model.providerAccessResolved, model.providerAccessError == nil,
        model.selectedProviderHasAccountAccess
      {
        HStack {
          Text("Provider account")
          Spacer(minLength: 0)
          Text(
            model.selectedProviderAccountError != nil
              ? String(
                localized: LocalizedStringResource(
                  "Account access unavailable", locale: model.interfaceLocale))
              : model.selectedProviderAccountConnected
                ? String(
                  localized: LocalizedStringResource("connected", locale: model.interfaceLocale))
                : String(
                  localized: LocalizedStringResource("not connected", locale: model.interfaceLocale)
                )
          )
        }
        .font(.callout)
      }
      Button("Manage provider access…") {
        model.prepareProviderSettingsDeepLink()
        openWindow(id: SettingsView.windowID)
      }.csAction()

      keyField
    }
  }

  private var providerPicker: some View {
    HStack(spacing: 12) {
      Text("Provider")
        .font(CSFont.mono(12, .medium))
        .foregroundStyle(.secondary)
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
          .foregroundStyle(.primary)
      }
      .menuStyle(.borderlessButton)
      .disabled(model.providerMutationPending)
      Spacer(minLength: 0)
    }
    .padding(.vertical, 12)
    .overlay(alignment: .bottom) {
      Rectangle().fill(CSColor.hairline(0.08)).frame(height: 1)
    }
  }

  private var keyField: some View {
    let account = model.selectedProvider?.apiKeyAccount ?? "LLM_OPENAI_API_KEY"
    let isSet = model.selectedProviderKeySet
    let isOptional =
      model.selectedProviderAccountConnected
      || model.selectedProvider?.keyRequired == false
    let statusColor =
      !model.providerAccessResolved || model.providerAccessError != nil
      ? CSColor.textFaint
      : isSet
        ? CSColor.oliveLight
        : (isOptional ? CSColor.textFaint : CSColor.terracottaLight)
    return VStack(alignment: .leading, spacing: 10) {
      HStack(spacing: 10) {
        Circle()
          .fill(statusColor.opacity(0.85))
          .frame(width: 7, height: 7)
        Text(SettingsViewModel.keyLabel(for: account))
          .font(CSFont.ui(13.5, .semibold))
          .foregroundStyle(CSColor.textBody)
        Text(account)
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textFaint)
        Spacer(minLength: 0)
        Text(
          !model.providerAccessResolved
            ? String(
              localized: LocalizedStringResource(
                "Checking provider access…", locale: model.interfaceLocale))
            : model.providerAccessError != nil
              ? String(
                localized: LocalizedStringResource(
                  "Provider access unavailable", locale: model.interfaceLocale))
              : isSet
                ? String(localized: LocalizedStringResource("set", locale: model.interfaceLocale))
                : String(
                  localized: LocalizedStringResource("not set", locale: model.interfaceLocale))
        )
        .font(CSFont.mono(10, .semibold))
        .foregroundStyle(statusColor)
      }
      HStack(spacing: 8) {
        SecureField(
          isSet
            ? String(
              localized: LocalizedStringResource("Replace key…", locale: model.interfaceLocale))
            : String(
              localized: LocalizedStringResource("Paste key…", locale: model.interfaceLocale)),
          text: $model.apiKeyDraft
        )
        .focused($keyFocused)
        .settingsInputChrome(isFocused: keyFocused)
        .onSubmit { model.saveApiKey() }
        Button("Save key") { model.saveApiKey() }.csAction(prominent: true)
          .disabled(model.providerMutationPending || !model.apiKeySaveAvailable)
        if model.providerMutationPending { ProgressView().controlSize(.small) }
      }
      if !model.apiKeySaveAvailable,
        !model.apiKeyDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
      {
        Text(
          "This draft is unsaved. Continue with dictation, then go Back in this Setup session to save it once provider access is available."
        )
        .font(.callout)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
      }
    }
    .padding(.vertical, 13)
    .overlay(alignment: .bottom) {
      Rectangle().fill(CSColor.hairline(0.08)).frame(height: 1)
    }
  }
}

// MARK: - Done

struct DoneStepView: View {
  @ObservedObject var model: OnboardingViewModel

  private let summaryOrder: [PermissionKind] = [
    .microphone, .accessibility, .inputMonitoring, .screenRecording,
    .speechRecognition, .fullDiskAccess,
  ]

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      EyebrowLabel(
        text: String(
          localized: LocalizedStringResource(
            "All set", locale: model.interfaceLocale, comment: "Setup step eyebrow")))
      Text("You're ready to talk.")
        .font(.title2.weight(.semibold))
        .foregroundStyle(.primary)
      Text(
        "Press Finish to close setup and start using Codescribe. Anything you skipped is available in Settings."
      )
      .font(.body)
      .lineSpacing(3)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)

      VStack(alignment: .leading, spacing: 8) {
        ForEach(summaryOrder) { kind in
          summaryRow(
            kind.displayName(locale: model.interfaceLocale),
            done: model.permissions.state(kind).isGranted,
            doneLabel: String(
              localized: LocalizedStringResource(
                "granted", locale: model.interfaceLocale,
                comment: "Permission status: this permission is granted")))
        }
        if model.providerAccessResolved, model.providerAccessError == nil {
          summaryRow(
            String(
              localized: LocalizedStringResource(
                "Provider API key", locale: model.interfaceLocale,
                comment: "Summary row: whether an API key is stored for the chosen AI provider")),
            done: model.selectedProviderKeySet,
            doneLabel: String(
              localized: LocalizedStringResource(
                "set", locale: model.interfaceLocale, comment: "Status chip: a value is stored")))
          if model.selectedProviderHasAccountAccess, model.selectedProviderAccountError == nil {
            summaryRow(
              String(
                localized: LocalizedStringResource(
                  "Provider account", locale: model.interfaceLocale)),
              done: model.selectedProviderAccountConnected,
              doneLabel: String(
                localized: LocalizedStringResource("connected", locale: model.interfaceLocale)),
              missingLabel: String(
                localized: LocalizedStringResource("not connected", locale: model.interfaceLocale)))
          }
        }
      }
      .padding(.top, 6)
      if model.providerAccessError != nil {
        Button("Retry provider access") { model.refreshProviderAccess() }
          .disabled(model.providerAccessPending || model.providerMutationPending)
      }
      Text(model.providerAccessDescription)
        .font(.callout)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
    }
  }

  private func summaryRow(
    _ label: String, done: Bool, doneLabel: String,
    missingLabel: String? = nil
  ) -> some View {
    HStack(spacing: 10) {
      CSIconView(
        icon: done ? .checkCircleFill : .circleEmpty,
        size: 12,
        weight: .semibold,
        color: done ? CSColor.oliveLight : CSColor.textFaint
      )
      Text(label)
        .font(CSFont.ui(13))
        .foregroundStyle(CSColor.textBody)
      Spacer(minLength: 0)
      Text(
        done
          ? doneLabel
          : (missingLabel
            ?? String(localized: LocalizedStringResource("optional", locale: model.interfaceLocale)))
      )
      .font(CSFont.mono(10, .semibold))
      .foregroundStyle(done ? CSColor.oliveLight : CSColor.textFaint)
    }
  }
}

// MARK: - Permission onboarding copy (ported from app/ui/onboarding/steps.rs)

extension PermissionKind {
  /// Wizard heading, mirroring the excised AppKit `PermissionKind::title`.
  func onboardingTitle(locale: Locale) -> String {
    switch self {
    case .microphone:
      return String(
        localized: LocalizedStringResource(
          "Microphone Access", locale: locale, comment: "Permission step heading"))
    case .accessibility:
      return String(
        localized: LocalizedStringResource(
          "Accessibility Access", locale: locale, comment: "Permission step heading"))
    case .inputMonitoring:
      return String(
        localized: LocalizedStringResource(
          "Input Monitoring Access", locale: locale, comment: "Permission step heading"))
    case .screenRecording:
      return String(
        localized: LocalizedStringResource(
          "Screen Recording Access", locale: locale, comment: "Permission step heading"))
    case .speechRecognition:
      return String(
        localized: LocalizedStringResource(
          "Speech Recognition Access", locale: locale, comment: "Permission step heading"))
    case .fullDiskAccess:
      return String(
        localized: LocalizedStringResource(
          "Full Disk Access", locale: locale, comment: "Permission step heading"))
    }
  }

  /// Why codescribe needs the scope, mirroring `PermissionKind::reason`.
  func onboardingReason(locale: Locale) -> String {
    switch self {
    case .microphone:
      return String(
        localized: LocalizedStringResource(
          "Transcribe your voice into text. Audio is processed locally on your Mac.",
          locale: locale,
          comment: "Why the app asks for the Microphone scope"))
    case .accessibility:
      return String(
        localized: LocalizedStringResource(
          "Type transcribed text into any application and control text insertion.", locale: locale,
          comment: "Why the app asks for the Accessibility scope"))
    case .inputMonitoring:
      return String(
        localized: LocalizedStringResource(
          "Detect keyboard shortcuts to start and stop voice recording.", locale: locale,
          comment: "Why the app asks for the Input Monitoring scope"))
    case .screenRecording:
      return String(
        localized: LocalizedStringResource(
          "Capture screen context to give the AI assistant visual awareness of what you're working on.",
          locale: locale,
          comment: "Why the app asks for the Screen Recording scope"))
    case .speechRecognition:
      return String(
        localized: LocalizedStringResource(
          "Power Apple live dictation on-device. Speech never leaves your Mac.", locale: locale,
          comment: "Why the app asks for the Speech Recognition scope"))
    case .fullDiskAccess:
      return String(
        localized: LocalizedStringResource(
          "Read project files for AI context. Optional — limits file-aware features if skipped.",
          locale: locale,
          comment: "Why the app asks for the Full Disk Access scope"))
    }
  }
}
