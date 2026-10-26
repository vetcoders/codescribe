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
    VStack(alignment: .leading, spacing: 10) {
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
      // Both notes under the picker carry the same weight, so one text style
      // for both: the quieter secondary callout.
      .font(.callout)
      .foregroundStyle(CSColor.secondaryInk)
      .fixedSize(horizontal: false, vertical: true)
      if model.interfaceLanguageNeedsRestart {
        Text(
          String(
            localized: LocalizedStringResource(
              "Codescribe will restart in this language and resume setup.",
              locale: model.interfaceLocale, comment: "Whole-app language application explanation"))
        )
        .font(.callout)
        .foregroundStyle(CSColor.secondaryInk)
        .fixedSize(horizontal: false, vertical: true)
      }
      if let error = model.lastError {
        Text(error).font(.callout).foregroundStyle(CSColor.dangerLight)
          .accessibilityIdentifier("onboarding-interface-language-error")
      }
    }
  }
}

// MARK: - Shared step components
//
// The heading, the category label and the reading column belong to the frame
// in OnboardingView.swift; these components fill that column.

/// A single radio-style selectable card: title + optional subtitle, with a
/// Brand-colored checkmark and the prominent card surface when selected.
/// Reused by every choice step.
struct OnboardingChoiceCard: View {
  let title: String
  let subtitle: String?
  let isSelected: Bool
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      HStack(alignment: .top, spacing: 12) {
        Image(systemName: isSelected ? "checkmark.circle.fill" : "circle")
          .foregroundStyle(isSelected ? CSColor.terracottaText : CSColor.secondaryInk)
        VStack(alignment: .leading, spacing: 3) {
          Text(title).font(.body.weight(.semibold))
          if let subtitle {
            Text(subtitle)
              .font(.callout)
              .foregroundStyle(CSColor.secondaryInk)
              .fixedSize(horizontal: false, vertical: true)
          }
        }
        Spacer(minLength: 0)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(6)
    }
    .csAction(prominent: isSelected)
    .accessibilityAddTraits(isSelected ? .isSelected : [])
  }
}

/// A static content card with the choice cards' surface and padding, for a
/// block that belongs together but is not a choice (the Whisper model).
private struct OnboardingCard<Content: View>: View {
  @Environment(\.displayScale) private var displayScale
  private let content: Content

  init(@ViewBuilder content: () -> Content) {
    self.content = content()
  }

  var body: some View {
    content
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(.horizontal, 20)
      .padding(.vertical, 14)
      .background(CSColor.chromeAccent.opacity(0.035), in: .rect(cornerRadius: CSRadius.chip))
      .overlay {
        RoundedRectangle(cornerRadius: CSRadius.chip)
          .strokeBorder(CSColor.chromeAccent.opacity(0.22), lineWidth: 1 / max(displayScale, 1))
      }
  }
}

/// A shared note line ("full editing lives in Settings") under a choice step.
private struct OnboardingStepNote: View {
  let text: String

  var body: some View {
    Text(text)
      .font(.callout)
      .foregroundStyle(CSColor.secondaryInk)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, 4)
  }
}

// MARK: - Mode (Basic vs Agentic operating lane)

struct ModeStepView: View {
  @ObservedObject var model: OnboardingViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      VStack(spacing: 10) {
        OnboardingChoiceCard(
          title: String(
            localized: LocalizedStringResource(
              "Dictation only", locale: model.interfaceLocale,
              comment: "Operating lane choice: dictation without the AI agent")),
          subtitle: String(
            localized: LocalizedStringResource(
              "You speak, Codescribe turns it into text.", locale: model.interfaceLocale,
              comment: "Operating lane choice detail")),
          isSelected: model.onboardingMode == .basic
        ) { model.selectMode(.basic) }

        OnboardingChoiceCard(
          title: String(
            localized: LocalizedStringResource(
              "Dictation + AI agent", locale: model.interfaceLocale,
              comment: "Operating lane choice: dictation plus the AI agent")),
          subtitle: String(
            localized: LocalizedStringResource(
              "Dictate, or talk to an AI agent.", locale: model.interfaceLocale,
              comment: "Operating lane choice detail")),
          isSelected: model.onboardingMode == .agentic
        ) { model.selectMode(.agentic) }
      }
    }
  }
}

// MARK: - Language (dictation language)

struct LanguageStepView: View {
  @ObservedObject var model: OnboardingViewModel

  private let choices: [CsLanguage] = [.auto, .english, .polish]

  var body: some View {
    VStack(spacing: 10) {
      ForEach(choices, id: \.self) { language in
        OnboardingChoiceCard(
          title: languageTitle(language),
          subtitle: nil,
          isSelected: model.selectedLanguage == language
        ) { model.selectLanguage(language) }
      }
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
}

// MARK: - Hotkeys (how recording starts)

/// How recording starts. One description instead of presets: Formatting and
/// the Agent start from the menu bar, and the step writes no binding.
struct HotkeyModeStepView: View {
  @ObservedObject var model: OnboardingViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      OnboardingCard {
        VStack(alignment: .leading, spacing: 6) {
          Text(model.dictationStartLine)
            .font(.body.weight(.semibold))
          Text(
            String(
              localized: LocalizedStringResource(
                "Start Formatting and the Agent from the menu bar.",
                locale: model.interfaceLocale,
                comment: "Setup step: Formatting and the Agent have no keyboard shortcut"))
          )
          .font(.callout)
          .foregroundStyle(CSColor.secondaryInk)
          .fixedSize(horizontal: false, vertical: true)
        }
      }

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
              subtitle: skillCaption(for: client),
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
                .font(CSFont.mono(11, .medium))
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
                .font(CSFont.mono(11, .medium))
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
        .font(CSFont.mono(11, .semibold))
        .foregroundStyle(CSColor.oliveLight)
      } else if model.agentReadinessPending {
        Text(
          String(
            localized: LocalizedStringResource(
              "Checking provider access…", locale: model.interfaceLocale,
              comment: "Setup Agent readiness while the provider snapshot is loading"))
        )
        .font(CSFont.mono(11, .medium))
        .foregroundStyle(CSColor.secondaryInk)
      } else if model.agentNeedsGlobalSetup {
        HStack(spacing: 10) {
          Text(
            String(
              localized: LocalizedStringResource(
                "Agent needs setup", locale: model.interfaceLocale,
                comment: "Global provider or native readiness problem, not a client installation"))
          )
          .font(CSFont.mono(11, .medium))
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
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.terracottaLight)
          .fixedSize(horizontal: false, vertical: true)
      }

    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  /// What a card stands for: the Codescribe skill for that client, written to
  /// its skills folder on Continue (or Set up). Selecting a card never joins a
  /// running agent session; a conversation connects later, when it invokes
  /// the skill. So the caption names the installation, never a connection.
  private func skillCaption(for client: AgentBridgeClient) -> String {
    if model.agentClientIsInstalled(client) {
      return String(
        localized: LocalizedStringResource(
          "Codescribe skill installed", locale: model.interfaceLocale,
          comment:
            "Detail under a coding-agent card: the Codescribe skill is in this client's skills folder"
        ))
    }
    return String(
      localized: LocalizedStringResource(
        "Install the Codescribe skill", locale: model.interfaceLocale,
        comment:
          "Detail under a coding-agent card: Continue installs the Codescribe skill for this client"
      ))
  }
}

// MARK: - Permissions

struct PermissionsStepView: View {
  @ObservedObject var model: OnboardingViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      // Settings › Creator › Permissions repeats this checklist with the same
      // request and System Settings actions (Full Disk Access through its
      // System Settings… link), and no grant blocks Continue, so each can wait.
      Text(
        String(
          localized: LocalizedStringResource(
            "You can do this later in Settings.",
            locale: model.interfaceLocale,
            comment: "Setup permissions step: grants can be given later in the app's Settings")
        )
      )
      .font(.callout)
      .foregroundStyle(CSColor.secondaryInk)
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
            .foregroundStyle(CSColor.secondaryInk)
          }
        }
        if !compact || !state.isGranted {
          Text(kind.onboardingReason(locale: model.interfaceLocale))
            .font(.callout)
            .foregroundStyle(CSColor.secondaryInk)
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
          .foregroundStyle(state.isGranted ? CSColor.oliveLight : CSColor.secondaryInk)
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
            "Download the dedicated Whisper model to transcribe entirely on your computer, without configuring cloud providers.",
            locale: model.interfaceLocale,
            comment:
              "Explain the benefit of the local transcription model"))
      )
      .font(.body)
      .foregroundStyle(CSColor.secondaryInk)
      .fixedSize(horizontal: false, vertical: true)
      // One card holds the model: name and install state, source and size,
      // location, and the download control in whatever state it is in.
      OnboardingCard {
        WhisperDownloadView(store: downloads)
      }
      .padding(.top, 4)
      if model.dictationLane.requiresLocalModel, downloads.readiness != .installed {
        Text(
          String(
            localized: LocalizedStringResource(
              "Local transcription needs this model. Install it to finish setup, or choose a cloud engine in Settings.",
              locale: model.interfaceLocale,
              comment:
                "Explain the current local-model requirement without attributing an engine choice to the user"
            ))
        )
        .font(.callout)
        .foregroundStyle(CSColor.secondaryInk)
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
        .foregroundStyle(CSColor.secondaryInk)
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
      if let description = accessDescription {
        Text(description)
          .font(.body)
          .lineSpacing(3)
          .foregroundStyle(CSColor.secondaryInk)
          .fixedSize(horizontal: false, vertical: true)
      }

      providerPicker

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
        .foregroundStyle(CSColor.secondaryInk)
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
          .font(CSFont.ui(13, .semibold))
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
        .font(CSFont.ui(14, .semibold))
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
        .font(CSFont.ui(14, .semibold))
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
          .foregroundStyle(CSColor.secondaryInk)
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

  /// The sentence names only the access paths the selected provider really
  /// has. An account is an alternative to the key only where account
  /// credentials serve requests: Responses vendors with an account row
  /// (OpenAI, xAI), the same test the Providers card uses. Anthropic speaks
  /// Messages, so its account row, shown with a manual client id, never
  /// replaces the key.
  private var accessDescription: String? {
    guard model.selectedProviderHasApiKeyAccount else { return nil }
    let accountServesRequests =
      model.selectedProviderHasAccountAccess && model.selectedProvider?.wire == "responses"
    if accountServesRequests {
      return String(
        localized: LocalizedStringResource(
          "One is enough: sign in with a provider account or add an API key. You can also do this later.",
          locale: model.interfaceLocale,
          comment: "Setup step description; the account and the key are alternatives"))
    }
    if model.selectedProviderRequiresApiKey {
      return String(
        localized: LocalizedStringResource(
          "Add an API key for this provider. You can also do this later.",
          locale: model.interfaceLocale,
          comment: "Setup step description for a provider whose only access path is an API key"))
    }
    return String(
      localized: LocalizedStringResource(
        "This provider doesn’t require an API key. Add one if your server expects it.",
        locale: model.interfaceLocale,
        comment: "Setup step description for a key-optional provider such as a local server"))
  }

  private var canSubmitApiKey: Bool {
    model.apiKeySaveAvailable
      && !model.apiKeyDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }

  /// These rows report stored identity only. The existing Settings lane Test
  /// measures service acceptance; setup must not paint presence as success.
  private var accountStatusColor: Color { CSColor.secondaryInk }
  private var keyStatusColor: Color { CSColor.secondaryInk }

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
      // The step heading already states the verdict; only a blocked one needs
      // a sentence here.
      if let summary = blockedSummary(readiness) {
        Text(summary)
          .font(.body)
          .lineSpacing(3)
          .foregroundStyle(CSColor.secondaryInk)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityIdentifier("onboarding-done-summary")
      }

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
        .foregroundStyle(CSColor.secondaryInk)
        .fixedSize(horizontal: false, vertical: true)
      }
      OnboardingStepNote(
        text: String(
          localized: LocalizedStringResource(
            "Anything you skipped is available later in Settings.",
            locale: model.interfaceLocale, comment: "Setup completion footnote")))
    }
  }

  private func blockedSummary(_ readiness: DictationReadiness) -> String? {
    guard !readiness.isReady else { return nil }
    return String(
      localized: LocalizedStringResource(
        "Finish unlocks once each required item below is ready.",
        locale: model.interfaceLocale,
        comment: "Setup summary while something required for transcription is missing"))
  }

  // MARK: Required

  private func requiredSection(_ readiness: DictationReadiness) -> some View {
    // No group label: each row already carries its own state, and Finish says
    // what it waits for.
    VStack(alignment: .leading, spacing: 8) {
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

  /// Local power only: the shared download control in its summary form (name
  /// and state up front, source and path under Details), plus the deliberate
  /// way out — changing the lane in Settings. Setup never switches it itself.
  private var localModelRow: some View {
    VStack(alignment: .leading, spacing: 8) {
      WhisperDownloadView(store: downloads, presentation: .summary)
      if downloads.readiness != .installed {
        HStack(spacing: 10) {
          Text(
            String(
              localized: LocalizedStringResource(
                "Local transcription needs this model. Install it to finish setup, or choose a cloud engine in Settings.",
                locale: model.interfaceLocale,
                comment:
                  "Explain the current local-model requirement without attributing an engine choice to the user"
              ))
          )
          .font(.callout)
          .foregroundStyle(CSColor.secondaryInk)
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
          color: state.done ? CSColor.oliveLight : CSColor.secondaryInk
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
              .foregroundStyle(CSColor.secondaryInk)
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
      return blocked ? CSColor.terracottaLight : CSColor.secondaryInk
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
      .foregroundStyle(CSColor.secondaryInk)
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
        .foregroundStyle(CSColor.secondaryInk)
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
          statusLabel: model.selectedProviderKeyStatus, presenceOnly: true)
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
          statusLabel: model.selectedProviderAccountStatus, presenceOnly: true)
      }
    }
  }

  private func sectionLabel(_ text: String) -> some View {
    Text(text)
      .font(.subheadline.weight(.semibold))
      .foregroundStyle(CSColor.secondaryInk)
  }

  private func summaryRow(
    _ label: String, done: Bool, doneLabel: String,
    missingLabel: String? = nil, statusLabel: String? = nil, presenceOnly: Bool = false
  ) -> some View {
    HStack(spacing: 10) {
      CSIconView(
        icon: done ? .checkCircleFill : .circleEmpty,
        size: 12,
        weight: .semibold,
        color: done && !presenceOnly ? CSColor.oliveLight : CSColor.secondaryInk
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
      .foregroundStyle(done && !presenceOnly ? CSColor.oliveLight : CSColor.secondaryInk)
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
          "Without it, the Agent can't use your screen as context. Dictation still works.",
          locale: locale,
          comment: "Consequence of a missing Screen Recording scope"))
    case .speechRecognition:
      return String(
        localized: LocalizedStringResource(
          "Without it, Apple dictation is unavailable. Local transcription still works.",
          locale: locale,
          comment: "Consequence of a missing Speech Recognition scope"))
    case .fullDiskAccess:
      return String(
        localized: LocalizedStringResource(
          "Without it, protected files stay unavailable to the Agent as context. Dictation still works.",
          locale: locale,
          comment: "Consequence of a missing Full Disk Access scope"))
    }
  }
}
