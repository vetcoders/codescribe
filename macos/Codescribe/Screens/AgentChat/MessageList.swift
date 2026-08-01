import SwiftUI
import AppKit

enum StreamScrollFollowAction: Equatable {
    case none
    case scrollToLiveEdge
}

/// Explicit state machine for the message viewport. Content growth is allowed
/// to move the viewport only while the operator is following the live edge.
struct StreamScrollFollowState: Equatable {
    enum Event: Equatable {
        case contentChanged
        case userScrollBegan
        case userViewportChanged(isAtLiveEdge: Bool)
        case userScrollEnded(isAtLiveEdge: Bool)
        case jumpToCurrent
        case threadChanged
        case streamFinished
    }

    private(set) var followingLive = true
    var showsJumpToCurrent: Bool { !followingLive }

    mutating func handle(_ event: Event) -> StreamScrollFollowAction {
        switch event {
        case .contentChanged:
            return followingLive ? .scrollToLiveEdge : .none
        case .userScrollBegan:
            followingLive = false
            return .none
        case .userViewportChanged:
            // Geometry changes during an active gesture may still report the
            // old live-edge position. Never let that stale signal steal the
            // viewport back from the operator.
            return .none
        case let .userScrollEnded(isAtLiveEdge):
            followingLive = isAtLiveEdge
            return .none
        case .jumpToCurrent, .threadChanged:
            followingLive = true
            return .scrollToLiveEdge
        case .streamFinished:
            return .none
        }
    }
}

/// Scrolling turn list: You (terracotta bubble, right) · Tool activity
/// (DisclosureGroup, mono) · Assistant (amber "reasoned · Xs" chip + body,
/// last turn streams with a blink caret). Auto-scrolls to the newest turn.
struct MessageList: View {
    let threadID: UUID
    let messages: [ChatMessage]
    /// Flips a bubble between raw mono and rich markdown. State lives in the
    /// store (per-message `renderMode`), never in this view.
    var onToggleRenderMode: (UUID) -> Void = { _ in }

    /// Follow-tail with pause-on-scroll (the overlay transcript pattern): auto-scroll
    /// to the newest turn only while the user is already at the bottom. Scrolling up
    /// during a stream pauses the follow; returning to the bottom resumes it, so the
    /// view stops fighting the user's manual scroll on a long streamed message.
    @State private var followState = StreamScrollFollowState()
    private let scrollSpace = "chatMessageScroll"
    private let bottomAnchor = "chatMessageBottom"

    var body: some View {
        GeometryReader { viewport in
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 16) {
                        ForEach(messages) { message in
                            turn(message)
                                .frame(maxWidth: .infinity, alignment: alignment(message.role))
                                .id(message.id)
                        }
                        Color.clear
                            .frame(height: 1)
                            .id(bottomAnchor)
                    }
                    .padding(20)
                    .background(
                        GeometryReader { content in
                            Color.clear.preference(
                                key: ChatBottomKey.self,
                                value: content.frame(in: .named(scrollSpace)).maxY
                            )
                        }
                    )
                    // Lives inside the document view so `enclosingScrollView`
                    // resolves to this message list rather than an outer pane.
                    .background(
                        ChatLiveScrollObserver { event in
                            _ = followState.handle(event)
                        }
                    )
                }
                .coordinateSpace(name: scrollSpace)
                .scrollContentBackground(.hidden)
                // Let the user drag-select message text and Cmd+C it (SwiftUI Text is
                // not selectable by default). Per-message "Copy" lives in the bubble
                // context menu below.
                .textSelection(.enabled)
                .onPreferenceChange(ChatBottomKey.self) { contentBottom in
                    let isAtLiveEdge = Self.followTailAfterScroll(
                        contentBottom: contentBottom,
                        viewportHeight: viewport.size.height
                    )
                    _ = followState.handle(.userViewportChanged(isAtLiveEdge: isAtLiveEdge))
                }
                .onChange(of: Self.tailSignature(messages)) { _, _ in
                    perform(followState.handle(.contentChanged), with: proxy)
                }
                .onChange(of: messages.last?.isStreaming == true) { wasStreaming, isStreaming in
                    if wasStreaming, !isStreaming {
                        _ = followState.handle(.streamFinished)
                    }
                }
                .onChange(of: threadID) { _, _ in
                    perform(followState.handle(.threadChanged), with: proxy)
                }
                .onAppear {
                    perform(.scrollToLiveEdge, with: proxy, animated: false)
                }
                .overlay(alignment: .bottom) {
                    let pillVisible = followState.showsJumpToCurrent
                    ZStack {
                        if pillVisible {
                            JumpToCurrentButton {
                                perform(followState.handle(.jumpToCurrent), with: proxy)
                            }
                            .padding(.bottom, 10)
                            .transition(.opacity.combined(with: .move(edge: .bottom)))
                        }
                    }
                    .animation(.easeOut(duration: 0.18), value: pillVisible)
                }
            }
        }
    }

    private func perform(
        _ action: StreamScrollFollowAction,
        with proxy: ScrollViewProxy,
        animated: Bool = true
    ) {
        guard action == .scrollToLiveEdge else { return }
        if animated {
            withAnimation(.easeOut(duration: 0.25)) {
                proxy.scrollTo(bottomAnchor, anchor: .bottom)
            }
        } else {
            proxy.scrollTo(bottomAnchor, anchor: .bottom)
        }
    }

    // MARK: Pure scroll/pill logic (XCTest-covered, see MessageListFollowTailTests)

    /// At-bottom decision: the content's bottom edge sits within `slack` of the
    /// viewport's bottom. Drives follow on/off from the scroll preference.
    static func followTailAfterScroll(contentBottom: CGFloat, viewportHeight: CGFloat,
                                      slack: CGFloat = 40) -> Bool {
        contentBottom <= viewportHeight + slack
    }

    /// Changes whenever a new turn lands or the streaming tail grows — the
    /// auto-scroll trigger. Deliberately cheap for the per-delta hot path:
    /// `utf8.count` is O(1) on native strings (grapheme `count` walks the whole
    /// text — 100k steps per tick on a large pasted turn), only the last two
    /// turns matter (the tool row + the streaming bubble; `messages.count`
    /// catches insertions), and no tool detail strings are concatenated.
    /// `renderMode` is excluded on purpose: a raw↔rich flip must not scroll.
    static func tailSignature(_ messages: [ChatMessage]) -> String {
        var signature = "\(messages.count)"
        for message in messages.suffix(2) {
            let running = message.toolLines.lazy.filter { $0.state == .running }.count
            signature += "|\(message.id)-\(message.text.utf8.count)"
                + "-\(message.reasoning.utf8.count)-\(message.toolLines.count)-\(running)"
        }
        return signature
    }

    private func alignment(_ role: ChatRole) -> Alignment {
        role == .you ? .trailing : .leading
    }

    @ViewBuilder
    private func turn(_ message: ChatMessage) -> some View {
        switch message.role {
        case .you: YouTurn(message: message)
        case .tool: ToolTurn(message: message)
        case .assistant: AssistantTurn(message: message, onToggleRenderMode: onToggleRenderMode)
        }
    }
}

/// Floating return affordance. It remains available after a stream settles:
/// finishing generation never takes the operator's chosen reading position.
private struct JumpToCurrentButton: View {
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                CSIconView(icon: .chevronDown, size: 9, weight: .semibold,
                           color: CSColor.chromeAccent)
                Text("Current")
                    .font(CSFont.mono(10.5, .medium))
                    .foregroundStyle(hovering ? CSColor.textHigh : CSColor.textBody)
            }
            .padding(.horizontal, 11)
            .padding(.vertical, 6)
            .background(CSColor.glassUnder.opacity(0.92))
            .background(CSColor.surfaceRaised(0.05))
            .overlay(
                RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous)
                    .strokeBorder(CSColor.hairline(0.12), lineWidth: 1)
            )
            .clipShape(RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityLabel("Jump to current")
        .help("Jump to the current reply")
    }
}

/// Carries the message list content's bottom-edge Y (in the scroll's coordinate
/// space) up to the follow-tail detector. Mirrors the overlay transcript's key.
private struct ChatBottomKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}

/// macOS 14-compatible user-intent detector. SwiftUI's geometry preference
/// reports position but cannot distinguish a wheel/trackpad/scrollbar gesture
/// from `ScrollViewProxy.scrollTo`; AppKit live-scroll notifications can.
private struct ChatLiveScrollObserver: NSViewRepresentable {
    let onEvent: (StreamScrollFollowState.Event) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(onEvent: onEvent)
    }

    func makeNSView(context: Context) -> AttachmentView {
        let view = AttachmentView()
        view.onAttach = { [weak coordinator = context.coordinator] scrollView in
            coordinator?.attach(to: scrollView)
        }
        return view
    }

    func updateNSView(_ nsView: AttachmentView, context: Context) {
        context.coordinator.onEvent = onEvent
        nsView.attachWhenReady()
    }

    static func dismantleNSView(_ nsView: AttachmentView, coordinator: Coordinator) {
        coordinator.detach()
    }

    final class AttachmentView: NSView {
        var onAttach: ((NSScrollView) -> Void)?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            attachWhenReady()
        }

        func attachWhenReady() {
            DispatchQueue.main.async { [weak self] in
                guard let self, let scrollView = enclosingScrollView else { return }
                onAttach?(scrollView)
            }
        }
    }

    final class Coordinator {
        var onEvent: (StreamScrollFollowState.Event) -> Void
        private weak var scrollView: NSScrollView?
        private var observers: [NSObjectProtocol] = []

        init(onEvent: @escaping (StreamScrollFollowState.Event) -> Void) {
            self.onEvent = onEvent
        }

        func attach(to scrollView: NSScrollView) {
            guard self.scrollView !== scrollView else { return }
            detach()
            self.scrollView = scrollView
            let center = NotificationCenter.default
            observers = [
                center.addObserver(
                    forName: NSScrollView.willStartLiveScrollNotification,
                    object: scrollView,
                    queue: .main
                ) { [weak self] _ in
                    self?.onEvent(.userScrollBegan)
                },
                center.addObserver(
                    forName: NSScrollView.didLiveScrollNotification,
                    object: scrollView,
                    queue: .main
                ) { [weak self] _ in
                    self?.reportViewport(asScrollEnd: true)
                },
                center.addObserver(
                    forName: NSScrollView.didEndLiveScrollNotification,
                    object: scrollView,
                    queue: .main
                ) { [weak self] _ in
                    self?.reportViewport()
                },
            ]
        }

        func detach() {
            let center = NotificationCenter.default
            observers.forEach(center.removeObserver)
            observers.removeAll()
            scrollView = nil
        }

        private func reportViewport(asScrollEnd: Bool = false) {
            guard let scrollView, let documentView = scrollView.documentView else { return }
            let visibleBottom = scrollView.contentView.documentVisibleRect.maxY
            let distanceFromLiveEdge = documentView.bounds.maxY - visibleBottom
            let isAtLiveEdge = distanceFromLiveEdge <= 40
            onEvent(asScrollEnd
                ? .userScrollEnded(isAtLiveEdge: isAtLiveEdge)
                : .userViewportChanged(isAtLiveEdge: isAtLiveEdge))
        }

        deinit {
            detach()
        }
    }
}

// MARK: - You

private struct YouTurn: View {
    let message: ChatMessage

    /// Copies the raw prompt text; for a text-less image turn falls back to the
    /// attachment filenames so the button still does something useful.
    private var copyText: String {
        message.text.isEmpty
            ? message.attachments.map(\.name).joined(separator: "\n")
            : message.text
    }

    private var hasContext: Bool {
        message.contextSelection != nil || message.contextApp != nil
    }

    var body: some View {
        VStack(alignment: .trailing, spacing: 5) {
            HStack(spacing: 8) {
                Text("You · \(message.timestamp)")
                    .font(CSFont.mono(10, .medium))
                    .foregroundStyle(CSColor.terracottaDeep.opacity(0.85))
                CopyMessageButton(text: copyText)
            }
            VStack(alignment: .leading, spacing: 9) {
                if !message.attachments.isEmpty {
                    WrapLayout(spacing: 6) {
                        ForEach(message.attachments) { AttachmentChip(attachment: $0) }
                    }
                }
                if !message.text.isEmpty {
                    MarkdownText(raw: message.text, bodyColor: ChatPalette.nameActive)
                }
                if hasContext {
                    ContextChip(
                        selection: message.contextSelection,
                        app: message.contextApp
                    )
                }
            }
            .padding(.horizontal, 15)
            .padding(.vertical, 12)
            // Calm surface, not an alarm plate (U17): the bubble sits on the
            // shared raised surface; terracotta stays on ACCENTS only — the
            // timestamp above and this thin border.
            .background(CSColor.surfaceRaised(0.06))
            .overlay(
                UnevenRoundedRectangle(
                    topLeadingRadius: 14, bottomLeadingRadius: 14,
                    bottomTrailingRadius: 4, topTrailingRadius: 14,
                    style: .continuous
                )
                .strokeBorder(CSColor.terracotta.opacity(0.18), lineWidth: 1)
            )
            .clipShape(UnevenRoundedRectangle(
                topLeadingRadius: 14, bottomLeadingRadius: 14,
                bottomTrailingRadius: 4, topTrailingRadius: 14,
                style: .continuous
            ))
            .contextMenu {
                CopyButton(text: message.text)
                if let wire = message.wireText {
                    // Debug affordance: the exact prompt the model received,
                    // skeleton and all.
                    Button("Copy full prompt") { chatCopy(wire) }
                }
            }
        }
        .frame(maxWidth: 760, alignment: .trailing)
    }
}

/// Collapsed "context ▸" disclosure inside the You bubble: reveals the selection
/// and frontmost app that rode along with an assistive voice turn. Collapsed by
/// default so the bubble reads as just the spoken instruction.
private struct ContextChip: View {
    let selection: String?
    let app: String?
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(.easeOut(duration: 0.18)) { expanded.toggle() }
            } label: {
                HStack(spacing: 4) {
                    CSIconView(
                        icon: expanded ? .chevronDown : .chevronRight,
                        size: 8,
                        weight: .semibold,
                        color: CSColor.textFaintAlt
                    )
                    Text("context")
                        .font(CSFont.mono(10, .medium))
                        .foregroundStyle(CSColor.textFaintAlt)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("Selection and app captured with this voice turn")

            if expanded {
                VStack(alignment: .leading, spacing: 5) {
                    if let app {
                        Text("app · \(app)")
                            .font(CSFont.mono(10.5, .medium))
                            .foregroundStyle(CSColor.textMuted)
                    }
                    if let selection {
                        Text(selection)
                            .font(CSFont.mono(10.5))
                            .foregroundStyle(CSColor.textBodyAlt)
                            .textSelection(.enabled)
                            .lineSpacing(3)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(8)
                            .background(CSColor.surfaceRaised(0.05))
                            .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
                    }
                }
            }
        }
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
        Button {
            if let url = attachment.url {
                NSWorkspace.shared.open(url)
            }
        } label: {
            HStack(spacing: 6) {
            if let thumbnail {
                Image(nsImage: thumbnail)
                    .resizable()
                    .aspectRatio(contentMode: .fill)
                    .frame(width: 18, height: 18)
                    .clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
            } else {
                CSIconView(icon: .photo, size: 11, color: CSColor.chromeAccent)
            }
            Text(attachment.name)
                .font(CSFont.mono(10.5, .medium))
                .foregroundStyle(CSColor.textBodyAlt)
                .lineLimit(1)
                .truncationMode(.middle)
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
        }
        .buttonStyle(.plain)
        .disabled(attachment.url == nil)
        .help(attachment.url == nil ? "Original attachment is unavailable" : "Open attachment")
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

/// One tool-activity line. Any settled line carrying result context becomes a
/// compact disclosure: the row is tappable and reveals the full tool summary,
/// collapsed by default so the list stays scannable. Both the verb/detail row and
/// the revealed reason are text-selectable.
private struct ToolLineRow: View {
    let line: ToolLine
    @State private var showReason = false

    private var reason: String? {
        guard let reason = line.reason, !reason.isEmpty else { return nil }
        return reason
    }

    private var isRunning: Bool { line.state == .running }
    private var isQuiet: Bool { line.state == .unknown || line.state == .cancelled }
    private var rowColor: Color {
        switch line.state {
        case .running:
            return CSColor.amber
        case .failed:
            return CSColor.terracottaLight
        case .cancelled, .unknown:
            return CSColor.textFaintAlt
        case .succeeded:
            return CSColor.oliveLight
        }
    }

    var body: some View {
        let hasDetail = reason != nil
        VStack(alignment: .leading, spacing: 4) {
            Button {
                if hasDetail { showReason.toggle() }
            } label: {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    if isRunning {
                        PulseDot()
                    }
                    (Text(line.verb).foregroundColor(rowColor)
                        + Text(" \(line.detail)\(isRunning ? " running..." : "")").foregroundColor(isQuiet ? CSColor.textFaintAlt : ChatPalette.toolBody))
                        .font(CSFont.mono(11.5, .medium))
                        .lineSpacing(4)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if hasDetail {
                        CSIconView(
                            icon: showReason ? .chevronDown : .chevronRight,
                            size: 8,
                            weight: .semibold,
                            color: rowColor.opacity(0.75)
                        )
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(!hasDetail)

            if hasDetail, showReason, let reason {
                Text(reason)
                    .font(CSFont.mono(10.5, .medium))
                    .foregroundStyle(line.state == .failed ? CSColor.terracottaLight : CSColor.textBodyAlt)
                    .textSelection(.enabled)
                    .lineSpacing(2)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.leading, 10)
            }
        }
    }
}

private struct ToolTurn: View {
    let message: ChatMessage
    @State private var expanded = false

    /// Whole-card plain-text export: one line per tool, `verb detail` for a
    /// successful line and `verb detail — reason` (full, untruncated) for a
    /// failed one. Mirrors what the rows render, minus the styling.
    private var copyText: String {
        message.toolLines.map { line in
            if let reason = line.reason, !reason.isEmpty {
                return "\(line.verb) \(line.detail) — \(reason)"
            }
            return "\(line.verb) \(line.detail)"
        }.joined(separator: "\n")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 8) {
                Text("Tool activity · \(message.timestamp)")
                    .font(CSFont.mono(10, .medium))
                    .foregroundStyle(CSColor.textFaintAlt)
                CopyMessageButton(text: copyText)
                Spacer(minLength: 0)
            }

            DisclosureGroup(isExpanded: $expanded) {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(message.toolLines) { line in
                        ToolLineRow(line: line)
                    }
                }
                .padding(.horizontal, 13)
                .padding(.vertical, 11)
            } label: {
                HStack(spacing: 8) {
                    let hasRunning = message.toolLines.contains(where: { $0.state == .running })
                    let hasCancelled = message.toolLines.contains(where: { $0.state == .cancelled })
                    CSIconView(
                        icon: hasRunning ? .more : hasCancelled ? .stop : .success,
                        size: 11,
                        color: hasRunning ? CSColor.amber : hasCancelled ? CSColor.textFaintAlt : CSColor.oliveLight
                    )
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
        .frame(maxWidth: 900, alignment: .leading)
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
    let onToggleRenderMode: (UUID) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 8) {
                Text("Assistant · \(message.timestamp)")
                    .font(CSFont.mono(10, .medium))
                    .foregroundStyle(CSColor.textFaintAlt)
                if !message.isThinking {
                    CopyMessageButton(text: message.text)
                    if !message.text.isEmpty {
                        RenderModeButton(mode: message.renderMode) {
                            onToggleRenderMode(message.id)
                        }
                    }
                }
                Spacer(minLength: 0)
            }

            VStack(alignment: .leading, spacing: 9) {
                if !message.reasoning.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    ReasoningDisclosure(
                        text: message.reasoning,
                        isLive: message.isThinking || message.isStreaming
                    )
                }
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
                    // Stream in the cheap raw view; settled messages render rich
                    // by default. The per-bubble toggle remains available when
                    // exact source text is more useful than presentation.
                    if message.wasStopped, message.text == "Stopped" {
                        Text("Stopped")
                            .font(CSFont.mono(11, .medium))
                            .foregroundStyle(CSColor.textFaintAlt)
                    } else if message.isStreaming {
                        RawText(raw: message.text, showsCaret: true)
                    } else if !message.text.isEmpty {
                        switch message.renderMode {
                        case .raw:
                            RawText(raw: message.text)
                        case .rich:
                            MarkdownText(raw: message.text)
                        }
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
        .frame(maxWidth: 900, alignment: .leading)
    }
}

private struct ReasoningDisclosure: View {
    let text: String
    let isLive: Bool
    @State private var expanded = false

    var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            Text(text)
                .font(CSFont.mono(10.5, .medium))
                .foregroundStyle(ChatPalette.thinking.opacity(0.86))
                .textSelection(.enabled)
                .lineSpacing(3)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 11)
                .padding(.vertical, 9)
        } label: {
            HStack(spacing: 7) {
                CSIconView(
                    icon: expanded ? .chevronDown : .chevronRight,
                    size: 8,
                    weight: .semibold,
                    color: ChatPalette.thinking.opacity(0.75)
                )
                Text(isLive ? "thinking..." : "reasoning summary")
                    .font(CSFont.mono(10.5, .semibold))
                    .foregroundStyle(ChatPalette.thinking)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 11)
            .padding(.vertical, 8)
            .contentShape(Rectangle())
        }
        .disclosureGroupStyle(FlatDisclosureStyle())
        .background(CSColor.surfaceRaised(0.018))
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
                .strokeBorder(CSColor.hairline(0.055), lineWidth: 1)
        )
        .clipShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))
    }
}

/// Plain mono body — the raw render mode. Exactly what streamed in, no markdown
/// pass at all, so a growing turn costs a plain `Text` re-eval per delta and the
/// settled turn is byte-for-byte the same view (no finalize re-render).
private struct RawText: View {
    let raw: String
    var showsCaret: Bool = false
    @Environment(\.csTextScale) private var textScale

    var body: some View {
        let content = Text(raw)
            .font(CSFont.mono(13 * textScale))
            .foregroundStyle(CSColor.textBodyAlt)
            .lineSpacing(4)
            .fixedSize(horizontal: false, vertical: true)
        if showsCaret {
            HStack(alignment: .bottom, spacing: 2) {
                content
                BlinkCaret()
            }
        } else {
            content.frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

/// Inline raw↔rich toggle in the assistant meta row, next to "copy". The label
/// names the mode a click switches TO (mirrors the copy button's action-verb
/// style). Mutation goes through the store via `onToggleRenderMode` — the view
/// holds no render-mode state.
private struct RenderModeButton: View {
    let mode: MessageRenderMode
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                CSIconView(icon: .setupWizard, size: 9)
                Text(mode == .raw ? "rich" : "raw")
                    .font(CSFont.mono(10, .medium))
            }
            .foregroundStyle(hovering ? CSColor.textMuted : CSColor.textFaintAlt)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(mode == .raw ? "Render as markdown" : "Show raw text")
    }
}

/// Puts a message's raw text on the general pasteboard — the single copy path
/// shared by the bubble context menu and the inline copy button.
private func chatCopy(_ text: String) {
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(text, forType: .string)
}

/// Right-click "Copy" that puts a message's raw text on the pasteboard.
private struct CopyButton: View {
    let text: String
    var body: some View {
        Button("Copy") { chatCopy(text) }
    }
}

/// Subtle inline "copy" affordance in a turn's meta row (mono 10, faint until
/// hovered). Copies the raw pre-render text via the same `chatCopy` path the
/// context menu uses, then flips to a green "copied" for ~1.5s. Disabled when
/// there is nothing to copy.
private struct CopyMessageButton: View {
    let text: String
    @State private var copied = false
    @State private var hovering = false

    var body: some View {
        Button {
            chatCopy(text)
            copied = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { copied = false }
        } label: {
            HStack(spacing: 4) {
                CSIconView(icon: copied ? .check : .copy, size: 9)
                Text(copied ? "copied" : "copy")
                    .font(CSFont.mono(10, .medium))
            }
            .foregroundStyle(labelColor)
        }
        .buttonStyle(.plain)
        .disabled(text.isEmpty)
        .onHover { hovering = $0 }
        .help("Copy message")
    }

    private var labelColor: Color {
        if copied { return CSColor.oliveLight }
        return hovering ? CSColor.textMuted : CSColor.textFaintAlt
    }
}

/// Amber "reasoned · Xs" pill.
private struct ReasonedChip: View {
    let seconds: Double
    var body: some View {
        Text("worked · \(String(format: "%.1f", seconds))s")
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
