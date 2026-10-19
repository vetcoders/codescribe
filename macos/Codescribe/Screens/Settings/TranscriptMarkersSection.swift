import SwiftUI

/// Dictation › Engine › Transcript markers: the switch that marks text
/// delivered to other apps, and the template editor behind a closed
/// disclosure. It moved here from the About page (Founder decision,
/// 2026-10-19): markers shape what a dictation delivers, so they belong with
/// dictation, not with the app's version and data.
struct TranscriptMarkersSection: View {
  @ObservedObject var model: SettingsViewModel
  @State private var showingTemplate = false

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionHeaderRow(
        String(localized: "Transcript markers", comment: "Dictation panel section")
      )
      SettingsControlRow(
        title: String(localized: "Add markers to text", comment: "Dictation panel: switch"),
        subtitle: String(
          localized: "Mark text delivered to other apps",
          comment: "Dictation panel: switch subtitle; the template decides the marker")
      ) {
        Toggle("", isOn: taggingBinding)
          .toggleStyle(.switch)
          .labelsHidden()
          .tint(CSColor.chromeAccent)
          .accessibilityLabel("Add markers to text")
          .accessibilityValue(model.settings.transcriptTaggingEnabled ? "On" : "Off")
      }
      .padding(.top, SettingsScale.headerGap)

      DisclosureGroup(isExpanded: $showingTemplate) {
        templateEditor
      } label: {
        Text(
          "Edit template and preview",
          comment: "Dictation panel: disclosure with the marker template editor"
        )
        .font(SettingsScale.helper)
        .foregroundStyle(.secondary)
      }
      .padding(.top, SettingsScale.headerGap)
      .accessibilityIdentifier("dictation-template-disclosure")
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
        .padding(.top, SettingsScale.headerGap)
        .accessibilityLabel("Transcript tag template editor")
        .accessibilityValue(model.settings.transcriptTagTemplate)

      if let warning = model.transcriptTagTemplateWarning {
        Text(warning)
          .font(SettingsScale.helper)
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
              comment: "Dictation panel: template field chip"))
        }
        Spacer(minLength: 0)
        Button(String(localized: "Restore default template", comment: "Dictation panel: button")) {
          model.restoreDefaultTranscriptTagTemplate()
        }
        .csFocusRing()
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.chromeAccent)
        .accessibilityLabel("Restore default transcript tag template")
      }
      .padding(.top, CSSpace.sm)

      Text("Template preview", comment: "Dictation panel: label over the rendered template")
        .font(SettingsScale.helper)
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

  // MARK: - Bindings

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
}
