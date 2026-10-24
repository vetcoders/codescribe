import SwiftUI

/// Maciej (2026-10-16): no outline on the bubble's agent name — hovering
/// brightens the color slightly, and nothing more; the click opens the chat.
private struct BubbleOwnerNameButton: View {
  let name: String
  let palette: OverlayAppearancePalette
  let textScale: CGFloat
  let messageID: String
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Text(verbatim: name)
        .font(.system(size: 10 * textScale, weight: .bold))
    }
    .buttonStyle(.plain)
    .conversationActionHover(
      restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
    )
    .accessibilityIdentifier("overlay-message-owner-\(messageID)")
    .accessibilityLabel(Text(verbatim: name))
    .accessibilityHint(Text("Opens this agent's conversation"))
  }
}

/// Maciej (2026-10-17): held controls hover like the owner name and Copy — muted at rest, brighter on hover.
private struct OverlayHeldControlButton: View {
  let title: LocalizedStringKey
  let hint: String
  let palette: OverlayAppearancePalette
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Text(title)
    }
    .buttonStyle(.plain)
    .conversationActionHover(
      restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
    )
    .accessibilityLabel(Text(title))
    .accessibilityHint(Text(verbatim: hint))
  }
}

enum OverlayConversationPresentation: Equatable, Sendable {
  case standard, intercom
}

/// Paints the passive observer's immutable conversation projection.
struct OverlayConversationView: View {
  @Environment(\.displayScale) private var displayScale
  @Environment(\.csTextScale) private var textScale
  let conversation: OverlayConversation
  let palette: OverlayAppearancePalette
  let topInset: CGFloat
  let bottomInset: CGFloat
  let pendingControls: Set<String>
  let controlErrors: [String: String]
  let onControl: (OverlayConversationMessage, Bool) -> Void
  var focusRevision: UInt64 = 0
  /// An explicit click in another native presentation can request the same
  /// editor without selecting or expanding the main overlay.
  var composerFocusRevision: UInt64 = 0
  var isPresented = true
  /// The agents list covers the conversation. The scroll keeps following it
  /// underneath; only the composer gives the keyboard up.
  var isCovered = false
  @Binding var draft: String
  let sending: Bool
  let sendError: String?
  let onSend: () -> Void
  var microphoneOpen = false
  var microphoneEnabled = false
  var playbackMuted: Bool?
  var playbackEnabled = false
  var onMicrophone: () -> Void = {}
  var onPlayback: () -> Void = {}
  var playbackError: String?
  var agentDescriptor: String? = nil
  /// Only a current owner with a detected Frame pane or terminal tab exposes a
  /// navigation action.
  var onFocusAgentPanel: (() -> Void)? = nil
  /// What that action does, for its accessibility hint.
  var focusAgentPanelHint: String? = nil
  /// Jumps to the sender's own conversation from the agent name in a reply bubble.
  var onOpenOwnerConversation: ((OverlayConversationOwner) -> Void)? = nil
  /// Wide windows dock the roster column, which already names the agent and
  /// carries its microphone and speaker — the identity pill evacuates instead
  /// of repeating it (Maciej, 2026-10-17).
  var hidesNavigation = false
  /// Intercom cards use the same message rows without another nested surface.
  /// The standard presentation remains the main and detached window default.
  var presentation: OverlayConversationPresentation = .standard
  var showsComposer = true
  /// A bounded text preview in a stretched group card; opening the full
  /// conversation restores the complete body without changing its document.
  var messagePreviewLineLimit: Int? = nil
  var onComposerEditorActive: (Bool) -> Void = { _ in }
  var onComposerTypingActivity: () -> Void = {}

  var replySelection: OverlayUserReplyReference? = nil
  var onClearReply: () -> Void = {}
  var draftMicrophoneRecording = false
  var draftMicrophoneBusy = false
  var onToggleDraftMicrophone: (() -> Void)? = nil
  var onStageCaptureAttachments: (([String]) -> Bool)? = nil
  var pendingCaptureAttachments: [String] = []
  var onRemoveCaptureAttachment: ((String) -> Void)? = nil
  var captureAttachmentError: String? = nil
  var onRetryCaptureAttachments: (() -> Void)? = nil
  var onReply: ((OverlayConversationMessage) -> Void)? = nil
  @State private var replyNavigationID: String?

  @State private var scrollFollow = StreamScrollFollowState()
  /// Conversation and presentation the focus handler last observed. Written
  /// after this turn's handlers, so a mini morph is not read as a re-select.
  @State private var scrollFocusConversationID: String?
  @State private var scrollFocusWasPresented = false
  @State private var composerHeight: CGFloat = 0
  @State private var navigationHeight: CGFloat = 32
  @State private var navigationPillHeight: CGFloat = 28
  /// Bumps the composer's focus after a held message returns to the draft.
  @State private var revisionFocus: UInt64 = 0
  @State private var pendingDeliveryControls: Set<String> = []
  @State private var deliveryControlErrors: [String: String] = [:]
  /// First rendered message. Pinned by id so an arriving reply never drops a
  /// row above someone reading; "Show earlier" moves it back a window.
  @State private var windowStartID: String?
  @State private var windowConversationID: String?

  var onDeliveryControl:
    (OverlayDeliveryControl, OverlayConversationMessage, OverlayConversationOwner?, String?)
      async throws -> Void = { control, message, owner, text in
        guard let identity = message.controlIdentity else { throw CocoaError(.fileReadCorruptFile) }
        try await RealAgentBridgeInstaller.controlBusMessage(
          control, identity: message.captureSessionID ?? identity,
          owner: owner, busPath: message.busPath, text: text)
      }

  var orderedMessages: [OverlayConversationMessage] { conversation.messages }
  private var followsLatest: Bool { scrollFollow.followingLive }
  private var contentInset: CGFloat { presentation == .intercom ? 12 : 20 }
  private var visibleComposerHeight: CGFloat { showsComposer ? composerHeight : 0 }
  private var visibleNavigationHeight: CGFloat { hidesNavigation ? 0 : navigationHeight }

  /// The bubble cap follows ⌘+ text scale until the window's own bound wins.
  /// The window bound keeps a margin on the far side so leading and trailing
  /// bubbles stay told apart; at 0.82 that margin was a sixth of the column
  /// and left an empty band beside the docked drawer (Founder, 2026-10-18).
  /// 0.92 of the text column is the same cue at a tenth of the width. The
  /// absolute 660 pt is a reading measure, not a margin, and does not move.
  static func bubbleCap(contentWidth: CGFloat, textScale: CGFloat) -> CGFloat {
    max(0, min(660 * textScale, (contentWidth - 40) * 0.92))
  }

  /// The newest receiving user turn owns the activity cue, independently of ACKs.
  var activeCaptureMessageID: String? {
    guard isPresented else { return nil }
    return orderedMessages.last(where: { $0.kind == .user && $0.captureState == .receiving })?.id
  }

  var body: some View {
    GeometryReader { geometry in
      ScrollViewReader { proxy in
        trackedMessages(
          maxBubbleWidth: presentation == .intercom
            ? max(0, geometry.size.width - 2 * contentInset)
            : Self.bubbleCap(contentWidth: geometry.size.width, textScale: textScale)
        )
        .onChange(of: replyNavigationID) { _, id in
          guard let id else { return }
          proxy.scrollTo(id, anchor: .center)
          replyNavigationID = nil
        }
        // One ScrollView per conversation. `.initialOffset` then starts at the
        // bottom instead of reusing the previous list's offset through the morph.
        // It places a list once, so the list is recreated when the history
        // finishes loading and when a list born empty gets its first rows;
        // otherwise it stayed at the top of everything that came after
        // (Maciej, 2026-10-19). The composer stays put.
        .id([
          conversation.id, conversation.historyLoaded ? "loaded" : "loading",
          orderedMessages.isEmpty ? "empty" : "rows",
        ])

        .padding(.trailing, OverlayResizeHit.scrollbarInset)
        .transaction { transaction in
          // Keep retained message geometry stable during the panel's transition.
          transaction.animation = nil
          transaction.disablesAnimations = true
        }
        .overlay(alignment: .bottom) {
          if showsComposer {
            composer
              .onGeometryChange(for: CGFloat.self) {
                $0.size.height
              } action: {
                composerHeight = $0
              }
          }
        }
        .overlay(alignment: .bottom) {
          ZStack {
            if !followsLatest {
              Button {
                scrollToLatest(proxy)
              } label: {
                Image(systemName: "chevron.down")
                  .font(.system(size: 12, weight: .semibold))
                  .foregroundStyle(palette.primaryText.color)
                  .frame(width: 16, height: 16)
              }
              .modifier(OverlayAgentControlStyle())
              .csFocusOutline()
              .accessibilityLabel("Jump to current")
              .overlayFastHint(
                String(localized: "Jump to the current reply"), palette: palette
              )
              .accessibilityHint(Text("Jump to the current reply"))
              .accessibilityIdentifier("overlay-conversation-jump-to-current")
              .padding(.bottom, visibleComposerHeight + 10)
              .transition(.opacity.combined(with: .move(edge: .bottom)))
            }
          }
          .animation(.easeOut(duration: 0.18), value: followsLatest)
        }
        .overlay(alignment: .top) {
          VStack(spacing: 0) {
            Color.clear.frame(height: topInset)
              .allowsHitTesting(false)
              .accessibilityHidden(true)
            if hidesNavigation {
              // The content margin follows: no phantom gap under the header.
              Color.clear.frame(height: 0)
                .onAppear { navigationHeight = 0 }
            } else {
              navigation
                .padding(.horizontal, 20)
                .padding(.top, 4)
                .padding(.bottom, 8)
                .onGeometryChange(for: CGFloat.self) {
                  $0.size.height
                } action: {
                  navigationHeight = $0
                }
            }
          }
        }
        .onAppear {
          settleMessageWindow(reset: true)
          scrollFocusConversationID = conversation.id
          scrollFocusWasPresented = isPresented
          if #available(macOS 15.0, *) {
            // `.initialOffset` places a new list. scrollTo here races the morph's
            // lazy row remeasurement, the same race the geometry handler refuses.
          } else {
            scrollToLatest(proxy)
          }
        }
        .onChange(of: conversation.id) { _, _ in
          settleMessageWindow(reset: true)
          if #available(macOS 15.0, *) {
            // The new `.id` rebuilds the list at the bottom anchor. scrollTo on
            // this frame races that remeasurement.
            _ = scrollFollow.handle(.jumpToCurrent)
          } else {
            scrollToLatest(proxy)
          }
          rememberScrollFocusAnchor()
        }
        .onChange(of: focusRevision) { _, _ in
          let reselectingOpenConversation =
            scrollFocusConversationID == conversation.id && scrollFocusWasPresented
          if #available(macOS 15.0, *) {
            if reselectingOpenConversation {
              scrollToLatest(proxy)
            } else {
              // Opening or switching during the morph: keep following, no scrollTo.
              _ = scrollFollow.handle(.jumpToCurrent)
            }
          } else {
            scrollToLatest(proxy)
          }
          rememberScrollFocusAnchor()
        }
        .onChange(of: isPresented) { _, presented in
          if #available(macOS 15.0, *) {
            // The size-change anchor keeps a following list at the bottom through
            // the morph; a list the user scrolled up stays where it was.
          } else if presented, followsLatest {
            scrollToLatest(proxy)
          }
          rememberScrollFocusAnchor()
        }
        .onChange(of: geometry.size) { _, _ in
          // Modern ScrollView owns size-change anchoring. A second scrollTo
          // on every resize frame races its lazy row remeasurement.
          if #available(macOS 15.0, *) {
            return
          } else if isPresented, followsLatest {
            scrollToLatest(proxy)
          }
        }
        .onChange(of: orderedMessages.last) { _, _ in
          settleMessageWindow()
          if #available(macOS 15.0, *) {
            // Size-change anchoring keeps the bottom while the morph grows the list.
            // scrollTo on the new last row races the lazy remeasurement.
            _ = scrollFollow.handle(.contentChanged)
          } else if scrollFollow.handle(.contentChanged) == .scrollToLiveEdge {
            scrollToLatest(proxy)
          }
        }
      }
      .foregroundStyle(palette.primaryText.color)
    }
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-conversation-body")
  }

  private var composer: some View {
    VStack(spacing: 8) {
      if conversation.owner != nil || conversation.channel == "0" {
        OverlayConversationComposer(
          palette: palette, draft: $draft,
          sending: sending || !pendingDeliveryControls.isEmpty, onSubmit: submit,
          onEditorActive: onComposerEditorActive, onTypingActivity: onComposerTypingActivity,
          focusRevision: revisionFocus &+ composerFocusRevision,
          isCovered: isCovered || !isPresented,
          placeholder: composerPlaceholder,
          replySelection: replySelection, onClearReply: onClearReply,
          draftMicrophoneRecording: draftMicrophoneRecording,
          draftMicrophoneBusy: draftMicrophoneBusy,
          liveSpeechActive: microphoneOpen,
          onToggleDraftMicrophone: onToggleDraftMicrophone,
          onStageCaptureAttachments: onStageCaptureAttachments,
          pendingCaptureAttachments: pendingCaptureAttachments,
          onRemoveCaptureAttachment: onRemoveCaptureAttachment,
          captureAttachmentError: captureAttachmentError,
          onRetryCaptureAttachments: onRetryCaptureAttachments,
          onNavigateReply: { navigateToReply(replySelection?.messageID) })
        if let sendError { composerError(sendError) }
        if let playbackError {
          composerError(playbackError)
            .accessibilityIdentifier("overlay-conversation-playback-error")
        }
      }
    }
    .padding(.horizontal, contentInset)
    .padding(.top, 6)
    .padding(.bottom, bottomInset)
    .frame(maxWidth: .infinity)
  }

  private var composerPlaceholder: String {
    conversation.channel == "0"
      ? String(localized: "Send to everyone") : String(localized: "Message the Agent")
  }

  private func composerError(_ text: String) -> some View {
    Text(verbatim: text)
      .font(.system(size: 10 * textScale)).foregroundStyle(palette.errorStatus.color)
  }

  private var navigation: some View {
    navigationGlassContainer(
      HStack(spacing: 0) {
        Spacer(minLength: 0)
        HStack(spacing: 6) {
          if conversation.owner != nil || conversation.channel == "0" {
            OverlayAgentAudioControls(
              open: microphoneOpen, muted: playbackMuted,
              microphoneEnabled: microphoneEnabled, playbackEnabled: playbackEnabled,
              palette: palette, onMicrophone: onMicrophone, onPlayback: onPlayback,
              broadcast: conversation.channel == "0", speaking: conversation.replySpeaking,
              diameter: navigationPillHeight
            )
            .fixedSize()
          }
          conversationNamePill
            .onGeometryChange(for: CGFloat.self) {
              $0.size.height
            } action: {
              navigationPillHeight = max(28, $0)
            }
        }
        // The gap between the controls and the name is not text either.
        .background { OverlayMenuCursorRegion() }
      }
    )
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-conversation-navigation")
  }

  @ViewBuilder
  private var conversationNamePill: some View {
    if let onFocusAgentPanel {
      focusedNamePill(onFocusAgentPanel)
    } else {
      namePillLabel
        .overlayFastHint(providerIdentity, palette: palette, enabled: !providerIdentity.isEmpty)
        .accessibilityAddTraits(.isHeader)
        .accessibilityIdentifier("overlay-conversation-name")
        .modifier(OverlayNamePillGlass(interactive: false))
    }
  }

  private var providerIdentity: String {
    conversation.owner.map { "\($0.provider) · \($0.providerSessionID)" } ?? ""
  }

  /// Interactive glass is the system's own hover and press feedback.
  @ViewBuilder
  private func focusedNamePill(_ action: @escaping () -> Void) -> some View {
    let focusHint = focusAgentPanelHint ?? OverlayAgentPanelFocusTarget.frame.help
    let pill =
      Button(action: action) { namePillLabel }
      .buttonStyle(.plain)
      .csFocusOutline()
      .overlayFastHint(focusHint, palette: palette)
      .accessibilityHint(Text(verbatim: focusHint))
      .accessibilityIdentifier("overlay-conversation-name")
      .modifier(OverlayNamePillGlass(interactive: true))
      .background { OverlayMenuCursorRegion() }
    if providerIdentity.isEmpty {
      pill
    } else {
      pill.accessibilityValue(Text(verbatim: providerIdentity))
    }
  }

  private var namePillLabel: some View {
    VStack(alignment: .leading, spacing: 1) {
      if conversation.channel == "0" {
        Text("0 · All")
      } else {
        Text(verbatim: conversation.displayTitle)

        if let descriptor = agentDescriptor ?? conversation.owner?.provider, !descriptor.isEmpty {
          Text(verbatim: descriptor)
            .font(.system(size: 11 * textScale, weight: .regular))
            .foregroundStyle(palette.mutedText.color)
            .lineLimit(1)
            .truncationMode(.middle)
            .accessibilityIdentifier("overlay-conversation-agent-descriptor")
        }
      }
    }
    .font(.system(size: 13 * textScale, weight: .bold))
    .foregroundStyle(palette.primaryText.color)
    .lineLimit(1)
    .truncationMode(.middle)
    .padding(.horizontal, 10)
    .padding(.vertical, 5)
    // Mockup: 224 × scale.
    .frame(
      width: conversation.channel == "0"
        ? nil : OverlayAgentDrawerLayout.pillWidth(textScale: textScale),
      alignment: .leading
    )
    .frame(minHeight: 28)
    .contentShape(Capsule())
    .accessibilityElement(children: .combine)
  }

  @ViewBuilder
  private func navigationGlassContainer<Content: View>(_ content: Content) -> some View {
    if #available(macOS 26.0, *) {
      GlassEffectContainer(spacing: 6) { content }
    } else {
      content
    }
  }

  @ViewBuilder
  private func trackedMessages(maxBubbleWidth: CGFloat) -> some View {
    if #available(macOS 15.0, *) {
      messageList(maxBubbleWidth: maxBubbleWidth)
        .defaultScrollAnchor(.bottom, for: .initialOffset)
        .defaultScrollAnchor(followsLatest ? .bottom : nil, for: .sizeChanges)
        .defaultScrollAnchor(.top, for: .alignment)
    } else {
      messageList(maxBubbleWidth: maxBubbleWidth)
    }
  }

  /// Rows per window of the rendered list. Every rendered row is built and
  /// measured: a LazyVStack sized the document from estimates, so a long thread
  /// opened past its last bubble, and rows realized while scrolling changed the
  /// document by thousands of points, which threw the reader off the bottom
  /// and left an empty viewport (Maciej, 2026-10-19). The Agent window bounds
  /// its list the same way; older rows wait behind "Show earlier".
  static let messageWindow = 40

  private var windowStart: Int {
    guard windowConversationID == conversation.id, let windowStartID else {
      return max(0, orderedMessages.count - Self.messageWindow)
    }
    // Only the oldest-row trim removes a pinned row, so its window starts at
    // the top that is left rather than jumping to the newest rows.
    return orderedMessages.firstIndex(where: { $0.id == windowStartID }) ?? 0
  }

  /// Pins the window to its current first row, or starts a fresh window at the
  /// newest rows. A list following the live edge drops rows beyond two windows;
  /// the bottom anchor keeps that removal out of sight.
  private func settleMessageWindow(reset: Bool = false) {
    let count = orderedMessages.count
    guard count > 0 else {
      windowStartID = nil
      return
    }
    var start = reset ? max(0, count - Self.messageWindow) : windowStart
    if followsLatest, count - start > 2 * Self.messageWindow {
      start = count - Self.messageWindow
    }
    pinMessageWindow(at: start)
  }

  private func showEarlierMessages() {
    pinMessageWindow(at: max(0, windowStart - Self.messageWindow))
  }

  private func pinMessageWindow(at start: Int) {
    guard orderedMessages.indices.contains(start) else { return }
    let id = orderedMessages[start].id
    if windowStartID != id { windowStartID = id }
    if windowConversationID != conversation.id { windowConversationID = conversation.id }
  }

  private func navigateToReply(_ id: String?) {
    guard let id, let index = orderedMessages.firstIndex(where: { $0.id == id }) else { return }
    pinMessageWindow(at: index)
    replyNavigationID = id
  }

  @ViewBuilder
  private func messageList(maxBubbleWidth: CGFloat) -> some View {
    let start = windowStart
    let list = ScrollView {
      VStack(alignment: .leading, spacing: presentation == .intercom ? 6 : 12) {
        if !conversation.historyLoaded {
          ProgressView()
            .controlSize(.small)
            .accessibilityIdentifier("overlay-conversation-history-loading")
        } else if orderedMessages.isEmpty {
          Text("No conversation messages yet")
            .font(.system(size: 13 * textScale))
            .foregroundStyle(palette.mutedText.color)
        }
        if start > 0 {
          ShowEarlierButton(hiddenCount: start, window: Self.messageWindow) {
            showEarlierMessages()
          }
          .frame(maxWidth: .infinity)
          .accessibilityIdentifier("overlay-conversation-show-earlier")
        }
        ForEach(orderedMessages[start...]) { message in
          messageRow(message, maxBubbleWidth: maxBubbleWidth)
            .id(message.id)
        }
        Color.clear.frame(height: 1).id("conversation-bottom")
      }
      .padding(.horizontal, contentInset)
      .padding(.vertical, 8)
      .frame(maxWidth: .infinity, alignment: .leading)
      .background {
        ChatLiveScrollObserver { event in
          guard isPresented else { return }
          _ = scrollFollow.handle(event)
        }
      }
      .csAutohidingScrollers()
    }
    .contentMargins(
      .bottom,
      showsComposer
        ? (visibleComposerHeight > 0 ? visibleComposerHeight : bottomInset + 54) : bottomInset)
    if #available(macOS 26.0, *) {
      // The header's glass alone is not a shade: bubbles stayed readable right
      // behind the mark (Founder, 2026-10-18). Apple pairs a glass bar with the
      // scroll edge effect, so the pinned header becomes a safe-area bar — an
      // empty one, since the window's own header paints it — and the system's
      // soft effect fades whatever runs under it. The bar replaces the top
      // content margin; both named the same height.
      list
        .safeAreaBar(edge: .top, spacing: 0) {
          Color.clear
            .frame(height: topInset + visibleNavigationHeight)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
        .scrollEdgeEffectStyle(.soft, for: .top)
        // The composer's glass sits on the floor with nothing pinned behind it.
        .scrollEdgeEffectHidden(true, for: .bottom)
    } else {
      list.contentMargins(.top, topInset + visibleNavigationHeight)
    }
  }

  /// How far a borderless-button menu sets its label in from its own leading
  /// edge (measured through the accessibility frames, 2026-10-18).
  static let receiptMenuLabelInset: CGFloat = 2

  static func receiptStatusText(for recipient: OverlayConversationRecipient) -> String {
    if recipient.acknowledged { return String(localized: "Read") }
    if recipient.accepted { return String(localized: "Queue accepted") }
    if recipient.queued { return String(localized: "Queued") }
    if recipient.broadcastSkipped == true { return skippedBroadcastText }
    return String(localized: "Addressed")
  }

  /// The follower's receipt for a broadcast it skipped: the agent was off
  /// channel 0 when this delivery reached it.
  static var skippedBroadcastText: String {
    String(
      localized: "Skipped: off channel 0",
      comment: "Agent row receipt: a broadcast skipped because the agent is off channel 0")
  }

  private func submit() {
    _ = scrollFollow.handle(.jumpToCurrent)
    onSend()
  }

  /// Pulls a held message off the clock, then returns its text to the ordinary
  /// composer. The draft changes only after withdraw returns; a failure leaves
  /// the typing where it was and names the error on the bubble.
  private func edit(_ message: OverlayConversationMessage) {
    // The painted countdown is a poll-time projection; a tap after the deadline
    // must not pull back text the follower has already handed to the agent.
    guard message.holdingSeconds(at: Date()) != nil else { return }
    let returning = message.text
    performDeliveryControl(.withdraw, message: message) {
      draft = draft.isEmpty ? returning : draft + "\n\n" + returning
      revisionFocus &+= 1
      onComposerTypingActivity()
    }
  }

  private func performDeliveryControl(
    _ control: OverlayDeliveryControl, message: OverlayConversationMessage,
    then succeeded: @escaping @MainActor () -> Void = {}
  ) {
    guard !pendingDeliveryControls.contains(message.id) else { return }
    // A broadcast mirror is still one broadcast, never a newly private edit.
    let owner = message.sourceChannel == "0" ? nil : conversation.owner
    guard owner != nil || message.sourceChannel == "0" else { return }
    pendingDeliveryControls.insert(message.id)
    deliveryControlErrors.removeValue(forKey: message.id)
    Task { @MainActor in
      defer { pendingDeliveryControls.remove(message.id) }
      do {
        try await onDeliveryControl(control, message, owner, nil)
        succeeded()
      } catch {
        deliveryControlErrors[message.id] = error.userFacingMessage
      }
    }
  }

  /// Records the anchor after this turn's handlers have read the previous one.
  /// A synchronous write would make a mini morph look like a re-select and call
  /// `scrollTo`, which races lazy row remeasurement (see the geometry handler).
  private func rememberScrollFocusAnchor() {
    let id = conversation.id
    let presented = isPresented
    Task { @MainActor in
      scrollFocusConversationID = id
      scrollFocusWasPresented = presented
    }
  }

  private func scrollToLatest(_ proxy: ScrollViewProxy) {
    _ = scrollFollow.handle(.jumpToCurrent)
    guard isPresented else { return }
    var transaction = Transaction(animation: nil)
    transaction.disablesAnimations = true
    withTransaction(transaction) {
      proxy.scrollTo(orderedMessages.last?.id ?? "conversation-bottom", anchor: .bottom)
    }
  }

  @ViewBuilder
  private func messageReceipts(_ message: OverlayConversationMessage) -> some View {
    if message.holdSecondsRemaining != nil {
      Text("Held")
        .font(.system(size: 10 * textScale))
        .foregroundStyle(palette.mutedText.color)
    } else if message.showsDeliveryReceipts && !message.recipients.isEmpty {
      if message.recipients.count > 1 {
        let readCount = message.recipients.filter { $0.acknowledged }.count
        Menu {
          ForEach(message.recipients, id: \.owner.id) { recipient in
            Text(verbatim: "\(recipient.owner.name) · \(Self.receiptStatusText(for: recipient))")
          }
        } label: {
          Text("Read \(readCount)/\(message.recipients.count)")
            .font(.system(size: 10 * textScale))
            .lineLimit(1)
        }
        .menuStyle(.borderlessButton)
        .conversationActionHover(
          restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
        )
        .fixedSize()
        // A borderless-button menu insets its label, so the expandable receipt
        // started right of a plain one and the bubble footers no longer lined
        // up (Founder, 2026-10-18). A plain-styled menu has no inset but leaves
        // the accessibility tree as an unlabelled unknown element, so the
        // native style stays and the frame steps back by its inset instead.
        .padding(.leading, -Self.receiptMenuLabelInset)
        .accessibilityHint(Text("Show details"))
        .accessibilityIdentifier("overlay-message-receipts-\(message.id)")
      } else {
        ForEach(message.recipients, id: \.owner.id) { recipient in
          Text(Self.receiptStatusText(for: recipient))
            .font(.system(size: 10 * textScale))
            .foregroundStyle(palette.mutedText.color)
        }
      }
    }
  }

  /// Sender and audience from this delivery, including a message that is only
  /// attachments. A speaker who is not the open conversation stays a button;
  /// the arrow and the audience are text, so the hit target is only the name.
  /// The open conversation does not choose the names.
  @ViewBuilder
  private func messageSpeaker(_ message: OverlayConversationMessage) -> some View {
    let route = OverlayChannelStatusView.messageRoute(of: message)
    let font = Font.system(size: 10 * textScale, weight: .bold)
    if let owner = message.owner, let onOpenOwnerConversation,
      owner.id != conversation.owner?.id, route.speaker == owner.name
    {
      HStack(spacing: 0) {
        BubbleOwnerNameButton(
          name: owner.name, palette: palette, textScale: textScale, messageID: message.id
        ) { onOpenOwnerConversation(owner) }
        if let audience = route.audience, !audience.isEmpty {
          Text(verbatim: " → \(audience)").font(font)
        }
      }
    } else {
      let fallback = message.owner?.name ?? conversation.name
      Text(verbatim: route.label.isEmpty ? fallback : route.label).font(font)
    }
  }

  private func messageRow(_ message: OverlayConversationMessage, maxBubbleWidth: CGFloat)
    -> some View
  {
    HStack(alignment: .top, spacing: 0) {
      if presentation == .standard && message.kind == .user { Spacer(minLength: 0) }
      messageBubble(message)
        .frame(
          maxWidth: maxBubbleWidth,
          alignment: presentation == .standard && message.kind == .user ? .trailing : .leading)
      if presentation == .standard && message.kind == .reply { Spacer(minLength: 0) }
    }
    .frame(maxWidth: .infinity)
    .id(message.id)
  }

  func messageBubble(_ message: OverlayConversationMessage) -> some View {
    VStack(alignment: .leading, spacing: 5) {
      HStack(spacing: 0) {
        messageSpeaker(message)
        // Founder, 2026-10-18: whether a reply was asked for is routing truth,
        // not a badge — `unsolicited` keeps steering the channels, unlabelled.
        Spacer(minLength: 8)
        MessageTimestampLabel(date: MessageTimestamp.date(fromISO8601: message.emittedAt))
          .font(.system(size: 10 * textScale))
          .accessibilityIdentifier("overlay-message-time-\(message.id)")
      }
      .foregroundStyle(palette.mutedText.color)
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier("overlay-message-route-\(message.id)")
      if let parent = message.userReplyTo {
        Button {
          navigateToReply(parent.messageID)
        } label: {
          VStack(alignment: .leading, spacing: 2) {
            Text(verbatim: parent.author).fontWeight(.semibold)
            Text(verbatim: parent.text).lineLimit(2)
          }
        }
        .buttonStyle(.plain)
        .font(.system(size: 10 * textScale))
        .conversationActionHover(
          restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
        )
        .accessibilityHint(Text("Show the message this replies to"))
        .accessibilityIdentifier("overlay-message-parent-\(message.id)")
      }
      if let question = addressedQuestion(for: message) {
        Text("In reply to: \(String(question.text.prefix(100)))")
          .font(.system(size: 10 * textScale))
          .foregroundStyle(palette.mutedText.color)
          .lineLimit(2)
      }
      Group {
        if let messagePreviewLineLimit {
          Text(verbatim: message.bodyText)
            .font(.system(size: 13 * textScale))
            .foregroundStyle(palette.primaryText.color)
            .lineLimit(messagePreviewLineLimit)
            .textSelection(.enabled)
        } else if OversizedBubblePolicy.isOversized(message.bodyText) {
          OversizedMessageBody(fullText: message.bodyText) { head in
            MarkdownText(
              raw: head, size: 13, bodyColor: palette.primaryText.color,
              showsUnderlineCaret: message.id == activeCaptureMessageID)
          }
        } else {
          MarkdownText(
            raw: message.bodyText, size: 13, bodyColor: palette.primaryText.color,
            showsUnderlineCaret: message.id == activeCaptureMessageID)
        }
      }
      .fixedSize(horizontal: false, vertical: true)
      .background(OverlaySelectableTextRegion())
      .accessibilityIdentifier("overlay-conversation-text-\(message.id)")
      if let attachments = message.attachments, !attachments.isEmpty {
        WrapLayout(spacing: 6) {
          ForEach(attachments, id: \.self) { pointer in
            HStack(spacing: 4) {
              AttachmentChip(
                attachment: MessageAttachment(
                  name: ConversationAttachmentLabel.display(
                    fileName: pointer.name, bytes: pointer.bytes),
                  url: URL(fileURLWithPath: pointer.path),
                  type: pointer.mediaType.isEmpty ? nil : pointer.mediaType))
              if message.captureState != .receiving, let time = pointer.attachmentTimeLabel {
                Text(verbatim: time)
                  .font(.system(size: 10 * textScale))
                  .monospacedDigit()
                  .foregroundStyle(palette.mutedText.color)
                  .accessibilityIdentifier("overlay-attachment-time-\(message.id)-\(pointer.path)")
              }
            }
          }
        }
        .accessibilityIdentifier("overlay-conversation-attachments-\(message.id)")
      }
      if let remaining = message.holdSecondsRemaining {
        heldMessageControls(message, seconds: remaining)
      }
      HStack {
        messageFooter(message)
        if !conversation.isHumanInbox, let onReply,
          message.captureState != .receiving, message.withdrawn != true
        {
          Spacer(minLength: 8)
          Button("Reply", systemImage: "arrowshape.turn.up.left") {
            onReply(message)
            revisionFocus &+= 1
          }
          .buttonStyle(.plain)
          .font(.system(size: 10 * textScale, weight: .semibold))
          .conversationActionHover(
            restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
          )
          .accessibilityHint(Text("Reply to this message"))
          .accessibilityIdentifier("overlay-message-reply-\(message.id)")
        }
      }
      .background { OverlayMenuCursorRegion() }
    }
    .padding(presentation == .intercom ? 0 : 12)
    .padding(.vertical, presentation == .intercom ? 6 : 0)
    .background(
      palette.primaryText.color.opacity(presentation == .intercom ? 0 : 0.06),
      in: RoundedRectangle(cornerRadius: 16, style: .continuous)
    )
    .overlay {
      RoundedRectangle(cornerRadius: 16, style: .continuous)
        .strokeBorder(
          presentation == .intercom
            ? Color.clear
            : message.kind == .user ? CSColor.terracotta.opacity(0.65) : palette.border.color,
          lineWidth: 1 / max(displayScale, 1)
        )
        .allowsHitTesting(false)
    }
  }

  private func heldMessageControls(_ message: OverlayConversationMessage, seconds: Int)
    -> some View
  {
    VStack(alignment: .leading, spacing: 5) {
      Text(verbatim: HeldDeliveryCopy.countdown(seconds: seconds))
        .monospacedDigit()
        .accessibilityIdentifier("overlay-message-hold-countdown-\(message.id)")
      HStack(spacing: 12) {
        // One control takes the message back: it leaves the clock and its text
        // returns to the composer, where deleting it is the plain withdraw.
        OverlayHeldControlButton(
          title: "Cancel/Edit", hint: String(localized: "Cancel/Edit"), palette: palette
        ) { edit(message) }
        .accessibilityIdentifier("overlay-message-cancel-edit-\(message.id)")
        OverlayHeldControlButton(
          title: "Send now", hint: String(localized: "Send now"), palette: palette
        ) {
          performDeliveryControl(.release, message: message)
        }
        .accessibilityIdentifier("overlay-message-release-\(message.id)")
      }
      // Buttons, not text: the bubble's selectable body must not lend them its
      // I-beam, and the gaps between them are not text either.
      .background { OverlayMenuCursorRegion() }
      .disabled(pendingDeliveryControls.contains(message.id) || message.controlIdentity == nil)
      if let error = deliveryControlErrors[message.id] {
        Text(verbatim: error)
          .foregroundStyle(palette.errorStatus.color)
          .accessibilityIdentifier("overlay-message-delivery-error-\(message.id)")
      }
    }
    .font(.system(size: 11 * textScale))
    .foregroundStyle(palette.mutedText.color)
  }

  /// Shared production footer, also mounted by the native compactness test.
  func messageFooter(_ message: OverlayConversationMessage) -> some View {
    OverlayConversationMessageFooter(
      message: message, palette: palette,
      pending: pendingControls.contains(message.id), error: controlErrors[message.id],
      onControl: { stop in onControl(message, stop) },
      receipts: messageReceipts(message))
  }

  private func addressedQuestion(
    for reply: OverlayConversationMessage
  ) -> OverlayConversationMessage? {
    guard reply.kind == .reply, let deliveryID = reply.replyTo, let owner = reply.owner
    else { return nil }
    let matches = conversation.messages.filter { message in
      message.kind == .user
        && message.recipients.contains {
          $0.deliveryID == deliveryID && $0.owner.id == owner.id
        }
    }
    if let occurrenceID = reply.replyToOccurrenceID {
      return matches.first { $0.id == occurrenceID }
    }
    return matches.count == 1 ? matches[0] : nil
  }
}

/// The name pill's capsule of glass; interactive when the pill is a button.
struct OverlayNamePillGlass: ViewModifier {
  let interactive: Bool

  @ViewBuilder
  func body(content: Content) -> some View {
    if #available(macOS 26.0, *) {
      content.glassEffect(interactive ? .regular.interactive() : .regular, in: Capsule())
    } else {
      content.background(.regularMaterial, in: Capsule())
    }
  }
}

/// Playback and copy share a single metadata row. Only a diagnostic adds a line.
private struct OverlayConversationMessageFooter<Receipts: View>: View {
  @Environment(\.csTextScale) private var textScale
  let message: OverlayConversationMessage
  let palette: OverlayAppearancePalette
  let pending: Bool
  let error: String?
  let onControl: (Bool) -> Void
  let receipts: Receipts

  private var supportsPlayback: Bool { message.kind == .reply && message.supportsSpeechPlayback }
  private var active: Bool {
    message.playback.map { ["waiting", "playing"].contains($0.state) } ?? false
  }

  /// The play control's hint follows the same states that enable or refuse it.
  private var playHint: String {
    if active { return String(localized: "Stop") }
    if pending { return String(localized: "Requesting playback") }
    return String(localized: "Play")
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 5) {
      HStack(spacing: 4) {
        if supportsPlayback {
          Button {
            onControl(active)
          } label: {
            Image(systemName: active ? "stop.fill" : "play.fill")
              .font(.system(size: 10 * textScale))
              .frame(width: 24, alignment: .trailing)
              .contentShape(Rectangle())
          }
          .buttonStyle(.plain)
          .conversationActionHover(
            restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
          )
          .disabled(!active && pending)
          .accessibilityLabel(Text(verbatim: playHint))
          .accessibilityHint(Text(verbatim: playHint))
          .accessibilityIdentifier("overlay-reply-\(active ? "stop" : "play")-\(message.id)")
          if let playback = message.playback {
            let label = ReplyPlaybackCopy.label(state: playback.state, reason: playback.reason)
            Text(verbatim: label)
              .lineLimit(1)
              .accessibilityHint(Text(verbatim: label))
              .accessibilityIdentifier("overlay-playback-status-\(message.id)")
          } else if pending {
            Text("Requesting playback").lineLimit(1)
          }
        }
        receipts
        Spacer(minLength: 8)
        CopyMessageButton(
          text: message.text,
          labelFont: .system(size: 10 * textScale, weight: .semibold),
          restingColor: palette.mutedText.color,
          palette: palette
        )
        .fixedSize()
        .accessibilityIdentifier("overlay-message-copy-\(message.id)")
      }
      // Play, receipts and Copy are controls: the arrow, never the I-beam.
      .background { OverlayMenuCursorRegion() }
      .font(.system(size: 10 * textScale))
      .foregroundStyle(palette.mutedText.color)
      // Founder, 2026-10-15: a red warning under a reply he had stopped
      // himself was "nie na miejscu". The status line names the outcome; this
      // line is only for a control that could not run, in the muted voice.
      if supportsPlayback, let error {
        Text(verbatim: error)
          .font(.system(size: 10 * textScale)).foregroundStyle(palette.mutedText.color)
      }
    }
  }
}

/// One line for a reply's playback: the bus publishes a state and a reason
/// code, and Swift owns the sentence. A stop the Founder caused (Stop, or a
/// take that began) reads as a stop, never as a failure or a raw code.
enum ReplyPlaybackCopy {
  static func label(state: String, reason: String?) -> String {
    switch reason {
    case "muted": return String(localized: "Automatic playback muted")
    case "stopped": return String(localized: "Speech stopped")
    case "take_started":
      return String(localized: "Speech stopped when a take began")
    case "take_live":
      return String(localized: "Not played while a take was live")
    case "playback_busy":
      return String(localized: "Not played while another reply was speaking")
    case "credential_missing":
      return String(localized: "Speech failed: no voice credential")
    case "credential_rejected":
      return String(localized: "Speech failed: the voice credential was rejected")
    case "quota_exhausted":
      return String(localized: "Speech failed: the voice quota is used up")
    case "network":
      return String(localized: "Speech failed: no connection to the voice service")
    default: break
    }
    return switch state {
    case "waiting": String(localized: "Waiting for speech")
    case "playing": String(localized: "Speaking")
    case "spoken": String(localized: "Spoken")
    case "failed": String(localized: "Speech failed")
    case "refused": String(localized: "Speech refused")
    case "stopped": String(localized: "Speech stopped")
    default: String(localized: "Reply text retained")
    }
  }
}

enum HeldDeliveryCopy {
  static func countdown(seconds: Int, bundle: Bundle = .main) -> String {
    let format = bundle.localizedString(forKey: "Sending in %lld s", value: nil, table: nil)
    return String(format: format, Int64(max(0, seconds)))
  }
}
