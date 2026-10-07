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
      .padding(.top, CSSpace.lg)
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
      failures[file] = PromptOperationFailure(
        operation: operation, detail: model.lastError ?? "")
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
      HStack(alignment: .firstTextBaseline, spacing: 10) {
        VStack(alignment: .leading, spacing: 2) {
          Text(title)
            .font(CSFont.ui(14, .semibold))
            .foregroundStyle(Color.primary)
          Text(file.editorSubtitle)
            .font(CSFont.ui(11.5))
            .foregroundStyle(Color.secondary)
        }
        Spacer(minLength: 0)
        HStack(spacing: 8) {
          restoreButton
          if editing {
            cancelButton
          }
          toggleButton
        }
      }

      sourceLine
        .padding(.top, 7)

      if let failure {
        failureLine(failure)
          .padding(.top, 6)
      }

      fileDetails
        .padding(.top, 4)

      content
        .padding(.top, CSSpace.control)
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

  private var restoreButton: some View {
    Button("Restore default…") {
      confirmingRestore = true
    }
    .csFocusRing()
    .font(CSFont.ui(11.5, .semibold))
    .foregroundStyle(Color.secondary)
    .help("Restore only \(title)")
    .accessibilityHint("Requires confirmation and keeps a recoverable backup.")
  }

  /// Which prompt the app is actually using, in words. The path and the file
  /// state sit under File details.
  private var sourceLine: some View {
    VStack(alignment: .leading, spacing: 3) {
      HStack(spacing: 10) {
        Text(promptSourceLabel(snapshot?.source))
          .font(CSFont.ui(11.5, .medium))
          .foregroundStyle(Color.secondary)
        if hasUnsavedChanges {
          Text("Unsaved changes")
            .font(CSFont.ui(11.5, .medium))
            .foregroundStyle(CSColor.chromeAccent)
        }
      }
      if snapshot?.source == "read_error" {
        Text("The custom prompt file could not be read, so the built-in prompt is in use.")
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.danger)
      }
    }
    .accessibilityElement(children: .combine)
    .accessibilityLabel("Prompt source")
    .accessibilityValue(Text(verbatim: promptSourceLabel(snapshot?.source)))
  }

  /// Why the last Save or Restore did nothing. The source line above still
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

  /// Collapsed by default: the on-disk path, whether a custom file exists or
  /// where saving would create one, and the raw read error when there is one.
  private var fileDetails: some View {
    DisclosureGroup(isExpanded: $detailsExpanded) {
      VStack(alignment: .leading, spacing: 4) {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
          Text("Path")
            .font(CSFont.ui(11))
            .foregroundStyle(Color.secondary)
          Text(pathDisplay)
            .font(CSFont.mono(10.5, .regular))
            .foregroundStyle(Color.secondary)
            .textSelection(.enabled)
        }
        Text(promptFileStatus(source: snapshot?.source, fileExists: fileExists))
          .font(CSFont.ui(11))
          .foregroundStyle(Color.secondary)
        if let error = snapshot?.readError, !error.isEmpty {
          HStack(alignment: .firstTextBaseline, spacing: 8) {
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
      .padding(.top, 4)
    } label: {
      Text("File details")
        .font(CSFont.ui(11.5, .medium))
        .foregroundStyle(Color.secondary)
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
    return String(localized: "Could not save \(title). The file on disk is unchanged.")
  case .restore:
    return String(localized: "Could not restore \(title). The custom prompt is still in use.")
  }
}

/// File-details sentence: an existing custom file, an existing-but-empty file,
/// or the path a custom prompt would be created at.
func promptFileStatus(source: String?, fileExists: Bool) -> String {
  switch source {
  case "custom_file":
    return String(localized: "Custom prompt file in use.")
  case "built_in_fallback":
    return fileExists
      ? String(localized: "The file exists but is empty, so the built-in prompt is in use.")
      : String(localized: "No custom prompt file yet. Saving creates one at this path.")
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
