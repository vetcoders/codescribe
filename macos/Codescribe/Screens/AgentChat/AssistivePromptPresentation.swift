import Foundation

// Presentation-side split of the assistive wire prompt (U17 chat-presentation-truth).
//
// The voice-assistive send path wraps the user's spoken instruction in a fixed
// skeleton before it reaches the LLM (`app/os/selection.rs::build_assistive_input`):
//
//   USER_INSTRUCTION:
//   <<<
//   {spoken instruction}
//   >
//
//   SELECTED_TEXT:               (or: `SELECTED_TEXT: no selection available.`
//   <<<                           or: `SELECTED_TEXT: carried in <codescribe_context>.`)
//   {selected text}
//   >
//
//   CONTEXT:                     (optional)
//   - frontmost_app: {app}
//
// Threads persisted before the EN label rename carry the legacy Polish labels
// (INSTRUKCJA_UŻYTKOWNIKA / ZAZNACZONY_TEKST / KONTEKST); both dialects parse.
//
// That skeleton is the WIRE truth — it stays exactly what the model receives and
// what ThreadStore persists. The UI must never show it as the user's words: the
// You-bubble renders the spoken instruction (display), with selection/context
// tucked behind a disclosure chip. This parser is the single seam between the
// two: it runs on the live `onTurnStarted` text AND on restored thread history,
// so old threads (already saved with the skeleton) render clean too.
//
// Parsing is anchored on the exact markers `build_assistive_input` emits. Any
// text that does not match the skeleton byte-for-byte is returned untouched —
// a composer message that merely mentions "INSTRUKCJA" must never be rewritten.
enum AssistivePromptParser {
    /// The wire prompt split into what the user said and what was attached.
    struct Parts: Equatable {
        /// The spoken instruction — the only thing the You-bubble shows by default.
        let instruction: String
        /// Captured selection, if the skeleton carried one (`nil` for the
        /// "brak dostępnego zaznaczenia" variant).
        let selectedText: String?
        /// Frontmost app from the KONTEKST section, if present.
        let frontmostApp: String?
    }

    // Exact skeleton markers from `build_assistive_input` — one source of truth
    // on the Rust side, mirrored (not reinterpreted) here. Two dialects: the
    // canonical English labels (current wires) and the legacy Polish labels
    // (threads persisted before the rename) — both must keep parsing.
    private struct Markers {
        let header: String
        let selectionHeredoc: String
        /// Single-line variants that close the instruction heredoc without a
        /// selection heredoc ("no selection" / "carried in codescribe_context").
        let selectionAbsent: [String]
        let contextPrefix: String
    }

    private static let english = Markers(
        header: "USER_INSTRUCTION:\n<<<\n",
        selectionHeredoc: "\n>\n\nSELECTED_TEXT:\n<<<\n",
        selectionAbsent: [
            "\n>\n\nSELECTED_TEXT: no selection available.\n",
            "\n>\n\nSELECTED_TEXT: carried in <codescribe_context>.\n",
        ],
        contextPrefix: "\nCONTEXT:\n- frontmost_app: "
    )

    private static let legacyPolish = Markers(
        header: "INSTRUKCJA_UŻYTKOWNIKA:\n<<<\n",
        selectionHeredoc: "\n>\n\nZAZNACZONY_TEKST:\n<<<\n",
        selectionAbsent: ["\n>\n\nZAZNACZONY_TEKST: brak dostępnego zaznaczenia.\n"],
        contextPrefix: "\nKONTEKST:\n- frontmost_app: "
    )

    private static let heredocClose = "\n>\n"

    /// Parse a wire prompt into its parts. Returns `nil` when `wire` is not an
    /// assistive skeleton (the caller then renders the text as-is).
    static func parse(_ wire: String) -> Parts? {
        for markers in [english, legacyPolish] where wire.hasPrefix(markers.header) {
            return parse(wire: wire, markers: markers)
        }
        return nil
    }

    private static func parse(wire: String, markers: Markers) -> Parts? {
        let body = String(wire.dropFirst(markers.header.count))

        // The instruction ends at the FIRST selection marker. The instruction is
        // spoken text, so it realistically never contains the heredoc skeleton;
        // taking the first occurrence keeps a pathological selection that embeds
        // the marker from stealing part of itself into the instruction.
        let heredocRange = body.range(of: markers.selectionHeredoc)
        let missingRange = markers.selectionAbsent
            .compactMap { body.range(of: $0) }
            .min { $0.lowerBound < $1.lowerBound }

        switch (heredocRange, missingRange) {
        case let (.some(heredoc), .some(missing)):
            return heredoc.lowerBound < missing.lowerBound
                ? parseWithSelection(body: body, marker: heredoc, markers: markers)
                : parseWithoutSelection(body: body, marker: missing, markers: markers)
        case let (.some(heredoc), nil):
            return parseWithSelection(body: body, marker: heredoc, markers: markers)
        case let (nil, .some(missing)):
            return parseWithoutSelection(body: body, marker: missing, markers: markers)
        case (nil, nil):
            return nil
        }
    }

    /// Rewrite a wire-skeleton user message for display: `text` becomes the
    /// spoken instruction, the full skeleton moves to `wireText`, and the
    /// selection/app land in the context fields. Non-skeleton messages (and
    /// non-user roles) pass through untouched, so this is safe to run on every
    /// restored message.
    static func presented(_ message: ChatMessage) -> ChatMessage {
        guard message.role == .you,
              message.wireText == nil,
              let parts = parse(message.text) else { return message }
        var presented = message
        presented.wireText = message.text
        presented.text = parts.instruction
        presented.contextSelection = parts.selectedText
        presented.contextApp = parts.frontmostApp
        return presented
    }

    // MARK: - Variants

    /// `SELECTED_TEXT:\n<<<\n{selection}\n>\n[\nCONTEXT…]` (or legacy PL labels).
    private static func parseWithSelection(
        body: String, marker: Range<String.Index>, markers: Markers
    ) -> Parts? {
        let instruction = String(body[..<marker.lowerBound])
        let rest = String(body[marker.upperBound...])

        // The selection heredoc closes right before the (optional) CONTEXT
        // section or the end of the prompt. Search from the back so a selection
        // that itself contains `\n>\n` stays intact.
        let contextSeam = heredocClose + markers.contextPrefix
        if let seam = rest.range(of: contextSeam, options: .backwards) {
            let selected = String(rest[..<seam.lowerBound])
            let app = trimmedContextValue(String(rest[seam.upperBound...]))
            return Parts(instruction: instruction, selectedText: selected, frontmostApp: app)
        }
        guard rest.hasSuffix(heredocClose) else { return nil }
        let selected = String(rest.dropLast(heredocClose.count))
        return Parts(instruction: instruction, selectedText: selected, frontmostApp: nil)
    }

    /// `SELECTED_TEXT: no selection available.\n[\nCONTEXT…]` (or the
    /// carried-in-codescribe_context variant, or legacy PL labels).
    private static func parseWithoutSelection(
        body: String, marker: Range<String.Index>, markers: Markers
    ) -> Parts? {
        let instruction = String(body[..<marker.lowerBound])
        let rest = String(body[marker.upperBound...])
        if rest.isEmpty {
            return Parts(instruction: instruction, selectedText: nil, frontmostApp: nil)
        }
        guard rest.hasPrefix(markers.contextPrefix) else { return nil }
        let app = trimmedContextValue(String(rest.dropFirst(markers.contextPrefix.count)))
        return Parts(instruction: instruction, selectedText: nil, frontmostApp: app)
    }

    /// The context value runs to the end of the prompt with one trailing newline.
    private static func trimmedContextValue(_ value: String) -> String? {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
