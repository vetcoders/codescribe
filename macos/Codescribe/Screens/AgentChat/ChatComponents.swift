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
            blockquoteView(text)
        case let .table(header, rows):
            tableView(header: header, rows: rows)
        case let .code(content):
            codeBlock(content, isLast: isLast)
        case .thematicBreak:
            Rectangle()
                .fill(CSColor.hairline(0.12))
                .frame(height: 1)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 4)
        }
    }

    /// Renders a list of already-parsed blocks (used for blockquote
    /// bodies). No streaming caret — nested content is never the live turn tail.
    // `AnyView` breaks the otherwise-recursive opaque return type: blockView ->
    // blockquoteView/calloutView -> blocksView -> blockView would define `some
    // View` in terms of itself, which the compiler rejects.
    private func blocksView(_ blocks: [MDBlock]) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                AnyView(blockView(block, isLast: false))
            }
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

    /// A `>` blockquote. Its inner text is parsed recursively so nested lists,
    /// code fences and quotes render properly (not half-raw). Terracotta hairline
    /// bar on the left, dimmed body.
    @ViewBuilder
    private func blockquoteView(_ text: String) -> some View {
        HStack(alignment: .top, spacing: 9) {
            RoundedRectangle(cornerRadius: 1, style: .continuous)
                .fill(CSColor.terracotta.opacity(0.55))
                .frame(width: 2.5)
            blocksView(MDBlock.parse(text))
                .opacity(0.9)
        }
        .fixedSize(horizontal: false, vertical: true)
    }

    /// A GFM pipe table. Header row is mono + high-contrast over a faint fill;
    /// hairline separators between rows. Columns share the bubble width via a
    /// custom `Layout` that weights each column by its longest cell but keeps a
    /// per-column minimum, so a single very long column can't crush the others
    /// down to word-per-line slivers. Cell text wraps normally within its width.
    @ViewBuilder
    private func tableView(header: [String], rows: [[String]]) -> some View {
        let columnCount = max(header.count, rows.map(\.count).max() ?? 0)
        let weights = columnWeights(header: header, rows: rows, count: columnCount)
        MDTableLayout(columns: columnCount, rowCount: rows.count + 1, weights: weights) {
            ForEach(0..<columnCount, id: \.self) { column in
                tableCell(cell(header, column), isHeader: true,
                          isLastRow: rows.isEmpty)
            }
            ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
                ForEach(0..<columnCount, id: \.self) { column in
                    tableCell(cell(row, column), isHeader: false,
                              isLastRow: index == rows.count - 1)
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

    /// Per-column weight = its longest cell length, floored so tiny columns stay
    /// legible and capped so one long column doesn't monopolize the width.
    private func columnWeights(header: [String], rows: [[String]],
                               count: Int) -> [CGFloat] {
        guard count > 0 else { return [] }
        var weights = [CGFloat](repeating: 1, count: count)
        func consider(_ row: [String]) {
            for column in 0..<count where column < row.count {
                weights[column] = max(weights[column], CGFloat(row[column].count))
            }
        }
        consider(header)
        rows.forEach(consider)
        return weights.map { min(max($0, 3), 48) }
    }

    @ViewBuilder
    private func tableCell(_ text: String, isHeader: Bool, isLastRow: Bool) -> some View {
        let cellSize = isHeader ? size - 2 : size - 1
        let font = isHeader ? CSFont.mono(cellSize, .semibold) : CSFont.ui(cellSize)
        let color = isHeader ? CSColor.textHigh : bodyColor
        let attr = Self.inlineAttributed(text, fontSize: cellSize,
                                         baseFont: font, baseColor: color)
        Text(attr)
            .lineSpacing(3)
            .multilineTextAlignment(.leading)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(isHeader ? CSColor.surfaceRaised(0.05) : Color.clear)
            .overlay(alignment: .bottom) {
                if !isLastRow {
                    Rectangle()
                        .fill(CSColor.hairline(isHeader ? 0.12 : 0.06))
                        .frame(height: 1)
                }
            }
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
        let escaped = escapingInlineHTML(text)
        guard var attr = try? AttributedString(markdown: escaped, options: options) else {
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

    /// Backslash-escape `<`/`>` that live outside inline code spans so raw HTML
    /// tags always render as literal, predictable text instead of being dropped
    /// by the markdown parser. Content inside `` `…` `` code spans is copied
    /// verbatim (code keeps its literal angle brackets, no stray backslashes).
    static func escapingInlineHTML(_ text: String) -> String {
        let chars = Array(text)
        var out = ""
        out.reserveCapacity(chars.count + 8)
        var index = 0
        while index < chars.count {
            let char = chars[index]
            if char == "`" {
                var open = 0
                while index < chars.count, chars[index] == "`" { open += 1; index += 1 }
                out += String(repeating: "`", count: open)
                // Copy verbatim until a backtick run of equal length closes it.
                while index < chars.count {
                    if chars[index] == "`" {
                        var close = 0
                        while index < chars.count, chars[index] == "`" {
                            close += 1; index += 1
                        }
                        out += String(repeating: "`", count: close)
                        if close == open { break }
                    } else {
                        out.append(chars[index]); index += 1
                    }
                }
                continue
            }
            switch char {
            case "<": out += "\\<"
            case ">": out += "\\>"
            default: out.append(char)
            }
            index += 1
        }
        return out
    }
}

/// Lays a pipe-table's cells into a proportional grid. Column widths come from
/// per-column weights (longest cell), each floored to `minColumn` so a very long
/// column can't squeeze the rest to slivers; leftover width is shared by weight.
/// Cells are placed row-major: `subviews[row * columns + column]`.
struct MDTableLayout: Layout {
    let columns: Int
    let rowCount: Int
    let weights: [CGFloat]
    var minColumn: CGFloat = 46

    struct Cache {
        var columnWidths: [CGFloat] = []
        var rowHeights: [CGFloat] = []
        var width: CGFloat = -1
    }

    func makeCache(subviews: Subviews) -> Cache { Cache() }

    /// Distribute `total` width across columns proportional to weight, but never
    /// below `minColumn`; columns pinned to the floor drop out and the remainder
    /// re-shares among the rest.
    private func columnWidths(for total: CGFloat) -> [CGFloat] {
        guard columns > 0 else { return [] }
        var widths = [CGFloat](repeating: minColumn, count: columns)
        var active = Array(0..<columns)
        var remaining = total
        while !active.isEmpty {
            let weightSum = active.reduce(CGFloat(0)) { $0 + weights[$1] }
            guard weightSum > 0 else {
                let each = max(minColumn, remaining / CGFloat(active.count))
                for column in active { widths[column] = each }
                break
            }
            var pinned: [Int] = []
            for column in active where remaining * weights[column] / weightSum < minColumn {
                pinned.append(column)
            }
            if pinned.isEmpty {
                for column in active {
                    widths[column] = remaining * weights[column] / weightSum
                }
                break
            }
            for column in pinned { widths[column] = minColumn; remaining -= minColumn }
            active.removeAll { pinned.contains($0) }
            if remaining <= 0 { break }
        }
        return widths
    }

    private func resolve(_ subviews: Subviews, total: CGFloat, cache: inout Cache) {
        if cache.width == total, !cache.columnWidths.isEmpty { return }
        let widths = columnWidths(for: total)
        var heights = [CGFloat](repeating: 0, count: rowCount)
        for (offset, subview) in subviews.enumerated() {
            let row = offset / columns
            let column = offset % columns
            guard row < rowCount, column < columns else { continue }
            let height = subview.sizeThatFits(
                ProposedViewSize(width: widths[column], height: nil)
            ).height
            heights[row] = max(heights[row], height)
        }
        cache.columnWidths = widths
        cache.rowHeights = heights
        cache.width = total
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews,
                      cache: inout Cache) -> CGSize {
        let total = proposal.width ?? 320
        resolve(subviews, total: total, cache: &cache)
        return CGSize(width: total, height: cache.rowHeights.reduce(0, +))
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize,
                       subviews: Subviews, cache: inout Cache) {
        resolve(subviews, total: bounds.width, cache: &cache)
        let widths = cache.columnWidths
        let heights = cache.rowHeights
        var xOffsets = [CGFloat](repeating: 0, count: columns)
        var accX: CGFloat = 0
        for column in 0..<columns { xOffsets[column] = accX; accX += widths[column] }
        var yOffsets = [CGFloat](repeating: 0, count: rowCount)
        var accY: CGFloat = 0
        for row in 0..<rowCount { yOffsets[row] = accY; accY += heights[row] }
        for (offset, subview) in subviews.enumerated() {
            let row = offset / columns
            let column = offset % columns
            guard row < rowCount, column < columns else {
                subview.place(at: bounds.origin,
                              proposal: ProposedViewSize(width: 0, height: 0))
                continue
            }
            subview.place(
                at: CGPoint(x: bounds.minX + xOffsets[column],
                            y: bounds.minY + yOffsets[row]),
                proposal: ProposedViewSize(width: widths[column], height: heights[row])
            )
        }
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
    case thematicBreak

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

            // A fenced code block opened with N backticks closes only on a line
            // whose backtick run is >= N (CommonMark). This lets a ````md block
            // carry inner ```ts fences verbatim instead of closing early.
            if let openTicks = openingFence(trimmed) {
                flush()
                var body: [String] = []
                i += 1
                while i < lines.count {
                    let closeTrim = lines[i].trimmingCharacters(in: .whitespaces)
                    if let closeTicks = closingFence(closeTrim), closeTicks >= openTicks {
                        i += 1  // consume the closing fence
                        break
                    }
                    body.append(lines[i])
                    i += 1
                }
                blocks.append(.code(body.joined(separator: "\n")))
                continue
            }

            if trimmed.isEmpty {
                flush()
                i += 1
                continue
            }

            // A thematic break: 3+ of the same `-`/`*`/`_` on a line of nothing
            // else (spaces allowed). Table separators are consumed by the table
            // branch below before they can reach here, so a lone `---` is an HR.
            if isThematicBreak(trimmed) {
                flush()
                blocks.append(.thematicBreak)
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

    /// Strip a single leading `>` marker (plus one optional space). Keeping the
    /// remainder intact — including any inner `>` or list indentation — lets the
    /// blockquote body be re-parsed recursively (nested quotes, lists, fences).
    private static func stripQuoteMarker(_ line: String) -> String {
        var slice = Substring(line)
        if slice.first == ">" { slice = slice.dropFirst() }
        if slice.first == " " { slice = slice.dropFirst() }
        return String(slice)
    }

    /// Leading backtick count of an opening fence (>= 3), or nil. The info
    /// string after the ticks must not itself contain a backtick (CommonMark),
    /// which keeps an inline `` `code` `` run from being read as a fence.
    private static func openingFence(_ s: String) -> Int? {
        let ticks = s.prefix { $0 == "`" }.count
        guard ticks >= 3 else { return nil }
        return s.dropFirst(ticks).contains("`") ? nil : ticks
    }

    /// Backtick count of a closing fence line (>= 3, nothing but ticks and
    /// trailing spaces), or nil.
    private static func closingFence(_ s: String) -> Int? {
        let ticks = s.prefix { $0 == "`" }.count
        guard ticks >= 3 else { return nil }
        return s.dropFirst(ticks).allSatisfy { $0 == " " } ? ticks : nil
    }

    /// A CommonMark thematic break: 3+ of a single `-`/`*`/`_`, with only spaces
    /// otherwise. Also matches spaced forms like `- - -` and `* * *`.
    private static func isThematicBreak(_ s: String) -> Bool {
        let core = s.filter { $0 != " " && $0 != "\t" }
        guard core.count >= 3, let first = core.first, "-*_".contains(first) else {
            return false
        }
        return core.allSatisfy { $0 == first }
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
