import SwiftUI
import AppKit

/// Scrolling turn list: You (terracotta bubble, right) · Tool activity
/// (DisclosureGroup, mono) · Assistant (amber "reasoned · Xs" chip + body,
/// last turn streams with a blink caret). Auto-scrolls to the newest turn.
struct MessageList: View {
    let messages: [ChatMessage]

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 16) {
                    ForEach(messages) { message in
                        turn(message)
                            .frame(maxWidth: .infinity, alignment: alignment(message.role))
                            .id(message.id)
                    }
                }
                .padding(20)
            }
            .scrollContentBackground(.hidden)
            // Let the user drag-select message text and Cmd+C it (SwiftUI Text is
            // not selectable by default). Per-message "Copy" lives in the bubble
            // context menu below.
            .textSelection(.enabled)
            .onChange(of: lastSignature) { _, _ in
                if let last = messages.last {
                    withAnimation(.easeOut(duration: 0.25)) {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
        }
    }

    /// Changes whenever a new turn lands or the streaming text grows.
    private var lastSignature: String {
        guard let last = messages.last else { return "" }
        return "\(messages.count)-\(last.text.count)"
    }

    private func alignment(_ role: ChatRole) -> Alignment {
        role == .you ? .trailing : .leading
    }

    @ViewBuilder
    private func turn(_ message: ChatMessage) -> some View {
        switch message.role {
        case .you: YouTurn(message: message)
        case .tool: ToolTurn(message: message)
        case .assistant: AssistantTurn(message: message)
        }
    }
}

// MARK: - You

private struct YouTurn: View {
    let message: ChatMessage
    var body: some View {
        VStack(alignment: .trailing, spacing: 5) {
            Text("You · \(message.timestamp)")
                .font(CSFont.mono(10, .medium))
                .foregroundStyle(CSColor.textFaintAlt)
            VStack(alignment: .leading, spacing: 9) {
                if !message.attachments.isEmpty {
                    WrapLayout(spacing: 6) {
                        ForEach(message.attachments) { AttachmentChip(attachment: $0) }
                    }
                }
                if !message.text.isEmpty {
                    CodeSpanText(raw: message.text, bodyColor: ChatPalette.nameActive)
                        .multilineTextAlignment(.leading)
                }
            }
            .padding(.horizontal, 15)
            .padding(.vertical, 12)
            .background(CSColor.terracotta.opacity(0.15))
            .overlay(
                UnevenRoundedRectangle(
                    topLeadingRadius: 14, bottomLeadingRadius: 14,
                    bottomTrailingRadius: 4, topTrailingRadius: 14,
                    style: .continuous
                )
                .strokeBorder(CSColor.terracotta.opacity(0.22), lineWidth: 1)
            )
            .clipShape(UnevenRoundedRectangle(
                topLeadingRadius: 14, bottomLeadingRadius: 14,
                bottomTrailingRadius: 4, topTrailingRadius: 14,
                style: .continuous
            ))
            .contextMenu { CopyButton(text: message.text) }
        }
        .frame(maxWidth: 510, alignment: .trailing)
    }
}

/// Attachment chip for a sent You turn — mirrors the composer's staged-chip
/// style (icon/thumbnail + mono filename), minus the remove button. Shows a
/// small inline thumbnail when the source image still loads; otherwise falls
/// back to a photo glyph. Loaded once on appear so scrolling doesn't re-decode.
private struct AttachmentChip: View {
    let attachment: MessageAttachment
    @State private var thumbnail: NSImage?

    var body: some View {
        HStack(spacing: 6) {
            if let thumbnail {
                Image(nsImage: thumbnail)
                    .resizable()
                    .aspectRatio(contentMode: .fill)
                    .frame(width: 18, height: 18)
                    .clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
            } else {
                Image(systemName: "photo")
                    .font(.system(size: 11))
                    .foregroundStyle(CSColor.terracottaLight)
            }
            Text(attachment.name)
                .font(CSFont.mono(10.5, .medium))
                .foregroundStyle(CSColor.textBodyAlt)
                .lineLimit(1)
                .frame(maxWidth: 160)
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 5)
        .background(CSColor.surfaceRaised(0.05))
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous)
                .strokeBorder(CSColor.hairline(0.10), lineWidth: 1)
        )
        .clipShape(RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous))
        .onAppear {
            if thumbnail == nil, let url = attachment.url {
                thumbnail = NSImage(contentsOf: url)
            }
        }
    }
}

/// Minimal wrapping layout: lays chips left→right, wrapping to a new row when the
/// next would exceed the proposed width. Hugs its content so the You bubble stays
/// tight around 1–N attachment chips instead of overflowing or forcing full width.
private struct WrapLayout: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout Void) -> CGSize {
        let maxWidth = proposal.width ?? .infinity
        var rowWidth: CGFloat = 0
        var rowHeight: CGFloat = 0
        var totalWidth: CGFloat = 0
        var totalHeight: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if rowWidth > 0, rowWidth + spacing + size.width > maxWidth {
                totalWidth = max(totalWidth, rowWidth)
                totalHeight += rowHeight + spacing
                rowWidth = size.width
                rowHeight = size.height
            } else {
                rowWidth += (rowWidth > 0 ? spacing : 0) + size.width
                rowHeight = max(rowHeight, size.height)
            }
        }
        totalWidth = max(totalWidth, rowWidth)
        totalHeight += rowHeight
        return CGSize(width: min(totalWidth, maxWidth), height: totalHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout Void) {
        var x = bounds.minX
        var y = bounds.minY
        var rowHeight: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > bounds.minX, x + size.width - bounds.minX > bounds.width {
                x = bounds.minX
                y += rowHeight + spacing
                rowHeight = 0
            }
            subview.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
    }
}

// MARK: - Tool activity

private struct ToolTurn: View {
    let message: ChatMessage
    @State private var expanded = true

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text("Tool activity · \(message.timestamp)")
                .font(CSFont.mono(10, .medium))
                .foregroundStyle(CSColor.textFaintAlt)

            DisclosureGroup(isExpanded: $expanded) {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(message.toolLines) { line in
                        (Text(line.verb).foregroundColor(CSColor.oliveLight)
                            + Text(" \(line.detail)").foregroundColor(ChatPalette.toolBody))
                            .font(CSFont.mono(11.5, .medium))
                            .lineSpacing(4)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .padding(.horizontal, 13)
                .padding(.vertical, 11)
            } label: {
                HStack(spacing: 8) {
                    Text("✓")
                        .font(.system(size: 11))
                        .foregroundStyle(CSColor.oliveLight)
                    Text(message.toolTitle)
                        .font(CSFont.mono(11, .semibold))
                        .foregroundStyle(ChatPalette.nameInactive)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 13)
                .padding(.vertical, 10)
                .contentShape(Rectangle())
            }
            .disclosureGroupStyle(FlatDisclosureStyle())
            .background(CSColor.surfaceRaised(0.025))
            .overlay(
                RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
                    .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
            )
            .clipShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))
        }
        .frame(maxWidth: 560, alignment: .leading)
    }
}

/// DisclosureGroup without the default chevron/indent — the label IS the header
/// row, with a hairline divider above the content when expanded.
private struct FlatDisclosureStyle: DisclosureGroupStyle {
    func makeBody(configuration: Configuration) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.easeOut(duration: 0.18)) {
                    configuration.isExpanded.toggle()
                }
            } label: {
                configuration.label
            }
            .buttonStyle(.plain)

            if configuration.isExpanded {
                Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
                configuration.content
            }
        }
    }
}

// MARK: - Assistant

private struct AssistantTurn: View {
    let message: ChatMessage

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text("Assistant · \(message.timestamp)")
                .font(CSFont.mono(10, .medium))
                .foregroundStyle(CSColor.textFaintAlt)

            VStack(alignment: .leading, spacing: 9) {
                if message.isThinking {
                    HStack(spacing: 8) {
                        PulseDot()
                        Text("thinking…")
                            .font(CSFont.mono(12, .medium))
                            .foregroundStyle(ChatPalette.thinking)
                    }
                } else {
                    if let secs = message.reasonedSeconds {
                        ReasonedChip(seconds: secs)
                    }
                    if message.isStreaming {
                        HStack(alignment: .bottom, spacing: 2) {
                            CodeSpanText(raw: message.text)
                            BlinkCaret()
                        }
                    } else {
                        CodeSpanText(raw: message.text)
                    }
                }
            }
            .padding(.horizontal, 15)
            .padding(.vertical, 13)
            .background(CSColor.surfaceRaised(0.03))
            .overlay(
                UnevenRoundedRectangle(
                    topLeadingRadius: 14, bottomLeadingRadius: 4,
                    bottomTrailingRadius: 14, topTrailingRadius: 14,
                    style: .continuous
                )
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
            )
            .clipShape(UnevenRoundedRectangle(
                topLeadingRadius: 14, bottomLeadingRadius: 4,
                bottomTrailingRadius: 14, topTrailingRadius: 14,
                style: .continuous
            ))
            .contextMenu { CopyButton(text: message.text) }
        }
        .frame(maxWidth: 560, alignment: .leading)
    }
}

/// Right-click "Copy" that puts a message's raw text on the pasteboard.
private struct CopyButton: View {
    let text: String
    var body: some View {
        Button("Copy") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
        }
    }
}

/// Amber "reasoned · Xs" pill.
private struct ReasonedChip: View {
    let seconds: Double
    var body: some View {
        Text("reasoned · \(String(format: "%.1f", seconds))s")
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(CSColor.amber)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(CSColor.amber.opacity(0.1))
            .overlay(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .strokeBorder(CSColor.amber.opacity(0.22), lineWidth: 1)
            )
            .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
    }
}

/// Amber softpulsing dot for the "thinking…" state.
private struct PulseDot: View {
    @State private var pulse = false
    var body: some View {
        Circle()
            .fill(CSColor.amber)
            .frame(width: 6, height: 6)
            .opacity(pulse ? 1 : 0.6)
            .onAppear { withAnimation(CSMotion.softpulse) { pulse = true } }
    }
}
