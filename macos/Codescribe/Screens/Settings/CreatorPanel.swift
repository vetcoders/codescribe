import SwiftUI

// Creator setup panel: interface language (the per-app macOS preference the
// setup wizard also writes), live permission checklist, editable voice/formatting
// controls (language, AI formatting, formatting level) written through the core
// router, plus quick-start cards and launchpad chips.
// Permission rows reflect LIVE AVAuthorization / AX / IOHID / CG status.

struct CreatorPanel: View {
  @ObservedObject var model: SettingsViewModel
  @State private var manualSkillClient: AgentBridgeClient?
  @State private var showingSkillLocations = false

  private static let permissionKinds: [PermissionKind] = [
    .microphone, .accessibility, .inputMonitoring, .screenRecording, .speechRecognition,
  ]

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsPageHeader(String(localized: "Get set up."))

      SettingsSectionLabel(
        String(localized: "Languages", comment: "Settings group: app language and speech language")
      )
      .padding(.top, SettingsScale.sectionGap)
      VStack(spacing: 8) {
        InterfaceLanguageRow(model: model)
        RecognitionLanguageRow(selection: languageBinding)
      }
      .padding(.top, SettingsScale.headerGap)

      // The checklist shows live status; the header link opens the privacy
      // pane itself so a granted scope can still be reviewed or revoked.
      SettingsSectionHeaderRow(String(localized: "Permissions")) {
        Button(
          String(
            localized: "System Settings…",
            comment: "Permissions header action: opens the Privacy & Security pane")
        ) { PermissionKind.openPrivacySettings() }
        .accessibilityIdentifier("settings-permissions-system-settings")
      }
      .padding(.top, SettingsScale.sectionGap)
      VStack(spacing: 0) {
        ForEach(Self.permissionKinds) { kind in
          PermissionChecklistRow(
            kind: kind,
            state: model.permissions.state(kind),
            note: model.permissionRepairNote(for: kind),
            onRequested: { model.notePermissionRequested(kind) },
            // A grant can change recording admission, not only this row.
            onStateChanged: { model.refreshDictationReadiness() }
          )
          if kind != Self.permissionKinds.last {
            // Aligned with the row text: inset + badge + the row's own spacing.
            Divider().padding(.leading, 2 * SettingsScale.rowInsetH + 20)
          }
        }
      }
      .settingsGroupedInset(padding: 0)
      .padding(.top, SettingsScale.headerGap)

      SettingsSectionLabel(String(localized: "Voice & formatting"))
        .padding(.top, SettingsScale.sectionGap)
      VStack(spacing: 8) {
        SettingsControlRow(title: String(localized: "AI formatting")) {
          Toggle("", isOn: formattingEnabledBinding)
            .toggleStyle(.switch)
            .labelsHidden()
            .tint(CSColor.chromeAccent)
        }
        SettingsControlRow(title: String(localized: "Formatting level")) {
          Picker("", selection: formattingLevelBinding) {
            ForEach(FormattingPolicyOption.allCases) { policy in
              Text(policy.visibleName).tag(policy.rawValue)
            }
          }
          .pickerStyle(.segmented)
          .labelsHidden()
          .fixedSize()
          .disabled(!model.settings.aiFormattingEnabled)
        }
        if model.maxAgentUnavailable {
          Text(
            String(
              localized: "Max uses the Agent model. Set up the Agent endpoint to use Max.",
              comment: "Shown when the Max level is selected but the Agent lane is unavailable"
            )
          )
          .font(.caption)
          .foregroundStyle(CSColor.terracotta)
          .frame(maxWidth: .infinity, alignment: .leading)
          .accessibilityIdentifier("settings-max-agent-unavailable")
        }
        if model.maxConsultationEnabled || !model.maxToolApprovals.isEmpty {
          MaxConsultationCard(model: model)
            .id(SettingsAnchor.maxConsultation)
        }
      }
      .padding(.top, SettingsScale.headerGap)

      agentBridgeSection
        .padding(.top, SettingsScale.sectionGap)

      SettingsSectionLabel(String(localized: "Quick start"))
        .padding(.top, SettingsScale.sectionGap)
      HStack(spacing: 10) {
        QuickStartCard(
          icon: .mic,
          title: "Test mic",
          accessibilityId: "settings-quickstart-test-mic"
        ) { model.performQuickStart(.testMic) }
        QuickStartCard(
          icon: .overlay,
          title: "Open widget",
          accessibilityId: "settings-quickstart-open-widget"
        ) { model.performQuickStart(.openWidget) }
        QuickStartCard(
          icon: .shortcuts,
          title: "Tune shortcuts",
          accessibilityId: "settings-quickstart-tune-shortcuts"
        ) { model.performQuickStart(.tuneShortcuts) }
      }
      .padding(.top, SettingsScale.headerGap)
    }
    .settingsPage()
    .onAppear { model.refreshCreatorAgentBridge() }
    .confirmationDialog(
      "Replace a manually installed Codescribe skill?",
      isPresented: Binding(
        get: { manualSkillClient != nil },
        set: { if !$0 { manualSkillClient = nil } }
      ),
      presenting: manualSkillClient
    ) { client in
      Button("Preserve original and install for \(client.displayName)") {
        model.adoptCreatorManualSkill(for: client)
        manualSkillClient = nil
      }
      Button("Cancel", role: .cancel) { manualSkillClient = nil }
    } message: { client in
      Text(
        "The Codescribe skill folder for \(client.displayName) will be moved to a retained backup beside it, then replaced with the copy bundled in this app. Your other skills and agent configuration are not changed. No listener will be started."
      )
    }
    .task { await model.refreshMaxToolApprovals() }
  }

  private var agentBridgeSection: some View {
    VStack(alignment: .leading, spacing: SettingsScale.rowGap) {
      SettingsSectionHeaderRow(String(localized: "Connect your coding agent")) {
        SettingsRefreshButton(
          axLabel: "Refresh coding agent skill status",
          action: model.refreshCreatorAgentBridge
        )
        .accessibilityIdentifier("settings-agent-bridge-refresh")
      }
      // One shared card: the heading, statuses and buttons already say what
      // this section does, so the old caption sentence is gone (Founder brief,
      // round 4, 2026-10-10).
      VStack(spacing: 0) {
        ForEach(AgentBridgeClient.allCases) { client in
          agentBridgeRow(client)
          Divider().padding(.leading, SettingsScale.rowInsetH)
        }
        skillLocations
      }
      .settingsGroupedInset(padding: 0)
      if !model.creatorAgentBridgeStatus.payloadAvailable {
        Text(model.creatorAgentBridgeStatus.detail)
          .font(SettingsScale.helper)
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
      if let notice = model.creatorAgentBridgeNotice {
        Text(notice).font(SettingsScale.helper).foregroundStyle(Color.primary).textSelection(.enabled)
      }
      if let error = model.creatorAgentBridgeError {
        Text(error)
          .font(SettingsScale.helper)
          .foregroundStyle(CSColor.terracotta)
          .textSelection(.enabled)
      }
    }
  }

  /// One client: its name, whether the installed skill is the one this app
  /// carries, and the action. "Installed" alone never answered the question
  /// the Update button raises (Founder review, 2026-10-18), so the row states
  /// the generation instead.
  private func agentBridgeRow(_ client: AgentBridgeClient) -> some View {
    let status = model.creatorAgentBridgeStatus
    let installed = status.installedClients.contains(client)
    return HStack(spacing: SettingsScale.rowInsetH) {
      VStack(alignment: .leading, spacing: 2) {
        Text(client.displayName)
          .font(SettingsScale.settingName)
          .foregroundStyle(.primary)
        if installed {
          Text(
            status.clientsWithCurrentSkill.contains(client)
              ? String(
                localized: "creator.agentBridge.clientCurrent", defaultValue: "Up to date",
                comment: "Status on an agent client row: the installed skill is this app's")
              : String(
                localized: "creator.agentBridge.clientOutdated",
                defaultValue: "Update available",
                comment: "Status on an agent client row: this app carries a newer skill")
          )
          .font(SettingsScale.helper)
          .foregroundStyle(.secondary)
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      Button(installed ? "Update skill" : "Install skill") {
        model.installCreatorAgentBridge(for: client)
      }
      .disabled(!status.payloadAvailable)
      .accessibilityIdentifier("settings-agent-bridge-\(client.rawValue)")
      if model.creatorAgentBridgeError != nil {
        Button("Replace manual copy…") { manualSkillClient = client }
          .disabled(!status.payloadAvailable)
          .accessibilityIdentifier("settings-agent-bridge-adopt-\(client.rawValue)")
      }
    }
    .padding(.horizontal, SettingsScale.rowInsetH)
    .padding(.vertical, SettingsScale.controlInsetV)
  }

  /// The folders the three clients read. A path is detail: the rows above name
  /// the client, this says where its skill lands (Founder review, 2026-10-18).
  private var skillLocations: some View {
    DisclosureGroup(isExpanded: $showingSkillLocations) {
      VStack(alignment: .leading, spacing: CSSpace.xxs) {
        ForEach(AgentBridgeClient.allCases) { client in
          Text(verbatim: "\(client.displayName): \(client.skillPathLabel)")
            .font(SettingsScale.metaKey)
            .foregroundStyle(.secondary)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
      }
      .padding(.top, CSSpace.xxs)
    } label: {
      Text(
        String(
          localized: "Skill locations",
          comment: "Disclosure on the coding-agent card: the folders each client reads")
      )
      .font(SettingsScale.settingName)
      .foregroundStyle(.primary)
    }
    .padding(.horizontal, SettingsScale.rowInsetH)
    .padding(.vertical, SettingsScale.controlInsetV)
    .accessibilityIdentifier("settings-agent-bridge-locations")
  }

  // MARK: - Bindings (read VM state, write through the router)

  private var languageBinding: Binding<CsLanguage> {
    Binding(
      get: { model.settings.whisperLanguage },
      set: { model.setLanguage($0) })
  }

  private var formattingEnabledBinding: Binding<Bool> {
    Binding(
      get: { model.settings.aiFormattingEnabled },
      set: { model.setFormattingEnabled($0) })
  }

  private var formattingLevelBinding: Binding<String> {
    Binding(
      get: {
        FormattingPolicyOption(storedValue: model.settings.formattingLevel)?.rawValue
          ?? FormattingPolicyOption.correction.rawValue
      },
      set: { model.setFormattingLevel($0) })
  }
}

/// One card for everything Max: the consultation row with its action, then the
/// approval requests underneath. Two separate blocks read as two features
/// (Founder walkthrough, 2026-10-10).
private struct MaxConsultationCard: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: SettingsScale.rowGap) {
      if model.maxConsultationEnabled {
        HStack(spacing: 12) {
          VStack(alignment: .leading, spacing: 2) {
            Text("Max consultation")
              .font(SettingsScale.settingName)
              .foregroundStyle(.primary)
            Text("Continue the consultation or start a new one. History stays.")
              .font(SettingsScale.helper)
              .foregroundStyle(.secondary)
          }
          .frame(maxWidth: .infinity, alignment: .leading)
          Button(model.newMaxConsultationPending ? "Starting…" : "New consultation") {
            Task { await model.beginNewMaxConsultation() }
          }
          .disabled(model.newMaxConsultationPending)
          .accessibilityIdentifier("settings-new-max-consultation")
        }
        if let notice = model.maxConsultationNotice {
          Text(notice)
            .font(SettingsScale.helper)
            .foregroundStyle(Color.primary)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        Divider()
      }
      MaxApprovalCards(model: model, embedded: true)
    }
    .settingsGroupedInset()
  }
}

/// The same permission cards are used by settings recovery and automatic display.
/// `embedded` draws the heading as a row title inside a host card (Creator);
/// the Agent window keeps the standalone section label.
struct MaxApprovalCards: View {
  @ObservedObject var model: SettingsViewModel
  var embedded: Bool = false

  var body: some View {
    VStack(alignment: .leading, spacing: SettingsScale.rowGap) {
      if embedded {
        HStack(spacing: SettingsScale.rowInsetH) {
          VStack(alignment: .leading, spacing: 2) {
            Text("Max permissions")
              .font(SettingsScale.settingName)
              .foregroundStyle(.primary)
            pendingLine
          }
          .frame(maxWidth: .infinity, alignment: .leading)
          refreshButton
        }
      } else {
        SettingsSectionLabel(String(localized: "Max permissions"))
        pendingLine
        refreshButton
      }
      ForEach(model.maxToolApprovals) { request in
        ToolApprovalCard(
          request: request,
          reject: {
            Task { await model.resolveMaxToolApproval(request, approved: false) }
          },
          allowOnce: {
            Task { await model.resolveMaxToolApproval(request, approved: true) }
          },
          allowAlways: {
            Task { await model.resolveMaxToolApproval(request, approved: true, remember: true) }
          }
        )
        .disabled(model.maxApprovalBusy || model.maxApprovalError != nil)
      }
      if let error = model.maxApprovalError {
        Text(error).foregroundStyle(CSColor.amber)
      }
    }
  }

  /// Whether anything is waiting, next to the heading. The cards below appear
  /// only when a request exists, so without this line an empty section could
  /// not be told apart from one that was never checked (Founder review,
  /// 2026-10-18). A running check is on the button and an unreadable one on
  /// `maxApprovalError`, so neither state repeats itself here. The line is a
  /// yes or no and carries no count: the requests themselves are listed
  /// directly below it, and a counted phrase would owe plural variations in
  /// both catalogs for information already on screen.
  @ViewBuilder private var pendingLine: some View {
    if model.maxApprovalError == nil, !model.maxApprovalBusy {
      Text(
        model.maxToolApprovals.isEmpty
          ? String(
            localized: "max.approvals.none", defaultValue: "No requests waiting",
            comment: "Line under the Max permissions heading: nothing needs a decision")
          : String(
            localized: "max.approvals.waiting", defaultValue: "Requests waiting",
            comment: "Line under the Max permissions heading: the cards below need a decision")
      )
      .font(SettingsScale.helper)
      .foregroundStyle(.secondary)
      .accessibilityIdentifier("settings-max-approvals-pending")
    }
  }

  private var refreshButton: some View {
    // The neighbouring "Max permissions" heading carries the context, so the
    // label stays short (Founder brief, round 4, 2026-10-10).
    Button(model.maxApprovalBusy ? "Checking…" : "Check requests") {
      Task { await model.refreshMaxToolApprovals() }
    }
    .disabled(model.maxApprovalBusy)
  }
}

// MARK: - Language identity

struct LanguageIdentityPresentation: Identifiable, Equatable {
  let language: CsLanguage
  let title: String
  let isFineTuned: Bool

  var id: String { language.shortCode }

  /// Row explanation: one sentence about the only non-obvious choice. The
  /// fine-tuned-models detail moved out of the base view (Founder brief,
  /// round 4, 2026-10-10).
  static let supportingCopy = String(
    localized: "Multilingual detects the language automatically.",
    comment: "Speech recognition language row explanation"
  )

  static let choices: [LanguageIdentityPresentation] = [
    .init(language: .auto, title: String(localized: "Multilingual"), isFineTuned: false),
    .init(language: .polish, title: String(localized: "Polish"), isFineTuned: true),
    .init(language: .english, title: String(localized: "English"), isFineTuned: true),
  ]
}

/// Same shape as the interface-language row directly above it: label, one
/// sentence, segmented control (Founder walkthrough, 2026-10-10).
private struct RecognitionLanguageRow: View {
  @Binding var selection: CsLanguage

  var body: some View {
    SettingsControlRow(
      title: String(
        localized: "Speech recognition language",
        comment: "Settings row: dictation language"),
      subtitle: LanguageIdentityPresentation.supportingCopy
    ) {
      Picker("", selection: $selection) {
        ForEach(LanguageIdentityPresentation.choices) { choice in
          Text(choice.title).tag(choice.language)
        }
      }
      .pickerStyle(.segmented)
      .labelsHidden()
      .fixedSize()
      .accessibilityLabel(
        String(
          localized: "Speech recognition language", comment: "Settings row: dictation language")
      )
      .accessibilityIdentifier("settings-recognition-language")
    }
  }
}

// MARK: - Labeled control row (shared shape for the editable settings rows)

struct SettingsControlRow<Control: View>: View {
  let title: String
  /// Omitted when the title already carries the whole meaning of the row.
  var subtitle: String? = nil
  /// Card inset; dense hosts (the AI-model lane editors) pass a tighter one.
  var inset: CGFloat = SettingsScale.cardPadding
  @ViewBuilder var control: () -> Control

  var body: some View {
    HStack(spacing: 12) {
      VStack(alignment: .leading, spacing: 2) {
        Text(title)
          .font(SettingsScale.settingName)
          .foregroundStyle(.primary)
        if let subtitle {
          Text(subtitle)
            .font(SettingsScale.helper)
            .foregroundStyle(.secondary)
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      control()
    }
    .settingsGroupedInset(padding: inset)
  }
}

// MARK: - Interface language row

/// Picker + restart action for the app language. Mirrors the wizard's first
/// step on the same preference: the choice saves at once, the whole app speaks
/// it after the host's idle-guarded restart, and a busy runtime keeps the
/// choice saved with a retry message instead of relaunching under a take.
private struct InterfaceLanguageRow: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      SettingsControlRow(
        title: String(localized: "Interface language", comment: "Settings row: app language"),
        subtitle: String(
          localized: "Menus, Settings and the Agent window. Dictation has its own language.",
          comment: "Interface language row explanation")
      ) {
        Picker("", selection: selection) {
          ForEach(InterfaceLanguage.allCases, id: \.self) { language in
            Text(language.nativeName).tag(language)
          }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(width: 200)
        .disabled(model.applyingInterfaceLanguage)
        .accessibilityIdentifier("settings-interface-language")
      }
      if model.interfaceLanguageNeedsRestart {
        HStack(spacing: 12) {
          Text(model.interfaceLanguageRestartExplanation)
            .font(SettingsScale.helper)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
          Button(model.interfaceLanguageRestartTitle) { model.applyInterfaceLanguage() }
            .disabled(model.applyingInterfaceLanguage)
            .accessibilityIdentifier("settings-interface-language-restart")
        }
      }
      if let notice = model.interfaceLanguageNotice {
        Text(notice)
          .font(SettingsScale.helper)
          .foregroundStyle(CSColor.terracotta)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityIdentifier("settings-interface-language-notice")
      }
    }
  }

  private var selection: Binding<InterfaceLanguage> {
    Binding(
      get: { model.interfaceLanguage },
      set: { model.selectInterfaceLanguage($0) }
    )
  }
}

// MARK: - Permission checklist row

/// One compact line inside the shared permissions group: status badge, name,
/// and — only while something is missing — the repair action. The five rows
/// share a single bordered card (Founder brief, round 4, 2026-10-10); the
/// per-row olive/terracotta wash lives on in the badge and the action link.
private struct PermissionChecklistRow: View {
  let kind: PermissionKind
  let state: PermissionState
  /// Shown under the name when the row's own button cannot fix the scope.
  var note: String? = nil
  /// Records that this scope was asked for, so a later miss can explain itself.
  var onRequested: (() -> Void)? = nil
  /// Re-probe after an in-app request (Speech Recognition dialog).
  var onStateChanged: (() -> Void)? = nil

  private var granted: Bool { state.isGranted }

  var body: some View {
    HStack(spacing: SettingsScale.rowInsetH) {
      statusBadge
      // A granted row says it with the badge; the status stays for VoiceOver.
      VStack(alignment: .leading, spacing: 2) {
        Text(kind.displayName)
          .font(CSFont.ui(13.5, .medium))
          .foregroundStyle(Color.primary)
          .accessibilityValue(state.label)
        if let note {
          Text(note)
            .font(.caption)
            .foregroundStyle(CSColor.terracotta)
            .fixedSize(horizontal: false, vertical: true)
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      if !granted {
        let repair = kind.repair(for: state)
        if repair.requestsNatively {
          Button {
            onRequested?()
            Task { @MainActor in
              // The return value is not a grant; the following probe is.
              _ = await kind.requestInApp()
              onStateChanged?()
            }
          } label: {
            Text("allow \(kind.displayName)")
              .font(CSFont.ui(11, .semibold))
              .foregroundStyle(CSColor.terracotta)
          }
          .csFocusRing()
        }
        if repair.opensSettings {
          Button {
            onRequested?()
            kind.openSystemSettings()
          } label: {
            Text("open System Settings")
              .font(CSFont.ui(11, .semibold))
              .foregroundStyle(CSColor.terracotta)
          }
          .csFocusRing()
        }
      }
    }
    .padding(.horizontal, SettingsScale.rowInsetH)
    .padding(.vertical, 7)
  }

  @ViewBuilder
  private var statusBadge: some View {
    ZStack {
      Circle().fill((granted ? CSColor.olive : CSColor.terracotta).opacity(0.2))
      CSIconView(
        icon: granted ? .success : .warning,
        size: 11,
        weight: .semibold,
        color: granted ? CSColor.oliveLight : CSColor.terracotta
      )
    }
    .frame(width: 20, height: 20)
  }
}

// MARK: - Quick start card

/// A quick-start card IS a button: every card carries a real action (routing or
/// dictation start). Cards must never render as inert click-bait again —
/// UI_DIVERGENCE_AUDIT pkt 4 called the previous inert tiles out as fake UX,
/// and the duplicate "Launchpads" decoration row below them was removed with it.
private struct QuickStartCard: View {
  let icon: CSIcon
  let title: LocalizedStringKey
  let accessibilityId: String
  let action: () -> Void

  @State private var hovered = false

  var body: some View {
    Button(action: action) {
      VStack(alignment: .leading, spacing: 0) {
        CSIconView(icon: icon, size: 16, color: Color.primary)
        Text(title)
          .font(CSFont.ui(13, .semibold))
          .foregroundStyle(Color.primary)
          .padding(.top, 9)
        Spacer(minLength: 0)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(.horizontal, 14)
      .padding(.vertical, 16)
      .background(
        RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
          .fill(Color.primary.opacity(hovered ? 0.1 : 0.05))
      )
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
      )
      .contentShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))
    }
    .csFocusRing(cornerRadius: CSRadius.card)
    .onHover { hovered = $0 }
    .accessibilityLabel(title)
    .accessibilityIdentifier(accessibilityId)
  }
}

#if DEBUG
  #Preview("Creator panel") {
    ScrollView { CreatorPanel(model: .preview) }
      .frame(width: 720, height: 620)
  }
#endif
