import SwiftUI

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
  var followsLiveChannel = false
  var isPresented = true
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
  var onComposerEditorActive: (Bool) -> Void = { _ in }
  var onComposerTypingActivity: () -> Void = {}

  @State private var scrollFollow = StreamScrollFollowState()
  @State private var composerHeight: CGFloat = 0
  @State private var navigationHeight: CGFloat = 32

  var orderedMessages: [OverlayConversationMessage] { conversation.messages }
  private var followsLatest: Bool { scrollFollow.followingLive }

  var body: some View {
    GeometryReader { geometry in
      ScrollViewReader { proxy in
        trackedMessages(maxBubbleWidth: max(0, min(660, (geometry.size.width - 40) * 0.82)))
          .padding(.trailing, OverlayResizeHit.scrollbarInset)
          .transaction { transaction in
            // Keep retained message geometry stable during the panel's transition.
            transaction.animation = nil
            transaction.disablesAnimations = true
          }
          .overlay(alignment: .top) {
            VStack(spacing: 0) {
              Color.clear.frame(height: topInset)
                .allowsHitTesting(false)
                .accessibilityHidden(true)
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
          .overlay(alignment: .bottom) {
            composer
              .onGeometryChange(for: CGFloat.self) {
                $0.size.height
              } action: {
                composerHeight = $0
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
                .help("Jump to the current reply")
                .accessibilityIdentifier("overlay-conversation-jump-to-current")
                .padding(.bottom, composerHeight + 10)
                .transition(.opacity.combined(with: .move(edge: .bottom)))
              }
            }
            .animation(.easeOut(duration: 0.18), value: followsLatest)
          }
          .onAppear { scrollToLatest(proxy) }
          .onChange(of: conversation.id) { _, _ in scrollToLatest(proxy) }
          .onChange(of: focusRevision) { _, _ in scrollToLatest(proxy) }
          .onChange(of: isPresented) { _, presented in
            if presented, followsLatest { scrollToLatest(proxy) }
          }
          .onChange(of: geometry.size) { _, _ in
            if isPresented, followsLatest { scrollToLatest(proxy) }
          }
          .onChange(of: orderedMessages.last) { previous, latest in
            if previous?.id != latest?.id || followsLatest || followsLiveChannel {
              scrollToLatest(proxy)
            }
          }
      }
      .foregroundStyle(palette.primaryText.color)
    }
    .accessibilityIdentifier("overlay-conversation-body")
  }

  private var composer: some View {
    VStack(spacing: 8) {
      if conversation.owner != nil || conversation.channel == "0" {
        OverlayConversationComposer(
          palette: palette, draft: $draft, sending: sending, onSubmit: submit,
          onEditorActive: onComposerEditorActive, onTypingActivity: onComposerTypingActivity)
        if let sendError {
          Text(verbatim: sendError)
            .font(.system(size: 10 * textScale)).foregroundStyle(palette.errorStatus.color)
        }
        if let playbackError {
          Text(verbatim: playbackError)
            .font(.system(size: 10 * textScale)).foregroundStyle(palette.errorStatus.color)
            .accessibilityIdentifier("overlay-conversation-playback-error")
        }
      }
    }
    .padding(.horizontal, 20)
    .padding(.top, 6)
    .padding(.bottom, bottomInset)
    .frame(maxWidth: .infinity)
  }

  private var navigation: some View {
    navigationGlassContainer(
      HStack(spacing: 6) {
        Spacer(minLength: 0)
        if conversation.owner != nil {
          OverlayAgentAudioControls(
            open: microphoneOpen, muted: playbackMuted,
            microphoneEnabled: microphoneEnabled, playbackEnabled: playbackEnabled,
            palette: palette, onMicrophone: onMicrophone, onPlayback: onPlayback
          )
          .fixedSize()
        }
        conversationNamePill
      }
    )
    .accessibilityIdentifier("overlay-conversation-navigation")
  }

  @ViewBuilder
  private var conversationNamePill: some View {
    let label = VStack(alignment: .leading, spacing: 1) {
      if conversation.channel == "0" {
        Text("0 · All")
      } else {
        Text(
          verbatim: conversation.channel.isEmpty
            ? conversation.name : "\(conversation.channel) · \(conversation.name)"
        )
        .help(
          Text(
            verbatim: conversation.owner.map { "\($0.provider) · \($0.providerSessionID)" }
              ?? ""))
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
    .frame(minHeight: 28)
    .accessibilityElement(children: .combine)
    .accessibilityAddTraits(.isHeader)
    .accessibilityIdentifier("overlay-conversation-name")

    if #available(macOS 26.0, *) {
      label.glassEffect(.regular, in: Capsule())
    } else {
      label.background(.regularMaterial, in: Capsule())
    }
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

  @ViewBuilder
  private func messageList(maxBubbleWidth: CGFloat) -> some View {
    let list = ScrollView {
      LazyVStack(alignment: .leading, spacing: 12) {
        if !conversation.historyLoaded {
          ProgressView()
            .controlSize(.small)
            .accessibilityIdentifier("overlay-conversation-history-loading")
        } else if orderedMessages.isEmpty {
          Text("No conversation messages yet")
            .font(.system(size: 13 * textScale))
            .foregroundStyle(palette.mutedText.color)
        }
        ForEach(orderedMessages) { message in
          messageRow(message, maxBubbleWidth: maxBubbleWidth)
            .id(message.id)
        }
        Color.clear.frame(height: 1).id("conversation-bottom")
      }
      .padding(.horizontal, 20)
      .padding(.vertical, 8)
      .frame(maxWidth: .infinity, alignment: .leading)
      .background {
        ChatLiveScrollObserver { event in
          guard isPresented else { return }
          _ = scrollFollow.handle(event)
        }
      }
    }
    .contentMargins(.top, topInset + navigationHeight)
    .contentMargins(.bottom, composerHeight > 0 ? composerHeight : bottomInset + 54)
    if #available(macOS 26.0, *) {
      // The within-window material owns these edges; a second system shade doubles them.
      list.scrollEdgeEffectHidden(true, for: [.top, .bottom])
    } else {
      list
    }
  }

  static func receiptStatusText(for recipient: OverlayConversationRecipient) -> String {
    if recipient.acknowledged { return String(localized: "Read") }
    if recipient.accepted { return String(localized: "Queue accepted") }
    if recipient.queued { return String(localized: "Queued") }
    return String(localized: "Addressed")
  }

  private func submit() {
    _ = scrollFollow.handle(.jumpToCurrent)
    onSend()
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
    if !message.recipients.isEmpty {
      if message.recipients.count > 1 {
        let readCount = message.recipients.filter { $0.acknowledged }.count
        Menu {
          ForEach(message.recipients, id: \.owner.id) { recipient in
            Text(verbatim: "\(recipient.owner.name) · \(Self.receiptStatusText(for: recipient))")
          }
        } label: {
          Text("Read \(readCount)/\(message.recipients.count)")
            .font(.system(size: 10 * textScale))
            .foregroundStyle(palette.mutedText.color)
            .lineLimit(1)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help("Show details")
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

  private func messageRow(_ message: OverlayConversationMessage, maxBubbleWidth: CGFloat)
    -> some View
  {
    HStack(alignment: .top, spacing: 0) {
      if message.kind == .user { Spacer(minLength: 0) }
      messageBubble(message)
        .frame(maxWidth: maxBubbleWidth, alignment: message.kind == .user ? .trailing : .leading)
      if message.kind == .reply { Spacer(minLength: 0) }
    }
    .frame(maxWidth: .infinity)
    .id(message.id)
  }

  private func messageBubble(_ message: OverlayConversationMessage) -> some View {
    VStack(alignment: .leading, spacing: 5) {
      HStack {
        if message.kind == .user {
          Text("You").font(.system(size: 10 * textScale, weight: .bold))
        } else {
          Text(verbatim: message.owner?.name ?? conversation.name)
            .font(.system(size: 10 * textScale, weight: .bold))
        }
        if message.unsolicited && message.kind == .reply {
          Text("Unsolicited reply").font(.system(size: 10 * textScale))
        }
        Spacer(minLength: 8)
        CopyMessageButton(text: message.text)
          .buttonStyle(.borderless)
          .accessibilityIdentifier("overlay-message-copy-\(message.id)")
      }
      .foregroundStyle(palette.mutedText.color)
      if let question = addressedQuestion(for: message) {
        Text("In reply to: \(String(question.text.prefix(100)))")
          .font(.system(size: 10 * textScale))
          .foregroundStyle(palette.mutedText.color)
          .lineLimit(2)
      }
      MarkdownText(raw: message.text, size: 13, bodyColor: palette.primaryText.color)
        .fixedSize(horizontal: false, vertical: true)
        .accessibilityIdentifier("overlay-conversation-text-\(message.id)")
      messageReceipts(message)
      if message.kind == .reply && message.supportsSpeechPlayback {
        HStack(spacing: 10) {
          let active = message.playback.map { ["waiting", "playing"].contains($0.state) } ?? false
          if active {
            Button("Stop", systemImage: "stop.fill") { onControl(message, true) }
              .font(.system(size: 13 * textScale))
              .accessibilityIdentifier("overlay-reply-stop-\(message.id)")
          } else {
            Button {
              onControl(message, false)
            } label: {
              Image(systemName: "play.fill")
            }
            .frame(width: 24, height: 24)
            .accessibilityLabel("Play")
            .help("Play")
            .disabled(pendingControls.contains(message.id) || message.owner == nil)
            .accessibilityIdentifier("overlay-reply-play-\(message.id)")
          }
          if let playback = message.playback {
            Text(playbackLabel(playback.state, reason: playback.reason))
              .font(.system(size: 10 * textScale))
          } else if pendingControls.contains(message.id) {
            Text("Requesting playback").font(.system(size: 10 * textScale))
          }
        }
        .buttonStyle(.borderless)
        if let reason = message.playback?.reason, reason != "muted" {
          Text(verbatim: reason)
            .font(.system(size: 10 * textScale)).foregroundStyle(palette.mutedText.color)
        }
        if let error = controlErrors[message.id] {
          Text(verbatim: error)
            .font(.system(size: 10 * textScale)).foregroundStyle(palette.errorStatus.color)
        }
      }
    }
    .padding(12)
    .background(
      palette.primaryText.color.opacity(0.06),
      in: RoundedRectangle(cornerRadius: 16, style: .continuous)
    )
    .overlay {
      RoundedRectangle(cornerRadius: 16, style: .continuous)
        .strokeBorder(
          message.kind == .user ? CSColor.terracotta.opacity(0.65) : palette.border.color,
          lineWidth: 1 / max(displayScale, 1)
        )
        .allowsHitTesting(false)
    }
  }

  private func playbackLabel(_ state: String, reason: String? = nil) -> String {
    if reason == "muted" { return String(localized: "Automatic playback muted") }
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
