import SwiftUI

/// Paints the passive observer's immutable conversation projection.
struct OverlayConversationView: View {
  let conversation: OverlayConversation
  let palette: OverlayAppearancePalette
  let topInset: CGFloat
  let bottomInset: CGFloat
  let pendingControls: Set<String>
  let controlErrors: [String: String]
  let onControl: (OverlayConversationMessage, Bool) -> Void
  let onShowMonitor: () -> Void
  var focusRevision: UInt64 = 0
  var followsLiveChannel = false
  @Binding var draft: String
  let sending: Bool
  let sendError: String?
  let onSend: () -> Void

  @State private var followsLatest = true
  @State private var composerHeight: CGFloat = 0

  var orderedMessages: [OverlayConversationMessage] { conversation.messages }

  var body: some View {
    GeometryReader { geometry in
      ScrollViewReader { proxy in
        trackedMessages(maxBubbleWidth: max(0, min(660, (geometry.size.width - 40) * 0.82)))
          .overlay(alignment: .top) {
            Color.clear
              .frame(height: topInset)
              .background { conversationChrome(top: true) }
              .allowsHitTesting(false)
              .accessibilityHidden(true)
          }
          .overlay(alignment: .bottom) {
            composer
              .background { conversationChrome(top: false) }
              .onGeometryChange(for: CGFloat.self) {
                $0.size.height
              } action: {
                composerHeight = $0
              }
          }
          .onAppear { scrollToLatest(proxy) }
          .onChange(of: conversation.id) { _, _ in scrollToLatest(proxy) }
          .onChange(of: focusRevision) { _, _ in scrollToLatest(proxy) }
          .onChange(of: composerHeight) { _, _ in
            if followsLatest { scrollToLatest(proxy) }
          }
          .onChange(of: orderedMessages.last) { _, _ in
            if followsLatest || followsLiveChannel { scrollToLatest(proxy) }
          }
      }
      .foregroundStyle(palette.primaryText.color)
    }
    .accessibilityIdentifier("overlay-conversation-body")
  }

  private var composer: some View {
    VStack(spacing: 8) {
      if conversation.owner != nil {
        OverlayConversationComposer(
          palette: palette, draft: $draft, sending: sending, onSubmit: submit)
        if let sendError {
          Text(verbatim: sendError).font(.caption).foregroundStyle(palette.errorStatus.color)
        }
      }
    }
    .padding(.horizontal, 20)
    .padding(.top, 6)
    .padding(.bottom, bottomInset)
    .frame(maxWidth: .infinity)
  }

  private func conversationChrome(top: Bool) -> some View {
    GeometryReader { geometry in
      let overlap: CGFloat = 8
      let height = geometry.size.height + overlap
      // Fade across all the chrome, rather than an opaque bar with a softened edge.
      OverlayScrollMaterial(top: top, fade: height)
        .frame(height: height)
        .offset(y: top ? 0 : -overlap)
    }
    .allowsHitTesting(false)
    .accessibilityHidden(true)
  }

  private var navigation: some View {
    HStack {
      Button("Capture channels", systemImage: "chevron.left", action: onShowMonitor)
        .buttonStyle(.plain)
        .font(.caption)
        .accessibilityIdentifier("overlay-conversation-back")
      Spacer()
      if conversation.channel == "0" {
        Text("0 · All").font(.headline)
      } else {
        Text(verbatim: conversation.name).font(.headline)
          .help(
            Text(
              verbatim: conversation.owner.map { "\($0.provider) · \($0.providerSessionID)" } ?? "")
          )
      }
    }
    .accessibilityIdentifier("overlay-conversation-navigation")
  }

  @ViewBuilder
  private func trackedMessages(maxBubbleWidth: CGFloat) -> some View {
    if #available(macOS 15.0, *) {
      messageList(maxBubbleWidth: maxBubbleWidth)
        .onScrollGeometryChange(for: Bool.self) { geometry in
          geometry.visibleRect.maxY >= geometry.contentSize.height - 48
        } action: { _, atBottom in
          followsLatest = atBottom
        }
    } else {
      messageList(maxBubbleWidth: maxBubbleWidth)
    }
  }

  @ViewBuilder
  private func messageList(maxBubbleWidth: CGFloat) -> some View {
    let list = ScrollView {
      LazyVStack(alignment: .leading, spacing: 12) {
        navigation
          .padding(.bottom, 4)
        if orderedMessages.isEmpty {
          Text("No conversation messages yet")
            .foregroundStyle(palette.mutedText.color)
        }
        ForEach(orderedMessages) { message in
          messageRow(message, maxBubbleWidth: maxBubbleWidth)
        }
        Color.clear.frame(height: 1).id("conversation-bottom")
      }
      .padding(.horizontal, 20)
      .padding(.vertical, 8)
      .frame(maxWidth: .infinity, alignment: .leading)
    }
    .contentMargins(.top, topInset)
    .contentMargins(.bottom, composerHeight > 0 ? composerHeight : bottomInset + 54)
    if #available(macOS 26.0, *) {
      // The within-window material owns these edges; a second system shade doubles them.
      list.scrollEdgeEffectHidden(true, for: [.top, .bottom])
    } else {
      list
    }
  }

  private func submit() {
    followsLatest = true
    onSend()
  }

  private func scrollToLatest(_ proxy: ScrollViewProxy) {
    followsLatest = true
    proxy.scrollTo("conversation-bottom", anchor: .bottom)
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
          Text("You").font(.caption.bold())
        } else {
          Text(verbatim: message.owner?.name ?? conversation.name).font(.caption.bold())
        }
        if message.unsolicited && message.kind == .reply {
          Text("Unsolicited reply").font(.caption)
        }
      }
      .foregroundStyle(palette.mutedText.color)
      if let question = addressedQuestion(for: message) {
        Text("In reply to: \(String(question.text.prefix(100)))")
          .font(.caption)
          .foregroundStyle(palette.mutedText.color)
          .lineLimit(2)
      }
      Text(verbatim: message.text)
        .textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
        .accessibilityIdentifier("overlay-conversation-text-\(message.id)")
      ForEach(message.recipients, id: \.owner.id) { recipient in
        VStack(alignment: .leading, spacing: 2) {
          if conversation.channel == "0" { Text(verbatim: recipient.owner.name) }
          HStack(spacing: 8) {
            if recipient.acknowledged {
              Text("Acknowledged")
            } else if recipient.accepted {
              Text("Queue accepted")
            } else if recipient.queued {
              Text("Queued")
            } else {
              Text("Addressed")
            }
          }
        }
        .font(.caption)
        .foregroundStyle(palette.mutedText.color)
      }
      if message.kind == .reply {
        HStack(spacing: 10) {
          let active = message.playback.map { ["waiting", "playing"].contains($0.state) } ?? false
          if active {
            Button("Stop", systemImage: "stop.fill") { onControl(message, true) }
              .accessibilityIdentifier("overlay-reply-stop-\(message.id)")
          } else {
            Button("Play", systemImage: "play.fill") { onControl(message, false) }
              .disabled(pendingControls.contains(message.id) || message.owner == nil)
              .accessibilityIdentifier("overlay-reply-play-\(message.id)")
          }
          if let playback = message.playback {
            Text(playbackLabel(playback.state)).font(.caption)
          } else if pendingControls.contains(message.id) {
            Text("Requesting playback").font(.caption)
          }
        }
        .buttonStyle(.borderless)
        if let reason = message.playback?.reason {
          Text(verbatim: reason).font(.caption).foregroundStyle(palette.mutedText.color)
        }
        if let error = controlErrors[message.id] {
          Text(verbatim: error).font(.caption).foregroundStyle(palette.errorStatus.color)
        }
      }
    }
    .padding(12)
    .background(
      message.kind == .user
        ? CSColor.terracotta.opacity(palette.appearance == .dark ? 0.20 : 0.12)
        : palette.primaryText.color.opacity(0.06),
      in: RoundedRectangle(cornerRadius: 16, style: .continuous)
    )
    .overlay {
      RoundedRectangle(cornerRadius: 16, style: .continuous)
        .strokeBorder(
          message.kind == .user ? CSColor.terracotta.opacity(0.28) : palette.border.color,
          lineWidth: 1
        )
        .allowsHitTesting(false)
    }
  }

  private func playbackLabel(_ state: String) -> String {
    switch state {
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
