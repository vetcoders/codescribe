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
                let text = String(after.dropFirst()).trimmingCharacters(in: .whitespaces)
                return .bullet(indent: indent, text: text)
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
}
