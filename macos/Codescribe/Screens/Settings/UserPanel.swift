import AppKit
import SwiftUI

// About panel. Codescribe has no account model, so this panel reports the
// running build, where local data lives, the transcript markers and the two
// resets — the facts about the app and its data, not a profile. Everyday
// information stays visible; technical values and the resets open on demand.
struct UserPanel: View {
  @ObservedObject var model: SettingsViewModel
  @State private var repairNotice: ConfigRepairNotice?
  @State private var showingVersionDetails = false
  @State private var showingConfigNotice = false
  @State private var showingTemplate = false
  @State private var showingResets = false
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
          localized: "Codescribe version, local data and privacy.",
          comment: "About panel blurb")
      )
      .onAppear { repairNotice = configRepairSummary().map(ConfigRepairNotice.init(raw:)) }

      versionSection
      localDataSection
      // The confirmation cannot be sent while the build ships without an
      // analytics domain, so the switch stays out of sight until it can. The
      // stored choice is kept and the switch returns with the service.
      if ActivationPingConfiguration.production.isEnabled {
        activationPingSection
      }
      transcriptMarkersSection
      informationSection
      resetSection
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
  }

  // MARK: - Version and configuration notice

  private var versionSection: some View {
    VStack(alignment: .leading, spacing: CSSpace.control) {
      VStack(alignment: .leading, spacing: 0) {
        HStack(alignment: .center, spacing: CSSpace.md) {
          VStack(alignment: .leading, spacing: 2) {
            Text(verbatim: "Codescribe \(model.buildInfo.version)")
              .font(.body.weight(.semibold))
              .foregroundStyle(.primary)
              .textSelection(.enabled)
              .accessibilityIdentifier("about-version")
            Text(
              "Build \(model.buildInfo.build)",
              comment: "About panel: the build number under the version")
            .font(.subheadline)
            .foregroundStyle(.secondary)
            .textSelection(.enabled)
          }
          .frame(maxWidth: .infinity, alignment: .leading)
          TrailingDisclosureButton(
            title: String(
              localized: "Version details",
              comment: "About panel: opens the commit and the build date"),
            isExpanded: $showingVersionDetails
          )
          .accessibilityIdentifier("about-version-details-toggle")
        }
        if showingVersionDetails {
          divider.padding(.vertical, CSSpace.md)
          VStack(alignment: .leading, spacing: CSSpace.sm) {
            detailRow(String(localized: "Commit"), model.buildInfo.commit, mono: true)
            detailRow(String(localized: "Built"), readableBuildDate, mono: false)
          }
          .accessibilityIdentifier("about-version-details")
        }
      }
      .settingsGroupedInset()

      if let repairNotice {
        // Weight follows the real problem: an entry that changes nothing gets
        // one quiet line, a real configuration error keeps the warning card
        // (Founder brief, round 16, 2026-10-10).
        if repairNotice.isBenignStaleEntry {
          benignConfigNotice(repairNotice)
        } else {
          configNoticeCard(repairNotice)
        }
      }
    }
    .padding(.top, CSSpace.section)
  }

  private var readableBuildDate: String {
    BuildDatePresentation.readable(model.buildInfo.builtAt, locale: locale)
      ?? model.buildInfo.builtAt
  }

  /// One line for an entry Codescribe does not read: it is out of date and
  /// nothing behaves differently because of it. The named keys, what each one
  /// does in this build and how to clear the notice stay in the expansion,
  /// unchanged, so nothing is lost by the quieter collapsed form.
  private func benignConfigNotice(_ notice: ConfigRepairNotice) -> some View {
    VStack(alignment: .leading, spacing: 0) {
      Button {
        showingConfigNotice.toggle()
      } label: {
        HStack(alignment: .firstTextBaseline, spacing: CSSpace.sm) {
          CSIconView(icon: .info, size: 12, weight: .medium, color: Color.secondary)
          Text(notice.compactTitle)
            .font(.callout)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
          CSIconView(
            icon: showingConfigNotice ? .chevronDown : .chevronRight, size: 10,
            weight: .semibold, color: Color.secondary)
          Spacer(minLength: 0)
        }
        .contentShape(Rectangle())
      }
      .csFocusRing()
      .accessibilityElement(children: .combine)
      .accessibilityValue(showingConfigNotice ? Text("Expanded") : Text("Collapsed"))
      .accessibilityIdentifier("about-config-notice")

      if showingConfigNotice {
        configNoticeDetails(notice)
          .settingsGroupedInset()
          .padding(.top, CSSpace.control)
          .accessibilityIdentifier("about-config-notice-details")
      }
    }
  }

  private func configNoticeCard(_ notice: ConfigRepairNotice) -> some View {
    VStack(alignment: .leading, spacing: 0) {
      Button {
        showingConfigNotice.toggle()
      } label: {
        HStack(alignment: .center, spacing: 10) {
          CSIconView(
            icon: notice.isWarning ? .warning : .info, size: 14, weight: .medium,
            color: notice.isWarning ? CSColor.amber : Color.secondary)
          VStack(alignment: .leading, spacing: 2) {
            Text(notice.title)
              .font(.body.weight(.semibold))
              .foregroundStyle(.primary)
            Text(notice.subtitle)
              .font(.subheadline)
              .foregroundStyle(.secondary)
          }
          .frame(maxWidth: .infinity, alignment: .leading)
          CSIconView(
            icon: showingConfigNotice ? .chevronDown : .chevronRight, size: 11,
            weight: .semibold, color: Color.secondary)
        }
        .contentShape(Rectangle())
      }
      .csFocusRing()
      .accessibilityElement(children: .combine)
      .accessibilityValue(showingConfigNotice ? Text("Expanded") : Text("Collapsed"))
      .accessibilityIdentifier("about-config-notice")

      if showingConfigNotice {
        divider.padding(.vertical, CSSpace.md)
        configNoticeDetails(notice)
          .accessibilityIdentifier("about-config-notice-details")
      }
    }
    .settingsGroupedInset()
  }

  private func configNoticeDetails(_ notice: ConfigRepairNotice) -> some View {
    VStack(alignment: .leading, spacing: 12) {
      ForEach(notice.reviewItems(envFile: envFileDisplay)) { item in
        VStack(alignment: .leading, spacing: 4) {
          Text(verbatim: item.key)
            .font(CSFont.mono(12, .semibold))
            .foregroundStyle(.primary)
            .textSelection(.enabled)
          Text(item.impact)
            .font(.callout)
            .foregroundStyle(.primary)
            .fixedSize(horizontal: false, vertical: true)
          Text(item.action)
            .font(.callout)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
      }
      if let outcome = notice.outcomeLine {
        Text(outcome)
          .font(.callout)
          .foregroundStyle(.primary)
          .fixedSize(horizontal: false, vertical: true)
      }
      // The launch record verbatim, for support; it repeats the key names.
      Text(verbatim: notice.raw)
        .font(CSFont.mono(10.5, .regular))
        .foregroundStyle(.secondary)
        .textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
        .accessibilityLabel(Text("Launch record", comment: "About panel: the raw repair line"))
        .accessibilityValue(notice.raw)
    }
  }

  /// The optional `.env` file the receipt reads, beside the app data.
  private var envFileDisplay: String {
    guard !model.configDir.isEmpty else { return ".env" }
    return displayPath(URL(fileURLWithPath: model.configDir).appendingPathComponent(".env").path)
  }

  // MARK: - Local data

  private var localDataSection: some View {
    VStack(alignment: .leading, spacing: 0) {
      ProvidersSectionHeader(String(localized: "Local data"))
        .padding(.top, CSSpace.section)
      VStack(spacing: 0) {
        pathRow(
          String(localized: "App data", comment: "About panel: the ~/.codescribe folder"),
          model.configDir)
        divider
        pathRow(String(localized: "Transcripts"), model.transcriptsPath)
      }
      .settingsGroupedInset(padding: 0)
      .padding(.top, CSSpace.control)
    }
  }

  // MARK: - First dictation confirmation

  private var activationPingSection: some View {
    VStack(alignment: .leading, spacing: 0) {
      ProvidersSectionHeader(
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
          .accessibilityLabel("Send a confirmation after the first successful dictation")
          .accessibilityValue(activationPingOptIn ? "On" : "Off")
      }
      .padding(.top, CSSpace.control)
    }
  }

  // MARK: - Transcript markers

  private var transcriptMarkersSection: some View {
    VStack(alignment: .leading, spacing: 0) {
      ProvidersSectionHeader(
        String(localized: "Transcript markers", comment: "About panel section")
      )
      .padding(.top, CSSpace.section)
      SettingsControlRow(
        title: String(localized: "Add markers to text", comment: "About panel: switch"),
        subtitle: String(
          localized: "Mark text delivered to other apps",
          comment: "About panel: switch subtitle; the template decides the marker")
      ) {
        Toggle("", isOn: taggingBinding)
          .toggleStyle(.switch)
          .labelsHidden()
          .tint(CSColor.chromeAccent)
          .accessibilityLabel("Add markers to text")
          .accessibilityValue(model.settings.transcriptTaggingEnabled ? "On" : "Off")
      }
      .padding(.top, CSSpace.control)

      DisclosureGroup(isExpanded: $showingTemplate) {
        templateEditor
      } label: {
        Text(
          "Edit template and preview",
          comment: "About panel: disclosure with the marker template editor"
        )
        .font(.callout)
        .foregroundStyle(.secondary)
      }
      .padding(.top, CSSpace.control)
      .accessibilityIdentifier("about-template-disclosure")
    }
  }

  private var templateEditor: some View {
    VStack(alignment: .leading, spacing: 0) {
      // The disclosure above already names this editor, so the field carries
      // no second label of its own (Founder brief, round 16, 2026-10-10).
      TextField("Transcript tag template", text: transcriptTemplateBinding, axis: .vertical)
        .font(CSFont.mono(11.5, .regular))
        .foregroundStyle(Color.primary)
        .textFieldStyle(.plain)
        .lineLimit(3...8)
        .settingsGroupedInset()
        .padding(.top, CSSpace.control)
        .accessibilityLabel("Transcript tag template editor")
        .accessibilityValue(model.settings.transcriptTagTemplate)

      if let warning = model.transcriptTagTemplateWarning {
        Text(warning)
          .font(.callout)
          .foregroundStyle(CSColor.danger)
          .fixedSize(horizontal: false, vertical: true)
          .padding(.top, CSSpace.sm)
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
        .font(.callout)
        .foregroundStyle(CSColor.chromeAccent)
        .accessibilityLabel("Restore default transcript tag template")
      }
      .padding(.top, CSSpace.sm)

      Text("Template preview", comment: "About panel: label over the rendered template")
        .font(.subheadline)
        .foregroundStyle(.secondary)
        .padding(.top, CSSpace.md)
      Text(model.transcriptTagPreview)
        .font(CSFont.mono(11.5, .regular))
        .foregroundStyle(Color.primary)
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
        .settingsGroupedInset()
        .padding(.top, CSSpace.xs)
        .accessibilityLabel("Transcript tag template preview")
        .accessibilityValue(model.transcriptTagPreview)
    }
  }

  // MARK: - Information and documentation

  /// The header leads and the three links read as one quiet list under it:
  /// link text below the header's size, the open-elsewhere icon at the right
  /// edge of every row, equal spacing and a full-row click area — not three
  /// cards competing with the heading (Founder brief, round 16, 2026-10-10).
  private var informationSection: some View {
    VStack(alignment: .leading, spacing: 0) {
      ProvidersSectionHeader(
        String(localized: "Information and documentation", comment: "About panel section"))
        .padding(.top, CSSpace.section)
      VStack(alignment: .leading, spacing: CSSpace.xxs) {
        externalLink(
          String(localized: "Privacy Policy"), Self.privacyURL,
          accessibility: "Open Privacy Policy")
        externalLink(
          String(localized: "Terms of Use", comment: "About panel: legal link"),
          Self.termsURL, accessibility: "Open Terms of Use")
        externalLink(
          String(localized: "Documentation", comment: "About panel: docs link"),
          Self.docsURL, accessibility: "Open Codescribe documentation")
      }
      .padding(.top, CSSpace.sm)
    }
  }

  private func externalLink(_ title: String, _ url: URL, accessibility: String) -> some View {
    Link(destination: url) {
      HStack(alignment: .center, spacing: CSSpace.sm) {
        Text(title)
          .font(.callout)
          .frame(maxWidth: .infinity, alignment: .leading)
        Image(systemName: "arrow.up.forward.square")
          .font(.system(size: 11, weight: .medium))
      }
      .foregroundStyle(CSColor.chromeAccent)
      .padding(.vertical, CSSpace.xs)
      .contentShape(Rectangle())
    }
    .csFocusRing()
    .accessibilityLabel(accessibility)
  }

  // MARK: - Reset data

  /// Both resets sit behind one closed row: most people never use them, and
  /// the confirmations keep every safeguard once the row is open. The header
  /// carries the same weight as every other section on the page — the red
  /// belongs to the destructive buttons inside, not to the heading (Founder
  /// brief, round 16, 2026-10-10).
  private var resetSection: some View {
    VStack(alignment: .leading, spacing: 0) {
      divider
        .padding(.top, CSSpace.section)

      ProvidersSectionHeader(
        String(localized: "Reset data", comment: "About panel: section holding both resets")
      ) {
        TrailingDisclosureButton(
          title: showingResets
            ? String(localized: "Collapse")
            : String(localized: "Expand", comment: "Opens a closed section"),
          isExpanded: $showingResets
        )
        .accessibilityIdentifier("about-reset-toggle")
      }
      .padding(.top, CSSpace.section)

      if showingResets {
        VStack(alignment: .leading, spacing: 0) {
          ResetAgentSection(model: model)
          divider.padding(.vertical, CSSpace.card)
          ResetAppDataSection(model: model)
        }
        .settingsGroupedInset()
        .padding(.top, CSSpace.control)
        .accessibilityIdentifier("about-resets")
      }
    }
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

  private func detailRow(_ label: String, _ value: String, mono: Bool) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: CSSpace.card) {
      Text(label)
        .font(.subheadline)
        .foregroundStyle(.secondary)
        .frame(width: 110, alignment: .leading)
      Text(value)
        .font(mono ? CSFont.mono(11.5, .medium) : .subheadline)
        .foregroundStyle(.primary)
        .textSelection(.enabled)
      Spacer(minLength: 0)
    }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(label)
    .accessibilityValue(value)
  }

  /// Home-relative path for reading; the copy button copies the full path.
  private func displayPath(_ path: String) -> String {
    (path as NSString).abbreviatingWithTildeInPath
  }

  private func pathRow(_ label: String, _ path: String) -> some View {
    let loaded = !path.isEmpty
    let display = loaded ? displayPath(path) : String(localized: "not loaded yet")
    return HStack(alignment: .center, spacing: 10) {
      VStack(alignment: .leading, spacing: 3) {
        Text(label)
          .font(.body)
          .foregroundStyle(Color.primary)
        Text(display)
          .font(CSFont.mono(11, .regular))
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
            .font(.system(size: 12, weight: .medium))
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

// MARK: - Trailing disclosure

/// "Version details ›" and "Expand ›": a text button at the end of a row that
/// opens content below it. The chevron turns down while the content is open.
private struct TrailingDisclosureButton: View {
  let title: String
  @Binding var isExpanded: Bool

  var body: some View {
    Button {
      isExpanded.toggle()
    } label: {
      HStack(spacing: 4) {
        Text(title)
        CSIconView(
          icon: isExpanded ? .chevronDown : .chevronRight, size: 10, weight: .semibold)
      }
      .font(.callout)
      .foregroundStyle(.secondary)
      .contentShape(Rectangle())
    }
    .csFocusRing()
    .accessibilityValue(isExpanded ? Text("Expanded") : Text("Collapsed"))
  }
}

// MARK: - Resets

/// The red stays on the two destructive buttons; the blocks themselves read
/// like the rest of the panel.
private struct DestructiveResetButton: View {
  let title: String
  let action: () -> Void

  var body: some View {
    Button(role: .destructive, action: action) {
      Text(title)
        .font(.callout.weight(.semibold))
        .foregroundStyle(CSColor.danger)
        .padding(.horizontal, 14)
        .padding(.vertical, 7)
        .background(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .fill(CSColor.danger.opacity(0.12))
        )
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .strokeBorder(CSColor.danger.opacity(0.42), lineWidth: 1)
        )
    }
    .csFocusRing()
  }
}

/// A deliberately narrow reset for Agent state. It is separate from the full
/// app-data reset so it cannot clear dictation, recordings, prompts or license.
/// The short block names the scope; the confirmation sheet shows the live
/// counts and every surface that stays.
private struct ResetAgentSection: View {
  @ObservedObject var model: SettingsViewModel
  @State private var confirming = false
  @State private var confirmationText = ""

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      Text("Reset Agent data", comment: "About panel: the Agent reset")
        .font(.body.weight(.semibold))
        .foregroundStyle(.primary)

      Text(
        "Moves Agent conversations, MCP configuration and tool state to Trash. Agent provider keys and MCP connector secrets are deleted permanently. Everything else stays.",
        comment:
          "About panel: short scope of the Agent reset; the confirmation shows the full scope"
      )
      .font(.callout)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, CSSpace.xxs)

      DestructiveResetButton(title: String(localized: "Reset Agent…")) {
        model.refreshAgentResetPreview()
        confirmationText = ""
        confirming = true
      }
      .padding(.top, CSSpace.md)
      .accessibilityLabel("Reset Agent. Destructive action.")
      .accessibilityHint(
        "Shows Agent-only impact and requires typing \(resetAgentConfirmationWord) before continuing."
      )
    }
    .frame(maxWidth: .infinity, alignment: .leading)
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
      Text("Reset app data", comment: "About panel: the full app-data reset")
        .font(.body.weight(.semibold))
        .foregroundStyle(.primary)

      Text(
        "Moves recordings, transcripts, conversations, logs, preferences and local configuration to Trash. Your base prompts stay unless you choose otherwise below.",
        comment:
          "About panel: short scope of the app-data reset; the confirmation shows the full scope"
      )
      .font(.callout)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, CSSpace.xxs)

      // The two opt-ins read as one group above the button; both keep their
      // exact wording, including what Trash cannot bring back.
      VStack(alignment: .leading, spacing: CSSpace.sm) {
        Toggle(isOn: $includeKeys) {
          Text(
            "Also remove API keys from Keychain (not recoverable from Trash)",
            comment: "About panel: reset checkbox"
          )
          .font(.callout)
          .foregroundStyle(Color.primary)
        }
        .toggleStyle(.checkbox)

        Toggle(isOn: $includePrompts) {
          Text(
            "Also reset my base prompts (assistive.txt, formatting.txt, formatting-smart.txt and formatting-max.txt)",
            comment: "About panel: reset checkbox; the four file names stay verbatim"
          )
          .font(.callout)
          .foregroundStyle(Color.primary)
          .fixedSize(horizontal: false, vertical: true)
        }
        .toggleStyle(.checkbox)
        .accessibilityHint(
          "Off by default. When enabled, all four prompt files move to Trash with the rest of the app data."
        )
      }
      .padding(.top, CSSpace.md)

      DestructiveResetButton(title: String(localized: "Move app data to Trash…")) {
        model.refreshResetPreview()
        confirmationText = ""
        confirming = true
      }
      .padding(.top, CSSpace.md)
      .accessibilityLabel("Reset app data. Destructive action.")
      .accessibilityHint(
        "Shows the live impact, names whether base prompts are preserved, and requires typing \(resetConfirmationWord) before data moves to Trash."
      )
    }
    .frame(maxWidth: .infinity, alignment: .leading)
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
