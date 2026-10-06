import SwiftUI

// Creator setup panel: live permission checklist + editable voice/formatting
// controls (language, AI formatting, formatting level) written through the core
// router, plus quick-start cards and launchpad chips.
// Permission rows reflect LIVE AVAuthorization / AX / IOHID / CG status.

struct CreatorPanel: View {
  @ObservedObject var model: SettingsViewModel
  @State private var manualSkillClient: AgentBridgeClient?

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsPageHeader(String(localized: "Get set up."))

      SettingsSectionLabel(String(localized: "Permissions"))
        .padding(.top, CSSpace.section)
      VStack(spacing: 8) {
        ForEach([
          PermissionKind.microphone,
          .accessibility,
          .inputMonitoring,
          .screenRecording,
          .speechRecognition,
        ]) { kind in
          PermissionChecklistRow(
            kind: kind,
            state: model.permissions.state(kind),
            onStateChanged: { model.refreshPermissions() }
          )
        }
      }
      .padding(.top, CSSpace.control)

      SettingsSectionLabel(String(localized: "Voice & formatting"))
        .padding(.top, CSSpace.section)
      VStack(spacing: 8) {
        LanguageIdentityRow(selection: languageBinding)
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
          .frame(width: 330)
          .disabled(!model.settings.aiFormattingEnabled)
        }
        if model.maxConsultationEnabled {
          SettingsControlRow(
            title: String(localized: "Max consultation"),
            subtitle: String(
              localized:
                "Continue across takes, or start fresh without deleting previous history."
            )
          ) {
            Button(model.newMaxConsultationPending ? "Starting…" : "New consultation") {
              Task { await model.beginNewMaxConsultation() }
            }
            .disabled(model.newMaxConsultationPending)
            .accessibilityIdentifier("settings-new-max-consultation")
          }
          if let notice = model.maxConsultationNotice {
            Text(notice)
              .font(.callout)
              .foregroundStyle(Color.primary)
              .frame(maxWidth: .infinity, alignment: .leading)
          }
        }
      }
      .padding(.top, CSSpace.control)

      if model.maxConsultationEnabled || !model.maxToolApprovals.isEmpty {
        MaxApprovalCards(model: model).padding(.top, CSSpace.section)
      }

      agentBridgeSection
        .padding(.top, CSSpace.section)

      SettingsSectionLabel(String(localized: "Quick start"))
        .padding(.top, CSSpace.section)
      HStack(spacing: 10) {
        QuickStartCard(
          icon: .mic,
          title: "Test mic",
          subtitle: "Levels and recognition",
          accessibilityId: "settings-quickstart-test-mic"
        ) { model.performQuickStart(.testMic) }
        QuickStartCard(
          icon: .overlay,
          title: "Open overlay",
          subtitle: "Start a dictation session",
          accessibilityId: "settings-quickstart-open-overlay"
        ) { model.performQuickStart(.openOverlay) }
        QuickStartCard(
          icon: .shortcuts,
          title: "Tune shortcuts",
          accessibilityId: "settings-quickstart-tune-shortcuts"
        ) { model.performQuickStart(.tuneShortcuts) }
      }
      .padding(.top, CSSpace.control)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
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
    VStack(alignment: .leading, spacing: CSSpace.control) {
      SettingsSectionLabel(String(localized: "Connect your coding agent"))
      Text("Install or update the skill directly from Codescribe.")
        .font(.callout)
        .foregroundStyle(Color.primary)
      ForEach(AgentBridgeClient.allCases) { client in
        let installed = model.creatorAgentBridgeStatus.installedClients.contains(client)
        SettingsControlRow(
          title: client.displayName,
          subtitle: installed
            ? String(
              localized: "creator.agentBridge.clientInstalled", defaultValue: "Installed",
              comment: "Status on an agent client row: the Codescribe skill is installed")
            : nil
        ) {
          Button(installed ? "Update skill" : "Install skill") {
            model.installCreatorAgentBridge(for: client)
          }
          .disabled(!model.creatorAgentBridgeStatus.payloadAvailable)
          .accessibilityIdentifier("settings-agent-bridge-\(client.rawValue)")
          if model.creatorAgentBridgeError != nil {
            Button("Replace manual copy…") { manualSkillClient = client }
              .disabled(!model.creatorAgentBridgeStatus.payloadAvailable)
              .accessibilityIdentifier("settings-agent-bridge-adopt-\(client.rawValue)")
          }
        }
      }
      Button("Refresh status", action: model.refreshCreatorAgentBridge)
      // Installer diagnostics stay collapsed; the launch synchronization result
      // is written to the app log by `App.swift`, never to the panel's notices.
      DisclosureGroup("Details") {
        VStack(alignment: .leading, spacing: 6) {
          Text(model.creatorAgentBridgeStatus.detail)
          if let launchDetail = model.creatorAgentBridgeLaunchDetail {
            Text(launchDetail)
          }
        }
        .font(CSFont.mono(10.5, .medium))
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
      }
      .font(CSFont.ui(11.5))
      .foregroundStyle(Color.secondary)
      if let notice = model.creatorAgentBridgeNotice {
        Text(notice).font(.callout).foregroundStyle(Color.primary).textSelection(.enabled)
      }
      if let error = model.creatorAgentBridgeError {
        Text(error)
          .font(.callout)
          .foregroundStyle(CSColor.terracotta)
          .textSelection(.enabled)
      }
    }
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

/// The same permission cards are used by settings recovery and automatic display.
struct MaxApprovalCards: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: CSSpace.control) {
      SettingsSectionLabel(String(localized: "Max permissions"))
      Button(model.maxApprovalBusy ? "Refreshing…" : "Refresh pending requests") {
        Task { await model.refreshMaxToolApprovals() }
      }
      .disabled(model.maxApprovalBusy)
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
}

// MARK: - Language identity

struct LanguageIdentityPresentation: Identifiable, Equatable {
  let language: CsLanguage
  let title: String
  let isFineTuned: Bool

  var id: String { language.shortCode }

  var accessibilityLabel: String {
    isFineTuned
      ? String(
        localized: "\(title), Fine-tuned",
        comment: "VoiceOver label for a language choice with a specialized model")
      : title
  }

  func accessibilityValue(isSelected: Bool) -> String {
    isSelected
      ? String(localized: "Selected", comment: "VoiceOver value for a chosen language")
      : String(localized: "Not selected", comment: "VoiceOver value for a language not chosen")
  }

  static let supportingCopy = String(
    localized: "Domain vocabulary and Dictionary entries improve speech recognition.",
    comment: "Dictionary is the name of the Voice Lab settings section"
  )

  static let choices: [LanguageIdentityPresentation] = [
    .init(language: .auto, title: String(localized: "Multilingual"), isFineTuned: false),
    .init(language: .polish, title: String(localized: "Polish"), isFineTuned: true),
    .init(language: .english, title: String(localized: "English"), isFineTuned: true),
  ]
}

private struct LanguageIdentityRow: View {
  @Binding var selection: CsLanguage

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      Text("Recognition language")
        .font(CSFont.ui(13.5, .semibold))
        .foregroundStyle(Color.primary)

      LanguageIdentityPicker(selection: $selection)

      Text(LanguageIdentityPresentation.supportingCopy)
        .font(CSFont.ui(10.5))
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)
    }
    .padding(.horizontal, 15)
    .padding(.vertical, 12)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(Color.primary.opacity(0.05))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
  }
}

private struct LanguageIdentityPicker: View {
  @Binding var selection: CsLanguage

  var body: some View {
    HStack(spacing: 5) {
      ForEach(LanguageIdentityPresentation.choices) { choice in
        let isSelected = selection == choice.language
        Button {
          selection = choice.language
        } label: {
          VStack(spacing: 3) {
            Text(choice.title)
              .font(CSFont.ui(11.5, .semibold))
              .lineLimit(1)
            if choice.isFineTuned {
              Text("Fine-tuned")
                .font(CSFont.ui(8.5, .semibold))
                .padding(.horizontal, 5)
                .padding(.vertical, 1.5)
                .background(
                  Capsule().fill(CSColor.chromeAccent.opacity(0.16))
                )
            } else {
              Text("Automatic detection")
                .font(CSFont.ui(8.5, .medium))
                .foregroundStyle(Color.secondary)
            }
          }
          .foregroundStyle(isSelected ? Color.primary : Color.secondary)
          .frame(maxWidth: .infinity, minHeight: 43)
          .padding(.horizontal, 5)
          .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
              .fill(isSelected ? CSColor.chromeAccent.opacity(0.12) : Color.clear)
          )
          .overlay(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
              .strokeBorder(
                isSelected ? CSColor.chromeAccent.opacity(0.5) : Color.primary.opacity(0.12),
                lineWidth: 1
              )
          )
        }
        .csFocusRing()
        .accessibilityLabel(choice.accessibilityLabel)
        .accessibilityValue(choice.accessibilityValue(isSelected: isSelected))
        .accessibilityAddTraits(isSelected ? [.isSelected] : [])
      }
    }
    .frame(maxWidth: 460)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Recognition language")
  }
}

// MARK: - Labeled control row (shared shape for the editable settings rows)

struct SettingsControlRow<Control: View>: View {
  let title: String
  /// Omitted when the title already carries the whole meaning of the row.
  var subtitle: String? = nil
  @ViewBuilder var control: () -> Control

  var body: some View {
    HStack(spacing: 12) {
      VStack(alignment: .leading, spacing: 2) {
        Text(title)
          .font(.body.weight(.semibold))
          .foregroundStyle(.primary)
        if let subtitle {
          Text(subtitle)
            .font(.subheadline)
            .foregroundStyle(.secondary)
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      control()
    }
    .settingsGroupedInset()
  }
}

// MARK: - Permission checklist row

private struct PermissionChecklistRow: View {
  let kind: PermissionKind
  let state: PermissionState
  /// Re-probe after an in-app request (Speech Recognition dialog).
  var onStateChanged: (() -> Void)? = nil

  private var granted: Bool { state.isGranted }

  var body: some View {
    HStack(spacing: 12) {
      statusBadge
      // A granted row says it with the badge; the status stays for VoiceOver.
      Text(kind.displayName)
        .font(CSFont.ui(13.5, .medium))
        .foregroundStyle(Color.primary)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityValue(state.label)
      if !granted {
        Button {
          if state == .notDetermined, kind.supportsInAppPermissionRequest {
            Task { @MainActor in
              _ = await kind.requestInApp()
              onStateChanged?()
            }
          } else {
            kind.openSystemSettings()
          }
        } label: {
          Text(
            state == .notDetermined && kind.supportsInAppPermissionRequest
              ? "allow \(kind.displayName)"
              : "open System Settings"
          )
          .font(CSFont.mono(11, .semibold))
          .foregroundStyle(CSColor.terracotta)
        }
        .csFocusRing()
      }
    }
    .padding(.horizontal, 15)
    .padding(.vertical, 13)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill((granted ? CSColor.olive : CSColor.terracotta).opacity(0.08))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder((granted ? CSColor.olive : CSColor.terracotta).opacity(0.22), lineWidth: 1)
    )
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
  /// Omitted when the title already says everything the card does.
  var subtitle: LocalizedStringKey? = nil
  let accessibilityId: String
  let action: () -> Void

  @State private var hovered = false

  @ViewBuilder
  var body: some View {
    let card = cardButton
    if let subtitle {
      card.accessibilityHint(subtitle)
    } else {
      card
    }
  }

  private var cardButton: some View {
    Button(action: action) {
      VStack(alignment: .leading, spacing: 0) {
        CSIconView(icon: icon, size: 16, color: Color.primary)
        Text(title)
          .font(CSFont.ui(13, .semibold))
          .foregroundStyle(Color.primary)
          .padding(.top, 9)
        if let subtitle {
          Text(subtitle)
            .font(CSFont.ui(11.5))
            .lineSpacing(2)
            .foregroundStyle(Color.secondary)
            .padding(.top, 3)
        }
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
