import SwiftUI

// Prompt editor: edits the three user-owned formatting prompts and the assistive
// prompt. Each is
// loaded with source/path provenance, edited in a TextEditor, and saved back
// through the core's atomic writer. Restore is explicit and per prompt.
//
// NOTE: these edit only the BASE files; the core still appends its `*_tuning.txt`
// at runtime (not shown here).
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

      // `.id(file)` gives each file its own editor identity, so EDIT mode and
      // a pending restore confirmation never carry over to another file.
      PromptEditor(
        title: file.editorTitle,
        subtitle: file.editorSubtitle,
        text: $drafts[draftOf: file],
        snapshot: snapshots[file],
        onSave: save,
        onRestore: restore
      )
      .id(file)
      .padding(.top, CSSpace.lg)
    }
    .onAppear(perform: loadAllSnapshotsIfNeeded)
  }

  private func save() -> Bool {
    let content = drafts[draftOf: file]
    if let level = file.formattingLevel {
      return apply(model.saveFormattingPrompt(level, content: content))
    }
    return apply(model.saveAssistivePrompt(content))
  }

  private func restore() -> Bool {
    if let level = file.formattingLevel {
      return apply(model.restoreFormattingPromptToDefault(level))
    }
    return apply(model.restoreAssistivePromptToDefault())
  }

  /// A failed save/restore returns nil and must not claim a refreshed snapshot.
  private func apply(_ updated: CsPromptSnapshot?) -> Bool {
    guard let updated else { return false }
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
  let title: String
  let subtitle: String
  @Binding var text: String
  let snapshot: CsPromptSnapshot?
  let onSave: () -> Bool
  let onRestore: () -> Bool

  /// VIEW (rendered markdown) by default; EDIT (raw editor) on demand. Saving
  /// returns to VIEW so the persisted prompt is shown rendered.
  @State private var editing = false
  @State private var confirmingRestore = false

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(alignment: .firstTextBaseline, spacing: 10) {
        VStack(alignment: .leading, spacing: 2) {
          Text(title)
            .font(CSFont.ui(14, .semibold))
            .foregroundStyle(CSColor.textHigh)
          Text(subtitle)
            .font(CSFont.ui(11.5))
            .foregroundStyle(CSColor.textMutedAlt)
        }
        Spacer(minLength: 0)
        HStack(spacing: 8) {
          restoreButton
          toggleButton
        }
      }

      sourceTruth
        .padding(.top, 7)

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
        "Only this base prompt file will change. The previous version remains recoverable in the prompt backups folder."
      )
    }
  }

  /// Edit ⇄ Save toggle. In EDIT it persists and flips back to VIEW; in VIEW it
  /// enters EDIT.
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
        .font(CSFont.ui(12, .semibold))
        .foregroundStyle(CSColor.chromeAccent)
        .padding(.horizontal, 14)
        .padding(.vertical, 7)
        .background(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .fill(CSColor.chromeAccent.opacity(0.14))
        )
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .strokeBorder(CSColor.chromeAccent.opacity(0.28), lineWidth: 1)
        )
    }
    .csFocusRing()
    .help(editing ? "Save the prompt" : "Edit the raw markdown")
  }

  private var restoreButton: some View {
    Button("Restore…") {
      confirmingRestore = true
    }
    .csFocusRing()
    .font(CSFont.ui(11.5, .semibold))
    .foregroundStyle(CSColor.textMutedAlt)
    .help("Restore only \(title.lowercased())")
    .accessibilityHint("Requires confirmation and keeps a recoverable backup.")
  }

  private var sourceTruth: some View {
    VStack(alignment: .leading, spacing: 3) {
      Text(promptSourceLabel(snapshot?.source))
        .font(CSFont.mono(10.5, .semibold))
        .foregroundStyle(
          snapshot?.source == "read_error" ? CSColor.dangerLight : CSColor.textMutedAlt)
      Text(snapshot?.path ?? "Path unavailable")
        .font(CSFont.mono(10.5, .regular))
        .foregroundStyle(CSColor.textMuted)
        .textSelection(.enabled)
      if let error = snapshot?.readError, !error.isEmpty {
        Text(error)
          .font(CSFont.mono(10.5, .regular))
          .foregroundStyle(CSColor.dangerLight)
      }
    }
    .accessibilityElement(children: .combine)
    .accessibilityLabel("Prompt source")
    .accessibilityValue(
      "\(promptSourceLabel(snapshot?.source)), \(snapshot?.path ?? "path unavailable")")
  }

  @ViewBuilder
  private var content: some View {
    if editing {
      TextEditor(text: $text)
        .font(CSFont.mono(12.5, .regular))
        .foregroundStyle(CSColor.textBody)
        .scrollContentBackground(.hidden)
        .frame(minHeight: 132)
        .csSettingsCard(padding: CSSpace.md)
    } else {
      // Reuse the chat markdown renderer (MarkdownText, ChatComponents.swift):
      // it is dependency-free (DesignSystem tokens only) and carries headings,
      // bold/italic, lists, inline code, and fenced code blocks.
      MarkdownText(raw: text.isEmpty ? "_No prompt set._" : text, size: 13)
        .frame(maxWidth: .infinity, alignment: .leading)
        .frame(minHeight: 132, alignment: .topLeading)
        .csSettingsCard(padding: CSSpace.md)
    }
  }
}

func promptSourceLabel(_ source: String?) -> String {
  switch source {
  case "custom_file": return "Custom file"
  case "built_in_fallback": return "Built-in fallback"
  case "read_error": return "Read error"
  default: return "Source unavailable"
  }
}

#if DEBUG
  #Preview("Prompt panel") {
    ScrollView { PromptPanel(model: .preview(.agent)) }
      .frame(width: 720, height: 620)
      .background(CSColor.windowWash)
      .preferredColorScheme(.dark)
  }
#endif
