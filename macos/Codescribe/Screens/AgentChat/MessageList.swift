import AppKit
import SwiftUI

enum StreamScrollFollowAction: Equatable {
  case none
  case scrollToLiveEdge
}

/// Operator-facing chat column density. Persisted via `ChatLayoutPolicy.defaultsKey`.
///
/// Plan P0-1: Comfortable / Wide / Full with remembered choice. Width still
/// derives from the viewport (not a static pt constant); the mode only changes
/// how aggressively the column fills available space and when the prose cap
/// kicks in so code fences / tables can claim room on wide monitors.
enum ChatWidthMode: String, CaseIterable, Identifiable {
  case comfortable
  case wide
  case full

  var id: String { rawValue }

  var label: String {
    switch self {
    case .comfortable: return "Comfortable"
    case .wide: return "Wide"
    case .full: return "Full"
    }
  }

  /// You-bubble share of usable width (chat-style trailing bubble).
  var youFraction: CGFloat {
    switch self {
    case .comfortable: return 0.58
    case .wide: return 0.72
    case .full: return 0.88
    }
  }

  /// Soft upper bound for leading (assistant/tool) prose. `nil` = fill usable.
  var proseComfortCap: CGFloat? {
    switch self {
    case .comfortable: return 720
    case .wide: return 920
    case .full: return nil
    }
  }

  static func resolve(_ raw: String) -> ChatWidthMode {
    ChatWidthMode(rawValue: raw) ?? .wide
  }
}

/// Container-relative bubble widths for AgentChat.
///
/// Wave 8ccf141a raised fixed caps (510→760 / ~900) but left a middle column on
/// wide windows. Plan step 1 wants width from the *viewport*, not a static pt
/// constant: You stays a readable bubble; assistant/tool use the full usable
/// column (subject to `ChatWidthMode`) so code fences and tables stop clipping.
enum ChatLayoutPolicy {
  /// `UserDefaults` / `@AppStorage` key for the operator width preference.
  static let defaultsKey = "codescribe.chatWidthMode"
  /// Horizontal padding applied by `MessageList` around the LazyVStack.
  static let listPadding: CGFloat = 20
  /// Minimum readable bubble width on a narrow window.
  static let minimumReadable: CGFloat = 280
  /// Default mode when the preference is missing or unknown.
  static let defaultMode: ChatWidthMode = .wide

  /// Usable content width after list padding (both sides).
  static func contentWidth(for containerWidth: CGFloat) -> CGFloat {
    max(0, containerWidth - listPadding * 2)
  }

  /// Max width for a You bubble given the scroll viewport width.
  static func youBubbleMaxWidth(
    containerWidth: CGFloat,
    mode: ChatWidthMode = defaultMode
  ) -> CGFloat {
    let usable = contentWidth(for: containerWidth)
    guard usable > 0 else { return minimumReadable }
    let proportional = usable * mode.youFraction
    return max(minimumReadable, min(usable, proportional))
  }

  /// Max width for assistant / tool turns. Fills the column up to the mode's
  /// prose comfort cap (Full has no cap) so ultrawide windows do not produce
  /// unreadable body lines while still giving code/tables more room than the
  /// old 900pt hard cap on typical laptop widths.
  static func leadingColumnMaxWidth(
    containerWidth: CGFloat,
    mode: ChatWidthMode = defaultMode
  ) -> CGFloat {
    let usable = contentWidth(for: containerWidth)
    guard usable > 0 else { return minimumReadable }
    guard let cap = mode.proseComfortCap else {
      return max(minimumReadable, usable)
    }
    return max(minimumReadable, min(usable, cap))
  }

  /// Hard ceiling for any turn's laid-out width inside the scroll document.
  /// Prevents a single long unbreakable token (or a mis-parsed wire dump)
  /// from growing the ScrollView's content width past the viewport — the
  /// class of bug that floats glyphs like `)"` outside the Agent window.
  static func documentWidth(for containerWidth: CGFloat) -> CGFloat {
    max(minimumReadable, containerWidth > 0 ? containerWidth : minimumReadable)
  }
}

/// Render disposition for one bubble's text (bolączka #3 residual, hang report
/// 2026-08-04): SwiftUI's shared `SelectionOverlay` livelocks the main thread
/// over unbounded transcript text, so a bubble past `OversizedBubblePolicy
/// .inlineUTF8Cap` must degrade to a head preview whose full text lives in a
/// contained text view with its own selection — never under the list-wide
/// `.textSelection(.enabled)`.
enum BubbleTextDisposition: Equatable {
  /// Full text inline, sharing the list selection surface.
  case inline
  /// Oversized: render only the first `headUTF8` bytes inline; full text
  /// behind an explicit reveal in a selection-contained view.
  case headPreview(headUTF8: Int)

  /// Whether this bubble's text participates in the list-wide selection
  /// overlay — the mechanism the 2026-08-04 hang livelocked on.
  var sharesListSelectionOverlay: Bool {
    switch self {
    case .inline: return true
    case .headPreview: return false
    }
  }
}

enum OversizedBubblePolicy {
  /// UTF-8 length past which a bubble stops rendering inline. 64 KiB: four
  /// times the agent tool-output spill limit, well below the 100k paste that
  /// reproduced the livelock.
  static let inlineUTF8Cap = 65_536
  /// UTF-8 length of the head shown inline for an oversized bubble.
  static let headPreviewUTF8 = 16_384

  static func disposition(utf8Count: Int) -> BubbleTextDisposition {
    utf8Count <= inlineUTF8Cap
      ? .inline
      : .headPreview(headUTF8: headPreviewUTF8)
  }
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
    case .userScrollEnded(let isAtLiveEdge):
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
  /// Hang guard (2026-08-04 livelock): LazyVStack pays O(items) layout phases
  /// per view-graph transaction, so an unbounded multi-hour thread eventually
  /// outruns the run loop even with healthy per-item costs. Only the newest
  /// window renders; "Show earlier" pages the history in on demand.
  @State private var visibleTurnBudget = MessageList.turnWindow
  /// Operator width density — shared with the header picker via `@AppStorage`.
  @AppStorage(ChatLayoutPolicy.defaultsKey) private var widthModeRaw = ChatLayoutPolicy.defaultMode
    .rawValue
  private let scrollSpace = "chatMessageScroll"
  private let bottomAnchor = "chatMessageBottom"

  private var widthMode: ChatWidthMode { ChatWidthMode.resolve(widthModeRaw) }

  /// Newest turns per page of the render window (`visibleTurnBudget` grows by
  /// this step on every "Show earlier"). Sized so a normal working session
  /// never sees the affordance while a 49-hour thread stays bounded.
  static let turnWindow = 120

  /// The rendered slice: newest `visibleTurnBudget` turns.
  private var visibleMessages: ArraySlice<ChatMessage> {
    messages.suffix(visibleTurnBudget)
  }

  private var hiddenTurnCount: Int {
    max(0, messages.count - visibleTurnBudget)
  }

  var body: some View {
    GeometryReader { viewport in
      let containerWidth = viewport.size.width
      ScrollViewReader { proxy in
        ScrollView {
          LazyVStack(spacing: 16) {
            if hiddenTurnCount > 0 {
              ShowEarlierButton(hiddenCount: hiddenTurnCount) {
                visibleTurnBudget += Self.turnWindow
              }
            }
            ForEach(visibleMessages) { message in
              turn(message, containerWidth: containerWidth, mode: widthMode)
                .frame(maxWidth: .infinity, alignment: alignment(message.role))
                .id(message.id)
            }
            Color.clear
              .frame(height: 1)
              .id(bottomAnchor)
          }
          // Pin the document to the viewport width so a single
          // long-line bubble cannot widen the scroll content and
          // paint outside the Agent window chrome (R1 collapse).
          .frame(
            maxWidth: ChatLayoutPolicy.documentWidth(for: containerWidth),
            alignment: .topLeading
          )
          .padding(ChatLayoutPolicy.listPadding)
          .clipped()
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
        // NO list-wide `.textSelection(.enabled)` here — the shared
        // SelectionOverlay spanning the whole LazyVStack is the exact
        // mechanism the 2026-08-04 livelock spun on (every view-graph
        // flush re-walked the full transcript's selection geometry).
        // Selection lives per body instead: MarkdownText root, RawText,
        // tool rows, reasoning and context chips all enable it locally,
        // so drag-select + Cmd+C keep working inside any bubble.
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
          // A fresh thread starts back at the bounded window; an
          // expanded budget must not leak across conversations.
          visibleTurnBudget = Self.turnWindow
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
  static func followTailAfterScroll(
    contentBottom: CGFloat, viewportHeight: CGFloat,
    slack: CGFloat = 40
  ) -> Bool {
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
      signature +=
        "|\(message.id)-\(message.text.utf8.count)"
        + "-\(message.reasoning.utf8.count)-\(message.toolLines.count)-\(running)"
    }
    return signature
  }

  private func alignment(_ role: ChatRole) -> Alignment {
    role == .you ? .trailing : .leading
  }

  @ViewBuilder
  private func turn(
    _ message: ChatMessage,
    containerWidth: CGFloat,
    mode: ChatWidthMode
  ) -> some View {
    switch message.role {
    case .you:
      YouTurn(message: message, containerWidth: containerWidth, mode: mode)
    case .tool:
      ToolTurn(message: message, containerWidth: containerWidth, mode: mode)
    case .assistant:
      AssistantTurn(
        message: message,
        containerWidth: containerWidth,
        mode: mode,
        onToggleRenderMode: onToggleRenderMode
      )
    }
  }
}

/// Top-of-list pager for the bounded render window: names how much history is
/// folded away and loads one more window per click. Deliberately a plain row,
/// not infinite scroll — paging must stay an explicit operator action so the
/// hang guard cannot be defeated by an idle scroll position.
private struct ShowEarlierButton: View {
  let hiddenCount: Int
  let action: () -> Void
  @State private var hovering = false

  var body: some View {
    Button(action: action) {
      HStack(spacing: 6) {
        CSIconView(
          icon: .chevronRight, size: 8, weight: .semibold,
          color: CSColor.textFaintAlt)
        Text("Show earlier · \(hiddenCount) turn\(hiddenCount == 1 ? "" : "s")")
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(hovering ? CSColor.textBody : CSColor.textFaintAlt)
      }
      .padding(.horizontal, 11)
      .padding(.vertical, 6)
      .background(CSColor.surfaceRaised(0.04))
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous)
          .strokeBorder(CSColor.hairline(0.10), lineWidth: 1)
      )
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous))
      .contentShape(Rectangle())
    }
    .csFocusRing(cornerRadius: CSRadius.pill)
    .onHover { hovering = $0 }
    .accessibilityLabel("Show earlier messages")
    .help("Render the previous \(MessageList.turnWindow) turns")
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
        CSIconView(
          icon: .chevronDown, size: 9, weight: .semibold,
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
    .csFocusRing(cornerRadius: CSRadius.pill)
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
      onEvent(
        asScrollEnd
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
  let containerWidth: CGFloat
  let mode: ChatWidthMode

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
          // Oversized paste (bolączka #3): fold above the display
          // budget so the list's selection overlay never carries the
          // whole payload. Copy above still exports the full text.
          if OversizedBubblePolicy.isOversized(message.text) {
            OversizedMessageBody(fullText: message.text) { head in
              MarkdownText(raw: head, bodyColor: ChatPalette.nameActive)
            }
          } else {
            MarkdownText(raw: message.text, bodyColor: ChatPalette.nameActive)
          }
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
      .clipShape(
        UnevenRoundedRectangle(
          topLeadingRadius: 14, bottomLeadingRadius: 14,
          bottomTrailingRadius: 4, topTrailingRadius: 14,
          style: .continuous
        )
      )
      .contextMenu {
        CopyButton(text: message.text)
        if let wire = message.wireText {
          // Debug affordance: the exact prompt the model received,
          // skeleton and all.
          Button("Copy full prompt") { chatCopy(wire) }
        }
      }
      // Clip the chrome so markdown/code/selection never paints past the
      // rounded bubble into the window chrome (R1 `)"` fragment class).
      .clipped()
    }
    .frame(
      maxWidth: ChatLayoutPolicy.youBubbleMaxWidth(
        containerWidth: containerWidth,
        mode: mode
      ),
      alignment: .trailing
    )
    .clipped()
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
      .csFocusRing(cornerRadius: 8)
      .help("Selection and app captured with this voice turn")

      if expanded {
        VStack(alignment: .leading, spacing: 5) {
          if let app {
            Text("app · \(app)")
              .font(CSFont.mono(10.5, .medium))
              .foregroundStyle(CSColor.textMuted)
          }
          if let selection {
            // Huge pasted selections (legacy assistive wires) must
            // not expand the You bubble unboundedly — fold them
            // through the same oversized policy as message bodies.
            Group {
              if OversizedBubblePolicy.isOversized(selection) {
                OversizedMessageBody(fullText: selection) { head in
                  Text(head)
                    .font(CSFont.mono(10.5))
                    .foregroundStyle(CSColor.textBodyAlt)
                    .lineSpacing(3)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
              } else {
                Text(selection)
                  .font(CSFont.mono(10.5))
                  .foregroundStyle(CSColor.textBodyAlt)
                  .textSelection(.enabled)
                  .lineSpacing(3)
                  .fixedSize(horizontal: false, vertical: true)
                  .frame(maxWidth: .infinity, alignment: .leading)
              }
            }
            .padding(8)
            .background(CSColor.surfaceRaised(0.05))
            .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
            .clipped()
          }
        }
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .clipped()
  }
}

/// Attachment chip for a sent You turn — mirrors the composer's staged-chip
/// style (icon/thumbnail + mono filename), minus the remove button. Shows a
/// small inline thumbnail when the source image still loads; otherwise falls
/// back to a photo glyph. Loaded once on appear so scrolling doesn't re-decode.
/// Click opens an in-app preview sheet (metadata + zoom + Reveal/Copy path);
/// restored turns with a nil URL surface an honest missing-asset state.
private struct AttachmentChip: View {
  let attachment: MessageAttachment
  @State private var thumbnail: NSImage?
  @State private var showPreview = false

  var body: some View {
    Button {
      showPreview = true
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
    .csFocusRing(cornerRadius: CSRadius.pill)
    .help(
      attachment.url == nil
        ? "Preview attachment (original file may be missing)" : "Preview attachment"
    )
    .onAppear {
      if thumbnail == nil, let url = attachment.url {
        thumbnail = NSImage(contentsOf: url)
      }
    }
    .sheet(isPresented: $showPreview) {
      AttachmentPreviewSheet(attachment: attachment)
    }
  }
}

/// In-app attachment inspector: image zoom when bytes still load, honest
/// fallback when the source path is gone (restored threads), plus Reveal in
/// Finder / Copy path / Open with default app.
struct AttachmentPreviewSheet: View {
  let attachment: MessageAttachment
  var onRemove: (() -> Void)? = nil
  @Environment(\.dismiss) private var dismiss
  @State private var image: NSImage?
  @State private var zoom: CGFloat = 1.0

  private var pathText: String {
    attachment.url?.path ?? "(no source path — restored turn keeps name only)"
  }

  private var fileExists: Bool {
    guard let url = attachment.url else { return false }
    return FileManager.default.fileExists(atPath: url.path)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      HStack(alignment: .firstTextBaseline) {
        VStack(alignment: .leading, spacing: 4) {
          Text(attachment.name)
            .font(CSFont.mono(13, .semibold))
            .foregroundStyle(CSColor.textBodyAlt)
            .textSelection(.enabled)
          Text(attachment.type)
            .font(CSFont.mono(10.5, .medium))
            .foregroundStyle(CSColor.textFaintAlt)
        }
        Spacer(minLength: 8)
        Button("Close") { dismiss() }
          .keyboardShortcut(.cancelAction)
      }

      Group {
        if let image {
          ScrollView([.horizontal, .vertical]) {
            Image(nsImage: image)
              .resizable()
              .aspectRatio(contentMode: .fit)
              .frame(
                width: max(240, image.size.width * zoom),
                height: max(160, image.size.height * zoom)
              )
              .frame(maxWidth: .infinity, maxHeight: .infinity)
          }
          .frame(minHeight: 280, maxHeight: 480)
          .background(CSColor.surfaceRaised(0.04))
          .clipShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))

          HStack(spacing: 10) {
            Text("Zoom")
              .font(CSFont.mono(10.5, .medium))
              .foregroundStyle(CSColor.textFaintAlt)
            Slider(value: $zoom, in: 0.5...3.0, step: 0.1)
            Text(String(format: "%.0f%%", zoom * 100))
              .font(CSFont.mono(10.5, .medium))
              .foregroundStyle(CSColor.textMuted)
              .frame(width: 44, alignment: .trailing)
          }
        } else if attachment.url == nil {
          missingBanner(
            title: "Original file not available",
            detail:
              "This turn was restored from history. Codescribe kept the filename but not the bytes or path on disk."
          )
        } else if !fileExists {
          missingBanner(
            title: "File missing on disk",
            detail: pathText
          )
        } else {
          missingBanner(
            title: "No inline preview",
            detail:
              "This type is not rendered in-app. Use Open to hand it to the system default app."
          )
        }
      }

      VStack(alignment: .leading, spacing: 4) {
        Text("Path")
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textFaintAlt)
        Text(pathText)
          .font(CSFont.mono(10.5))
          .foregroundStyle(CSColor.textBodyAlt)
          .textSelection(.enabled)
          .lineLimit(3)
          .truncationMode(.middle)
      }
      .padding(10)
      .frame(maxWidth: .infinity, alignment: .leading)
      .background(CSColor.surfaceRaised(0.04))
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))

      HStack(spacing: 10) {
        if let onRemove {
          Button("Remove", role: .destructive) {
            onRemove()
            dismiss()
          }
        }

        Button("Copy path") {
          chatCopy(pathText)
        }
        .disabled(attachment.url == nil)

        Button("Reveal in Finder") {
          if let url = attachment.url {
            NSWorkspace.shared.activateFileViewerSelecting([url])
          }
        }
        .disabled(!fileExists)

        Button("Open") {
          if let url = attachment.url {
            NSWorkspace.shared.open(url)
          }
        }
        .disabled(!fileExists)
        .keyboardShortcut(.defaultAction)

        Spacer(minLength: 0)
      }
    }
    .padding(18)
    .frame(minWidth: 520, idealWidth: 640, minHeight: 420)
    .onAppear {
      if image == nil, let url = attachment.url, fileExists {
        image = NSImage(contentsOf: url)
      }
    }
  }

  private func missingBanner(title: String, detail: String) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      Text(title)
        .font(CSFont.mono(12, .semibold))
        .foregroundStyle(CSColor.terracottaLight)
      Text(detail)
        .font(CSFont.mono(11))
        .foregroundStyle(CSColor.textMuted)
        .fixedSize(horizontal: false, vertical: true)
    }
    .padding(14)
    .frame(maxWidth: .infinity, minHeight: 160, alignment: .leading)
    .background(CSColor.surfaceRaised(0.04))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(CSColor.terracotta.opacity(0.22), lineWidth: 1)
    )
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))
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

  func placeSubviews(
    in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout Void
  ) {
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

/// One tool-activity line. Settled lines with summary, call id, or duration
/// become a compact disclosure: tappable row → structured inspect panel
/// (status, duration, call id, result/error, copy technical). Collapsed by
/// default so the list stays scannable.
private struct ToolLineRow: View {
  let line: ToolLine
  @State private var showInspect = false

  private var isRunning: Bool { line.state == .running }
  private var isQuiet: Bool { line.state == .unknown || line.state == .cancelled }
  private var canInspect: Bool { line.hasInspectPayload }
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
    VStack(alignment: .leading, spacing: 4) {
      Button {
        if canInspect { showInspect.toggle() }
      } label: {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
          if isRunning {
            PulseDot()
          }
          (Text(line.verb).foregroundColor(rowColor)
            + Text(" \(line.detail)\(isRunning ? " running..." : "")").foregroundColor(
              isQuiet ? CSColor.textFaintAlt : ChatPalette.toolBody))
            .font(CSFont.mono(11.5, .medium))
            .lineSpacing(4)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
          if let duration = ToolInspectPresentation.durationLabel(ms: line.durationMs), !showInspect
          {
            Text(duration)
              .font(CSFont.mono(10, .medium))
              .foregroundStyle(CSColor.textFaintAlt)
          }
          if canInspect {
            CSIconView(
              icon: showInspect ? .chevronDown : .chevronRight,
              size: 8,
              weight: .semibold,
              color: rowColor.opacity(0.75)
            )
          }
        }
        .contentShape(Rectangle())
      }
      .csFocusRing(cornerRadius: 8)
      .disabled(!canInspect)

      if canInspect, showInspect {
        ToolInspectPanel(line: line)
          .padding(.leading, 10)
      }
    }
  }
}

/// Expanded inspect surface for one tool call (no chain-of-thought — only
/// operator-useful fields already present on the UI line).
private struct ToolInspectPanel: View {
  let line: ToolLine

  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      inspectRow(label: "status", value: ToolInspectPresentation.statusLabel(for: line.state))
      if let duration = ToolInspectPresentation.durationLabel(ms: line.durationMs) {
        inspectRow(label: "duration", value: duration)
      }
      if let callID = line.callID, !callID.isEmpty {
        inspectRow(label: "call id", value: callID)
      }
      if let reason = line.reason, !reason.isEmpty {
        VStack(alignment: .leading, spacing: 2) {
          Text(line.state == .failed ? "error" : "result")
            .font(CSFont.mono(9.5, .semibold))
            .foregroundStyle(CSColor.textFaintAlt)
            .textCase(.uppercase)
          Text(reason)
            .font(CSFont.mono(10.5, .medium))
            .foregroundStyle(line.state == .failed ? CSColor.terracottaLight : CSColor.textBodyAlt)
            .textSelection(.enabled)
            .lineSpacing(2)
            .fixedSize(horizontal: false, vertical: true)
        }
      } else {
        Text("No result summary was stored for this call.")
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textFaintAlt)
      }
      // Honest residual: full request/response bodies and artifact store
      // links need bridge event fields beyond the current ToolLine contract.
      HStack(spacing: 10) {
        CopyMessageButton(text: line.technicalCopyText)
        Text("request/response bodies not on this event")
          .font(CSFont.mono(9.5, .medium))
          .foregroundStyle(CSColor.textFaintAlt)
          .lineLimit(1)
      }
      .padding(.top, 2)
    }
    .padding(.horizontal, 10)
    .padding(.vertical, 8)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: 8, style: .continuous)
        .fill(CSColor.surfaceRaised(0.04))
    )
    .overlay(
      RoundedRectangle(cornerRadius: 8, style: .continuous)
        .strokeBorder(CSColor.hairline(0.06), lineWidth: 1)
    )
  }

  private func inspectRow(label: String, value: String) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: 8) {
      Text(label)
        .font(CSFont.mono(9.5, .semibold))
        .foregroundStyle(CSColor.textFaintAlt)
        .textCase(.uppercase)
        .frame(width: 64, alignment: .leading)
      Text(value)
        .font(CSFont.mono(10.5, .medium))
        .foregroundStyle(CSColor.textBodyAlt)
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
  }
}

private struct ToolTurn: View {
  let message: ChatMessage
  let containerWidth: CGFloat
  let mode: ChatWidthMode
  @State private var expanded = false

  /// Whole-card plain-text export: one technical block per tool line.
  private var copyText: String {
    message.toolLines.map(\.technicalCopyText).joined(separator: "\n---\n")
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
            color: hasRunning
              ? CSColor.amber : hasCancelled ? CSColor.textFaintAlt : CSColor.oliveLight
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
      .clipped()
    }
    .frame(
      maxWidth: ChatLayoutPolicy.leadingColumnMaxWidth(
        containerWidth: containerWidth,
        mode: mode
      ),
      alignment: .leading
    )
    .clipped()
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
      .csFocusRing(cornerRadius: 8)

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
  let containerWidth: CGFloat
  let mode: ChatWidthMode
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
            // A runaway stream keeps only its live tail in the
            // SwiftUI text stack — bounds both the per-delta
            // re-layout and the shared selection overlay.
            if OversizedBubblePolicy.isOversized(message.text) {
              StreamWindowNote(fullText: message.text)
            }
            RawText(
              raw: OversizedBubblePolicy.streamingWindow(message.text),
              showsCaret: true
            )
          } else if !message.text.isEmpty {
            if OversizedBubblePolicy.isOversized(message.text) {
              OversizedMessageBody(fullText: message.text) { head in
                switch message.renderMode {
                case .raw:
                  RawText(raw: head)
                case .rich:
                  MarkdownText(raw: head)
                }
              }
            } else {
              switch message.renderMode {
              case .raw:
                RawText(raw: message.text)
              case .rich:
                MarkdownText(raw: message.text)
              }
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
      .clipShape(
        UnevenRoundedRectangle(
          topLeadingRadius: 14, bottomLeadingRadius: 4,
          bottomTrailingRadius: 14, topTrailingRadius: 14,
          style: .continuous
        )
      )
      .contextMenu { CopyButton(text: message.text) }
      .clipped()
    }
    .frame(
      maxWidth: ChatLayoutPolicy.leadingColumnMaxWidth(
        containerWidth: containerWidth,
        mode: mode
      ),
      alignment: .leading
    )
    .clipped()
  }
}

private struct ReasoningDisclosure: View {
  let text: String
  let isLive: Bool
  @State private var expanded: Bool

  init(text: String, isLive: Bool) {
    self.text = text
    self.isLive = isLive
    // A live summary is status, not optional archaeology: show it as soon
    // as the provider emits it. Settled turns remain compact and reopenable.
    _expanded = State(initialValue: isLive)
  }

  var body: some View {
    DisclosureGroup(isExpanded: $expanded) {
      // Reasoning shares the display budget: long chains fold like any
      // other oversized body instead of feeding the selection overlay.
      Group {
        if OversizedBubblePolicy.isOversized(text) {
          OversizedMessageBody(fullText: text) { head in
            reasoningBody(head)
          }
        } else {
          reasoningBody(text)
        }
      }
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
    .onChange(of: isLive) { _, live in
      if live { expanded = true }
    }
    .background(CSColor.surfaceRaised(0.018))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(CSColor.hairline(0.055), lineWidth: 1)
    )
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))
  }

  private func reasoningBody(_ content: String) -> some View {
    Text(content)
      .font(CSFont.mono(10.5, .medium))
      .foregroundStyle(ChatPalette.thinking.opacity(0.86))
      .textSelection(.enabled)
      .lineSpacing(3)
      .fixedSize(horizontal: false, vertical: true)
      .frame(maxWidth: .infinity, alignment: .leading)
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
    // Selection is per-body (the list-wide overlay was the livelock fuel);
    // raw mode must stay copyable like the markdown render.
    let content = Text(raw)
      .font(CSFont.mono(13 * textScale))
      .foregroundStyle(CSColor.textBodyAlt)
      .lineSpacing(4)
      .fixedSize(horizontal: false, vertical: true)
      .textSelection(.enabled)
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
    .csFocusRing(cornerRadius: 8)
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
    .csFocusRing(cornerRadius: 8)
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
