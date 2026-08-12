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
    /// Prefix-matched carried-line variant: the suffix carries a live
    /// selection count ("… (3 selections).") so exact matching would rot
    /// with every count. The matched range extends through the line's
    /// trailing newline. Covers the pre-counter wires too (same prefix).
    let selectionCarriedPrefix: String?
    let contextPrefix: String
  }

  private static let english = Markers(
    header: "USER_INSTRUCTION:\n<<<\n",
    selectionHeredoc: "\n>\n\nSELECTED_TEXT:\n<<<\n",
    selectionAbsent: [
      "\n>\n\nSELECTED_TEXT: no selection available.\n"
    ],
    selectionCarriedPrefix: "\n>\n\nSELECTED_TEXT: carried in <codescribe_context>",
    contextPrefix: "\nCONTEXT:\n- frontmost_app: "
  )

  private static let legacyPolish = Markers(
    header: "INSTRUKCJA_UŻYTKOWNIKA:\n<<<\n",
    selectionHeredoc: "\n>\n\nZAZNACZONY_TEKST:\n<<<\n",
    selectionAbsent: ["\n>\n\nZAZNACZONY_TEKST: brak dostępnego zaznaczenia.\n"],
    selectionCarriedPrefix: nil,
    contextPrefix: "\nKONTEKST:\n- frontmost_app: "
  )

  /// Alternate selection-section openers seen in older gpt-5.5 / June-era
  /// wires where the instruction heredoc close (`>`) was dropped, doubled,
  /// or written with a single newline. Exact canonical markers stay first;
  /// these are a second pass so salvage never rewrites a plain composer
  /// message that merely mentions "SELECTED_TEXT".
  private static let englishHeredocAlts = [
    "\n>\nSELECTED_TEXT:\n<<<\n",
    "\n\nSELECTED_TEXT:\n<<<\n",
    "\nSELECTED_TEXT:\n<<<\n",
  ]
  private static let legacyHeredocAlts = [
    "\n>\nZAZNACZONY_TEKST:\n<<<\n",
    "\n\nZAZNACZONY_TEKST:\n<<<\n",
    "\nZAZNACZONY_TEKST:\n<<<\n",
  ]
  private static let englishAbsentAlts = [
    "\n>\nSELECTED_TEXT: no selection available.\n",
    "\n\nSELECTED_TEXT: no selection available.\n",
    "\nSELECTED_TEXT: no selection available.\n",
  ]
  private static let legacyAbsentAlts = [
    "\n>\nZAZNACZONY_TEKST: brak dostępnego zaznaczenia.\n",
    "\n\nZAZNACZONY_TEKST: brak dostępnego zaznaczenia.\n",
    "\nZAZNACZONY_TEKST: brak dostępnego zaznaczenia.\n",
  ]

  private static let heredocClose = "\n>\n"

  /// Parse a wire prompt into its parts. Returns `nil` when `wire` is not an
  /// assistive skeleton (the caller then renders the text as-is).
  static func parse(_ wire: String) -> Parts? {
    for markers in [english, legacyPolish] where wire.hasPrefix(markers.header) {
      if let parts = parse(wire: wire, markers: markers) {
        return parts
      }
      // Canonical markers failed — try the tolerant second pass used by
      // June-era / gpt-5.5 wires that still open with the same header.
      if let parts = parseTolerant(wire: wire, markers: markers) {
        return parts
      }
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
    let missingRange =
      (markers.selectionAbsent.compactMap { body.range(of: $0) }
      + [carriedRange(in: body, markers: markers)].compactMap { $0 })
      .min { $0.lowerBound < $1.lowerBound }

    switch (heredocRange, missingRange) {
    case (.some(let heredoc), .some(let missing)):
      return heredoc.lowerBound < missing.lowerBound
        ? parseWithSelection(body: body, marker: heredoc, markers: markers)
        : parseWithoutSelection(body: body, marker: missing, markers: markers)
    case (.some(let heredoc), nil):
      return parseWithSelection(body: body, marker: heredoc, markers: markers)
    case (nil, .some(let missing)):
      return parseWithoutSelection(body: body, marker: missing, markers: markers)
    case (nil, nil):
      return nil
    }
  }

  /// Second-pass parse for wires that open with a known header but use a
  /// non-canonical seam between the instruction and the selection section.
  private static func parseTolerant(wire: String, markers: Markers) -> Parts? {
    let body = String(wire.dropFirst(markers.header.count))
    let isEnglish = markers.header.hasPrefix("USER_INSTRUCTION")
    let heredocAlts = isEnglish ? englishHeredocAlts : legacyHeredocAlts
    let absentAlts = isEnglish ? englishAbsentAlts : legacyAbsentAlts

    let heredocRange = heredocAlts.compactMap { body.range(of: $0) }
      .min { $0.lowerBound < $1.lowerBound }
    let missingRange =
      (absentAlts.compactMap { body.range(of: $0) }
      + [carriedRange(in: body, markers: markers)].compactMap { $0 })
      .min { $0.lowerBound < $1.lowerBound }

    switch (heredocRange, missingRange) {
    case (.some(let heredoc), .some(let missing)):
      return heredoc.lowerBound < missing.lowerBound
        ? parseWithSelection(body: body, marker: heredoc, markers: markers)
        : parseWithoutSelection(body: body, marker: missing, markers: markers)
    case (.some(let heredoc), nil):
      return parseWithSelection(body: body, marker: heredoc, markers: markers)
    case (nil, .some(let missing)):
      return parseWithoutSelection(body: body, marker: missing, markers: markers)
    case (nil, nil):
      // Header matched but no selection section — still salvage the
      // spoken instruction so the bubble never renders the raw skeleton.
      return salvageInstructionOnly(body: body, markers: markers)
    }
  }

  /// Last-resort salvage: header is known, but the body is truncated or uses
  /// an unknown selection dialect. Prefer a short spoken instruction over a
  /// full-wire dump that collapses the Agent window layout.
  private static func salvageInstructionOnly(body: String, markers: Markers) -> Parts? {
    var instruction = body
    // Strip a trailing unclosed heredoc close if present.
    if instruction.hasSuffix(heredocClose) {
      instruction = String(instruction.dropLast(heredocClose.count))
    } else if instruction.hasSuffix("\n>") {
      instruction = String(instruction.dropLast(2))
    }
    instruction = instruction.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !instruction.isEmpty else { return nil }
    // Refuse salvage when the body still looks like pure composer text that
    // never opened a heredoc — callers only reach here after a known header.
    return Parts(instruction: instruction, selectedText: nil, frontmostApp: nil)
  }

  /// Rewrite a wire-skeleton user message for display: `text` becomes the
  /// spoken instruction, the full skeleton moves to `wireText`, and the
  /// selection/app land in the context fields. Non-skeleton messages (and
  /// non-user roles) pass through untouched, so this is safe to run on every
  /// restored message.
  static func presented(_ message: ChatMessage) -> ChatMessage {
    guard message.role == .you,
      message.wireText == nil,
      let parts = parse(message.text)
    else { return message }
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
    // Instruction may still carry a trailing heredoc close when the
    // selection marker was matched via a tolerant alt that did not
    // consume the `>` line — strip it so the bubble shows speech only.
    let cleanedInstruction = stripTrailingHeredocClose(String(body[..<marker.lowerBound]))
    let rest = String(body[marker.upperBound...])

    // The selection heredoc closes right before the (optional) CONTEXT
    // section or the end of the prompt. Search from the back so a selection
    // that itself contains `\n>\n` stays intact.
    let contextSeams = [
      heredocClose + markers.contextPrefix,
      "\n>" + markers.contextPrefix,
      markers.contextPrefix,
    ]
    for seamToken in contextSeams {
      if let seam = rest.range(of: seamToken, options: .backwards) {
        let selected = String(rest[..<seam.lowerBound])
          .trimmingCharacters(in: .whitespacesAndNewlines)
        let app = trimmedContextValue(String(rest[seam.upperBound...]))
        return Parts(
          instruction: cleanedInstruction,
          selectedText: selected.isEmpty ? nil : selected,
          frontmostApp: app
        )
      }
    }
    if rest.hasSuffix(heredocClose) {
      let selected = String(rest.dropLast(heredocClose.count))
        .trimmingCharacters(in: .whitespacesAndNewlines)
      return Parts(
        instruction: cleanedInstruction,
        selectedText: selected.isEmpty ? nil : selected,
        frontmostApp: nil
      )
    }
    if rest.hasSuffix("\n>") {
      let selected = String(rest.dropLast(2))
        .trimmingCharacters(in: .whitespacesAndNewlines)
      return Parts(
        instruction: cleanedInstruction,
        selectedText: selected.isEmpty ? nil : selected,
        frontmostApp: nil
      )
    }
    // Selection present but never closed — still show the spoken instruction
    // and keep the open selection payload behind the context chip rather than
    // dumping the whole wire into the bubble (R1 layout collapse).
    let selected = rest.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !cleanedInstruction.isEmpty || !selected.isEmpty else { return nil }
    return Parts(
      instruction: cleanedInstruction,
      selectedText: selected.isEmpty ? nil : selected,
      frontmostApp: nil
    )
  }

  private static func stripTrailingHeredocClose(_ text: String) -> String {
    var result = text
    if result.hasSuffix(heredocClose) {
      result = String(result.dropLast(heredocClose.count))
    } else if result.hasSuffix("\n>") {
      result = String(result.dropLast(2))
    } else if result.hasSuffix(">") {
      result = String(result.dropLast())
    }
    return result.trimmingCharacters(in: .whitespacesAndNewlines)
  }

  /// `SELECTED_TEXT: no selection available.\n[\nCONTEXT…]` (or the
  /// carried-in-codescribe_context variant, or legacy PL labels).
  private static func parseWithoutSelection(
    body: String, marker: Range<String.Index>, markers: Markers
  ) -> Parts? {
    let instruction = stripTrailingHeredocClose(String(body[..<marker.lowerBound]))
    let rest = String(body[marker.upperBound...])
    if rest.isEmpty {
      return Parts(instruction: instruction, selectedText: nil, frontmostApp: nil)
    }
    // Tolerant context prefix: exact, or bare "CONTEXT:" / "KONTEKST:" block.
    if rest.hasPrefix(markers.contextPrefix) {
      let app = trimmedContextValue(String(rest.dropFirst(markers.contextPrefix.count)))
      return Parts(instruction: instruction, selectedText: nil, frontmostApp: app)
    }
    let bareContext =
      markers.contextPrefix.hasPrefix("\nCONTEXT")
      ? "\nCONTEXT:\n"
      : "\nKONTEKST:\n"
    if let range = rest.range(of: bareContext) {
      let after = String(rest[range.upperBound...])
      // Prefer frontmost_app line when present.
      if let appLine = after.split(separator: "\n", omittingEmptySubsequences: false)
        .map(String.init)
        .first(where: { $0.hasPrefix("- frontmost_app:") })
      {
        let app = trimmedContextValue(
          String(appLine.dropFirst("- frontmost_app:".count))
        )
        return Parts(instruction: instruction, selectedText: nil, frontmostApp: app)
      }
      return Parts(instruction: instruction, selectedText: nil, frontmostApp: nil)
    }
    // Unknown trailer after a known absent-selection marker: still salvage
    // the spoken instruction so restore never dumps the skeleton.
    return Parts(instruction: instruction, selectedText: nil, frontmostApp: nil)
  }

  /// Range of the prefix-matched carried-line variant, extended through the
  /// line's trailing newline so callers can split exactly like an exact match.
  private static func carriedRange(
    in body: String, markers: Markers
  ) -> Range<String.Index>? {
    guard let prefix = markers.selectionCarriedPrefix,
      let start = body.range(of: prefix)
    else { return nil }
    guard let lineBreak = body[start.upperBound...].firstIndex(of: "\n") else {
      return start.lowerBound..<body.endIndex
    }
    return start.lowerBound..<body.index(after: lineBreak)
  }

  /// The context value runs to the end of the prompt with one trailing newline.
  private static func trimmedContextValue(_ value: String) -> String? {
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    return trimmed.isEmpty ? nil : trimmed
  }
}
