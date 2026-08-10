import AppKit
import SwiftUI

// MARK: - Policy string helpers
//
// `OversizedBubblePolicy` (MessageList.swift) owns the thresholds and the
// disposition contract — ChatRenderCostTests pins it. These helpers turn a
// disposition into actual strings: the grapheme-safe head slice, the live
// streaming window, and the fold-chip size label. Slicing behavior is pinned
// by OversizedTextSliceTests.

extension OversizedBubblePolicy {
    /// Window (in characters) scanned backwards from the head cut for a
    /// newline, so the fold lands on a line boundary when one is near —
    /// folding mid-line reads like data loss, folding at a break like a fold.
    static let newlineSearchWindow = 400

    static func isOversized(_ text: String) -> Bool {
        disposition(utf8Count: text.utf8.count) != .inline
    }

    /// Grapheme-safe head of at most `headPreviewUTF8` bytes. Walks whole
    /// `Character`s so emoji and combining sequences never split; only the
    /// head region is ever visited, not the full payload.
    static func head(of text: String) -> String {
        guard case .headPreview(let headUTF8) = disposition(utf8Count: text.utf8.count) else {
            return text
        }
        var bytes = 0
        var cut = text.startIndex
        while cut < text.endIndex {
            let next = text.index(after: cut)
            bytes += text[cut].utf8.count
            if bytes > headUTF8 { break }
            cut = next
        }
        let hardCut = String(text[..<cut])
        let searchStart = hardCut.index(
            hardCut.endIndex,
            offsetBy: -min(newlineSearchWindow, hardCut.count),
            limitedBy: hardCut.startIndex
        ) ?? hardCut.startIndex
        if let lastNewline = hardCut[searchStart...].lastIndex(of: "\n") {
            return String(hardCut[..<lastNewline])
        }
        return hardCut
    }

    /// Live streaming window: the TAIL, not the head — a streaming turn is
    /// followed at its live edge, and the window also caps what the per-delta
    /// plain-`Text` re-evaluation has to lay out.
    static func streamingWindow(_ text: String) -> String {
        guard isOversized(text) else { return text }
        var bytes = 0
        var cut = text.endIndex
        while cut > text.startIndex {
            let prev = text.index(before: cut)
            bytes += text[prev].utf8.count
            if bytes > headPreviewUTF8 { break }
            cut = prev
        }
        return String(text[cut...])
    }

    /// Human size for the fold chip ("Show full text · 142 KB").
    static func byteSummary(_ text: String) -> String {
        let bytes = text.utf8.count
        if bytes < 1024 * 1024 {
            return "\(max(1, bytes / 1024)) KB"
        }
        return String(format: "%.1f MB", Double(bytes) / (1024 * 1024))
    }
}

// MARK: - Degraded bubble body

/// Body of one oversized bubble: caller-styled head plus a fold chip that
/// expands into `FullTextView`. The head stays a SwiftUI view (markdown or
/// mono, the caller decides via the builder) so a degraded bubble keeps the
/// turn's visual language; only the full payload switches to AppKit, which is
/// what takes it out of the list-wide selection overlay
/// (`BubbleTextDisposition.sharesListSelectionOverlay == false`).
struct OversizedMessageBody<Head: View>: View {
    let fullText: String
    @ViewBuilder let head: (String) -> Head
    @State private var showFull = false
    @Environment(\.csTextScale) private var textScale

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if showFull {
                FullTextView(
                    text: fullText,
                    font: CSFont.nsMono(13 * textScale)
                )
                .frame(maxWidth: .infinity)
                .frame(height: 380)
                .background(CSColor.surfaceRaised(0.04))
                .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
            } else {
                head(OversizedBubblePolicy.head(of: fullText))
            }
            foldChip
        }
    }

    private var foldChip: some View {
        Button {
            showFull.toggle()
        } label: {
            HStack(spacing: 5) {
                CSIconView(
                    icon: showFull ? .chevronDown : .chevronRight,
                    size: 8,
                    weight: .semibold,
                    color: CSColor.textFaintAlt
                )
                Text(showFull
                    ? "Collapse"
                    : "Show full text · \(OversizedBubblePolicy.byteSummary(fullText))")
                    .font(CSFont.mono(10.5, .medium))
                    .foregroundStyle(CSColor.textMuted)
            }
            .contentShape(Rectangle())
        }
        .csFocusRing(cornerRadius: 8)
        .help(showFull
            ? "Fold this message back to its head"
            : "Open the full text in a scrollable, selectable view")
    }
}

/// One-line honesty note above a windowed live stream: the display shows the
/// tail, the data keeps everything.
struct StreamWindowNote: View {
    let fullText: String

    var body: some View {
        Text("live view shows the newest output · full text kept (\(OversizedBubblePolicy.byteSummary(fullText)))")
            .font(CSFont.mono(9.5, .medium))
            .foregroundStyle(CSColor.textFaintAlt)
    }
}

// MARK: - AppKit full-text host

/// Read-only, selectable `NSTextView` in its own `NSScrollView`. TextKit 2
/// lays out only the visible viewport, and selection is handled by AppKit
/// inside this view — the chat list's shared SwiftUI selection overlay never
/// touches the oversized payload.
struct FullTextView: NSViewRepresentable {
    let text: String
    let font: NSFont

    func makeNSView(context: Context) -> NSScrollView {
        // Explicit TextKit 2 stack (viewport-based layout); the convenience
        // `NSTextView.scrollableTextView()` can still wire up TextKit 1.
        let textView = NSTextView(usingTextLayoutManager: true)
        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = false
        textView.drawsBackground = false
        textView.textContainerInset = NSSize(width: 8, height: 8)
        textView.autoresizingMask = [.width]
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.textContainer?.widthTracksTextView = true
        apply(to: textView)

        let scrollView = NSScrollView()
        scrollView.documentView = textView
        scrollView.hasVerticalScroller = true
        scrollView.drawsBackground = false
        return scrollView
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        guard let textView = scrollView.documentView as? NSTextView else { return }
        // The bubble is settled when this view exists; the identity check
        // keeps redundant SwiftUI update passes from re-laying-out the payload.
        if textView.string != text || textView.font != font {
            apply(to: textView)
        }
    }

    private func apply(to textView: NSTextView) {
        textView.font = font
        textView.textColor = NSColor(CSColor.textBodyAlt)
        textView.string = text
    }
}
