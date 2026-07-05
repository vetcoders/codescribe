import SwiftUI

// Prompt editor: edits the two BASE prompt files the core uses — the formatting
// prompt (`formatting.txt`) and the assistive prompt (`assistive.txt`). Each is
// loaded via the config engine, edited in a TextEditor, and saved back through
// `setFormattingPrompt` / `setAssistivePrompt`. "Restore defaults" resets both
// to their built-in defaults via `resetPromptsToDefaults`.
//
// NOTE: these edit only the BASE files; the core still appends its `*_tuning.txt`
// at runtime (not shown here).

struct PromptPanel: View {
    @ObservedObject var model: SettingsViewModel

    @State private var formatting: String = ""
    @State private var assistive: String = ""
    @State private var loaded = false

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
                title: "Formatting prompt",
                subtitle: "Rewrites raw transcripts (formatting.txt)",
                text: $formatting,
                onSave: { model.saveFormattingPrompt(formatting) }
            )
            .padding(.top, 22)

            PromptEditor(
                title: "Assistive prompt",
                subtitle: "Base system prompt for the voice assistant (assistive.txt)",
                text: $assistive,
                onSave: { model.saveAssistivePrompt(assistive) }
            )
            .padding(.top, 18)

            Button(action: restoreDefaults) {
                Text("Restore defaults")
                    .font(CSFont.ui(12, .semibold))
                    .foregroundStyle(CSColor.terracottaLight)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 9)
                    .background(
                        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                            .fill(CSColor.terracotta.opacity(0.12))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                            .strokeBorder(CSColor.terracotta.opacity(0.26), lineWidth: 1)
                    )
            }
            .buttonStyle(.plain)
            .padding(.top, 16)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
        .onAppear {
            guard !loaded else { return }
            formatting = model.formattingPrompt()
            assistive = model.assistivePrompt()
            loaded = true
        }
    }

    private func restoreDefaults() {
        model.resetPromptsToDefaults()
        formatting = model.defaultFormattingPrompt()
        assistive = model.defaultAssistivePrompt()
    }
}

// MARK: - Single prompt editor block

private struct PromptEditor: View {
    let title: String
    let subtitle: String
    @Binding var text: String
    let onSave: () -> Void

    /// VIEW (rendered markdown) by default; EDIT (raw editor) on demand. Saving
    /// returns to VIEW so the persisted prompt is shown rendered.
    @State private var editing = false

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
                toggleButton
            }

            content
                .padding(.top, 11)
        }
    }

    /// Edit ⇄ Save toggle. In EDIT it persists and flips back to VIEW; in VIEW it
    /// enters EDIT.
    private var toggleButton: some View {
        Button(action: {
            if editing {
                onSave()
                editing = false
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

#Preview("Prompt panel") {
    ScrollView { PromptPanel(model: .preview(.prompts)) }
        .frame(width: 720, height: 620)
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
