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

  var body: some View {
    ScrollView {
      LazyVStack(alignment: .leading, spacing: 14) {
        if conversation.channel == "0" { Text("0 · All").font(.headline) }
        else { Text(verbatim: conversation.name).font(.headline) }
        if let owner = conversation.owner {
          Text(verbatim: "\(owner.provider) · \(owner.providerSessionID)")
            .font(.caption)
            .foregroundStyle(palette.mutedText.color)
            .textSelection(.enabled)
        }
        if conversation.messages.isEmpty {
          Text("No conversation messages yet")
            .foregroundStyle(palette.mutedText.color)
        }
        ForEach(conversation.messages) { message in
          messageRow(message)
        }
      }
      .padding(.horizontal, 20)
      .padding(.top, topInset)
      .padding(.bottom, bottomInset)
      .frame(maxWidth: .infinity, alignment: .leading)
    }
    .foregroundStyle(palette.primaryText.color)
    .accessibilityIdentifier("overlay-conversation-body")
  }

  private func messageRow(_ message: OverlayConversationMessage) -> some View {
    VStack(alignment: .leading, spacing: 5) {
      HStack {
        if message.kind == .user { Text("You").font(.caption.bold()) }
        else { Text(verbatim: message.owner?.name ?? conversation.name).font(.caption.bold()) }
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
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityIdentifier("overlay-conversation-text-\(message.id)")
      ForEach(message.recipients, id: \.owner.id) { recipient in
        VStack(alignment: .leading, spacing: 2) {
          Text(verbatim: "\(recipient.owner.name) · \(recipient.owner.provider) · \(recipient.owner.providerSessionID)")
          HStack(spacing: 8) {
            if recipient.queued { Text("Queued") }
            if recipient.accepted { Text("Queue accepted") }
            if recipient.acknowledged { Text("Acknowledged") }
            if !recipient.queued && !recipient.accepted && !recipient.acknowledged {
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
    .padding(10)
    .background(palette.desktopBackground.color.opacity(0.65), in: RoundedRectangle(cornerRadius: 8))
    .id(message.id)
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
      message.kind == .user && message.recipients.contains {
        $0.deliveryID == deliveryID && $0.owner.id == owner.id
      }
    }
    if let occurrenceID = reply.replyToOccurrenceID {
      return matches.first { $0.id == occurrenceID }
    }
    return matches.count == 1 ? matches[0] : nil
  }
}
