import Foundation
import UniformTypeIdentifiers

/// A typed message during its send hold. It lives only in the app until the
/// deadline or Send now: no helper call, bus row, follower wake or agent copy
/// exists before then, so Cancel/Edit never races a backend (Maciej,
/// 2026-11-08 09:13Z: what happens in the UI stays in the UI until the hold
/// ends). Publication then carries a zero hold, never a second one.
struct OverlayHeldSend: Codable, Equatable, Sendable {
  enum Phase: String, Codable, Sendable {
    /// On the clock. Only this phase can be cancelled or released.
    case holding
    /// Handed to the helper. No promise of cancellation is shown any more.
    case dispatching
    /// Published as `messageID`; the row stays until the reader shows it.
    case dispatched
  }

  static let idPrefix = "local-hold-"

  let id: String
  /// The conversation that shows the row: the owner's, or "0" for All.
  let conversationID: String
  /// Nil is a channel-0 broadcast.
  let owner: OverlayConversationOwner?
  /// The outgoing draft, attachment pointers included, exactly as Enter froze it.
  let text: String
  let reply: OverlayUserReplyReference?
  let alsoSentTo: [OverlayConversationOwner]
  let acceptedAt: Date
  let deadline: Date
  var phase: Phase
  var messageID: String?
  var dispatchedAt: Date?

  enum CodingKeys: String, CodingKey {
    case id, owner, text, reply, deadline, phase
    case conversationID = "conversation_id"
    case alsoSentTo = "also_sent_to"
    case acceptedAt = "accepted_at"
    case messageID = "message_id"
    case dispatchedAt = "dispatched_at"
  }

  static func isLocal(_ id: String?) -> Bool { id?.hasPrefix(idPrefix) == true }

  static func newID() -> String { idPrefix + UUID().uuidString.lowercased() }

  static func newMessageID() -> String {
    String(UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased().prefix(24))
  }

  /// The row the conversation paints, shaped as the reader shapes a typed
  /// message, so the held bubble, Cancel/Edit, Send now and empty-Return
  /// release work unchanged. Only a holding row carries a deadline and a
  /// control identity.
  func row(order: UInt64, now: Date) -> OverlayConversationMessage {
    let parts = ConversationAttachmentPointer.extract(from: text)
    var message = OverlayConversationMessage(
      id: id, kind: .user, text: parts.text, order: order,
      emittedAt: ISO8601DateFormatter().string(from: acceptedAt), owner: nil,
      recipients: ([owner].compactMap { $0 } + alsoSentTo).map {
        OverlayConversationRecipient(
          owner: $0, deliveryID: nil, queued: false, accepted: false, acknowledged: false)
      },
      deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
    message.sourceChannel = owner?.channel ?? "0"
    message.userReplyTo = reply
    message.attachments = parts.paths.isEmpty ? nil : parts.paths.map(Self.attachment)
    message.deliverySealed = true
    if phase == .holding {
      message.holdUntil = deadline
      message.controlIdentity = id
    }
    return message.projectingDelivery(at: now)
  }

  private static func attachment(_ path: String) -> OverlayMessageAttachment {
    let url = URL(fileURLWithPath: path)
    let bytes =
      ((try? FileManager.default.attributesOfItem(atPath: path))?[.size] as? NSNumber)?.intValue
      ?? 0
    return OverlayMessageAttachment(
      name: url.lastPathComponent, path: path,
      mediaType: UTType(filenameExtension: url.pathExtension)?.preferredMIMEType
        ?? "application/octet-stream",
      bytes: bytes)
  }
}

/// Held sends outlive the process in the same private directory as drafts.
/// A relaunch never publishes them: they return to their composers for review.
enum OverlayHeldSendStore {
  static let schema = "codescribe.overlay-held-sends.v1"
  static let filename = "overlay-held-sends.v1.json"

  private struct Payload: Codable {
    var schema: String
    var sends: [OverlayHeldSend]
  }

  static func url(cacheRoot: URL) -> URL { cacheRoot.appendingPathComponent(filename) }

  static func load(cacheRoot: URL) -> [OverlayHeldSend] {
    let url = url(cacheRoot: cacheRoot)
    guard
      let size = (try? FileManager.default.attributesOfItem(atPath: url.path))?[.size]
        as? NSNumber, size.intValue <= 8 << 20,
      let data = try? Data(contentsOf: url)
    else { return [] }
    let decoder = JSONDecoder()
    decoder.dateDecodingStrategy = .iso8601
    guard let payload = try? decoder.decode(Payload.self, from: data),
      payload.schema == schema
    else { return [] }
    return payload.sends
  }

  static func save(cacheRoot: URL, sends: [OverlayHeldSend]) throws {
    let url = url(cacheRoot: cacheRoot)
    guard !sends.isEmpty else {
      if FileManager.default.fileExists(atPath: url.path) {
        try FileManager.default.removeItem(at: url)
      }
      return
    }
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    encoder.dateEncodingStrategy = .iso8601
    try OverlayPrivateFile.write(
      try encoder.encode(Payload(schema: schema, sends: sends)), to: url)
  }
}

enum OverlayHeldSendError: LocalizedError, Equatable {
  /// The hold already ended: the helper has the message, the app cannot take
  /// it back.
  case alreadySending

  var errorDescription: String? {
    String(
      localized: "This message is already being sent.",
      comment: "Cancel/Edit came after the send hold ended")
  }
}
