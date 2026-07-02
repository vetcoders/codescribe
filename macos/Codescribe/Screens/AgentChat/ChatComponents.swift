import SwiftUI
import AppKit

// Screen-local helpers for Agent Chat. Off-token shades from the mock that the
// shared CSColor palette does not carry verbatim live here (and ONLY here).

enum ChatPalette {
    static let nameInactive = Color(hex: 0xC7CABF)   // inactive thread name / segmented body
    static let nameActive = Color(hex: 0xF0EEE7)     // active thread name / titles / you-bubble text
    static let activeThreadSub = Color(hex: 0x9A7A6A) // "active · restored" subtitle
    static let toolBody = Color(hex: 0x9AA093)        // tool-activity detail text
    static let thinking = Color(hex: 0x8A8D87)        // "thinking…" label
    static let sendGlyph = Color(hex: 0x0A0A0A)       // ↑ glyph on terracotta button
}

/// Expanding terracotta ring + solid dot — the composer mic affordance.
struct RippleMic: View {
    @State private var animate = false
    var body: some View {
        ZStack {
            Circle()
                .strokeBorder(CSColor.terracotta, lineWidth: 1)
                .frame(width: 12, height: 12)
                .scaleEffect(animate ? 2.7 : 0.5)
                .opacity(animate ? 0 : 0.7)
            Circle()
                .fill(CSColor.terracotta)
                .frame(width: 6, height: 6)
        }
        .frame(width: 12, height: 12)
        .onAppear { withAnimation(CSMotion.ripple) { animate = true } }
    }
}

/// Blinking terracotta caret shown while a turn streams.
struct BlinkCaret: View {
    @State private var on = true
    var body: some View {
        Rectangle()
            .fill(CSColor.terracotta)
            .frame(width: 7, height: 15)
            .opacity(on ? 1 : 0)
            .onAppear { withAnimation(CSMotion.blink) { on = false } }
    }
}

/// Block-level markdown body for a chat turn: paragraphs, `#`–`###` headings,
/// bullet / ordered lists, fenced ``` code blocks, plus inline **bold**,
/// *italic*, `code` spans (olive + mono) and [links](url) (terracotta, open in
/// the default browser via NSWorkspace). Block structure is parsed here; each
/// block's inline text is handed to `AttributedString(markdown:)`, which falls
/// back to the raw string on failure — so a bubble is never empty or crashes on
/// half-streamed markdown.
///
/// Performance: the stored inputs are all value types, so SwiftUI re-evaluates
/// `body` (and re-parses) ONLY when the text changes. During a stream that is
/// the single growing turn, never the whole history.
struct MarkdownText: View {
    let raw: String
    var size: CGFloat = 14
    var bodyColor: Color = CSColor.textBodyAlt
    var showsCaret: Bool = false

    var body: some View {
        let blocks = MDBlock.parse(raw)
        VStack(alignment: .leading, spacing: 7) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { index, block in
                blockView(block, isLast: index == blocks.count - 1)
            }
        }
        .environment(\.openURL, OpenURLAction { url in
            NSWorkspace.shared.open(url)
            return .handled
        })
    }

    @ViewBuilder
    private func blockView(_ block: MDBlock, isLast: Bool) -> some View {
        switch block {
        case let .paragraph(text):
            inlineText(text, baseFont: CSFont.ui(size), baseColor: bodyColor,
                       fontSize: size, isLast: isLast)
        case let .heading(level, text):
            let hSize = headingSize(level)
            inlineText(text, baseFont: CSFont.ui(hSize, .bold), baseColor: CSColor.textHigh,
                       fontSize: hSize, isLast: isLast)
                .padding(.top, level <= 2 ? 3 : 1)
        case let .bullet(indent, text):
            listRow(marker: "•", indent: indent, text: text, isLast: isLast)
        case let .ordered(indent, number, text):
            listRow(marker: "\(number).", indent: indent, text: text, isLast: isLast)
        case let .task(indent, done, text):
            taskRow(indent: indent, done: done, text: text, isLast: isLast)
        case let .blockquote(text):
            blockquoteView(text, isLast: isLast)
        case let .table(header, rows):
            tableView(header: header, rows: rows)
        case let .code(content):
            codeBlock(content, isLast: isLast)
        }
    }

    private func headingSize(_ level: Int) -> CGFloat {
        switch level {
        case 1: return size + 7
        case 2: return size + 4
        case 3: return size + 2
        default: return size + 1
        }
    }

    @ViewBuilder
    private func inlineText(_ text: String, baseFont: Font, baseColor: Color,
                            fontSize: CGFloat, isLast: Bool) -> some View {
        let attr = Self.inlineAttributed(text, fontSize: fontSize,
                                         baseFont: baseFont, baseColor: baseColor)
        let content = Text(attr).lineSpacing(5)
        if isLast, showsCaret {
            HStack(alignment: .bottom, spacing: 2) {
                content.fixedSize(horizontal: false, vertical: true)
                BlinkCaret()
            }
        } else {
            content
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    @ViewBuilder
    private func listRow(marker: String, indent: Int, text: String, isLast: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            Text(marker)
                .font(CSFont.mono(size - 2))
                .foregroundStyle(CSColor.textMutedAlt)
                .frame(minWidth: 14, alignment: .trailing)
            inlineText(text, baseFont: CSFont.ui(size), baseColor: bodyColor,
                       fontSize: size, isLast: isLast)
        }
        .padding(.leading, CGFloat(min(indent, 4)) * 16)
    }

    /// A `- [x]` / `- [ ]` checklist row: a filled/empty SF Symbol checkbox in
    /// place of the literal brackets, olive when done, faint when open. Done
    /// items read slightly dimmer so an open task stands out.
    @ViewBuilder
    private func taskRow(indent: Int, done: Bool, text: String, isLast: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            Image(systemName: done ? "checkmark.square.fill" : "square")
                .font(.system(size: size - 1))
                .foregroundStyle(done ? CSColor.olive : CSColor.textFaint)
                .frame(minWidth: 14, alignment: .trailing)
            inlineText(text, baseFont: CSFont.ui(size),
                       baseColor: done ? CSColor.textMutedAlt : bodyColor,
                       fontSize: size, isLast: isLast)
        }
        .padding(.leading, CGFloat(min(indent, 4)) * 16)
    }

    /// A `>` blockquote (single- or multi-line; deeper `>>` nesting collapses to
    /// one level). Terracotta hairline bar on the left, dimmed body text.
    @ViewBuilder
    private func blockquoteView(_ text: String, isLast: Bool) -> some View {
        HStack(alignment: .top, spacing: 9) {
            RoundedRectangle(cornerRadius: 1, style: .continuous)
                .fill(CSColor.terracotta.opacity(0.55))
                .frame(width: 2.5)
            inlineText(text, baseFont: CSFont.ui(size), baseColor: CSColor.textMutedAlt,
                       fontSize: size, isLast: isLast)
        }
        .fixedSize(horizontal: false, vertical: true)
    }

    /// A GFM pipe table. Header row is mono + high-contrast over a faint fill;
    /// hairline separators between rows; each cell renders inline markdown and
    /// wraps rather than overflowing (columns share width via `maxWidth:
    /// .infinity`), so a wide table never blows out the bubble.
    @ViewBuilder
    private func tableView(header: [String], rows: [[String]]) -> some View {
        let columnCount = max(header.count, rows.map(\.count).max() ?? 0)
        Grid(alignment: .topLeading, horizontalSpacing: 0, verticalSpacing: 0) {
            GridRow {
                ForEach(0..<columnCount, id: \.self) { column in
                    tableCell(cell(header, column), isHeader: true)
                }
            }
            Divider().overlay(CSColor.hairline(0.12))
            ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
                GridRow {
                    ForEach(0..<columnCount, id: \.self) { column in
                        tableCell(cell(row, column), isHeader: false)
                    }
                }
                if index < rows.count - 1 {
                    Divider().overlay(CSColor.hairline(0.06))
                }
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
        )
        .fixedSize(horizontal: false, vertical: true)
    }

    private func cell(_ row: [String], _ column: Int) -> String {
        column < row.count ? row[column] : ""
    }

    @ViewBuilder
    private func tableCell(_ text: String, isHeader: Bool) -> some View {
        let cellSize = isHeader ? size - 2 : size - 1
        let font = isHeader ? CSFont.mono(cellSize, .semibold) : CSFont.ui(cellSize)
        let color = isHeader ? CSColor.textHigh : bodyColor
        let attr = Self.inlineAttributed(text, fontSize: cellSize,
                                         baseFont: font, baseColor: color)
        Text(attr)
            .lineSpacing(3)
            .multilineTextAlignment(.leading)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(isHeader ? CSColor.surfaceRaised(0.05) : Color.clear)
    }

    @ViewBuilder
    private func codeBlock(_ content: String, isLast: Bool) -> some View {
        let block = Text(content.isEmpty ? " " : content)
            .font(CSFont.mono(size - 1))
            .foregroundColor(CSColor.textBodyAlt)
            .lineSpacing(4)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 11)
            .padding(.vertical, 9)
            .background(CSColor.surfaceRaised(0.05))
            .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .strokeBorder(CSColor.hairline(0.06), lineWidth: 1)
            )
        if isLast, showsCaret {
            HStack(alignment: .bottom, spacing: 2) {
                block
                BlinkCaret()
            }
        } else {
            block
        }
    }

    /// Inline markdown → styled `AttributedString`. Bold / italic ride the
    /// parser's `inlinePresentationIntent` (SwiftUI applies them over our base
    /// font); we override `code` runs to mono + olive and `link` runs to
    /// terracotta. On parse failure returns the plain raw string so the caller
    /// still shows something.
    static func inlineAttributed(_ text: String, fontSize: CGFloat,
                                 baseFont: Font, baseColor: Color) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(
            allowsExtendedAttributes: true,
            interpretedSyntax: .inlineOnlyPreservingWhitespace,
            failurePolicy: .returnPartiallyParsedIfPossible
        )
        guard var attr = try? AttributedString(markdown: text, options: options) else {
            var raw = AttributedString(text)
            raw.font = baseFont
            raw.foregroundColor = baseColor
            return raw
        }
        attr.font = baseFont
        attr.foregroundColor = baseColor

        var codeRanges: [Range<AttributedString.Index>] = []
        var linkRanges: [Range<AttributedString.Index>] = []
        for run in attr.runs {
            if let intent = run.inlinePresentationIntent, intent.contains(.code) {
                codeRanges.append(run.range)
            }
            if run.link != nil {
                linkRanges.append(run.range)
            }
        }
        for range in codeRanges {
            attr[range].font = CSFont.mono(fontSize - 1)
            attr[range].foregroundColor = CSColor.oliveLight
        }
        for range in linkRanges {
            attr[range].foregroundColor = CSColor.terracotta
        }
        return attr
    }
}

/// One block of parsed markdown. Line-based, intentionally small — enough for
/// agent chat prose, not a CommonMark engine.
enum MDBlock: Equatable {
    case paragraph(String)
    case heading(level: Int, text: String)
    case bullet(indent: Int, text: String)
    case ordered(indent: Int, number: Int, text: String)
    case task(indent: Int, done: Bool, text: String)
    case blockquote(String)
    case table(header: [String], rows: [[String]])
    case code(String)

    /// Split raw text into blocks. Consecutive plain lines (no blank line
    /// between) coalesce into one paragraph, preserving their newlines.
    static func parse(_ raw: String) -> [MDBlock] {
        var blocks: [MDBlock] = []
        var paragraph: [String] = []

        func flush() {
            if !paragraph.isEmpty {
                blocks.append(.paragraph(paragraph.joined(separator: "\n")))
                paragraph.removeAll(keepingCapacity: true)
            }
        }

        let lines = raw.components(separatedBy: "\n")
        var i = 0
        while i < lines.count {
            let line = lines[i]
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if trimmed.hasPrefix("```") {
                flush()
                var body: [String] = []
                i += 1
                while i < lines.count,
                      !lines[i].trimmingCharacters(in: .whitespaces).hasPrefix("```") {
                    body.append(lines[i])
                    i += 1
                }
                if i < lines.count { i += 1 }  // consume closing fence when present
                blocks.append(.code(body.joined(separator: "\n")))
                continue
            }

            if trimmed.isEmpty {
                flush()
                i += 1
                continue
            }

            // Table: a pipe row immediately followed by a `|---|---|` separator.
            // The separator gate keeps stray-pipe prose from misfiring.
            if trimmed.contains("|"), i + 1 < lines.count,
               isTableSeparator(lines[i + 1]) {
                flush()
                let header = tableCells(trimmed)
                i += 2  // consume the header and separator rows
                var rows: [[String]] = []
                while i < lines.count {
                    let rowLine = lines[i].trimmingCharacters(in: .whitespaces)
                    guard !rowLine.isEmpty, rowLine.contains("|") else { break }
                    rows.append(tableCells(rowLine))
                    i += 1
                }
                blocks.append(.table(header: header, rows: rows))
                continue
            }

            if trimmed.hasPrefix(">") {
                flush()
                var quote: [String] = []
                while i < lines.count {
                    let qline = lines[i].trimmingCharacters(in: .whitespaces)
                    guard qline.hasPrefix(">") else { break }
                    quote.append(stripQuoteMarker(qline))
                    i += 1
                }
                blocks.append(.blockquote(quote.joined(separator: "\n")))
                continue
            }

            if let heading = headingBlock(trimmed) {
                flush()
                blocks.append(heading)
                i += 1
                continue
            }

            if let item = listBlock(line) {
                flush()
                blocks.append(item)
                i += 1
                continue
            }

            paragraph.append(trimmed)
            i += 1
        }
        flush()
        return blocks
    }

    private static func headingBlock(_ s: String) -> MDBlock? {
        var level = 0
        var idx = s.startIndex
        while idx < s.endIndex, s[idx] == "#", level < 6 {
            level += 1
            idx = s.index(after: idx)
        }
        guard level > 0, idx < s.endIndex, s[idx] == " " else { return nil }
        let text = String(s[idx...]).trimmingCharacters(in: .whitespaces)
        return .heading(level: level, text: text)
    }

    private static func listBlock(_ line: String) -> MDBlock? {
        let leading = line.prefix { $0 == " " }.count
        let indent = leading / 2
        let content = line.drop { $0 == " " }

        if let first = content.first, "-*+".contains(first) {
            let after = content.dropFirst()
            if after.first == " " {
                let body = String(after.dropFirst()).trimmingCharacters(in: .whitespaces)
                if let task = taskBlock(indent: indent, body: body) {
                    return task
                }
                return .bullet(indent: indent, text: body)
            }
        }

        let digits = content.prefix { $0.isNumber }
        if !digits.isEmpty {
            let rest = content.dropFirst(digits.count)
            if rest.first == ".", rest.dropFirst().first == " " {
                let text = String(rest.dropFirst(2)).trimmingCharacters(in: .whitespaces)
                return .ordered(indent: indent, number: Int(digits) ?? 1, text: text)
            }
        }
        return nil
    }

    /// A `[x]` / `[X]` / `[ ]` checkbox prefix on a bullet body → task item.
    private static func taskBlock(indent: Int, body: String) -> MDBlock? {
        guard body.hasPrefix("[") else { return nil }
        let inner = body.dropFirst()
        guard let mark = inner.first, inner.dropFirst().first == "]" else { return nil }
        let rest = inner.dropFirst(2)
        guard rest.isEmpty || rest.first == " " else { return nil }
        let done: Bool
        switch mark {
        case "x", "X": done = true
        case " ": done = false
        default: return nil
        }
        return .task(indent: indent, done: done,
                     text: String(rest).trimmingCharacters(in: .whitespaces))
    }

    /// Strip every leading `>` / space so `>> quoted` collapses to `quoted`
    /// (nesting is flattened to a single level, per the chat renderer's scope).
    private static func stripQuoteMarker(_ line: String) -> String {
        var slice = Substring(line)
        while let first = slice.first, first == ">" || first == " " {
            slice = slice.dropFirst()
        }
        return String(slice)
    }

    /// Split a pipe row into trimmed cells, dropping the optional outer pipes.
    static func tableCells(_ line: String) -> [String] {
        var body = line.trimmingCharacters(in: .whitespaces)
        if body.hasPrefix("|") { body.removeFirst() }
        if body.hasSuffix("|") { body.removeLast() }
        return body.components(separatedBy: "|")
            .map { $0.trimmingCharacters(in: .whitespaces) }
    }

    /// True for a GFM separator row such as `|---|:--:|`: every cell is dashes
    /// with optional alignment colons, and at least one cell carries a dash.
    static func isTableSeparator(_ line: String) -> Bool {
        let cells = tableCells(line)
        guard !cells.isEmpty else { return false }
        return cells.allSatisfy { cell in
            !cell.isEmpty && cell.contains("-")
                && cell.allSatisfy { $0 == "-" || $0 == ":" }
        }
    }
}
