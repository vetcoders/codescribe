import SwiftUI

// Prompt editor: edits the three user-owned formatting prompts and the Agent
// prompt. Each is loaded with source/path provenance, edited in a TextEditor,
// and saved back through the core's atomic writer. Restore is explicit and per
// prompt.
//
// NOTE: these edit only the BASE files; the core may still append its
// `*_tuning.txt` at runtime (not shown here).
//
// Lives on Agent › Prompts — the one home for every prompt file. The four files
// used to be four sidebar rows; now a segmented picker switches the editor.

struct PromptPanel: View {
  @ObservedObject var model: SettingsViewModel

  /// Which prompt file the editor shows. View state: the Agent tab bar owns
  /// the Prompts tab, this picks one of its four files.
  @State private var file: PromptFile = .correction
  @State private var drafts: [PromptFile: String] = [:]
  @State private var snapshots: [PromptFile: CsPromptSnapshot] = [:]
  /// Files open in EDIT. Kept per file (not inside the editor) so a prompt
  /// left mid-edit comes back as the same unsaved draft in EDIT, never as a
  /// rendered "saved" version.
  @State private var editingFiles: Set<PromptFile> = []
  /// The last save/restore that returned no refreshed snapshot, per file. A
  /// failed restore is shown as a failure, never as a completed restore.
  @State private var failures: [PromptFile: PromptOperationFailure] = [:]

  /// One prompt at a time. Four stacked TextEditors in a single scroll meant
  /// every visit wheeled past prompts you did not come for.
  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      Picker("Prompt file", selection: $file) {
        ForEach(PromptFile.allCases) { file in
          Text(file.title).tag(file)
        }
      }
      .pickerStyle(.segmented)
      .labelsHidden()
      .fixedSize()
      .accessibilityIdentifier("settings-prompt-file")

      // `.id(file)` gives each file its own editor identity, so a pending
      // restore confirmation and focus never carry over to another file.
      PromptEditor(
        file: file,
        draft: $drafts[draftOf: file],
        editing: editing(of: file),
        snapshot: snapshots[file],
        failure: failures[file],
        onSave: save,
        onRestore: restore,
        onDiscard: discard
      )
      .id(file)
      .padding(.top, CSSpace.md)
    }
    .onAppear(perform: loadAllSnapshotsIfNeeded)
  }

  private func editing(of file: PromptFile) -> Binding<Bool> {
    Binding(
      get: { editingFiles.contains(file) },
      set: { open in
        if open {
          editingFiles.insert(file)
        } else {
          editingFiles.remove(file)
        }
      }
    )
  }

  private func save() -> Bool {
    let content = drafts[draftOf: file]
    if let level = file.formattingLevel {
      return apply(model.saveFormattingPrompt(level, content: content), .save)
    }
    return apply(model.saveAssistivePrompt(content), .save)
  }

  /// The engine backs the custom file up and removes it; the refreshed
  /// snapshot then reads "Built-in prompt". Only the shown file is touched.
  private func restore() -> Bool {
    if let level = file.formattingLevel {
      return apply(model.restoreFormattingPromptToDefault(level), .restore)
    }
    return apply(model.restoreAssistivePromptToDefault(), .restore)
  }

  /// Drops the unsaved draft of the shown file; the saved snapshot stands.
  private func discard() {
    drafts[file] = snapshots[file]?.content ?? ""
    editingFiles.remove(file)
    failures[file] = nil
  }

  /// A failed save/restore returns nil and must not claim a refreshed snapshot:
  /// the previous snapshot stands and the failure is shown under the source.
  private func apply(_ updated: CsPromptSnapshot?, _ operation: PromptOperationFailure.Operation)
    -> Bool
  {
    guard let updated else {
      let detail = model.lastError ?? ""
      if let level = file.formattingLevel {
        if let current = model.formattingPromptSnapshot(level: level) { snapshots[file] = current }
      } else {
        snapshots[file] = model.assistivePromptSnapshot()
      }
      failures[file] = PromptOperationFailure(operation: operation, detail: detail)
      return false
    }
    failures[file] = nil
    drafts[file] = updated.content
    snapshots[file] = updated
    return true
  }

  private func loadAllSnapshotsIfNeeded() {
    guard snapshots.isEmpty else { return }
    let formattingLoaded =
      model.formattingPromptSnapshot(level: .correction)
      ?? model.formattingPromptSnapshot()
    let smartLoaded = model.formattingPromptSnapshot(level: .smart)
    let maxLoaded = model.formattingPromptSnapshot(level: .max)
    let assistiveLoaded = model.assistivePromptSnapshot()
    drafts = [
      .correction: formattingLoaded.content,
      .smart: smartLoaded?.content ?? "",
      .max: maxLoaded?.content ?? "",
      .assistive: assistiveLoaded.content,
    ]
    snapshots = [.correction: formattingLoaded, .assistive: assistiveLoaded]
    snapshots[.smart] = smartLoaded
    snapshots[.max] = maxLoaded
  }
}

/// Unsaved text per prompt file; a file never loaded reads as empty. A named
/// subscript (not `[key, default:]`) so the editor can bind to it by key path.
extension Dictionary where Key == PromptFile, Value == String {
  fileprivate subscript(draftOf file: PromptFile) -> String {
    get { self[file] ?? "" }
    set { self[file] = newValue }
  }
}

// MARK: - Single prompt editor block

private struct PromptEditor: View {
  let file: PromptFile
  @Binding var draft: String
  @Binding var editing: Bool
  let snapshot: CsPromptSnapshot?
  let failure: PromptOperationFailure?
  let onSave: () -> Bool
  let onRestore: () -> Bool
  let onDiscard: () -> Void

  @State private var confirmingRestore = false
  @State private var detailsExpanded = false
  @FocusState private var editorFocused: Bool

  private var title: String { file.editorTitle }

  /// What is on disk (or the built-in text standing in for it). VIEW renders
  /// this, never the draft, so an unsaved edit cannot pose as the saved prompt.
  private var savedText: String { snapshot?.content ?? "" }

  private var hasUnsavedChanges: Bool { editing && draft != savedText }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      header

      if snapshot?.source == "read_error" {
        Text("The custom prompt file could not be read, so the built-in prompt is in use.")
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.danger)
          .fixedSize(horizontal: false, vertical: true)
          .padding(.top, CSSpace.xxs)
      }

      if let failure {
        failureLine(failure)
          .padding(.top, CSSpace.xs)
      }

      promptBody
        .padding(.top, CSSpace.md)

      fileDetails
        .padding(.top, CSSpace.sm)
    }
    .alert("Restore \(title) to the built-in default?", isPresented: $confirmingRestore) {
      Button("Cancel", role: .cancel) {}
      Button("Restore this prompt", role: .destructive) {
        if onRestore() {
          editing = false
        }
      }
    } message: {
      Text(
        "Only \(title) will change: its custom file is removed and the built-in prompt takes over. The previous version remains recoverable in the prompt backups folder."
      )
    }
  }

  /// Name, one purpose sentence, the quiet source tag and the actions on one
  /// line (Founder brief, round 9, 2026-10-10). Everything technical — path,
  /// whether a custom file exists, where saving would create one — used to sit
  /// as separate rows between the name and the prompt; it is under File details
  /// now, so the prompt itself starts one line below its own name.
  private var header: some View {
    HStack(alignment: .firstTextBaseline, spacing: CSSpace.md) {
      VStack(alignment: .leading, spacing: 2) {
        Text(title)
          .font(CSFont.ui(14, .semibold))
          .foregroundStyle(Color.primary)
        Text(file.editorSubtitle)
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
      Spacer(minLength: CSSpace.sm)
      // Hugs its own width so a long prompt name or translation wraps on the
      // left instead of squeezing the tag and the buttons.
      HStack(spacing: CSSpace.sm) {
        sourceTag
        if hasUnsavedChanges {
          Text("Unsaved changes")
            .font(CSFont.ui(11.5, .medium))
            .foregroundStyle(CSColor.chromeAccent)
        }
        if editing {
          cancelButton
        }
        toggleButton
      }
      .fixedSize()
    }
  }

  /// Which prompt the app is actually using, as a quiet capsule instead of a
  /// row of its own. The full sentence stays as the VoiceOver value, and the
  /// file state it used to carry lives under File details.
  @ViewBuilder
  private var sourceTag: some View {
    if let tag = promptSourceTag(snapshot?.source) {
      Text(tag)
        .font(CSFont.ui(10.5, .semibold))
        .lineLimit(1)
        .foregroundStyle(Color.secondary)
        .padding(.horizontal, 7)
        .padding(.vertical, 3)
        .background(Capsule().fill(Color.primary.opacity(0.08)))
        .overlay(Capsule().strokeBorder(Color.primary.opacity(0.12), lineWidth: 1))
        .accessibilityElement()
        .accessibilityLabel("Prompt source")
        .accessibilityValue(Text(verbatim: promptSourceLabel(snapshot?.source)))
    }
  }

  /// The prompt itself. In VIEW a quiet caption names what the card holds, so
  /// the rendered text is not mistaken for an editable field.
  private var promptBody: some View {
    VStack(alignment: .leading, spacing: CSSpace.xxs) {
      if !editing {
        Text("Preview of the original prompt text.")
          .font(CSFont.ui(11))
          .foregroundStyle(Color.secondary)
      }
      content
    }
  }

  /// Edit ⇄ Save toggle. In EDIT it persists and flips back to VIEW; in VIEW it
  /// enters EDIT. Save is the solid accent button so the committing action is
  /// unmistakable next to the tinted Edit.
  private var toggleButton: some View {
    Button(action: {
      if editing {
        if onSave() {
          editing = false
        }
      } else {
        editing = true
      }
    }) {
      Text(editing ? "Save" : "Edit")
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(editing ? Color.white : CSColor.chromeAccent)
        .padding(.horizontal, 16)
        .padding(.vertical, 7)
        .background(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .fill(CSColor.chromeAccent.opacity(editing ? 1 : 0.14))
        )
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .strokeBorder(CSColor.chromeAccent.opacity(editing ? 0 : 0.28), lineWidth: 1)
        )
    }
    .csFocusRing()
    .help(editing ? "Save the prompt" : "Edit the raw markdown")
  }

  /// Leaves EDIT without writing: the draft reverts to the saved prompt.
  private var cancelButton: some View {
    Button("Cancel", action: onDiscard)
      .csFocusRing()
      .font(CSFont.ui(11.5, .semibold))
      .foregroundStyle(Color.secondary)
      .help("Discard unsaved changes")
  }

  /// Sits on the File details line, not beside Edit: with only the built-in
  /// prompt and no custom file there is nothing for the engine to remove
  /// (`restore_prompt_to_default` then writes an audit receipt and returns), so
  /// the action does not hold a place in the header (Founder brief, round 9,
  /// 2026-10-10). Disabled rather than hidden, so the line does not reflow when
  /// a custom file appears or goes away.
  private var restoreButton: some View {
    Button("Restore default") {
      confirmingRestore = true
    }
    .csFocusRing()
    .font(CSFont.ui(11.5, .semibold))
    .foregroundStyle(Color.secondary)
    .disabled(!canRestore)
    .help(
      canRestore
        ? Text("Restore only \(title)")
        : Text("The built-in prompt is already in use.")
    )
    .accessibilityHint("Requires confirmation and keeps a recoverable backup.")
  }

  /// Whether restoring can change anything: a custom file to back up and
  /// remove. An unreadable override counts — it is the state restoring reports
  /// on — and so does an existing empty file, which the built-in text stands in
  /// for until it is removed.
  private var canRestore: Bool {
    guard let source = snapshot?.source else { return false }
    return source == "custom_file" || source == "read_error" || fileExists
  }

  /// Why the last Save or Restore did nothing. The source tag above still
  /// names the prompt actually in use, so a failed restore cannot pose as done.
  private func failureLine(_ failure: PromptOperationFailure) -> some View {
    VStack(alignment: .leading, spacing: 2) {
      Text(promptFailureLabel(failure.operation, title: title))
        .font(CSFont.ui(11.5, .medium))
        .foregroundStyle(CSColor.danger)
      if !failure.detail.isEmpty {
        Text(failure.detail)
          .font(CSFont.mono(10.5, .regular))
          .foregroundStyle(CSColor.danger)
          .textSelection(.enabled)
      }
    }
    .accessibilityElement(children: .combine)
    .accessibilityIdentifier("settings-prompt-failure")
  }

  /// Collapsed by default and the last line of the panel: the on-disk path,
  /// whether a custom file exists or is created on save, and the raw read error
  /// when there is one. Restore default rides on the same line.
  private var fileDetails: some View {
    DisclosureGroup(isExpanded: $detailsExpanded) {
      VStack(alignment: .leading, spacing: CSSpace.xxs) {
        HStack(alignment: .firstTextBaseline, spacing: CSSpace.sm) {
          Text("Path")
            .font(CSFont.ui(11))
            .foregroundStyle(Color.secondary)
          // One line, truncated in the middle: a deep path must not widen the
          // pane past the minimum window width.
          Text(pathDisplay)
            .font(CSFont.mono(10.5, .regular))
            .foregroundStyle(Color.secondary)
            .lineLimit(1)
            .truncationMode(.middle)
            .textSelection(.enabled)
        }
        Text(promptFileStatus(source: snapshot?.source, fileExists: fileExists))
          .font(CSFont.ui(11))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
        if let error = snapshot?.readError, !error.isEmpty {
          HStack(alignment: .firstTextBaseline, spacing: CSSpace.sm) {
            Text("Read error")
              .font(CSFont.ui(11))
              .foregroundStyle(CSColor.danger)
            Text(error)
              .font(CSFont.mono(10.5, .regular))
              .foregroundStyle(CSColor.danger)
              .textSelection(.enabled)
          }
        }
      }
      .padding(.top, CSSpace.xxs)
    } label: {
      HStack(alignment: .firstTextBaseline, spacing: CSSpace.sm) {
        Text("File details")
          .font(CSFont.ui(11.5, .medium))
          .foregroundStyle(Color.secondary)
        Spacer(minLength: CSSpace.sm)
        restoreButton
      }
    }
    .accessibilityIdentifier("settings-prompt-file-details")
  }

  /// Provenance path, or the authored fallback when no snapshot loaded.
  private var pathDisplay: String {
    snapshot?.path ?? String(localized: "Path unavailable")
  }

  private var fileExists: Bool {
    guard let path = snapshot?.path, !path.isEmpty else { return false }
    return FileManager.default.fileExists(atPath: (path as NSString).expandingTildeInPath)
  }

  @ViewBuilder
  private var content: some View {
    if editing {
      TextEditor(text: $draft)
        .focused($editorFocused)
        .font(CSFont.mono(12.5, .regular))
        .foregroundStyle(Color.primary)
        .scrollContentBackground(.hidden)
        .frame(minHeight: 132)
        .settingsGroupedInset(padding: CSSpace.md)
        .overlay {
          CSFocusOutline(isFocused: editorFocused, cornerRadius: CSRadius.card)
        }
    } else {
      // Reuse the chat markdown renderer (MarkdownText, ChatComponents.swift):
      // it is dependency-free (DesignSystem tokens only) and carries headings,
      // bold/italic, lists, inline code, and fenced code blocks.
      MarkdownText(
        raw: savedText.isEmpty
          ? String(
            localized: "_No prompt set._",
            comment: "Placeholder for an empty prompt file; underscores render as italic")
          : savedText,
        size: 13
      )
      .frame(maxWidth: .infinity, alignment: .leading)
      .frame(minHeight: 132, alignment: .topLeading)
      .settingsGroupedInset(padding: CSSpace.md)
    }
  }
}

/// The prompt the app runs with, named for people: a custom file or the
/// built-in text. A read error still means the built-in text is in use.
func promptSourceLabel(_ source: String?) -> String {
  switch source {
  case "custom_file": return String(localized: "Source: Custom prompt")
  case "built_in_fallback": return String(localized: "Source: Built-in prompt")
  case "read_error": return String(localized: "Source: Built-in prompt (file unreadable)")
  default: return String(localized: "Source unavailable")
  }
}

/// The header tag: one word for the same truth as `promptSourceLabel`, which
/// stays the VoiceOver value. `nil` when no snapshot loaded, so an unknown
/// source shows no tag at all instead of an empty capsule.
///
/// Keyed, not literal: "Custom" already names an unrelated preset elsewhere in
/// Settings, and these two tags must agree in gender with the word "prompt" in
/// languages that inflect.
func promptSourceTag(_ source: String?) -> String? {
  switch source {
  case "custom_file":
    return String(
      localized: "settings.prompt.source.custom", defaultValue: "Custom",
      comment: "Tag next to the prompt name: a custom prompt file is in use")
  case "built_in_fallback", "read_error":
    return String(
      localized: "settings.prompt.source.builtIn", defaultValue: "Built-in",
      comment: "Tag next to the prompt name: the built-in prompt text is in use")
  default: return nil
  }
}

/// A Save or Restore that returned no refreshed snapshot, with the engine's
/// error text. Equatable so tests can assert the exact failure shown.
struct PromptOperationFailure: Equatable {
  enum Operation: Equatable {
    case save
    case restore
  }

  let operation: Operation
  let detail: String
}

/// Names the failed operation and states what did not change, so the line
/// cannot be read as a success in either direction.
func promptFailureLabel(_ operation: PromptOperationFailure.Operation, title: String) -> String {
  switch operation {
  case .save:
    return String(localized: "Could not complete saving \(title). Check the current source shown above.")
  case .restore:
    return String(localized: "Could not complete restoring \(title). Check the current source shown above.")
  }
}

/// File-details sentence: an existing custom file, an existing-but-empty file,
/// or that saving creates one. The path sits on the line above, so the sentence
/// no longer repeats it (Founder brief, round 9, 2026-10-10).
func promptFileStatus(source: String?, fileExists: Bool) -> String {
  switch source {
  case "custom_file":
    return String(localized: "Custom prompt file in use.")
  case "built_in_fallback":
    return fileExists
      ? String(localized: "The file exists but is empty, so the built-in prompt is in use.")
      : String(localized: "The custom file is created on save.")
  case "read_error":
    return String(localized: "The file exists but could not be read.")
  default:
    return String(localized: "Path unavailable")
  }
}

#if DEBUG
  #Preview("Prompt panel") {
    ScrollView { PromptPanel(model: .preview(.agent)) }
      .frame(width: 720, height: 620)
  }
#endif
