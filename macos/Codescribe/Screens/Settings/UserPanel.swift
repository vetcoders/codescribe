import AppKit
import SwiftUI

// About panel. Codescribe has no account model, so this panel reports the
// running build, where local data lives, the privacy switches and the two
// resets — the facts about the app and its data, not a profile.
struct UserPanel: View {
  @ObservedObject var model: SettingsViewModel
  @State private var repairNotice: ConfigRepairNotice?
  @State private var showingDiagnostics = false
  @State private var showingTemplate = false
  @AppStorage(ActivationPing.optInDefaultsKey) private var activationPingOptIn = false

  private static let docsURL = URL(
    string: "https://github.com/vetcoders/codescribe/tree/develop/docs")!
  /// Public trust pages on the Codescribe website.
  private static let privacyURL = URL(string: "https://codescribe.vetcoders.io/privacy")!
  private static let termsURL = URL(string: "https://codescribe.vetcoders.io/terms")!

  private var locale: Locale {
    InterfaceLanguage.preferred(from: Bundle.main.preferredLocalizations).locale
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsPageHeader(
        String(localized: "About the app and your data", comment: "About panel header"),
        blurb: String(
          localized:
            "Check the Codescribe version, where your data lives and the privacy settings.",
          comment: "About panel blurb")
      )
      .onAppear { repairNotice = configRepairSummary().map(ConfigRepairNotice.init(raw:)) }

      buildSection
      localDataSection
      activationPingSection
      transcriptMarkersSection
      legalSection

      ResetAgentSection(model: model)
        .padding(.top, CSSpace.section)

      ResetAppDataSection(model: model)
        .padding(.top, 30)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
  }

  // MARK: - Running build

  private var buildSection: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(String(localized: "Running build"))
        .padding(.top, CSSpace.section)
      VStack(spacing: 0) {
        infoRow("Version", "\(model.buildInfo.version) (\(model.buildInfo.build))")
        divider
        infoRow("Commit", model.buildInfo.commit)
        divider
        infoRow("Built", readableBuildDate)
      }
      .settingsGroupedInset()

      if let repairNotice {
        Text(repairNotice.headline)
          .font(CSFont.ui(12.5))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
          .padding(.top, CSSpace.control)
          .accessibilityIdentifier("about-config-notice")
      }

      DisclosureGroup(isExpanded: $showingDiagnostics) {
        VStack(alignment: .leading, spacing: 6) {
          diagnosticLine(
            String(
              localized: "Build timestamp: \(model.buildInfo.builtAt)",
              comment: "About panel details: the raw CSBuiltAt value"))
          if let repairNotice {
            if let keys = repairNotice.reviewKeysLine {
              diagnosticLine(keys)
            }
            diagnosticLine(repairNotice.raw)
          }
        }
        .padding(.top, 6)
        .accessibilityIdentifier("about-diagnostics")
      } label: {
        Text("Details", comment: "About panel: disclosure with raw diagnostic values")
          .font(CSFont.mono(10.5, .semibold))
          .foregroundStyle(Color.secondary)
      }
      .padding(.top, CSSpace.control)
    }
  }

  private var readableBuildDate: String {
    BuildDatePresentation.readable(model.buildInfo.builtAt, locale: locale)
      ?? model.buildInfo.builtAt
  }

  private func diagnosticLine(_ text: String) -> some View {
    Text(text)
      .font(CSFont.mono(10.5, .regular))
      .foregroundStyle(Color.secondary)
      .textSelection(.enabled)
      .fixedSize(horizontal: false, vertical: true)
  }

  // MARK: - Local data

  private var localDataSection: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(String(localized: "Local data"))
        .padding(.top, CSSpace.section)
      VStack(spacing: 0) {
        pathRow(
          String(localized: "App data", comment: "About panel: the ~/.codescribe folder"),
          model.configDir)
        divider
        pathRow(String(localized: "Transcripts"), model.transcriptsPath)
      }
      .settingsGroupedInset()
    }
  }

  // MARK: - First dictation confirmation

  private var activationPingSection: some View {
    let availability = ActivationPingAvailability(optIn: activationPingOptIn)
    return VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(
        String(localized: "First dictation confirmation", comment: "About panel section")
      )
      .padding(.top, CSSpace.section)
      SettingsControlRow(
        title: String(
          localized: "Send a confirmation after the first successful dictation",
          comment: "About panel: opt-in switch title"),
        subtitle: String(
          localized:
            "One anonymous event with the app version and the macOS version. Never audio or text.",
          comment: "About panel: opt-in switch subtitle")
      ) {
        Toggle("", isOn: $activationPingOptIn)
          .toggleStyle(.switch)
          .labelsHidden()
          .tint(CSColor.chromeAccent)
          .disabled(!availability.serviceEnabled)
          .accessibilityLabel("Send a confirmation after the first successful dictation")
          .accessibilityValue(activationPingOptIn ? "On" : "Off")
      }
      .padding(.top, CSSpace.control)

      Text(availability.stateLine)
        .font(CSFont.mono(10.5, .regular))
        .foregroundStyle(Color.secondary)
        .padding(.top, 7)
        .accessibilityIdentifier("about-ping-state")
      if let unavailable = availability.unavailableLine {
        Text(unavailable)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
          .padding(.top, 4)
          .accessibilityIdentifier("about-ping-unavailable")
      }
    }
  }

  // MARK: - Transcript source markers

  private var transcriptMarkersSection: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(
        String(localized: "Transcript source markers", comment: "About panel section")
      )
      .padding(.top, CSSpace.section)
      SettingsControlRow(
        title: String(localized: "Add markers to transcripts", comment: "About panel: switch"),
        subtitle: String(
          localized: "Wrap delivered dictation in a marker that names its source",
          comment: "About panel: switch subtitle")
      ) {
        Toggle("", isOn: taggingBinding)
          .toggleStyle(.switch)
          .labelsHidden()
          .tint(CSColor.chromeAccent)
          .accessibilityLabel("Add markers to transcripts")
          .accessibilityValue(model.settings.transcriptTaggingEnabled ? "On" : "Off")
      }
      .padding(.top, CSSpace.control)

      DisclosureGroup(isExpanded: $showingTemplate) {
        templateEditor
      } label: {
        Text("Template and preview", comment: "About panel: disclosure with the marker template")
          .font(CSFont.mono(10.5, .semibold))
          .foregroundStyle(Color.secondary)
      }
      .padding(.top, CSSpace.control)
    }
  }

  private var templateEditor: some View {
    VStack(alignment: .leading, spacing: 0) {
      Text("Template")
        .font(CSFont.mono(10, .semibold))
        .foregroundStyle(Color.secondary)
        .padding(.top, 8)
      TextField("Transcript tag template", text: transcriptTemplateBinding, axis: .vertical)
        .font(CSFont.mono(11.5, .regular))
        .foregroundStyle(Color.primary)
        .textFieldStyle(.plain)
        .lineLimit(3...8)
        .settingsGroupedInset()
        .accessibilityLabel("Transcript tag template editor")
        .accessibilityValue(model.settings.transcriptTagTemplate)

      if let warning = model.transcriptTagTemplateWarning {
        Text(warning)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.danger)
          .padding(.top, 7)
          .accessibilityLabel("Transcript tag template warning")
          .accessibilityValue(warning)
      }

      // Each chip appends its field to the template; the fields themselves
      // are the contract and stay the same in every language.
      HStack(spacing: 6) {
        ForEach(transcriptTagTemplatePlaceholders, id: \.self) { placeholder in
          Button {
            model.insertTranscriptTagPlaceholder(placeholder)
          } label: {
            Text(placeholder)
              .font(CSFont.mono(10, .semibold))
              .foregroundStyle(Color.secondary)
              .padding(.horizontal, 7)
              .padding(.vertical, 4)
              .background(
                Capsule(style: .continuous)
                  .fill(Color.primary.opacity(0.08))
              )
              .overlay(
                Capsule(style: .continuous)
                  .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
              )
          }
          .buttonStyle(.plain)
          .csFocusRing()
          .accessibilityLabel(
            String(
              localized: "Insert \(placeholder) into the template",
              comment: "About panel: template field chip"))
        }
        Spacer(minLength: 0)
        Button(String(localized: "Restore default template", comment: "About panel: button")) {
          model.restoreDefaultTranscriptTagTemplate()
        }
        .csFocusRing()
        .font(CSFont.mono(10.5, .semibold))
        .foregroundStyle(CSColor.chromeAccent)
        .accessibilityLabel("Restore default transcript tag template")
      }
      .padding(.top, 9)

      Text("Template preview", comment: "About panel: label over the rendered template")
        .font(CSFont.mono(10, .semibold))
        .foregroundStyle(Color.secondary)
        .padding(.top, 12)
      Text(model.transcriptTagPreview)
        .font(CSFont.mono(11.5, .regular))
        .foregroundStyle(Color.primary)
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
        .settingsGroupedInset()
        .accessibilityLabel("Transcript tag template preview")
        .accessibilityValue(model.transcriptTagPreview)
    }
  }

  // MARK: - Legal & docs

  private var legalSection: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(String(localized: "Legal & docs"))
        .padding(.top, CSSpace.section)
      VStack(alignment: .leading, spacing: 10) {
        legalLink(
          String(localized: "Privacy Policy"), Self.privacyURL,
          accessibility: "Open Privacy Policy")
        legalLink(
          String(localized: "Terms of Use and License", comment: "About panel: legal link"),
          Self.termsURL, accessibility: "Open Terms of Use and License")
        legalLink(
          String(localized: "Codescribe documentation", comment: "About panel: docs link"),
          Self.docsURL, accessibility: "Open Codescribe documentation")
      }
      .padding(.top, CSSpace.control)
    }
  }

  private func legalLink(_ title: String, _ url: URL, accessibility: String) -> some View {
    Link(destination: url) {
      HStack(spacing: 6) {
        Text(title)
        Text(verbatim: "↗")
      }
      .font(CSFont.mono(11, .semibold))
      .foregroundStyle(CSColor.chromeAccent)
    }
    .accessibilityLabel(accessibility)
  }

  // MARK: - Bindings and rows

  private var taggingBinding: Binding<Bool> {
    Binding(
      get: { model.settings.transcriptTaggingEnabled },
      set: { model.setTranscriptTaggingEnabled($0) }
    )
  }

  private var transcriptTemplateBinding: Binding<String> {
    Binding(
      get: { model.settings.transcriptTagTemplate },
      set: { model.setTranscriptTagTemplate($0) }
    )
  }

  private func infoRow(_ label: LocalizedStringKey, _ value: String) -> some View {
    HStack(spacing: 14) {
      Text(label)
        .font(CSFont.ui(12.5, .medium))
        .foregroundStyle(Color.secondary)
        .frame(width: 90, alignment: .leading)
      Text(value)
        .font(CSFont.mono(11.5, .medium))
        .foregroundStyle(Color.primary)
        .textSelection(.enabled)
        .accessibilityLabel(label)
        .accessibilityValue(value)
      Spacer(minLength: 0)
    }
    .padding(.horizontal, CSSpace.card)
    .padding(.vertical, CSSpace.md)
  }

  private func pathRow(_ label: String, _ path: String) -> some View {
    let loaded = !path.isEmpty
    let display = loaded ? path : String(localized: "not loaded yet")
    return HStack(alignment: .center, spacing: 10) {
      VStack(alignment: .leading, spacing: 5) {
        Text(label)
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(Color.primary)
        Text(display)
          .font(CSFont.mono(10.5, .regular))
          .foregroundStyle(Color.secondary)
          .textSelection(.enabled)
          .lineLimit(2)
          .truncationMode(.middle)
          .accessibilityLabel(label)
          .accessibilityValue(display)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      if loaded {
        Button {
          NSPasteboard.general.clearContents()
          NSPasteboard.general.setString(path, forType: .string)
        } label: {
          Image(systemName: "doc.on.doc")
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(Color.secondary)
        }
        .buttonStyle(.plain)
        .csFocusRing()
        .help(String(localized: "Copy path", comment: "About panel: copy button tooltip"))
        .accessibilityLabel(
          String(localized: "Copy the \(label) path", comment: "About panel: copy button"))
      }
    }
    .padding(.horizontal, CSSpace.card)
    .padding(.vertical, CSSpace.md)
  }

  private var divider: some View {
    Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
  }

}

// MARK: - Danger zone

/// A deliberately narrow reset for Agent state. It is separate from the full
/// app-data reset so it cannot clear dictation, recordings, prompts or license.
/// The short card names the scope; the confirmation sheet shows the live counts
/// and every surface that stays.
private struct ResetAgentSection: View {
  @ObservedObject var model: SettingsViewModel
  @State private var confirming = false
  @State private var confirmationText = ""

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(String(localized: "Reset Agent"))
        .foregroundStyle(CSColor.danger)

      Text(
        "Moves Agent conversations, MCP configuration and tool state to Trash. Agent provider keys and MCP connector secrets are deleted permanently. Everything else stays.",
        comment:
          "About panel: short scope of the Agent reset; the confirmation shows the full scope"
      )
      .font(CSFont.mono(11, .medium))
      .foregroundStyle(Color.secondary)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, 6)

      Button(role: .destructive) {
        model.refreshAgentResetPreview()
        confirmationText = ""
        confirming = true
      } label: {
        Text("Reset Agent…")
          .font(CSFont.ui(12, .semibold))
          .foregroundStyle(CSColor.danger)
          .padding(.horizontal, 16)
          .padding(.vertical, 8)
          .background(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
              .fill(CSColor.danger.opacity(0.14))
          )
          .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
              .strokeBorder(CSColor.danger.opacity(0.42), lineWidth: 1)
          )
      }
      .csFocusRing()
      .padding(.top, 13)
      .accessibilityLabel("Reset Agent. Destructive action.")
      .accessibilityHint(
        "Shows Agent-only impact and requires typing \(resetAgentConfirmationWord) before continuing."
      )
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .padding(.horizontal, 16)
    .padding(.vertical, 16)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(CSColor.danger.opacity(0.055))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(CSColor.danger.opacity(0.55), lineWidth: 1)
    )
    .alert("Reset Agent?", isPresented: $confirming) {
      TextField("Type \(resetAgentConfirmationWord) to continue", text: $confirmationText)
      Button("Cancel", role: .cancel) { confirmationText = "" }
      Button("Reset Agent", role: .destructive) { model.resetAgentData() }
        .disabled(!resetAgentConfirmationMatches(confirmationText))
    } message: {
      Text(model.resetAgentImpactDescription())
    }
  }
}

/// The full-data reset lives only at the foot of About, away from MCP editing.
/// Data is recoverable from Trash; Keychain deletion and prompt reset stay
/// opt-in, and the confirmation sheet shows the live scope before anything moves.
private struct ResetAppDataSection: View {
  @ObservedObject var model: SettingsViewModel
  @State private var includeKeys = false
  @State private var includePrompts = false
  @State private var confirming = false
  @State private var confirmationText = ""

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(String(localized: "Danger zone"))
        .foregroundStyle(CSColor.danger)

      Text(
        "Moves recordings, transcripts, conversations, logs, preferences and local configuration to Trash. Your base prompts stay unless you choose otherwise below.",
        comment:
          "About panel: short scope of the app-data reset; the confirmation shows the full scope"
      )
      .font(CSFont.mono(11, .medium))
      .foregroundStyle(Color.secondary)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, 6)

      Toggle(isOn: $includeKeys) {
        Text(
          "Also remove API keys from Keychain (not recoverable from Trash)",
          comment: "About panel: reset checkbox"
        )
        .font(CSFont.ui(12.5, .medium))
        .foregroundStyle(Color.primary)
      }
      .toggleStyle(.checkbox)
      .padding(.top, 13)

      Toggle(isOn: $includePrompts) {
        Text(
          "Also reset my base prompts (assistive.txt, formatting.txt, formatting-smart.txt and formatting-max.txt)",
          comment: "About panel: reset checkbox; the four file names stay verbatim"
        )
        .font(CSFont.ui(12.5, .medium))
        .foregroundStyle(Color.primary)
      }
      .toggleStyle(.checkbox)
      .padding(.top, 9)
      .accessibilityHint(
        "Off by default. When enabled, all four prompt files move to Trash with the rest of the app data."
      )

      Button(role: .destructive) {
        model.refreshResetPreview()
        confirmationText = ""
        confirming = true
      } label: {
        Text("Move app data to Trash…")
          .font(CSFont.ui(12, .semibold))
          .foregroundStyle(CSColor.danger)
          .padding(.horizontal, 16)
          .padding(.vertical, 8)
          .background(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
              .fill(CSColor.danger.opacity(0.14))
          )
          .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
              .strokeBorder(CSColor.danger.opacity(0.42), lineWidth: 1)
          )
      }
      .csFocusRing()
      .padding(.top, 13)
      .accessibilityLabel("Reset app data. Destructive action.")
      .accessibilityHint(
        "Shows the live impact, names whether base prompts are preserved, and requires typing \(resetConfirmationWord) before data moves to Trash."
      )
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .padding(.horizontal, 16)
    .padding(.vertical, 16)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(CSColor.danger.opacity(0.055))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(CSColor.danger.opacity(0.55), lineWidth: 1)
    )
    .alert("Move app data to Trash?", isPresented: $confirming) {
      TextField("Type \(resetConfirmationWord) to continue", text: $confirmationText)
      Button("Cancel", role: .cancel) {
        confirmationText = ""
      }
      Button("Move to Trash & Relaunch", role: .destructive) {
        model.resetAppData(includeKeys: includeKeys, includePrompts: includePrompts)
      }
      .disabled(!resetConfirmationMatches(confirmationText))
    } message: {
      Text(model.resetImpactDescription(includeKeys: includeKeys, includePrompts: includePrompts))
    }
  }
}

#if DEBUG
  #Preview("About panel") {
    ScrollView { UserPanel(model: .preview(.user)) }
      .frame(width: 720, height: 720)
  }
#endif
