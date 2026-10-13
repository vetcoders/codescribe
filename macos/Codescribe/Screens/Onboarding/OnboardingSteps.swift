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
        blurb: nil
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
      Text(
        String(
          localized: LocalizedStringResource(
            "You’ll choose your dictation language later.", locale: model.interfaceLocale,
            comment: "Interface language is independent of speech recognition"))
      )
      .font(.body)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)
      if model.interfaceLanguageNeedsRestart {
        Text(
          String(
            localized: LocalizedStringResource(
              "Codescribe will restart in this language and resume setup.",
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
  let eyebrow: String?
  let title: String
  let blurb: String?

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      if let eyebrow {
        EyebrowLabel(text: eyebrow)
      }
      Text(title)
        .font(.title2.weight(.semibold))
        .foregroundStyle(.primary)
        .fixedSize(horizontal: false, vertical: true)
      if let blurb {
        Text(blurb)
          .font(.body)
          .lineSpacing(3)
          .foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
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
            "How do you want to work?", locale: model.interfaceLocale,
            comment: "Setup step heading")),
        blurb: nil
      )

      VStack(spacing: 10) {
        OnboardingChoiceCard(
          title: String(
            localized: LocalizedStringResource(
              "Basic — dictation only", locale: model.interfaceLocale,
              comment: "Operating lane choice; Basic is the lane name")),
          subtitle: String(
            localized: LocalizedStringResource(
              "You speak, Codescribe turns it into text.", locale: model.interfaceLocale,
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
              "Dictate, or work by voice with an AI agent.", locale: model.interfaceLocale,
              comment: "Operating lane choice detail")),
          isSelected: model.onboardingMode == .agentic
        ) { model.selectMode(.agentic) }
      }
      .padding(.top, 4)
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
        blurb: nil)

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
          "Auto-detect when you speak several languages.",
          locale: model.interfaceLocale,
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
        blurb: nil)

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
            "You can change the shortcuts later in Settings › Shortcuts.",
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
    VStack(alignment: .leading, spacing: 10) {
      VStack(spacing: 8) {
        ForEach(AgentBridgeClient.allCases) { client in
          VStack(alignment: .leading, spacing: 6) {
            OnboardingChoiceCard(
              title: client.displayName,
              subtitle: String(
                localized: LocalizedStringResource(
                  "Connect to the active session", locale: model.interfaceLocale,
                  comment: "Detail under a coding-assistant checkbox")),
              isSelected: model.selectedAgentClients.contains(client)
            ) { model.toggleAgentClient(client) }

            if model.agentClientNeedsSetup(client) {
              HStack(spacing: 10) {
                Text(
                  String(
                    localized: LocalizedStringResource(
                      "\(client.displayName) needs setup", locale: model.interfaceLocale,
                      comment: "Status below an agent client card; %@ is Codex or Claude Code"))
                )
                .font(CSFont.mono(10.5, .medium))
                .foregroundStyle(CSColor.terracottaLight)
                Spacer(minLength: 0)
                Button(
                  String(
                    localized: LocalizedStringResource(
                      "Set up", locale: model.interfaceLocale,
                      comment: "Button below an agent client card"))
                ) {
                  if model.agentClientIsInstalled(client) {
                    model.prepareAgentDiagnosticsDeepLink()
                    openWindow(id: SettingsView.windowID)
                  } else {
                    model.setUpAgentClient(client)
                  }
                }
                .csAction()
              }
              .padding(.horizontal, 12)
            }

            if model.agentClientShowsError(client), let error = model.agentBridgeError {
              Text(error)
                .font(CSFont.mono(10.5, .medium))
                .foregroundStyle(CSColor.terracottaLight)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 12)
            }
          }
        }
      }

      if model.agentBridgeReadyToGo {
        Text(
          String(
            localized: LocalizedStringResource(
              "Ready to go ✓", locale: model.interfaceLocale,
              comment: "Agent setup status below the client cards"))
        )
        .font(CSFont.mono(10.5, .semibold))
        .foregroundStyle(CSColor.oliveLight)
      } else if model.agentReadinessPending {
        Text(
          String(
            localized: LocalizedStringResource(
              "Checking provider access…", locale: model.interfaceLocale,
              comment: "Setup Agent readiness while the provider snapshot is loading"))
        )
        .font(CSFont.mono(10.5, .medium))
        .foregroundStyle(.secondary)
      } else if model.agentNeedsGlobalSetup {
        HStack(spacing: 10) {
          Text(
            String(
              localized: LocalizedStringResource(
                "Agent needs setup", locale: model.interfaceLocale,
                comment: "Global provider or native readiness problem, not a client installation"))
          )
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.terracottaLight)
          Spacer(minLength: 0)
          Button(
            String(
              localized: LocalizedStringResource(
                "Open diagnostics", locale: model.interfaceLocale,
                comment: "Open Agent diagnostics from the Setup global readiness row"))
          ) {
            model.prepareAgentDiagnosticsDeepLink()
            openWindow(id: SettingsView.windowID)
          }
          .csAction()
        }
      }

      if model.agentBridgeErrorClient == nil, let error = model.agentBridgeError {
        Text(error)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.terracottaLight)
          .fixedSize(horizontal: false, vertical: true)
      }

    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }
}

// MARK: - Permissions

struct PermissionsStepView: View {
  @ObservedObject var model: OnboardingViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      Text(
        String(
          localized: LocalizedStringResource(
            "Choose which features to allow", locale: model.interfaceLocale,
            comment: "Heading above the unified onboarding permission checklist"))
      )
      .font(.title2.weight(.semibold))
      Text(
        String(
          localized: LocalizedStringResource(
            "You can continue without granting these permissions.",
            locale: model.interfaceLocale,
            comment: "Permissions never block Continue")
        )
      )
      .font(.callout)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)
      ForEach(PermissionKind.allCases) { kind in
        OnboardingPermissionRow(kind: kind, model: model)
        Divider()
      }
      Button(
        String(
          localized: LocalizedStringResource(
            "Refresh status", locale: model.interfaceLocale,
            comment: "Refresh all onboarding permission statuses without prompting"))
      ) {
        model.refreshPermissions()
      }
      .csAction()
      OnboardingStepNote(
        text: String(
          localized: LocalizedStringResource(
            "You can change them later in System Settings.", locale: model.interfaceLocale,
            comment: "Setup permissions footnote; System Settings is the macOS application")))
    }
  }
}

private struct OnboardingPermissionRow: View {
  let kind: PermissionKind
  @ObservedObject var model: OnboardingViewModel
  /// The Done chapter lists granted scopes without repeating their reason.
  var compact = false
  private var state: PermissionState { model.permissions.state(kind) }

  private var actionTitle: String {
    let repair = kind.repair(for: state)
    if repair.requestsNatively {
      return String(
        localized: LocalizedStringResource(
          "Allow \(kind.displayName(locale: model.interfaceLocale))", locale: model.interfaceLocale,
          comment: "Permission row action; the placeholder is a macOS privacy scope"))
    }
    return String(
      localized: LocalizedStringResource(
        "Open System Settings", locale: model.interfaceLocale,
        comment: "Open this permission's native macOS privacy pane"))
  }

  var body: some View {
    HStack(alignment: .top, spacing: 12) {
      VStack(alignment: .leading, spacing: 4) {
        HStack(spacing: 8) {
          Text(kind.displayName(locale: model.interfaceLocale)).font(.body.weight(.semibold))
          if kind.isOptionalForSetup {
            Text(
              String(
                localized: LocalizedStringResource(
                  "Optional", locale: model.interfaceLocale,
                  comment: "Screen Recording and Full Disk Access never block setup completion"))
            )
            .font(.caption)
            .foregroundStyle(.secondary)
          }
        }
        if !compact || !state.isGranted {
          Text(kind.onboardingReason(locale: model.interfaceLocale))
            .font(.callout)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
        if let note = model.permissionRepairNote(for: kind) {
          Text(note)
            .font(.callout)
            .foregroundStyle(CSColor.terracottaLight)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("onboarding-permission-note-\(kind.id)")
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      VStack(alignment: .trailing, spacing: 6) {
        Text(state.label(locale: model.interfaceLocale))
          .font(.caption.weight(.medium))
          .foregroundStyle(state.isGranted ? CSColor.oliveLight : Color.secondary)
        if !state.isGranted {
          let repair = kind.repair(for: state)
          if repair.requestsNatively {
            Button(actionTitle) { model.grantPermission(for: kind) }
              .csAction()
              .controlSize(.small)
          }
          if repair.opensSettings {
            Button {
              model.openSystemSettings(for: kind)
            } label: {
              Text(
                String(
                  localized: LocalizedStringResource(
                    "Open System Settings", locale: model.interfaceLocale,
                    comment: "Open this permission's native macOS privacy pane"))
              )
            }
            .csAction()
            .controlSize(.small)
          }
        }
      }
    }
    .padding(.vertical, 4)
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("onboarding-permission-\(kind.id)")
  }
}

// MARK: - Local transcription model

struct LocalModelStepView: View {
  @ObservedObject var model: OnboardingViewModel
  /// Observed here too, so the background-download note appears with the
  /// transfer the shared control starts. The step itself never starts one.
  @ObservedObject var downloads: WhisperDownloadStore

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      Text(
        String(
          localized: LocalizedStringResource(
            "Whisper model for local dictation", locale: model.interfaceLocale,
            comment: "Heading for the model that enables fully local transcription"))
      )
      .font(.title2.weight(.semibold))
      Text(
        String(
          localized: LocalizedStringResource(
            "Download the dedicated Whisper model to transcribe entirely on your computer, without configuring cloud providers.",
            locale: model.interfaceLocale,
            comment:
              "Explain the benefit of the local transcription model"))
      )
      .font(.body)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)
      WhisperDownloadView(store: downloads)
      if model.dictationLane.requiresLocalModel, downloads.readiness != .installed {
        Text(
          String(
            localized: LocalizedStringResource(
              "Local power is selected, so setup finishes once this model is installed.",
              locale: model.interfaceLocale,
              comment: "Local power is the engine name; setup waits for its Whisper model"))
        )
        .font(.callout)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
      }
      if downloads.inFlight {
        Text(
          String(
            localized: LocalizedStringResource(
              "You can move on to the next step. The download finishes in the background.",
              locale: model.interfaceLocale,
              comment:
                "Whisper download never blocks navigation, closing the wizard or completing setup"))
        )
        .font(.callout)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
      }
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
      Text(
        String(
          localized: LocalizedStringResource(
            "Connect an AI provider", locale: model.interfaceLocale,
            comment: "Setup step heading"))
      )
      .font(.title2.weight(.semibold))
      .foregroundStyle(.primary)
      Text(
        String(
          localized: LocalizedStringResource(
            "Set up an Agent account or an API key. You can also do this later.",
            locale: model.interfaceLocale, comment: "Setup step description"))
      )
      .font(.body)
      .lineSpacing(3)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)

      providerPicker
        .padding(.top, 4)

      if let error = model.providerSelectionError {
        providerSelectionError(error)
      }

      if let error = model.providerAccessError {
        inlineError(error)
      }

      if model.selectedProviderHasAccountAccess {
        accountField
      }

      if model.selectedProviderHasApiKeyAccount {
        keyField
      }
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

  private var accountField: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack(spacing: 10) {
        Circle()
          .fill(accountStatusColor.opacity(0.85))
          .frame(width: 7, height: 7)
        Text(
          String(
            localized: LocalizedStringResource(
              "Agent account", locale: model.interfaceLocale,
              comment: "Setup provider account row label"))
        )
        .font(CSFont.ui(13.5, .semibold))
        .foregroundStyle(CSColor.textBody)
        Spacer(minLength: 0)
        Text(model.selectedProviderAccountStatus)
          .font(CSFont.mono(10, .semibold))
          .foregroundStyle(accountStatusColor)
        Button(accountActionTitle, action: openProviderSettings)
          .csAction()
          .disabled(model.providerMutationPending)
      }
      if let error = model.selectedProviderAccountError {
        inlineError(error)
      }
    }
    .padding(.vertical, 13)
    .overlay(alignment: .bottom) {
      Rectangle().fill(CSColor.hairline(0.08)).frame(height: 1)
    }
  }

  private var keyField: some View {
    return VStack(alignment: .leading, spacing: 10) {
      HStack(spacing: 10) {
        Circle()
          .fill(keyStatusColor.opacity(0.85))
          .frame(width: 7, height: 7)
        Text(
          String(
            localized: LocalizedStringResource(
              "API key", locale: model.interfaceLocale,
              comment: "Setup provider API key row label"))
        )
        .font(CSFont.ui(13.5, .semibold))
        .foregroundStyle(CSColor.textBody)
        Spacer(minLength: 0)
        Text(model.selectedProviderKeyStatus)
          .font(CSFont.mono(10, .semibold))
          .foregroundStyle(keyStatusColor)
        Button(keyActionTitle) {
          model.beginApiKeyEditing()
          keyFocused = true
        }
        .csAction()
        .disabled(model.providerMutationPending)
      }
      if model.apiKeyEditorExpanded {
        HStack(spacing: 8) {
          SecureField(
            String(
              localized: LocalizedStringResource(
                "Paste key…", locale: model.interfaceLocale,
                comment: "Setup API key editor placeholder")),
            text: $model.apiKeyDraft
          )
          .focused($keyFocused)
          .settingsInputChrome(isFocused: keyFocused)
          .onSubmit { model.saveApiKey() }
          Button(
            String(
              localized: LocalizedStringResource(
                "Save key", locale: model.interfaceLocale,
                comment: "Setup button: save the provider API key"))
          ) { model.saveApiKey() }
          .csAction(prominent: true)
          .disabled(model.providerMutationPending || !canSubmitApiKey)
          if model.providerMutationPending { ProgressView().controlSize(.small) }
        }
        if !model.apiKeySaveAvailable,
          !model.apiKeyDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        {
          Text(
            String(
              localized: LocalizedStringResource(
                "This draft is unsaved. Continue with dictation, then go Back in this Setup session to save it once provider access is available.",
                locale: model.interfaceLocale,
                comment: "Setup API key editor note shown while provider access is unavailable"))
          )
          .font(.callout)
          .foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)
        }
        if let error = model.apiKeySaveError {
          keySaveError(error)
        }
      }
    }
    .padding(.vertical, 13)
    .overlay(alignment: .bottom) {
      Rectangle().fill(CSColor.hairline(0.08)).frame(height: 1)
    }
  }

  private var canSubmitApiKey: Bool {
    model.apiKeySaveAvailable
      && !model.apiKeyDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }

  private var accountStatusColor: Color {
    if model.providerAccessPending || model.providerAccessError != nil
      || model.selectedProviderAccountError != nil
    {
      return CSColor.textFaint
    }
    return model.selectedProviderAccountConnected ? CSColor.oliveLight : CSColor.terracottaLight
  }

  private var keyStatusColor: Color {
    if model.providerAccessPending || model.providerAccessError != nil { return CSColor.textFaint }
    if model.selectedProviderKeySet { return CSColor.oliveLight }
    return model.selectedProviderRequiresApiKey ? CSColor.terracottaLight : CSColor.textFaint
  }

  private var accountActionTitle: String {
    if model.selectedProviderAccountConnected {
      return String(
        localized: LocalizedStringResource(
          "Manage", locale: model.interfaceLocale,
          comment: "Setup account and API key row action once connected or stored"))
    }
    return String(
      localized: LocalizedStringResource(
        "Connect", locale: model.interfaceLocale,
        comment: "Setup Agent account row action for a disconnected account"))
  }

  private var keyActionTitle: String {
    if model.selectedProviderKeySet {
      return String(
        localized: LocalizedStringResource(
          "Manage", locale: model.interfaceLocale,
          comment: "Setup account and API key row action once connected or stored"))
    }
    return String(
      localized: LocalizedStringResource(
        "Add", locale: model.interfaceLocale,
        comment: "Setup API key row action when no key is stored"))
  }

  private func openProviderSettings() {
    model.prepareProviderSettingsDeepLink()
    openWindow(id: SettingsView.windowID)
  }

  private func inlineError(_ message: String) -> some View {
    HStack(spacing: 8) {
      Text(message)
        .font(.callout)
        .foregroundStyle(CSColor.terracottaLight)
        .lineLimit(1)
        .help(message)
      Spacer(minLength: 0)
      Button(
        String(
          localized: LocalizedStringResource(
            "Try again", locale: model.interfaceLocale,
            comment: "Setup provider access retry button"))
      ) { model.refreshProviderAccess() }
      .csAction()
      .disabled(model.providerAccessPending || model.providerMutationPending)
    }
  }

  private func providerSelectionError(_ message: String) -> some View {
    HStack(spacing: 8) {
      Text(message)
        .font(.callout)
        .foregroundStyle(CSColor.terracottaLight)
        .lineLimit(1)
        .help(message)
      Spacer(minLength: 0)
      Button(
        String(
          localized: LocalizedStringResource(
            "Try again", locale: model.interfaceLocale,
            comment: "Setup provider selection retry button"))
      ) { model.retryProviderSelection() }
      .csAction()
      .disabled(model.providerMutationPending)
    }
  }

  private func keySaveError(_ message: String) -> some View {
    HStack(spacing: 8) {
      Text(message)
        .font(.callout)
        .foregroundStyle(CSColor.terracottaLight)
        .lineLimit(1)
        .help(message)
      Spacer(minLength: 0)
      Button(
        String(
          localized: LocalizedStringResource(
            "Try again", locale: model.interfaceLocale,
            comment: "Setup API key save retry button"))
      ) { model.saveApiKey() }
      .csAction()
      .disabled(model.providerMutationPending || !canSubmitApiKey)
    }
  }
}

// MARK: - Done

/// The setup gate. The required rows are the inputs of `DictationReadiness`
/// for the selected lane, each with its repair in place; Finish stays off
/// until all of them are ready. Optional rows never block Finish.
struct DoneStepView: View {
  @ObservedObject var model: OnboardingViewModel
  /// Observed so the Whisper row repaints with the shared transfer.
  @ObservedObject var downloads: WhisperDownloadStore
  @Environment(\.openWindow) private var openWindow

  var body: some View {
    let readiness = model.dictationReadiness
    VStack(alignment: .leading, spacing: 16) {
      EyebrowLabel(
        text: String(
          localized: LocalizedStringResource(
            "Summary", locale: model.interfaceLocale, comment: "Setup step eyebrow")))
      Text(
        String(
          localized: LocalizedStringResource(
            "Review your setup.", locale: model.interfaceLocale,
            comment: "Review setup choices without claiming recording readiness"))
      )
      .font(.title2.weight(.semibold))
      .foregroundStyle(.primary)
      Text(summary(readiness))
        .font(.body)
        .lineSpacing(3)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
        .accessibilityIdentifier("onboarding-done-summary")

      requiredSection(readiness)
        .padding(.top, 6)
      optionalSection
        .padding(.top, 6)

      if !readiness.isReady {
        Text(
          String(
            localized: LocalizedStringResource(
              "You can close this window. Setup resumes here the next time Codescribe starts.",
              locale: model.interfaceLocale,
              comment: "Setup Done chapter while not ready: closing keeps setup unfinished"))
        )
        .font(.callout)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
      }
      OnboardingStepNote(
        text: String(
          localized: LocalizedStringResource(
            "Anything you skipped is available later in Settings.",
            locale: model.interfaceLocale, comment: "Setup completion footnote")))
    }
  }

  private func summary(_ readiness: DictationReadiness) -> String {
    guard readiness.isReady else {
      return String(
        localized: LocalizedStringResource(
          "Finish unlocks once each required item below is ready.",
          locale: model.interfaceLocale,
          comment: "Setup summary while something required for transcription is missing"))
    }
    let lane = readiness.lane.displayName(locale: model.interfaceLocale)
    return String(
      localized: LocalizedStringResource(
        "Everything is ready for transcription (\(lane)).", locale: model.interfaceLocale,
        comment: "Setup summary when ready; the placeholder is the dictation engine name"))
  }

  // MARK: Required

  private func requiredSection(_ readiness: DictationReadiness) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      sectionLabel(
        String(
          localized: LocalizedStringResource(
            "Required for transcription", locale: model.interfaceLocale,
            comment: "Setup Done group: what Finish waits for")))
      ForEach(PermissionKind.allCases.filter(\.isRequiredForSetup)) { kind in
        OnboardingPermissionRow(kind: kind, model: model, compact: true)
        Divider()
      }
      if readiness.lane.requiresLocalModel {
        localModelRow
        Divider()
      }
      recordingRow(readiness)
    }
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("onboarding-done-required")
  }

  /// Local power only: the shared download control, plus the deliberate way
  /// out — changing the lane in Settings. Setup never switches it itself.
  private var localModelRow: some View {
    VStack(alignment: .leading, spacing: 8) {
      WhisperDownloadView(store: downloads)
      if downloads.readiness != .installed {
        HStack(spacing: 10) {
          Text(
            String(
              localized: LocalizedStringResource(
                "Local power is selected, so setup finishes once this model is installed.",
                locale: model.interfaceLocale,
                comment: "Local power is the engine name; setup waits for its Whisper model"))
          )
          .font(.callout)
          .foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)
          Spacer(minLength: 0)
          Button(
            String(
              localized: LocalizedStringResource(
                "Change engine in Settings…", locale: model.interfaceLocale,
                comment: "Open Settings › Dictation to choose another transcription engine"))
          ) {
            model.prepareDictationSettingsDeepLink()
            openWindow(id: SettingsView.windowID)
          }
          .csAction()
        }
      }
    }
    .accessibilityIdentifier("onboarding-done-local-model")
  }

  /// The controller's admission verdict: the check that runs before a
  /// microphone opens. Calibration is repaired here, through the shared
  /// recorder; anything else links to Settings › Audio.
  private func recordingRow(_ readiness: DictationReadiness) -> some View {
    let blocker = readiness.admissionBlocker
    let state = recordingState(blocker)
    return VStack(alignment: .leading, spacing: 8) {
      HStack(alignment: .top, spacing: 10) {
        CSIconView(
          icon: state.done ? .checkCircleFill : .circleEmpty,
          size: 12,
          weight: .semibold,
          color: state.done ? CSColor.oliveLight : CSColor.textFaint
        )
        .padding(.top, 3)
        VStack(alignment: .leading, spacing: 3) {
          Text(
            String(
              localized: LocalizedStringResource(
                "Recording readiness", locale: model.interfaceLocale))
          )
          .font(.body.weight(.semibold))
          if let detail = state.detail {
            Text(verbatim: detail)
              .font(.callout)
              .foregroundStyle(.secondary)
              .fixedSize(horizontal: false, vertical: true)
              .textSelection(.enabled)
          }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        Text(state.status)
          .font(.caption.weight(.medium))
          .foregroundStyle(state.tint)
          .multilineTextAlignment(.trailing)
      }
      if model.calibrationPending, let startedAt = model.calibrationStartedAt {
        calibrationProgress(startedAt: startedAt)
      }
      if let error = model.calibrationError {
        Text(verbatim: error)
          .font(.callout)
          .foregroundStyle(CSColor.terracottaLight)
          .fixedSize(horizontal: false, vertical: true)
          .textSelection(.enabled)
      }
      recordingAction(blocker)
    }
    .padding(.vertical, 4)
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("onboarding-done-recording")
  }

  private struct RecordingRowState {
    let done: Bool
    let blocked: Bool
    let status: String
    let detail: String?

    var tint: Color {
      if done { return CSColor.oliveLight }
      return blocked ? CSColor.terracottaLight : Color.secondary
    }
  }

  private func recordingState(_ blocker: DictationReadiness.Blocker?) -> RecordingRowState {
    let locale = model.interfaceLocale
    if !model.permissions.microphone.isGranted, blocker == nil || blocker?.isCalibration == true {
      // The probe already names the microphone above; nothing can be measured without it.
      return RecordingRowState(
        done: false, blocked: true,
        status: String(
          localized: LocalizedStringResource(
            "Calibration waits for microphone access", locale: locale)),
        detail: nil)
    }
    switch blocker {
    case nil:
      return RecordingRowState(
        done: true, blocked: false,
        status: String(localized: LocalizedStringResource("Microphone calibrated", locale: locale)),
        detail: model.admission?.deviceName)
    case .admission(let code, let message):
      let status: String
      if code == "admission_calibration_no_profile" {
        status = String(
          localized: LocalizedStringResource(
            "No calibration for the current microphone", locale: locale))
      } else {
        // An unknown code keeps the controller's own sentence, never the code.
        status = AdmissionRefusalCopy.headline(code: code, fallback: message, locale: locale)
      }
      return RecordingRowState(done: false, blocked: true, status: status, detail: nil)
    case .admissionUnknown(let error?):
      return RecordingRowState(
        done: false, blocked: true,
        status: String(
          localized: LocalizedStringResource(
            "Recording readiness could not be checked.", locale: locale,
            comment: "Setup could not read the recording admission verdict")),
        detail: error)
    case .admissionUnknown(nil), .permission, .localModel:
      return RecordingRowState(
        done: false, blocked: false,
        status: String(localized: LocalizedStringResource("Checking…", locale: locale)),
        detail: nil)
    }
  }

  @ViewBuilder
  private func recordingAction(_ blocker: DictationReadiness.Blocker?) -> some View {
    switch blocker {
    case .some(let refusal) where refusal.isCalibration:
      if model.permissions.microphone.isGranted {
        calibrationAction
      }
    case .some(.admission):
      Button(
        String(
          localized: LocalizedStringResource(
            "Open Audio settings", locale: model.interfaceLocale,
            comment: "Open Settings › Audio from setup to repair recording readiness"))
      ) {
        model.prepareAudioSettingsDeepLink()
        openWindow(id: SettingsView.windowID)
      }
      .csAction()
    case .some(.admissionUnknown(.some(_))):
      Button(
        String(
          localized: LocalizedStringResource(
            "Check again", locale: model.interfaceLocale,
            comment: "Setup: read the recording readiness verdict again"))
      ) { model.refreshAdmission() }
      .csAction()
      .disabled(model.admissionPending)
    default:
      EmptyView()
    }
  }

  /// Shown only with microphone access: the instruction and the one action
  /// that clears a calibration refusal.
  private var calibrationAction: some View {
    HStack(spacing: 10) {
      Text(
        String(
          localized: LocalizedStringResource(
            "Speak normally for about ten seconds while Codescribe measures your microphone. The audio is not kept.",
            locale: model.interfaceLocale,
            comment: "Setup calibration instruction shown with the Calibrate microphone button"))
      )
      .font(.callout)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)
      Spacer(minLength: 0)
      Button(calibrationTitle) { model.calibrateMicrophone() }
      .csAction(prominent: true)
      .disabled(!model.calibrationAvailable)
      .accessibilityIdentifier("onboarding-done-calibrate")
    }
  }

  private var calibrationTitle: String {
    let locale = model.interfaceLocale
    return model.calibrationPending
      ? String(localized: LocalizedStringResource("Measuring…", locale: locale))
      : String(localized: LocalizedStringResource("Calibrate microphone", locale: locale))
  }

  private func calibrationProgress(startedAt: Date) -> some View {
    TimelineView(.periodic(from: startedAt, by: 0.2)) { context in
      let elapsed = context.date.timeIntervalSince(startedAt)
      HStack(spacing: 10) {
        ProgressView(value: SettingsViewModel.calibrationProgress(elapsedSeconds: elapsed))
          .progressViewStyle(.linear)
          .tint(CSColor.chromeAccent)
        Text(
          String(
            localized: LocalizedStringResource(
              "\(SettingsViewModel.calibrationRemainingSeconds(elapsedSeconds: elapsed)) s left",
              locale: model.interfaceLocale))
        )
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(.secondary)
      }
    }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(
      String(
        localized: LocalizedStringResource(
          "Calibration capture progress", locale: model.interfaceLocale)))
  }

  // MARK: Optional

  private var optionalSection: some View {
    VStack(alignment: .leading, spacing: 8) {
      sectionLabel(
        String(
          localized: LocalizedStringResource(
            "Optional features", locale: model.interfaceLocale,
            comment: "Setup Done group: items that never block Finish")))
      ForEach(PermissionKind.allCases.filter(\.isOptionalForSetup)) { kind in
        summaryRow(
          kind.displayName(locale: model.interfaceLocale),
          done: model.permissions.state(kind).isGranted,
          doneLabel: model.permissions.state(kind).label(locale: model.interfaceLocale),
          missingLabel: String(
            localized: LocalizedStringResource(
              "Optional", locale: model.interfaceLocale,
              comment: "Optional scope skipped in setup")))
      }
      if model.selectedProviderRequiresApiKey {
        summaryRow(
          String(
            localized: LocalizedStringResource(
              "Provider API key", locale: model.interfaceLocale,
              comment: "Summary row: whether an API key is stored for the chosen AI provider")),
          done: model.providerAccessResolved && !model.providerAccessPending
            && model.providerAccessError == nil && model.selectedProviderKeySet,
          doneLabel: model.selectedProviderKeyStatus,
          statusLabel: model.selectedProviderKeyStatus)
      }
      if model.selectedProviderHasAccountAccess {
        summaryRow(
          String(
            localized: LocalizedStringResource(
              "Agent account", locale: model.interfaceLocale,
              comment: "Summary row: provider account used by Agent features")),
          done: model.providerAccessResolved && !model.providerAccessPending
            && model.providerAccessError == nil && model.selectedProviderAccountError == nil
            && model.selectedProviderAccountConnected,
          doneLabel: model.selectedProviderAccountStatus,
          statusLabel: model.selectedProviderAccountStatus)
      }
    }
  }

  private func sectionLabel(_ text: String) -> some View {
    Text(text)
      .font(.subheadline.weight(.semibold))
      .foregroundStyle(.secondary)
  }

  private func summaryRow(
    _ label: String, done: Bool, doneLabel: String,
    missingLabel: String? = nil, statusLabel: String? = nil
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
        statusLabel
          ?? (done
            ? doneLabel
            : (missingLabel
              ?? String(
                localized: LocalizedStringResource(
                  "Optional", locale: model.interfaceLocale, comment: "Status chip"))))
      )
      .font(CSFont.mono(10, .semibold))
      .foregroundStyle(done ? CSColor.oliveLight : CSColor.textFaint)
    }
  }
}

// MARK: - Permission onboarding copy (ported from app/ui/onboarding/steps.rs)

extension PermissionKind {
  /// Why codescribe needs the scope, mirroring `PermissionKind::reason`.
  func onboardingReason(locale: Locale) -> String {
    switch self {
    case .microphone:
      return String(
        localized: LocalizedStringResource(
          "Without it, voice recording is unavailable.",
          locale: locale,
          comment: "Consequence of a missing Microphone scope"))
    case .accessibility:
      return String(
        localized: LocalizedStringResource(
          "Without it, automatic text insertion in other apps is unavailable.",
          locale: locale,
          comment: "Consequence of a missing Accessibility scope"))
    case .inputMonitoring:
      return String(
        localized: LocalizedStringResource(
          "Without it, global recording shortcuts are unavailable.",
          locale: locale,
          comment: "Consequence of a missing Input Monitoring scope"))
    case .screenRecording:
      return String(
        localized: LocalizedStringResource(
          "Optional — without it, the Agent cannot use your screen as context; dictation still works.",
          locale: locale,
          comment: "Consequence of a missing Screen Recording scope"))
    case .speechRecognition:
      return String(
        localized: LocalizedStringResource(
          "Without it, Apple dictation is unavailable; downloaded Whisper can still transcribe locally.",
          locale: locale,
          comment: "Consequence of a missing Speech Recognition scope"))
    case .fullDiskAccess:
      return String(
        localized: LocalizedStringResource(
          "Optional — without it, protected files stay unavailable to the Agent as context; dictation still works.",
          locale: locale,
          comment: "Consequence of a missing Full Disk Access scope"))
    }
  }
}
