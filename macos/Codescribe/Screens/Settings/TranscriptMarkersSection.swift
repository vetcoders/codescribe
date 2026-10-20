import SwiftUI

/// Dictation › Engine › Transcript markers: the switch that marks text
/// delivered to other apps, and the template editor behind a closed
/// disclosure. It moved here from the About page (Founder decision,
/// 2026-10-19): markers shape what a dictation delivers, so they belong with
/// dictation, not with the app's version and data.
struct TranscriptMarkersSection: View {
  @ObservedObject var model: SettingsViewModel
  @State private var showingTemplate = false
  @State private var templateView: TemplateView = .edit

  /// One block shows either the raw template or its rendering; the segment
  /// above it flips between them (Founder, 2026-10-20: one line, not two
  /// stacked boxes).
  enum TemplateView: Hashable {
    case edit
    case preview
  }

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
          .settingsSwitch()
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
      HStack {
        Spacer(minLength: 0)
        Picker("Template view", selection: $templateView) {
          Text("Edit", comment: "Dictation panel: template segment, the raw template")
            .tag(TemplateView.edit)
          Text("Preview", comment: "Dictation panel: template segment, the rendered template")
            .tag(TemplateView.preview)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
        .accessibilityIdentifier("dictation-template-view")
      }
      .padding(.top, SettingsScale.headerGap)

      // The disclosure above already names this editor, so the field carries
      // no second label of its own (Founder brief, round 16, 2026-10-10).
      // The same box shows the rendering when the segment says Preview.
      Group {
        switch templateView {
        case .edit:
          TextField("Transcript tag template", text: transcriptTemplateBinding, axis: .vertical)
            .font(CSFont.mono(11.5, .regular))
            .foregroundStyle(Color.primary)
            .textFieldStyle(.plain)
            .lineLimit(3...8)
            .accessibilityLabel("Transcript tag template editor")
            .accessibilityValue(model.settings.transcriptTagTemplate)
        case .preview:
          Text(model.transcriptTagPreview)
            .font(CSFont.mono(11.5, .regular))
            .foregroundStyle(Color.primary)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityLabel("Transcript tag template preview")
            .accessibilityValue(model.transcriptTagPreview)
        }
      }
      .settingsGroupedInset()
      .padding(.top, CSSpace.xs)

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
            templateView = .edit
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
