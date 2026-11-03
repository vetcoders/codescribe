import SwiftUI

/// Settings › General › Formatting: the formatter Keywords list. Names and
/// terms the user says often reach Correction, Smart and Max as context, so
/// a plausible near-miss from the recognizer can resolve to the familiar
/// spelling. The list is not a Dictionary rule (variant → canonical) and does
/// not bias speech recognition.
///
/// The view keeps an unsaved draft only. The saved list is
/// `SettingsViewModel.formatterKeywords`; Save sends the whole draft through
/// `saveFormatterKeywords`, and the normalized list the settings owner stored
/// replaces the draft. A refused save keeps the draft and its error on screen
/// until a later save succeeds or the draft is discarded.
struct FormatterKeywordsSection: View {
  @ObservedObject var model: SettingsViewModel

  @State private var rows: [String] = []
  @State private var newTerm = ""
  @State private var saving = false
  @State private var saveError: String?
  @State private var loaded = false
  @FocusState private var newTermFocused: Bool

  private var isDirty: Bool { rows != model.formatterKeywords }

  private var trimmedNewTerm: String {
    newTerm.trimmingCharacters(in: .whitespacesAndNewlines)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(
        String(localized: "Keywords", comment: "Formatter Keywords section title"))

      Text(
        "Names and terms you often say. Correction, Smart and Max use them as context and prefer them when the transcript heard something similar."
      )
      .font(SettingsScale.helper)
      .foregroundStyle(CSColor.secondaryInk)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, SettingsScale.headerGap)

      Text(
        "Dictionary rules, which always replace one spelling with another, and speech recognition stay separate."
      )
      .font(SettingsScale.helper)
      .foregroundStyle(CSColor.secondaryInk)
      .fixedSize(horizontal: false, vertical: true)

      if rows.isEmpty {
        Text("No keywords yet. Formatting works from the transcript alone.")
          .font(SettingsScale.helper)
          .foregroundStyle(CSColor.secondaryInk)
          .padding(.top, SettingsScale.rowGap)
          .accessibilityIdentifier("settings-formatter-keywords-empty")
      } else {
        VStack(spacing: SettingsScale.rowGap) {
          ForEach(rows.indices, id: \.self) { index in
            keywordRow(index: index)
          }
        }
        .padding(.top, SettingsScale.rowGap)
      }

      addRow
        .padding(.top, SettingsScale.rowGap)

      if let saveError {
        VStack(alignment: .leading, spacing: 2) {
          Text("Keywords were not saved.")
            .font(SettingsScale.helper.weight(.semibold))
          Text(verbatim: saveError)
            .font(SettingsScale.helper)
            .textSelection(.enabled)
        }
        .foregroundStyle(CSColor.dangerLight)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.top, SettingsScale.rowGap)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("settings-formatter-keywords-error")
      }

      actionRow
        .padding(.top, SettingsScale.rowGap)
    }
    .onAppear {
      guard !loaded else { return }
      loaded = true
      rows = model.formatterKeywords
    }
    // A reload that changed the saved list replaces the draft only when the
    // draft still matched the previous saved list; unsaved edits stay.
    .onChange(of: model.formatterKeywords) { previous, current in
      if rows == previous { rows = current }
    }
  }

  private func keywordRow(index: Int) -> some View {
    HStack(spacing: CSSpace.sm) {
      TextField(
        "Keyword",
        text: Binding(
          get: { index < rows.count ? rows[index] : "" },
          set: { if index < rows.count { rows[index] = $0 } }
        )
      )
      .textFieldStyle(.plain)
      .font(SettingsScale.controlLabel)
      .foregroundStyle(Color.primary)
      .frame(maxWidth: .infinity, alignment: .leading)
      .disabled(saving)

      Button {
        if index < rows.count { rows.remove(at: index) }
      } label: {
        CSIconView(icon: .remove, size: 13, weight: .semibold, color: CSColor.secondaryInk)
      }
      .csFocusRing()
      .disabled(saving)
      .help("Remove keyword")
      .accessibilityLabel("Remove keyword")
    }
    .padding(.horizontal, SettingsScale.rowInsetH)
    .padding(.vertical, SettingsScale.rowInsetV)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
        .fill(Color.primary.opacity(CSAlpha.fill))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
        .strokeBorder(Color.primary.opacity(CSAlpha.stroke), lineWidth: 1)
    )
  }

  private var addRow: some View {
    HStack(spacing: CSSpace.sm) {
      TextField("Add a name or term", text: $newTerm)
        .textFieldStyle(.plain)
        .focused($newTermFocused)
        .font(SettingsScale.controlLabel)
        .foregroundStyle(Color.primary)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, SettingsScale.rowInsetH)
        .padding(.vertical, SettingsScale.rowInsetV)
        .background(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .fill(Color.primary.opacity(CSAlpha.fill))
        )
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .strokeBorder(Color.primary.opacity(CSAlpha.stroke), lineWidth: 1)
        )
        .overlay {
          CSFocusOutline(isFocused: newTermFocused, cornerRadius: CSRadius.input)
        }
        .onSubmit(addTerm)
        .disabled(saving)
        .accessibilityIdentifier("settings-formatter-keywords-new")

      SettingsChipButton(enabled: !trimmedNewTerm.isEmpty && !saving, action: addTerm) {
        Label("Add", systemImage: "plus")
          .font(SettingsScale.controlLabel)
          .foregroundStyle(trimmedNewTerm.isEmpty ? CSColor.secondaryInk : Color.primary)
      }
      .help("Add the term to the list. Nothing is saved until Save changes.")
    }
  }

  private var actionRow: some View {
    HStack(spacing: CSSpace.sm) {
      if isDirty {
        Text("Saved keywords apply from the next take.")
          .font(SettingsScale.helper)
          .foregroundStyle(CSColor.secondaryInk)
      }

      Spacer()

      if isDirty || saveError != nil {
        SettingsChipButton(enabled: !saving, action: discard) {
          Text("Discard changes")
            .font(SettingsScale.controlLabel)
            .foregroundStyle(CSColor.secondaryInk)
        }
        .help("Go back to the saved list")
      }

      if saving {
        ProgressView()
          .controlSize(.small)
          .accessibilityLabel("Saving keywords")
      }

      SettingsChipButton(enabled: isDirty && !saving, action: save) {
        Text("Save changes")
          .font(SettingsScale.controlLabel)
          .foregroundStyle(isDirty && !saving ? Color.primary : CSColor.secondaryInk)
      }
      .accessibilityIdentifier("settings-formatter-keywords-save")
    }
  }

  /// Adds the typed term to the draft. Spelling, duplicates and limits are
  /// the settings owner's decision at Save, not the view's.
  private func addTerm() {
    let term = trimmedNewTerm
    guard !term.isEmpty, !saving else { return }
    rows.append(term)
    newTerm = ""
    newTermFocused = true
  }

  private func discard() {
    rows = model.formatterKeywords
    saveError = nil
  }

  private func save() {
    guard !saving else { return }
    saving = true
    let draft = rows
    Task { @MainActor in
      defer { saving = false }
      do {
        rows = try await model.saveFormatterKeywords(draft)
        saveError = nil
      } catch {
        saveError = error.userFacingMessage
      }
    }
  }
}
