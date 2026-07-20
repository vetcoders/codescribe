import SwiftUI

// Prompt editor: edits the three user-owned formatting prompts and the assistive
// prompt. Each is
// loaded with source/path provenance, edited in a TextEditor, and saved back
// through the core's atomic writer. Restore is explicit and per prompt.
//
// NOTE: these edit only the BASE files; the core still appends its `*_tuning.txt`
// at runtime (not shown here).

struct PromptPanel: View {
    @ObservedObject var model: SettingsViewModel

    @State private var formatting: String = ""
    @State private var formattingSmart: String = ""
    @State private var formattingMax: String = ""
    @State private var assistive: String = ""
    @State private var formattingSnapshot: CsPromptSnapshot?
    @State private var formattingSmartSnapshot: CsPromptSnapshot?
    @State private var formattingMaxSnapshot: CsPromptSnapshot?
    @State private var assistiveSnapshot: CsPromptSnapshot?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            EyebrowLabel(text: "Settings · Prompts")
            Text("Prompt editor.")
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
                .padding(.top, 6)

            Text("Edits the BASE prompt files. The core still appends its tuning prompt at runtime.")
                .font(CSFont.ui(12.5))
                .lineSpacing(2)
                .foregroundStyle(CSColor.textMutedAlt)
                .padding(.top, 8)

            PromptEditor(
                title: "Correction prompt",
                subtitle: "Correction only AI formatting (formatting.txt)",
                text: $formatting,
                snapshot: formattingSnapshot,
                onSave: {
                    guard let updated = model.saveFormattingPrompt(.correction, content: formatting) else { return false }
                    formatting = updated.content
                    formattingSnapshot = updated
                    return true
                },
                onRestore: {
                    guard let updated = model.restoreFormattingPromptToDefault(.correction) else { return false }
                    formatting = updated.content
                    formattingSnapshot = updated
                    return true
                }
            )
            .padding(.top, 22)

            PromptEditor(
                title: "Smart prompt",
                subtitle: "Balanced transcript editing (formatting-smart.txt)",
                text: $formattingSmart,
                snapshot: formattingSmartSnapshot,
                onSave: {
                    guard let updated = model.saveFormattingPrompt(.smart, content: formattingSmart) else { return false }
                    formattingSmart = updated.content
                    formattingSmartSnapshot = updated
                    return true
                },
                onRestore: {
                    guard let updated = model.restoreFormattingPromptToDefault(.smart) else { return false }
                    formattingSmart = updated.content
                    formattingSmartSnapshot = updated
                    return true
                }
            )
            .padding(.top, 18)

            PromptEditor(
                title: "Max prompt",
                subtitle: "Maximum supported prose polish (formatting-max.txt)",
                text: $formattingMax,
                snapshot: formattingMaxSnapshot,
                onSave: {
                    guard let updated = model.saveFormattingPrompt(.max, content: formattingMax) else { return false }
                    formattingMax = updated.content
                    formattingMaxSnapshot = updated
                    return true
                },
                onRestore: {
                    guard let updated = model.restoreFormattingPromptToDefault(.max) else { return false }
                    formattingMax = updated.content
                    formattingMaxSnapshot = updated
                    return true
                }
            )
            .padding(.top, 18)

            PromptEditor(
                title: "Assistive prompt",
                subtitle: "Base system prompt for the voice assistant (assistive.txt)",
                text: $assistive,
                snapshot: assistiveSnapshot,
                onSave: {
                    guard let updated = model.saveAssistivePrompt(assistive) else { return false }
                    assistive = updated.content
                    assistiveSnapshot = updated
                    return true
                },
                onRestore: {
                    guard let updated = model.restoreAssistivePromptToDefault() else { return false }
                    assistive = updated.content
                    assistiveSnapshot = updated
                    return true
                }
            )
            .padding(.top, 18)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
        .onAppear {
            guard formattingSnapshot == nil, formattingSmartSnapshot == nil,
                  formattingMaxSnapshot == nil, assistiveSnapshot == nil else { return }
            let formattingLoaded = model.formattingPromptSnapshot(level: .correction)
                ?? model.formattingPromptSnapshot()
            let smartLoaded = model.formattingPromptSnapshot(level: .smart)
            let maxLoaded = model.formattingPromptSnapshot(level: .max)
            let assistiveLoaded = model.assistivePromptSnapshot()
            formatting = formattingLoaded.content
            formattingSmart = smartLoaded?.content ?? ""
            formattingMax = maxLoaded?.content ?? ""
            assistive = assistiveLoaded.content
            formattingSnapshot = formattingLoaded
            formattingSmartSnapshot = smartLoaded
            formattingMaxSnapshot = maxLoaded
            assistiveSnapshot = assistiveLoaded
        }
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
                .padding(.top, 11)
        }
        .alert("Restore \(title) to the built-in default?", isPresented: $confirmingRestore) {
            Button("Cancel", role: .cancel) {}
            Button("Restore this prompt", role: .destructive) {
                if onRestore() {
                    editing = false
                }
            }
        } message: {
            Text("Only this base prompt file will change. The previous version remains recoverable in the prompt backups folder.")
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
                .foregroundStyle(CSColor.terracottaLight)
                .padding(.horizontal, 14)
                .padding(.vertical, 7)
                .background(
                    RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                        .fill(CSColor.terracotta.opacity(0.14))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                        .strokeBorder(CSColor.terracotta.opacity(0.28), lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
        .help(editing ? "Save the prompt" : "Edit the raw markdown")
    }

    private var restoreButton: some View {
        Button("Restore…") {
            confirmingRestore = true
        }
        .buttonStyle(.plain)
        .font(CSFont.ui(11.5, .semibold))
        .foregroundStyle(CSColor.textMutedAlt)
        .help("Restore only \(title.lowercased())")
        .accessibilityHint("Requires confirmation and keeps a recoverable backup.")
    }

    private var sourceTruth: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(promptSourceLabel(snapshot?.source))
                .font(CSFont.mono(10.5, .semibold))
                .foregroundStyle(snapshot?.source == "read_error" ? CSColor.dangerLight : CSColor.textMutedAlt)
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
        .accessibilityValue("\(promptSourceLabel(snapshot?.source)), \(snapshot?.path ?? "path unavailable")")
    }

    @ViewBuilder
    private var content: some View {
        if editing {
            TextEditor(text: $text)
                .font(CSFont.mono(12.5, .regular))
                .foregroundStyle(CSColor.textBody)
                .scrollContentBackground(.hidden)
                .padding(10)
                .frame(minHeight: 132)
                .background(card)
                .overlay(cardBorder)
        } else {
            // Reuse the chat markdown renderer (MarkdownText, ChatComponents.swift):
            // it is dependency-free (DesignSystem tokens only) and carries headings,
            // bold/italic, lists, inline code, and fenced code blocks.
            MarkdownText(raw: text.isEmpty ? "_No prompt set._" : text, size: 13)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
                .frame(minHeight: 132, alignment: .topLeading)
                .background(card)
                .overlay(cardBorder)
        }
    }

    private var card: some View {
        RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
            .fill(CSColor.surfaceRaised(0.025))
    }

    private var cardBorder: some View {
        RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
            .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
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
    ScrollView { PromptPanel(model: .preview(.prompts)) }
        .frame(width: 720, height: 620)
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
#endif
