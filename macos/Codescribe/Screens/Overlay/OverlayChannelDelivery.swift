import CoreFoundation
import CryptoKit
import Foundation

/// Read-only delivery evidence. Receipt means the agent acknowledged the envelope,
/// not that it executed the request. No transcript text participates in identity.
struct OverlayChannelDelivery: Equatable, Identifiable, Sendable {
  enum Stage: String, Codable, Sendable { case held, sent, queued, received }

  let channel: String
  let agent: String
  let deliveryID: String?
  let stage: Stage?
  let isOpen: Bool
  var broadcastIdentity: AgentPlaybackIdentity? = nil
  var broadcastPolicy: AgentBroadcastPolicy = .on
  /// The follower's skip receipt for exactly that delivery
  /// (`runtime/receipts/<message_id>/<delivery_id>.skipped.json`): off
  /// channel 0 a broadcast is never queued (`queue_delivery` in bus-demux.py),
  /// so "sent" would otherwise wait for a receipt that never comes.
  var broadcastSkipped = false
  var id: String { channel }

  /// Metadata projection of the bus; never a document reducer or delivery writer.
  struct Bus: Codable {
    private var seals: [String: Seal] = [:]
    private var sessions: [String: Session] = [:]
    private var endedSessions: Set<String> = []
    private var receipts: [String: Receipt] = [:]
    private var sealOrder: UInt64 = 0
    private var seenSeals: Set<String> = []
    private var messages: [String: OverlayConversationMessage] = [:]
    private var messageOrder: [String] = []
    private var captureRecipients: [String: [OverlayConversationOwner]] = [:]
    private var historicalOwners: [String: OverlayConversationOwner] = [:]
    private var playbackByReply: [String: OverlayReplyPlayback] = [:]
    private var retiredPlaybackTickets: [String: [String]] = [:]
    private var messageSequence: UInt64 = 0
    private(set) var revision: UInt64 = 0
    /// Older projections omitted shared-journal attachments and broadcast sessions.
    private(set) var destinationVersion: Int? = 2
    /// A pre-hold checkpoint must replay its source rows to recover hold metadata.
    private var holdProjectionVersion: Int = 1
    /// Synthesized Decodable requires this key. Older checkpoints fail decode
    /// and the reader replays its bounded source tail to recover Play metadata.
    private var speechProjectionVersion: Int = 1
    private var receiptCursor: Int = 0
    private var directoryCursor: Int = 0
    private var messageRevisions: [String: UInt64] = [:]
    private var deliveryOrigins: [String: DeliveryOrigin] = [:]
    private var captureAttachmentStages: [String: OverlayCaptureAttachmentReceipt]? = nil
    private var captureAttachmentCommits: [String: OverlayCaptureAttachmentCommit]? = nil
    private var activeCaptureDocuments: [String: String]? = nil
    private var sessionOpenedAt: [String: String] = [:]
    private(set) var panelFocusResults: [String: OverlayPanelFocusResult] = [:]
    private struct DeliveryOrigin: Codable {
      let captureSession: String
      let channel: String
      let documentIndex: String
      let audience: String
      let terminal: Bool
    }
    private enum CodingKeys: String, CodingKey {
      case seals, sessions, endedSessions, receipts, sealOrder, seenSeals, messages, messageOrder
      case captureRecipients, historicalOwners, playbackByReply, retiredPlaybackTickets,
        messageSequence, messageRevisions, deliveryOrigins, captureAttachmentStages,
        captureAttachmentCommits, activeCaptureDocuments, revision, sessionOpenedAt,
        destinationVersion, holdProjectionVersion, speechProjectionVersion
    }
    private static let historyLimit = 256
    private static let historyByteLimit = 16 << 20

    private struct Seal: Codable {
      let phaseID: String
      let audience: String
      let order: UInt64
      let captureSessionID: String
      let recipientIDs: [String]
    }
    private struct Session: Codable {
      let provider: String
      let providerSession: String
      let sessionID: String
      let openedAt: String
      let isOpen: Bool
    }
    private struct Receipt: Codable {
      let channel: String
      let agent: String
    }

    mutating func consume(_ row: [String: Any]) {
      revision &+= 1
      consumeConversation(row)
      pruneHistory()
      let schema = row["schema"] as? String
      if let focusResult = OverlayPanelFocusResult(row: row) {
        panelFocusResults[focusResult.requestID] = focusResult
        if panelFocusResults.count > 64,
          let oldestKey = panelFocusResults.min(by: { $0.value.emittedAt < $1.value.emittedAt })?
            .key
        {
          panelFocusResults.removeValue(forKey: oldestKey)
        }
        return
      }
      if schema == "codescribe.agent-ack.v1", row["kind"] as? String == "agent_ack",
        let id = row["delivery_id"] as? String,
        let channel = row["channel"] as? String, let agent = row["agent"] as? String
      {
        receipts[id] = Receipt(channel: channel, agent: agent)
        return
      }
      if schema == "codescribe.channel-session.v1", row["kind"] as? String == "channel_session",
        let channel = row["channel"] as? String,
        let sessionID = row["session_id"] as? String,
        let openedAt = row["opened_at"] as? String,
        let state = row["state"] as? String
      {
        // Broadcast sessions have a frozen recipient list, not one provider.
        let provider = row["provider"] as? String ?? ""
        let providerSession = row["provider_session_id"] as? String ?? ""
        guard channel == "0" || (!provider.isEmpty && !providerSession.isEmpty) else { return }
        if state == "segment_sealed" {
          endedSessions.insert(sessionID)
          return
        }
        // A delayed close from an earlier take cannot close its successor.
        if state != "open", let current = sessions[channel], current.openedAt != openedAt {
          return
        }
        guard state == "open" || state == "sealed" else { return }
        if state == "open"
          && !admitsOpening(channel: channel, session: sessionID, openedAt: openedAt)
        {
          return
        }
        if state == "open", let entries = row["recipients"] as? [[String: Any]] {
          let owners = entries.compactMap(OverlayConversationOwner.init(row:))
          if !owners.isEmpty && owners.count == entries.count {
            captureRecipients[sessionID] = owners
          }
        }
        if state == "sealed" { endedSessions.insert(sessionID) }
        sessions[channel] = Session(
          provider: provider, providerSession: providerSession, sessionID: sessionID,
          openedAt: openedAt, isOpen: state == "open" && row["loud"] as? Bool == true)
        return
      }
      guard schema == "codescribe.transcript.v1" || schema == "codescribe.transcript-evidence.v1",
        let sessionID = row["session_id"] as? String
      else { return }
      if row["status"] as? String == "session_ended"
        || row["reducer_action"] as? String == "session_ended"
      {
        endedSessions.insert(sessionID)
        return
      }
      guard let audience = row["audience"] as? String, !audience.isEmpty else { return }
      let phaseID: String
      if schema == "codescribe.transcript-evidence.v1" {
        // Channel documents become deliveries at capture closure. A ledger
        // terminal observation still belongs to the same evolving message.
        if Self.channelMessageKey(sessionID) != nil { return }
        guard row["reducer_action"] as? String == "record_ledger_terminal_seal",
          row["reducer_revision"] is NSNumber, row["rendered_text"] is String
        else { return }
        phaseID = Self.identity([
          "terminal-seal", sessionID, Self.coordinate(row["reducer_revision"]),
          "record_ledger_terminal_seal",
        ])
      } else {
        guard row["status"] as? String == "transcript_sealed", row["sequence"] is NSNumber
        else { return }
        phaseID = Self.identity([
          "clean", sessionID, Self.coordinate(row["sequence"]),
          Self.coordinate(row["utterance_id"]), "transcript_sealed",
        ])
      }
      // Repeated terminal rows are one phase, not a newer utterance.
      guard seenSeals.insert(audience + "\0" + phaseID).inserted else { return }
      sealOrder &+= 1
      let owners =
        (row["recipients"] as? [[String: Any]])?
        .compactMap(OverlayConversationOwner.init(row:)) ?? captureRecipients[sessionID] ?? []
      seals[audience] = Seal(
        phaseID: phaseID, audience: audience, order: sealOrder,
        captureSessionID: sessionID, recipientIDs: owners.map(\.id))
    }

    func project(channel: String, binding: [String: Any], lease: [String: Any])
      -> OverlayChannelDelivery?
    {
      guard let agent = binding["audience"] as? String, !agent.isEmpty,
        let provider = binding["provider"] as? String,
        let providerSession = binding["provider_session_id"] as? String, !providerSession.isEmpty,
        lease["schema"] as? String == "codescribe.agent-bridge.lease.v1",
        lease["provider"] as? String == provider,
        lease["provider_session_id"] as? String == providerSession,
        let leaseID = lease["lease_id"] as? String
      else { return nil }
      let session = sessions[channel]
      let isOpen =
        session.map {
          $0.provider == provider && $0.providerSession == providerSession
            && $0.isOpen && !endedSessions.contains($0.sessionID)
        } ?? false
      let ownerID = [provider, providerSession, leaseID].joined(separator: "\0")
      let seal = [seals[agent], seals["*"]].compactMap { $0 }.filter { seal in
        if !seal.recipientIDs.isEmpty { return seal.recipientIDs.contains(ownerID) }
        return session?.sessionID == seal.captureSessionID && session?.provider == provider
          && session?.providerSession == providerSession && seal.audience == agent
      }.max { $0.order < $1.order }
      var deliveryID = seal.map {
        Self.identity([
          "native_bus_demux", leaseID, $0.phaseID, "seal", Self.routedAudience($0.audience),
        ])
      }
      var stage: Stage? = deliveryID == nil ? nil : .sent
      var skipped = false
      if let deliveryID {
        let pending = lease["pending"] as? [[String: Any]] ?? []
        if pending.contains(where: {
          $0["delivery_id"] as? String == deliveryID && $0["kind"] as? String == "seal"
        }) {
          stage = .queued
        }
        if let receipt = receipts[deliveryID], receipt.channel == channel, receipt.agent == agent {
          stage = .received
        }
        let observations = messages.values.flatMap(\.recipients).filter {
          $0.owner.id == ownerID && $0.deliveryID == deliveryID
        }
        if observations.contains(where: \.queued) { stage = .queued }
        if observations.contains(where: \.acknowledged) { stage = .received }
        skipped = stage == .sent && observations.contains { $0.broadcastSkipped == true }
      }
      // A hold or a withdrawal is the only news this projection adds on top
      // of the seal-derived stage; an ordinary delivery keeps its receipts.
      if let message = messageOrder.reversed().compactMap({ messages[$0] }).first(where: {
        $0.kind == .user && $0.showsDeliveryReceipts
          && $0.recipients.contains { $0.owner.id == ownerID }
      }) {
        var addressed = message
        addressed.recipients = message.recipients.filter { $0.owner.id == ownerID }
        if addressed.withdrawn == true {
          // A withdrawn take never reached the agent: no receipt to paint.
          deliveryID = nil
          stage = nil
        } else if addressed.holdingSeconds(at: Date()) != nil {
          deliveryID = addressed.recipients.first?.deliveryID
          stage = .held
          skipped = false
        }
      }
      var delivery = OverlayChannelDelivery(
        channel: channel, agent: agent, deliveryID: deliveryID, stage: stage, isOpen: isOpen)
      delivery.broadcastSkipped = skipped
      return delivery
    }

    mutating func observeLease(_ lease: [String: Any], channel: String, binding: [String: Any]) {
      guard
        let owner = OverlayConversationOwner(
          lease: lease, channel: channel,
          name: binding["audience"] as? String ?? ""),
        lease["schema"] as? String == "codescribe.agent-bridge.lease.v1",
        owner.provider == binding["provider"] as? String,
        owner.providerSessionID == binding["provider_session_id"] as? String
      else { return }
      if historicalOwners[owner.id] == nil {
        historicalOwners[owner.id] = owner
        revision &+= 1
      } else {
        reconcileOwner(owner)
      }
      let pending = lease["pending"] as? [[String: Any]] ?? []
      for envelope in pending {
        guard let delivery = envelope["delivery_id"] as? String,
          envelope["provider"] as? String == owner.provider,
          envelope["provider_session_id"] as? String == owner.providerSessionID,
          envelope["lease_id"] as? String == owner.leaseID,
          envelope["bus"] as? String == lease["bus"] as? String
        else { continue }
        associateEnvelope(envelope, owner: owner, delivery: delivery)
        updateRecipient(
          delivery: delivery, owner: owner, queued: true, accepted: false,
          acknowledged: false)
      }
      pruneHistory()
    }

    mutating func reconcileOwner(_ owner: OverlayConversationOwner) {
      guard let previous = historicalOwners[owner.id], previous != owner else { return }
      // Verified roster/archive metadata repairs the conversation, including
      // cached broadcast-route channels. Historical message labels stay intact.
      historicalOwners[owner.id] = owner
      revision &+= 1
    }

    mutating func observeAcceptance(_ receipt: [String: Any]) {
      guard receipt["schema"] as? String == "codescribe.native-queue.receipt.v1",
        receipt["disposition"] as? String == "provider_accepted",
        let delivery = receipt["delivery_id"] as? String,
        let owner = OverlayConversationOwner(row: receipt)
      else { return }
      updateRecipient(
        delivery: delivery, owner: owner, queued: false, accepted: true,
        acknowledged: false)
    }

    typealias ReceiptCoordinate = (
      owner: OverlayConversationOwner, delivery: String, accepted: Bool
    )

    /// Recipients whose provider acceptance (`wakeups/`) is still worth a
    /// poll: their row is neither acknowledged nor skipped (flags painted by
    /// the files, the ack row or the lease). `accepted` says acceptance was
    /// already observed, so the reader skips that file. A rotating window of
    /// 128 per poll.
    mutating func receiptCoordinates() -> [ReceiptCoordinate] {
      var result: [ReceiptCoordinate] = []
      var seen: Set<String> = []
      for id in messageOrder {
        guard let message = messages[id] else { continue }
        for recipient in message.recipients
        where recipient.broadcastSkipped != true && !recipient.acknowledged {
          if let delivery = recipient.deliveryID,
            seen.insert(recipient.owner.id + "\0" + delivery).inserted
          {
            result.append((recipient.owner, delivery, recipient.accepted))
          }
        }
      }
      guard !result.isEmpty else {
        receiptCursor = 0
        return []
      }
      let start = receiptCursor % result.count
      let count = min(result.count, 128)
      receiptCursor = (start + count) % result.count
      return (0..<count).map { result[(start + $0) % result.count] }
    }

    /// The `runtime/receipts/<message_id>/` directories still worth a
    /// listing: a message whose files do not yet settle it (`settled(_:)`),
    /// a channel-0 text whose admitted list is unknown (its `sent.json` may
    /// still be written), or one of the newest 32 replies whose playback is
    /// unknown or running. A rotating window of 128 per poll; a settled
    /// directory leaves it, whichever bus's copy the files settled.
    mutating func receiptDirectories() -> [String] {
      var unheard: Set<String> = []
      for id in messageOrder.reversed() where unheard.count < 32 {
        guard let message = messages[id], message.kind == .reply,
          message.playback.map({ $0.state == "waiting" || $0.state == "playing" }) ?? true
        else { continue }
        unheard.insert(id)
      }
      var result: [String] = []
      var seen: Set<String> = []
      for id in messageOrder {
        guard let message = messages[id], let directory = receiptDirectory(of: message),
          !seen.contains(directory)
        else { continue }
        let settled = Self.settled(message)
        let listed =
          message.sentTo != nil || message.sourceChannel != "0"
          || !(id.hasPrefix("typed:") || id.hasPrefix("reply:"))
        if settled, listed, !unheard.contains(id) { continue }
        seen.insert(directory)
        result.append(directory)
      }
      guard !result.isEmpty else {
        directoryCursor = 0
        return []
      }
      let start = directoryCursor % result.count
      let count = min(result.count, 128)
      directoryCursor = (start + count) % result.count
      return (0..<count).map { result[(start + $0) % result.count] }
    }

    /// Whether the receipt files leave nothing to wait for: the directory
    /// was composed, every copy's delivery is `read`, `skipped` or withdrawn
    /// there, and so is every agent on the list (the admitted list, else the
    /// copies). Only the files settle a message: the `agent-ack` bus row and
    /// the provider's acceptance paint a copy's row but never stop the
    /// listing, so a `read` file written just before that row is still
    /// composed. A message never composed is never settled; a message
    /// without copies on this bus is settled by its list alone.
    private static func settled(_ message: OverlayConversationMessage) -> Bool {
      guard let receipt = message.receipt else { return false }
      var states: [String: OverlayReceiptState.State] = [:]
      for delivery in receipt.deliveries {
        if let id = delivery.deliveryID, states[id] == nil { states[id] = delivery.state }
      }
      let withdrawn = Set(receipt.withdrawn.map(\.deliveryID))
      func done(_ id: String?, _ state: OverlayReceiptState.State?) -> Bool {
        if state == .read || state == .skipped { return true }
        guard let id else { return false }
        return withdrawn.contains(id)
      }
      for recipient in message.recipients {
        let id = recipient.deliveryID
        if !done(id, id.flatMap { states[$0] }) { return false }
      }
      let listed = message.sentTo ?? message.recipients.map(\.reach)
      for recipient in receipt.summary(listed: listed).recipients {
        if !done(recipient.deliveryID, recipient.state) { return false }
      }
      return true
    }

    /// The `runtime/receipts/<message_id>/` directory of a message: its
    /// `message_id` for typed and peer text, `channel_message_identity` for a
    /// channel take (the same digest as `channelMessageKey`). A plain
    /// dictation seal has none and leaves no receipt.
    private func receiptDirectory(of message: OverlayConversationMessage) -> String? {
      let id = message.id
      if id.hasPrefix("typed:") || id.hasPrefix("reply:") {
        let identity = String(id.drop(while: { $0 != ":" }).dropFirst())
        return identity.count == 24 ? identity : nil
      }
      guard let session = message.captureSessionID, Self.channelMessageKey(session) == id else {
        return nil
      }
      return String(id.dropFirst("utterance:".count))
    }

    var verifiedCaptureAttachmentCommitPaths: [String: String] {
      (captureAttachmentCommits ?? [:]).reduce(into: [:]) { result, entry in
        result[entry.key] = entry.value.commitBus
      }
    }

    var referencedCaptureAttachmentCommitBuses: Set<String> {
      Set(
        (captureAttachmentStages ?? [:]).values.compactMap {
          $0.recipientBusPaths.contains($0.commitBus) ? $0.commitBus : nil
        })
    }

    /// The capture's opening and frozen audience exist before its first words.
    /// This is read-only bus evidence for a paste waiting for that first segment.
    var activeCaptureAttachmentSessions: [OverlayCaptureAttachmentSession] {
      sessions.compactMap { channel, session in
        let documentID = activeCaptureDocuments?[session.sessionID] ?? session.sessionID
        guard session.isOpen,
          !endedSessions.contains(session.sessionID),
          !endedSessions.contains(documentID),
          let recipients = captureRecipients[documentID] ?? captureRecipients[session.sessionID],
          !recipients.isEmpty
        else { return nil }
        return OverlayCaptureAttachmentSession(
          id: documentID, channel: channel, openedAt: session.openedAt,
          recipientIDs: recipients.map(\.id).sorted())
      }
    }

    func conversations(
      busPath: String, now: Date = Date(),
      verifiedCaptureAttachmentCommits: [String: String] = [:],
      sharedCaptureAttachmentBusPath: String? = nil
    ) -> [OverlayConversation] {
      var verifiedCommits = (captureAttachmentCommits ?? [:]).reduce(into: [String: String]()) {
        result, entry in
        if entry.value.commitBus == busPath {
          result[entry.key] = entry.value.commitBus
        }
      }
      for (stageID, commitBus) in verifiedCaptureAttachmentCommits {
        guard !commitBus.isEmpty else { continue }
        verifiedCommits[stageID] = commitBus
      }
      let ordered = messageOrder.compactMap { messages[$0] }.filter { $0.withdrawn != true }
        .map { message in
          var result = message
          result.busPath = busPath
          result.sourceRevision = messageRevisions[message.id]
          Self.mergeCommittedCaptureAttachments(
            into: &result, commits: verifiedCommits, stages: captureAttachmentStages ?? [:],
            sharedBusPath: sharedCaptureAttachmentBusPath)
          return result.projectingDelivery(at: now)
        }
      let broadcastQuestions = ordered.filter {
        $0.kind == .user && ($0.sourceChannel ?? deliveryOrigins[$0.id]?.channel) == "0"
      }
      let broadcastIDs = Set(broadcastQuestions.map(\.id))
      let broadcastDeliveries = Set(
        broadcastQuestions.flatMap { message in
          message.recipients.compactMap { recipient in
            recipient.deliveryID.map { recipient.owner.id + "\0" + $0 }
          }
        })
      let broadcast = ordered.filter { message in
        if message.kind == .user { return broadcastIDs.contains(message.id) }
        // Peer publication carries its own destination, independently of a
        // question or reply association. A private route also outranks causality.
        if let destination = message.sourceChannel { return destination == "0" }
        guard let owner = message.owner, let delivery = message.replyTo,
          broadcastDeliveries.contains(owner.id + "\0" + delivery)
        else { return false }
        return message.replyToOccurrenceID.map { broadcastIDs.contains($0) } ?? true
      }
      let broadcastReplyIDs = Set(broadcast.filter { $0.kind == .reply }.map(\.id))
      var result = [
        OverlayConversation(
          id: "0", channel: "0", name: "All", owner: nil,
          messages: broadcast),
        .humanInbox(messages: ordered.filter { $0.sourceChannel == "10" }),
      ]
      // Materialize identity strings once per row, not once per historical
      // owner comparison on every poll. These coordinates are local to this
      // projection pass; message ownership and destination remain unchanged.
      let addressedRows = ordered.map { row in
        (
          row: row, authorID: row.owner?.id,
          recipients: row.recipients.map { (id: $0.owner.id, channel: $0.owner.channel) }
        )
      }
      for (ownerID, owner) in historicalOwners.sorted(by: { $0.key < $1.key }) {
        let rows = addressedRows.filter { addressed in
          let row = addressed.row
          // Broadcast replies live only in All; retain private delivery evidence.
          if broadcastReplyIDs.contains(row.id) { return false }
          if row.kind == .reply, let destination = row.sourceChannel {
            if destination == "10" {
              return addressed.authorID == ownerID
            }
            // All is one shared conversation. Direct peer messages belong to
            // their frozen recipient and to their author when a recipient was admitted.
            return destination != "0"
              && (addressed.recipients.contains {
                $0.id == ownerID && $0.channel == destination
              }
                || (!addressed.recipients.isEmpty && addressed.authorID == ownerID))
          }
          return addressed.authorID == ownerID
            || addressed.recipients.contains { $0.id == ownerID }
        }.map { $0.row }
        result.append(
          OverlayConversation(
            id: ownerID, channel: owner.channel,
            name: owner.name, owner: owner, messages: rows))
      }
      return result
    }

    /// The receipt files of one message directory, as the reader listed them
    /// (`docs/TRANSCRIPT_BUS.md`, Receipts): the composer turns them into the
    /// message's receipt state, the one source of `delivered`, `read`,
    /// `skipped`, the admitted list and a reply's playback. A delivery is
    /// keyed by `delivery_id`, never by lease: after a channel handover the
    /// successor lease acknowledges the delivery its predecessor received,
    /// one recipient. A file whose name and content disagree, or that names
    /// another message, marks nothing.
    mutating func observeReceipts(
      _ files: [(name: String, row: [String: Any])], messageID: String
    ) {
      let composed = OverlayReceiptState.compose(files: files, messageID: messageID)
      var changed = false
      for id in messageOrder {
        guard var message = messages[id], receiptDirectory(of: message) == messageID else {
          continue
        }
        var rowChanged = false
        if message.receipt != composed {
          message.receipt = composed
          rowChanged = true
        }
        if let sentTo = composed.sentTo, message.sentTo != sentTo {
          message.sentTo = sentTo
          rowChanged = true
        }
        for index in message.recipients.indices {
          guard let delivery = message.recipients[index].deliveryID,
            let state = composed.deliveries.first(where: { $0.deliveryID == delivery })?.state
          else { continue }
          switch state {
          case .read where !message.recipients[index].acknowledged:
            message.recipients[index].acknowledged = true
            rowChanged = true
          case .delivered where !message.recipients[index].queued:
            message.recipients[index].queued = true
            rowChanged = true
          case .skipped where message.recipients[index].broadcastSkipped != true:
            message.recipients[index].broadcastSkipped = true
            rowChanged = true
          default:
            break
          }
        }
        if rowChanged {
          messages[id] = message
          changed = true
        }
        guard message.kind == .reply, let replyID = message.replyID else { continue }
        for playback in composed.playbacks {
          guard let owner = playback.owner else { continue }
          if applyPlayback(
            OverlayReplyPlayback(
              replyID: replyID, ticket: playback.ticket, state: playback.state,
              reason: playback.reason, spoken: playback.spoken, emittedAt: playback.at ?? ""),
            owner: owner)
          {
            changed = true
          }
        }
      }
      if changed { revision &+= 1 }
    }

    /// Applies one playback observation under the ticket rules; true when the
    /// projection changed.
    @discardableResult
    private mutating func applyPlayback(
      _ playback: OverlayReplyPlayback, owner: OverlayConversationOwner
    ) -> Bool {
      let replyID = playback.replyID
      let ticket = playback.ticket
      let state = playback.state
      guard let message = messages["reply:" + replyID], message.owner?.id == owner.id else {
        return false
      }
      if retiredPlaybackTickets[replyID]?.contains(ticket) == true { return false }
      // Delayed observations cannot rewind playback or finish another ticket.
      if let current = playbackByReply[replyID] {
        if playback.emittedAt < current.emittedAt { return false }
        if current.ticket == ticket, current.state != "waiting" && current.state != "playing",
          state == "waiting" || state == "playing"
        {
          return false
        }
      }
      if let current = playbackByReply[replyID], current.ticket != ticket,
        state != "waiting" && state != "playing"
      {
        return false
      }
      if let current = playbackByReply[replyID], current.ticket != ticket {
        var retired = retiredPlaybackTickets[replyID] ?? []
        retired.append(current.ticket)
        retiredPlaybackTickets[replyID] = Array(retired.suffix(64))
      }
      if playbackByReply[replyID] == playback { return false }
      playbackByReply[replyID] = playback
      messages["reply:" + replyID]?.playback = playback
      return true
    }

    private mutating func associateEnvelope(
      _ envelope: [String: Any], owner: OverlayConversationOwner,
      delivery: String
    ) {
      guard let kind = envelope["kind"] as? String, kind == "seal" || kind == "message" else {
        return
      }
      let session = envelope["session_id"] as? String ?? ""
      let key: String
      let revision: UInt64
      if kind == "message",
        envelope["producer_schema"] as? String == "codescribe.agent-user-message.v1",
        let identity = envelope["message_id"] as? String
      {
        key = "typed:" + identity
        revision = 0
      } else if envelope["producer_schema"] as? String == "codescribe.transcript-evidence.v1",
        let messageKey = Self.channelMessageKey(session)
      {
        key = messageKey
        revision = (envelope["reducer_revision"] as? NSNumber)?.uint64Value ?? 0
      } else if envelope["producer_schema"] as? String == "codescribe.transcript-evidence.v1" {
        guard let occurrenceSession = envelope["occurrence_session_id"] as? String,
          let epoch = envelope["capture_epoch"] as? NSNumber,
          let start = envelope["sample_start"] as? NSNumber,
          let end = envelope["sample_end"] as? NSNumber
        else { return }
        key =
          "utterance:"
          + Self.identity([
            "occurrence", occurrenceSession,
            epoch.stringValue, start.stringValue, end.stringValue,
          ])
        revision = (envelope["reducer_revision"] as? NSNumber)?.uint64Value ?? 0
      } else {
        guard envelope["utterance_id"] is NSNumber || envelope["utterance_id"] is String else {
          return
        }
        key =
          "utterance:"
          + Self.identity(["utterance", session, Self.coordinate(envelope["utterance_id"])])
        revision = (envelope["sequence"] as? NSNumber)?.uint64Value ?? 0
      }
      guard revision >= (messageRevisions[key] ?? 0), var message = messages[key],
        let index = message.recipients.firstIndex(where: { $0.owner.id == owner.id })
      else { return }
      if message.recipients[index].deliveryID != delivery {
        self.revision &+= 1
        message.recipients[index] = OverlayConversationRecipient(
          owner: message.recipients[index].owner,
          deliveryID: delivery, queued: false, accepted: false, acknowledged: false)
      }
      if kind == "seal", message.captureState != .finished {
        // A finished take is a projection change: the checkpoint and the
        // reader's conversation memo both key on this revision.
        self.revision &+= 1
        message.captureState = .finished
      }
      messages[key] = message
    }

    private mutating func consumeConversation(_ row: [String: Any]) {
      let schema = row["schema"] as? String
      if schema == "codescribe.transcript.v1" || schema == "codescribe.transcript-evidence.v1",
        let session = row["session_id"] as? String,
        let key = Self.channelMessageKey(session),
        let rate = Self.captureSampleRateHz(row), var message = messages[key]
      {
        // A terminal coverage receipt may be the first row that carries the
        // capture rate. Keep it for every committed chip of this same take.
        message.captureSampleRateHz = rate
        messages[key] = message
      }
      if schema == "codescribe.capture-attachment.v1",
        row["kind"] as? String == "capture_attachment"
      {
        consumeCaptureAttachment(row)
        return
      }
      if schema == "codescribe.capture-attachment-commit.v1",
        row["kind"] as? String == "capture_attachment_commit"
      {
        consumeCaptureAttachmentCommit(row)
        return
      }
      if schema == "codescribe.delivery-control.v1" {
        consumeDeliveryControl(row)
        return
      }
      // Agent-authored text shares the reply presentation and playback owner.
      // A typed Founder message may name channel 10 as its sender, but remains
      // a user message routed through its recipient coordinates.
      if schema == "codescribe.agent-user-message.v1",
        row["kind"] as? String == "agent_user_message", row["source"] as? String != "typed",
        row["sender"] is [String: Any],
        let identity = row["message_id"] as? String, identity.count == 24,
        identity.allSatisfy({ "0123456789abcdef".contains($0) })
      {
        var reply = row
        reply["schema"] = "codescribe.agent-reply.v1"
        reply["kind"] = "agent_reply"
        reply["reply_id"] = identity
        consumeConversation(reply)
        return
      }
      if schema == "codescribe.agent-user-message.v1",
        row["kind"] as? String == "agent_user_message",
        let identity = row["message_id"] as? String, identity.count == 24,
        identity.allSatisfy({ "0123456789abcdef".contains($0) }),
        let text = row["text"] as? String, !text.isEmpty,
        let audience = row["audience"] as? String,
        let entries = row["recipients"] as? [[String: Any]]
      {
        let owners = entries.compactMap(OverlayConversationOwner.init(row:))
        guard owners.count == 1 else { return }
        let key = "typed:" + identity
        if var existing = messages[key] {
          guard row["origin_channel"] as? String == "0", existing.sourceChannel == "0",
            existing.revisedText != nil || existing.text == text
          else { return }
          if existing.sentTo == nil { existing.sentTo = OverlayReceiptState.stampedReach(row) }
          for owner in owners
          where !existing.recipients.contains(where: { $0.owner.id == owner.id }) {
            historicalOwners[owner.id] = historicalOwners[owner.id] ?? owner
            existing.recipients.append(
              OverlayConversationRecipient(
                owner: owner,
                deliveryID: Self.identity([
                  "native_bus_demux", owner.leaseID, identity, "message",
                  Self.routedAudience(audience),
                ]), queued: false, accepted: false, acknowledged: false))
          }
          store(existing)
          return
        }
        let recipients = owners.map { owner in
          historicalOwners[owner.id] = historicalOwners[owner.id] ?? owner
          return OverlayConversationRecipient(
            owner: owner,
            deliveryID: Self.identity([
              "native_bus_demux", owner.leaseID, identity, "message",
              Self.routedAudience(audience),
            ]), queued: false, accepted: false, acknowledged: false)
        }
        var message = OverlayConversationMessage(
          id: key, kind: .user, text: text, order: nextOrder(),
          emittedAt: row["emitted_at"] as? String ?? "", owner: nil, recipients: recipients,
          deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
        message.sourceChannel = (row["origin_channel"] as? String) ?? (row["channel"] as? String)
        message.sentTo = OverlayReceiptState.stampedReach(row)
        message.attachments = OverlayMessageAttachment.pointers(row["attachments"])
        message.userReplyTo = (row["user_reply_to"] as? [String: Any]).flatMap(
          OverlayUserReplyReference.init(row:))
        message.controlIdentity = identity
        message.deliverySealed = true
        message.holdUntil = (row["hold_until"] as? String).flatMap(Self.openingDate)
        store(message)
        return
      }
      if schema == "codescribe.channel-session.v1", let session = row["session_id"] as? String,
        let channel = row["channel"] as? String
      {
        if row["state"] as? String == "open" {
          guard let openedAt = row["opened_at"] as? String,
            admitsOpening(channel: channel, session: session, openedAt: openedAt),
            let opened = Self.openingDate(openedAt)
          else { return }
          sessionOpenedAt[session] = openedAt
          for origin in deliveryOrigins.values
          where origin.channel == channel
            && origin.captureSession != session
            && Self.channelSessionID(of: origin.captureSession) != session
          {
            guard
              let previous = sessionOpenedAt[origin.captureSession]
                ?? sessionOpenedAt[Self.channelSessionID(of: origin.captureSession)],
              let earlier = Self.openingDate(previous), earlier < opened
            else { continue }
            finalizeRefused(session: origin.captureSession)
          }
        } else {
          endedSessions.insert(session)
          // A predecessor close can settle its own receipt, never its successor's.
          finalizeRefused(
            session: session, holdUntil: (row["hold_until"] as? String).flatMap(Self.openingDate))
        }
      }
      if schema == "codescribe.transcript.v1" || schema == "codescribe.transcript-evidence.v1",
        row["status"] as? String == "session_ended"
          || row["reducer_action"] as? String == "session_ended",
        let session = row["session_id"] as? String
      {
        finalizeRefused(
          session: session, holdUntil: (row["hold_until"] as? String).flatMap(Self.openingDate))
      }
      if schema == "codescribe.channel-recipients.v1",
        let session = row["session_id"] as? String,
        let entries = row["recipients"] as? [[String: Any]]
      {
        captureRecipients[session] = entries.compactMap(OverlayConversationOwner.init(row:))
        var documents = activeCaptureDocuments ?? [:]
        documents[Self.channelSessionID(of: session)] = session
        activeCaptureDocuments = documents
        for owner in captureRecipients[session] ?? [] { historicalOwners[owner.id] = owner }
        return
      }
      // The app's own row after an agent's ACK: it paints the copy's row and
      // the channel glyph as read, a hint ahead of the next directory
      // listing. It is not a receipt: the count and the end of the listing
      // come from the `read` file alone (`receiptDirectories`, `settled`).
      if schema == "codescribe.agent-ack.v1",
        let delivery = row["delivery_id"] as? String,
        let owner = OverlayConversationOwner(row: row)
      {
        updateRecipient(
          delivery: delivery, owner: owner, queued: false, accepted: false,
          acknowledged: true)
        return
      }
      if schema == "codescribe.agent-reply-playback.v1",
        let replyID = row["reply_id"] as? String,
        let owner = OverlayConversationOwner(row: row),
        let ticket = row["playback_ticket"] as? String,
        let state = row["state"] as? String
      {
        applyPlayback(
          OverlayReplyPlayback(
            replyID: replyID, ticket: ticket, state: state,
            reason: row["reason"] as? String ?? row["tts_error"] as? String,
            spoken: row["spoken"] as? Bool == true,
            emittedAt: row["emitted_at"] as? String ?? ""),
          owner: owner)
        return
      }
      if schema == "codescribe.agent-reply.v1", let replyID = row["reply_id"] as? String,
        let text = row["text"] as? String
      {
        // Peer replies carry a routing channel at the top level. Their author
        // belongs to sender; a malformed sender cannot borrow route metadata.
        let author = row["sender"] == nil ? row : row["sender"] as? [String: Any] ?? [:]
        guard let owner = OverlayConversationOwner(row: author) else { return }
        let destination = row["origin_channel"] as? String ?? row["channel"] as? String
        if row["sender"] != nil {
          guard
            let destination,
            (destination.count == 1 && "0123456789".contains(destination)) || destination == "10"
          else { return }
        }
        let key = "reply:" + replyID
        if let previous = messages[key], previous.owner?.id != owner.id { return }
        historicalOwners[owner.id] = historicalOwners[owner.id] ?? owner
        let addressed =
          row["association"] as? String == "addressed"
          && row["delivery_id"] is String
        let order = messages[key]?.order ?? nextOrder()
        // Peer text freezes its recipients at publication (`send_peer_command`
        // in bus-demux.py): the owner lease the sender's demux resolved, never
        // a later lease of the same name. Each entry observes the delivery
        // identity the recipient's demux derives for a message with no
        // audience, so that agent's ACK marker settles it. A replayed row keeps
        // the recipients it already has.
        var recipients = messages[key]?.recipients ?? []
        if row["peer_to"] is String, let frozen = row["recipients"] as? [[String: Any]] {
          for recipient in frozen.compactMap(OverlayConversationOwner.init(row:))
          where recipient.id != owner.id
            && !recipients.contains(where: { $0.owner.id == recipient.id })
          {
            historicalOwners[recipient.id] = historicalOwners[recipient.id] ?? recipient
            recipients.append(
              OverlayConversationRecipient(
                owner: recipient,
                deliveryID: Self.identity([
                  "native_bus_demux", recipient.leaseID, replyID, "message", "",
                ]), queued: false, accepted: false, acknowledged: false))
          }
        }
        var message = OverlayConversationMessage(
          id: key, kind: .reply,
          text: text, order: order,
          emittedAt: row["emitted_at"] as? String ?? "", owner: historicalOwners[owner.id],
          recipients: recipients,
          deliveryID: addressed ? row["delivery_id"] as? String : nil,
          replyTo: addressed ? row["delivery_id"] as? String : nil,
          unsolicited: !addressed, playback: playbackByReply[replyID], busPath: "")
        if row["sender"] != nil {
          message.sourceChannel = destination
        }
        message.sentTo = messages[key]?.sentTo ?? OverlayReceiptState.stampedReach(row)
        message.supportsSpeechPlayback = true
        message.provider = owner.provider
        message.providerSessionID = owner.providerSessionID
        if addressed, let session = row["session_id"] as? String,
          let messageKey = Self.channelMessageKey(session)
        {
          message.replyToOccurrenceID = messageKey
        } else if addressed, let occurrenceSession = row["occurrence_session_id"] as? String,
          !occurrenceSession.isEmpty, let epoch = row["capture_epoch"] as? NSNumber,
          UInt64(epoch.stringValue) != nil, let start = row["sample_start"] as? NSNumber,
          let end = row["sample_end"] as? NSNumber,
          let first = UInt64(start.stringValue), let last = UInt64(end.stringValue), last > first
        {
          message.replyToOccurrenceID =
            "utterance:"
            + Self.identity([
              "occurrence", occurrenceSession,
              epoch.stringValue, start.stringValue, end.stringValue,
            ])
        }
        store(message)
        return
      }
      guard schema == "codescribe.transcript.v1" || schema == "codescribe.transcript-evidence.v1",
        let session = row["session_id"] as? String,
        let audience = row["audience"] as? String, !audience.isEmpty,
        row["status"] as? String != "session_ended",
        row["reducer_action"] as? String != "session_ended"
      else { return }
      let owners: [OverlayConversationOwner]
      if let entries = row["recipients"] as? [[String: Any]] {
        owners = entries.compactMap(OverlayConversationOwner.init(row:))
        captureRecipients[session] = owners
        var documents = activeCaptureDocuments ?? [:]
        documents[Self.channelSessionID(of: session)] = session
        activeCaptureDocuments = documents
      } else {
        owners = captureRecipients[session] ?? []
      }
      // An audience label alone cannot choose a current provider session.
      guard !owners.isEmpty else { return }
      let evidence = schema == "codescribe.transcript-evidence.v1"
      let channelMessage = Self.channelMessageKey(session)
      let text =
        channelMessage != nil
        ? row["rendered_text"] as? String ?? row["text"] as? String
        : row["label"] as? String ?? row["text"] as? String ?? row["rendered_text"] as? String
      guard let text, !text.isEmpty else { return }
      if !evidence && !(row["utterance_id"] is NSNumber || row["utterance_id"] is String) { return }
      if evidence
        && (row["occurrence_session_id"] as? String == nil
          || !(row["capture_epoch"] is NSNumber) || !(row["sample_start"] is NSNumber)
          || !(row["sample_end"] is NSNumber))
      {
        return
      }
      let ledgerCoordinates =
        row["occurrence_session_id"] is String
        && row["capture_epoch"] is NSNumber && row["sample_start"] is NSNumber
        && row["sample_end"] is NSNumber
      let captureEpoch = UInt64(Self.coordinate(row["capture_epoch"]))
      let sampleStart = UInt64(Self.coordinate(row["sample_start"]))
      let sampleEnd = UInt64(Self.coordinate(row["sample_end"]))
      let occurrence =
        ledgerCoordinates
        ? Self.identity([
          "occurrence", row["occurrence_session_id"] as? String ?? session,
          Self.coordinate(row["capture_epoch"]), Self.coordinate(row["sample_start"]),
          Self.coordinate(row["sample_end"]),
        ])
        : Self.identity(["utterance", session, Self.coordinate(row["utterance_id"])])
      let key = channelMessage ?? "utterance:" + occurrence
      let revisionKey = evidence || channelMessage != nil ? "reducer_revision" : "sequence"
      let revision = (row[revisionKey] as? NSNumber)?.uint64Value ?? 0
      guard revision >= (messageRevisions[key] ?? 0) else {
        // A stale render cannot replace newer text; its exact ledger entry
        // still remains physical evidence when first observed on this reader.
        if channelMessage != nil, var message = messages[key] {
          var occurrences = message.occurrenceIDs ?? []
          let identity = "utterance:" + occurrence
          if !occurrences.contains(identity) { occurrences.append(identity) }
          message.occurrenceIDs = occurrences
          messages[key] = message
        }
        return
      }
      messageRevisions[key] = revision
      let phase: String
      let sealedRow = row["status"] as? String == "transcript_sealed"
      // Only the closing snapshot (`terminal: true`, stamped at channel close
      // with its hold deadline) finishes a channel take; a phrase terminal
      // inside the take is still a revision of the same document.
      let closingRow = sealedRow && row["terminal"] as? Bool == true
      let channelClosed =
        endedSessions.contains(session)
        || endedSessions.contains(Self.channelSessionID(of: session))
        || closingRow
      let terminal =
        evidence
        ? row["reducer_action"] as? String == "record_ledger_terminal_seal" || closingRow
        : sealedRow
      if evidence || channelMessage != nil {
        let components = session.split(separator: "-")
        let channel =
          components.count > 2 && components[0] == "agent" && components[1] == "channel"
          ? String(components[2]) : ""
        deliveryOrigins[key] = DeliveryOrigin(
          captureSession: session, channel: channel,
          documentIndex: Self.coordinate(row["document_index"]), audience: audience,
          terminal: terminal)
      }
      if channelMessage != nil {
        phase =
          channelClosed
          ? Self.identity(["channel-message-seal", session])
          : Self.identity([
            "channel-message-revision", session, Self.coordinate(row["reducer_revision"]),
            row["reducer_action"] as? String ?? "",
          ])
      } else if evidence && terminal {
        phase = Self.identity([
          "terminal-seal", session, Self.coordinate(row["reducer_revision"]),
          "record_ledger_terminal_seal",
        ])
      } else if evidence {
        phase = Self.identity([
          "evidence", session, Self.coordinate(row["sequence"]),
          Self.coordinate(row["reducer_revision"]), row["reducer_action"] as? String ?? "",
          row["occurrence_session_id"] as? String ?? "", Self.coordinate(row["capture_epoch"]),
          Self.coordinate(row["sample_start"]), Self.coordinate(row["sample_end"]),
          Self.coordinate(row["document_index"]),
        ])
      } else {
        phase = Self.identity([
          "clean", session, Self.coordinate(row["sequence"]),
          Self.coordinate(row["utterance_id"]), row["status"] as? String ?? "",
        ])
      }
      var recipients: [OverlayConversationRecipient] = []
      let kind =
        channelMessage != nil
        ? channelClosed ? "seal" : "revised"
        : terminal
          ? "seal"
          : evidence || row["status"] as? String == "utterance_revised"
            ? "revised" : row["status"] as? String == "utterance_draft" ? "draft" : "event"
      for owner in owners {
        historicalOwners[owner.id] = historicalOwners[owner.id] ?? owner
        let delivery = Self.identity([
          "native_bus_demux", owner.leaseID, phase,
          kind, Self.routedAudience(audience),
        ])
        let old = messages[key]?.recipients.first {
          $0.owner.id == owner.id && $0.deliveryID == delivery
        }
        recipients.append(
          old
            ?? OverlayConversationRecipient(
              owner: owner, deliveryID: delivery,
              queued: false, accepted: false, acknowledged: false))
      }
      let order = messages[key]?.order ?? nextOrder()
      var message = OverlayConversationMessage(
        id: key, kind: .user,
        text: text, order: order,
        // A take is stamped when its first words land, not when its channel
        // session opened: the next session opens as the previous one seals,
        // so the open time can precede the speech by the whole pause, and
        // Maciej's take then sat above a reply he had already read
        // (2026-10-23, 08:02:19 open, 08:02:29 first words, 08:02:30 reply).
        emittedAt: channelMessage != nil
          ? messages[key]?.emittedAt ?? row["emitted_at"] as? String ?? sessionOpenedAt[session]
            ?? sessionOpenedAt[Self.channelSessionID(of: session)] ?? ""
          : row["emitted_at"] as? String ?? "", owner: nil, recipients: recipients,
        deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
      let captureFinished =
        channelClosed || messages[key]?.captureState == .finished
      let documentFinished =
        channelMessage == nil
        && (terminal || row["status"] as? String == "transcript_sealed")
      message.captureState = captureFinished || documentFinished ? .finished : .receiving
      if channelMessage != nil {
        var occurrences = messages[key]?.occurrenceIDs ?? []
        let identity = "utterance:" + occurrence
        if !occurrences.contains(identity) { occurrences.append(identity) }
        message.occurrenceIDs = occurrences
      }
      if channelMessage != nil, ledgerCoordinates,
        let occurrenceSessionID = row["occurrence_session_id"] as? String,
        !occurrenceSessionID.isEmpty,
        let captureEpoch, let sampleStart, let sampleEnd, sampleEnd > sampleStart,
        let sourceSequence = UInt64(Self.coordinate(row["sequence"]))
      {
        let segment = OverlayCaptureAttachmentSegment(
          occurrenceID: "utterance:" + occurrence,
          occurrenceSessionID: occurrenceSessionID,
          captureEpoch: captureEpoch,
          sampleStart: sampleStart,
          sampleEnd: sampleEnd,
          sourceSequence: sourceSequence,
          reducerRevision: UInt64(Self.coordinate(row["reducer_revision"])) ?? 0,
          documentIndex: Self.coordinate(row["document_index"]),
          audience: audience,
          recipientIDs: owners.map(\.id),
          sampleRateHz: Self.captureSampleRateHz(row))
        var segments = messages[key]?.captureAttachmentSegments ?? []
        if let index = segments.firstIndex(where: {
          $0.occurrenceID == segment.occurrenceID
            && $0.sourceSequence == segment.sourceSequence
            && $0.reducerRevision == segment.reducerRevision
        }) {
          segments[index] = segment
        } else {
          segments.append(segment)
        }
        message.captureAttachmentSegments = segments
        message.activeCaptureAttachmentOccurrenceID = segment.occurrenceID
        message.activeCaptureAttachmentSourceSequence = segment.sourceSequence
        message.activeCaptureAttachmentReducerRevision = segment.reducerRevision
      }
      let components = session.split(separator: "-")
      message.sourceChannel =
        row["channel"] as? String
        ?? (components.count > 2 && components[0] == "agent" && components[1] == "channel"
          ? String(components[2]) : nil)
      message.captureSessionID = session
      message.captureAudience = audience
      message.captureSampleRateHz =
        Self.captureSampleRateHz(row) ?? messages[key]?.captureSampleRateHz
      let utterance = Self.coordinate(row["utterance_id"])
      message.controlIdentity = utterance.isEmpty ? session : utterance
      // Ledger previews are not deliveries, even when they carry hold metadata.
      message.deliverySealed =
        messages[key]?.deliverySealed == true
        || (evidence ? channelClosed : terminal)
      message.holdUntil =
        (row["hold_until"] as? String).flatMap(Self.openingDate)
        ?? messages[key]?.holdUntil
      message.revisedText = messages[key]?.revisedText
      message.withdrawn = messages[key]?.withdrawn
      message.holdReleased = messages[key]?.holdReleased
      if let revised = message.revisedText { message.text = revised }
      store(message)
    }

    private mutating func consumeCaptureAttachment(_ row: [String: Any]) {
      guard let stageID = row["stage_id"] as? String,
        Self.isCaptureStageID(stageID),
        let session = row["session_id"] as? String,
        Self.channelMessageKey(session) != nil,
        let epoch = (row["capture_epoch"] as? NSNumber)?.uint64Value,
        let sequence = (row["source_sequence"] as? NSNumber)?.uint64Value,
        let occurrenceSession = row["occurrence_session_id"] as? String,
        !occurrenceSession.isEmpty,
        let start = (row["sample_start"] as? NSNumber)?.uint64Value,
        let end = (row["sample_end"] as? NSNumber)?.uint64Value, end > start,
        let documentIndex = (row["document_index"] as? NSNumber)?.uint64Value,
        let reducerRevision = (row["reducer_revision"] as? NSNumber)?.uint64Value,
        let audience = row["audience"] as? String, !audience.isEmpty,
        let rawRecipients = row["recipients"] as? [[String: Any]],
        let commitBus = row["commit_bus"] as? String, commitBus.hasPrefix("/"),
        let attachments = OverlayMessageAttachment.pointers(row["attachments"]),
        !attachments.isEmpty
      else { return }
      let owners = rawRecipients.compactMap(OverlayConversationOwner.init(row:))
      let recipientBusPaths = rawRecipients.compactMap { $0["bus"] as? String }
      guard !owners.isEmpty, owners.count == rawRecipients.count,
        recipientBusPaths.count == rawRecipients.count
      else { return }
      let occurrenceID =
        "utterance:"
        + Self.identity([
          "occurrence", occurrenceSession, String(epoch), String(start), String(end),
        ])
      var stages = captureAttachmentStages ?? [:]
      stages[stageID] = OverlayCaptureAttachmentReceipt(
        stageID: stageID,
        captureSessionID: session,
        captureEpoch: epoch,
        sourceSequence: sequence,
        occurrenceSessionID: occurrenceSession,
        sampleStart: start,
        sampleEnd: end,
        documentIndex: documentIndex,
        reducerRevision: reducerRevision,
        audience: audience,
        recipientIDs: owners.map(\.id),
        recipientBusPaths: recipientBusPaths,
        commitBus: commitBus,
        attachments: attachments,
        occurrenceID: occurrenceID)
      captureAttachmentStages = stages
    }

    private mutating func consumeCaptureAttachmentCommit(_ row: [String: Any]) {
      guard let stageID = row["stage_id"] as? String,
        Self.isCaptureStageID(stageID),
        let stage = captureAttachmentStages?[stageID],
        row["session_id"] as? String == stage.captureSessionID,
        (row["capture_epoch"] as? NSNumber)?.uint64Value == stage.captureEpoch,
        (row["source_sequence"] as? NSNumber)?.uint64Value == stage.sourceSequence,
        row["commit_bus"] as? String == stage.commitBus,
        let emittedAt = row["emitted_at"] as? String,
        Self.openingDate(emittedAt) != nil
      else { return }
      var commits = captureAttachmentCommits ?? [:]
      commits[stageID] = OverlayCaptureAttachmentCommit(
        stageID: stageID, commitBus: stage.commitBus)
      captureAttachmentCommits = commits
    }

    private static func mergeCommittedCaptureAttachments(
      into message: inout OverlayConversationMessage,
      commits: [String: String],
      stages: [String: OverlayCaptureAttachmentReceipt],
      sharedBusPath: String?
    ) {
      guard message.kind == .user, let session = message.captureSessionID else { return }
      let segments = message.captureAttachmentSegments ?? []
      let recipients = Set(message.recipients.map { $0.owner.id })
      var paths = Set((message.attachments ?? []).map(\.path))
      var attachments = message.attachments ?? []
      for (stageID, commitBus) in commits.sorted(by: { $0.key < $1.key }) {
        guard let stage = stages[stageID], stage.stageID == stageID,
          stage.commitBus == commitBus, stage.captureSessionID == session,
          stage.recipientBusPaths.contains(commitBus) || commitBus == sharedBusPath,
          stage.audience == message.captureAudience,
          !recipients.isEmpty, Set(stage.recipientIDs) == recipients
        else { continue }
        let sourceRows = segments.filter {
          $0.captureEpoch == stage.captureEpoch && $0.sourceSequence == stage.sourceSequence
        }
        var attachedAtSample: UInt64 = 0
        var attachedAtSeconds: Double = 0
        if !sourceRows.isEmpty {
          // An observed coordinate must agree exactly. Missing history can
          // lose timing, but a conflicting source cannot lend this stage trust.
          guard
            let segment = sourceRows.first(where: {
              $0.occurrenceID == stage.occurrenceID
                && $0.reducerRevision == stage.reducerRevision
                && $0.occurrenceSessionID == stage.occurrenceSessionID
                && $0.sampleStart == stage.sampleStart && $0.sampleEnd == stage.sampleEnd
                && UInt64($0.documentIndex) == stage.documentIndex
                && $0.audience == stage.audience && $0.recipientIDs == stage.recipientIDs
            })
          else { continue }
          let origin =
            segments.filter { $0.captureEpoch == segment.captureEpoch }
            .map(\.sampleStart).min() ?? segment.sampleStart
          let rate = segment.sampleRateHz ?? message.captureSampleRateHz
          attachedAtSample = segment.sampleStart
          attachedAtSeconds =
            rate.map {
              Double(segment.sampleStart - min(origin, segment.sampleStart)) / $0
            } ?? 0
        }
        // A verified commit remains a visible pointer when its exact source
        // row fell outside retained history. The shared contract paints 0:00.
        for var attachment in stage.attachments where paths.insert(attachment.path).inserted {
          attachment.attachedAtSample = attachedAtSample
          attachment.attachedAtSeconds = attachedAtSeconds
          attachments.append(attachment)
        }
      }
      message.attachments = attachments.isEmpty ? nil : attachments
    }

    private static func isCaptureStageID(_ value: String) -> Bool {
      value.utf8.count == 24
        && value.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }

    private mutating func consumeDeliveryControl(_ row: [String: Any]) {
      guard let kind = row["kind"] as? String,
        let control = OverlayDeliveryControl(rawValue: kind),
        let emittedAt = (row["emitted_at"] as? String).flatMap(Self.openingDate),
        let provider = row["provider"] as? String,
        let providerSession = row["provider_session_id"] as? String,
        let lease = row["lease_id"] as? String
      else { return }
      let typed = row["message_id"] as? String
      let utterance = Self.coordinate(row["utterance_id"])
      let identity = row["identity"] as? String
      let candidates = messageOrder.filter { id in
        guard let message = messages[id], message.kind == .user else { return false }
        let matches: Bool
        if let typed {
          matches = id == "typed:" + typed
        } else if !utterance.isEmpty {
          matches =
            !id.hasPrefix("typed:") && message.controlIdentity == utterance
            && ((row["session_id"] as? String).map { $0 == message.captureSessionID } ?? true)
        } else if let identity {
          matches =
            identity == message.controlIdentity || identity == id
            || identity == message.captureSessionID
        } else {
          matches = false
        }
        guard matches, message.holdingSeconds(at: emittedAt) != nil else { return false }
        guard
          message.recipients.contains(where: { recipient in
            recipient.owner.provider == provider
              && recipient.owner.providerSessionID == providerSession
              && recipient.owner.leaseID == lease
          })
        else { return false }
        return true
      }
      // Reused numeric utterance labels do not identify a take on their own.
      guard candidates.count == 1, let id = candidates.first, var message = messages[id] else {
        return
      }
      switch control {
      case .revise:
        guard let text = row["text"] as? String, !text.isEmpty else { return }
        message.revisedText = text
        message.text = text
      case .withdraw:
        message.withdrawn = true
      case .release:
        message.holdReleased = true
      }
      messages[id] = message
    }

    private func admitsOpening(channel: String, session: String, openedAt: String) -> Bool {
      guard !endedSessions.contains(session), let incoming = Self.openingDate(openedAt) else {
        return false
      }
      guard let current = sessions[channel] else { return true }
      if current.sessionID == session { return current.openedAt == openedAt }
      guard let previous = Self.openingDate(current.openedAt) else { return false }
      return incoming > previous
    }

    private static func openingDate(_ value: String) -> Date? {
      let formatter = ISO8601DateFormatter()
      formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
      if let date = formatter.date(from: value) { return date }
      formatter.formatOptions = [.withInternetDateTime]
      return formatter.date(from: value)
    }

    private mutating func finalizeRefused(session: String, holdUntil: Date? = nil) {
      for (id, origin) in deliveryOrigins
      where origin.captureSession == session
        || Self.channelSessionID(of: origin.captureSession) == session
      {
        let originSession = origin.captureSession
        guard var message = messages[id] else { continue }
        message.deliverySealed = true
        message.holdUntil = holdUntil ?? message.holdUntil
        if origin.terminal && Self.channelMessageKey(originSession) == nil {
          messages[id] = message
          continue
        }
        let phase =
          Self.channelMessageKey(originSession) != nil
          ? Self.identity(["channel-message-seal", originSession])
          : Self.identity(["coverage-refused-seal", originSession, origin.documentIndex])
        for index in message.recipients.indices {
          let owner = message.recipients[index].owner
          let delivery = Self.identity([
            "native_bus_demux", owner.leaseID, phase, "seal", Self.routedAudience(origin.audience),
          ])
          if message.recipients[index].deliveryID != delivery {
            message.recipients[index] = OverlayConversationRecipient(
              owner: owner, deliveryID: delivery,
              queued: false, accepted: false, acknowledged: false)
          }
        }
        message.captureState = .finished
        messages[id] = message
        if Self.channelMessageKey(originSession) != nil,
          seenSeals.insert(origin.audience + "\0" + phase).inserted
        {
          sealOrder &+= 1
          seals[origin.audience] = Seal(
            phaseID: phase, audience: origin.audience, order: sealOrder,
            captureSessionID: originSession, recipientIDs: message.recipients.map { $0.owner.id })
        }
      }
    }

    private mutating func nextOrder() -> UInt64 {
      messageSequence &+= 1
      return messageSequence
    }

    private mutating func store(_ message: OverlayConversationMessage) {
      if messages[message.id] == nil { messageOrder.append(message.id) }
      messages[message.id] = message
    }

    private mutating func updateRecipient(
      delivery: String, owner: OverlayConversationOwner,
      queued: Bool, accepted: Bool, acknowledged: Bool
    ) {
      var changed = false
      for id in messageOrder {
        guard var message = messages[id] else { continue }
        var rowChanged = false
        for index in message.recipients.indices
        where message.recipients[index].owner.id == owner.id
          && message.recipients[index].deliveryID == delivery
        {
          let before = message.recipients[index]
          if (queued && !before.queued) || (accepted && !before.accepted)
            || (acknowledged && !before.acknowledged)
          {
            message.recipients[index].queued = before.queued || queued
            message.recipients[index].accepted = before.accepted || accepted
            message.recipients[index].acknowledged = before.acknowledged || acknowledged
            rowChanged = true
          }
        }
        if rowChanged {
          messages[id] = message
          changed = true
        }
      }
      if changed { revision &+= 1 }
    }

    private mutating func pruneHistory() {
      var bytes = messages.values.reduce(0) { $0 + $1.text.utf8.count }
      while messageOrder.count > Self.historyLimit
        || (bytes > Self.historyByteLimit && messageOrder.count > 1)
      {
        let id = messageOrder.removeFirst()
        bytes -= messages.removeValue(forKey: id)?.text.utf8.count ?? 0
        messageRevisions.removeValue(forKey: id)
        deliveryOrigins.removeValue(forKey: id)
      }
      let retainedCaptureSessions = Set(messages.values.compactMap(\.captureSessionID))
      if let stages = captureAttachmentStages {
        captureAttachmentStages = stages.filter {
          retainedCaptureSessions.contains($0.value.captureSessionID)
        }
      }
      if let commits = captureAttachmentCommits {
        let retainedStageIDs = Set((captureAttachmentStages ?? [:]).keys)
        captureAttachmentCommits = commits.filter { retainedStageIDs.contains($0.key) }
      }
      let replies = Set(messages.values.compactMap { $0.playback?.replyID })
      playbackByReply = playbackByReply.filter { replies.contains($0.key) }
      retiredPlaybackTickets = retiredPlaybackTickets.filter { replies.contains($0.key) }
      let ownerIDs = Set(
        messages.values.flatMap { row in
          row.recipients.map { $0.owner.id } + (row.owner.map { [$0.id] } ?? [])
        })
      if historicalOwners.count > Self.historyLimit {
        historicalOwners = historicalOwners.filter { ownerIDs.contains($0.key) }
      }
      if captureRecipients.count > Self.historyLimit {
        for key in captureRecipients.keys.sorted().prefix(
          captureRecipients.count - Self.historyLimit)
        {
          captureRecipients.removeValue(forKey: key)
        }
      }
      if sessionOpenedAt.count > Self.historyLimit {
        let retained = Set(
          deliveryOrigins.values.map(\.captureSession) + sessions.values.map(\.sessionID))
        sessionOpenedAt = sessionOpenedAt.filter { retained.contains($0.key) }
      }
      if receipts.count > Self.historyLimit * 2 {
        let deliveryIDs = Set(messages.values.flatMap { $0.recipients.compactMap(\.deliveryID) })
        receipts = receipts.filter { deliveryIDs.contains($0.key) }
      }
      if seals.count > Self.historyLimit {
        let keep = Set(
          seals.values.sorted { $0.order > $1.order }.prefix(Self.historyLimit).map(\.audience))
        seals = seals.filter { keep.contains($0.key) }
      }
      if sessions.count > Self.historyLimit {
        let keep = Set(
          sessions.values.sorted { $0.openedAt > $1.openedAt }.prefix(Self.historyLimit).map(
            \.sessionID))
        sessions = sessions.filter { keep.contains($0.value.sessionID) }
      }
      if seenSeals.count > Self.historyLimit * 2 {
        seenSeals = Set(seals.values.map { $0.audience + "\0" + $0.phaseID })
      }
      if endedSessions.count > Self.historyLimit * 2 {
        let retained = Set(
          sessions.values.map(\.sessionID) + deliveryOrigins.values.map(\.captureSession))
        endedSessions = endedSessions.intersection(retained)
      }
      if let documents = activeCaptureDocuments, documents.count > Self.historyLimit {
        let keep = Set(sessions.values.map(\.sessionID))
        activeCaptureDocuments = documents.filter { keep.contains($0.key) }
      }
    }

    static func channelSessionID(of session: String) -> String {
      guard let range = session.range(of: "-s", options: .backwards) else { return session }
      let prefix = String(session[..<range.lowerBound])
      let suffix = session[range.upperBound...]
      guard prefix.hasPrefix("agent-channel-"), !suffix.isEmpty,
        suffix.allSatisfy({ $0.isASCII && $0.isNumber }),
        let index = UInt32(suffix), index >= 1
      else { return session }
      return prefix
    }

    static func sameCaptureSession(_ lhs: String, _ rhs: String) -> Bool {
      if lhs == rhs { return true }
      let leftParent = channelSessionID(of: lhs)
      let rightParent = channelSessionID(of: rhs)
      return leftParent == rightParent && (lhs == leftParent || rhs == rightParent)
    }

    private static func captureSampleRateHz(_ row: [String: Any]) -> Double? {
      let raw: Any?
      if row["schema"] as? String == "codescribe.transcript-evidence.v1" {
        raw = (row["seal_coverage"] as? [String: Any])?["sample_rate_hz"]
      } else {
        raw = row["sample_rate_hz"]
      }
      guard let number = raw as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(),
        number.doubleValue.isFinite, number.doubleValue > 0
      else { return nil }
      return number.doubleValue
    }

    private static func coordinate(_ value: Any?) -> String {
      if let string = value as? String { return string }
      if let number = value as? NSNumber { return number.stringValue }
      return ""
    }

    private static func channelMessageKey(_ session: String) -> String? {
      let parts = session.split(separator: "-")
      guard parts.count > 2, parts[0] == "agent", parts[1] == "channel",
        !parts[2].isEmpty, parts[2].allSatisfy({ $0.isNumber })
      else { return nil }
      return "utterance:" + identity(["channel-message", session])
    }

    // Exact bus-demux.py _identity contract, including its NUL separator and
    // terminal phase coalescing. Fixed Python-produced vectors guard drift.
    private static func routedAudience(_ audience: String) -> String {
      audience.folding(options: .caseInsensitive, locale: Locale(identifier: "en_US_POSIX"))
    }

    private static func identity(_ parts: [String]) -> String {
      SHA256.hash(data: Data(parts.joined(separator: "\0").utf8))
        .prefix(12).map { String(format: "%02x", $0) }.joined()
    }
  }
}

/// Frozen recipient identity. Labels never choose or replace a provider session.
struct OverlayConversationOwner: Codable, Equatable, Hashable, Sendable {
  let provider: String
  let providerSessionID: String
  let leaseID: String
  let name: String
  let channel: String
  var id: String { [provider, providerSessionID, leaseID].joined(separator: "\0") }

  init?(row: [String: Any]) {
    guard let provider = row["provider"] as? String, !provider.isEmpty,
      let session = row["provider_session_id"] as? String, !session.isEmpty,
      let lease = row["lease_id"] as? String, lease.count == 32,
      lease.allSatisfy({ "0123456789abcdef".contains($0) })
    else { return nil }
    self.provider = provider
    providerSessionID = session
    leaseID = lease
    name =
      row["name"] as? String ?? row["agent"] as? String ?? row["audience"] as? String ?? provider
    channel = row["channel"] as? String ?? ""
  }

  init?(lease: [String: Any], channel: String, name: String) {
    var row = lease
    row["channel"] = channel
    row["name"] = name
    self.init(row: row)
  }
}

struct OverlayConversationRecipient: Codable, Equatable, Sendable {
  let owner: OverlayConversationOwner
  var deliveryID: String?
  /// The copy is in the owner's mailbox: the lease's `pending`, or the
  /// follower's `delivered` receipt.
  var queued: Bool
  var accepted: Bool
  /// The owner's `read` receipt for this delivery, the agent's acknowledgment.
  var acknowledged: Bool
  /// The owner's follower recorded this channel-0 delivery as skipped
  /// (`<delivery_id>.skipped.json`): it was off channel 0. Final.
  var broadcastSkipped: Bool? = nil

  var reach: OverlayConversationReach { .init(name: owner.name, channel: owner.channel) }
}

/// One agent as a publisher's admitted list names it: by name and channel
/// only, the way `sent_to` is written (`docs/TRANSCRIPT_BUS.md`, Receipts).
struct OverlayConversationReach: Codable, Equatable, Hashable, Sendable {
  let name: String
  let channel: String
}

/// What the "Read X/Y" footer shows for one message: `read` is X, the
/// denominator is Y by the contract's formula, and `unreceipted` lists the
/// admitted agents with no copy and no receipt at all (`no_receipt`).
struct OverlayReadReceipt: Equatable, Sendable {
  let read: Int
  let denominator: Int
  let unreceipted: [OverlayConversationReach]
}

struct OverlayReplyPlayback: Codable, Equatable, Sendable {
  let replyID: String
  let ticket: String
  let state: String
  let reason: String?
  let spoken: Bool
  let emittedAt: String
}

/// User-selected parent, separate from the agent-to-user `reply_to` delivery link.
struct OverlayUserReplyReference: Codable, Equatable, Sendable {
  let messageID: String
  let author: String
  let text: String

  enum CodingKeys: String, CodingKey {
    case messageID = "message_id"
    case author, text
  }

  init(message: OverlayConversationMessage) {
    messageID = message.id
    let fallbackAuthor =
      message.kind == .user ? String(localized: "You") : String(localized: "Agent")
    let rawAuthor =
      message.kind == .user ? fallbackAuthor : (message.owner?.name ?? fallbackAuthor)
    let sanitizedAuthor = Self.stripControlScalars(from: rawAuthor)
    let normalizedAuthor =
      sanitizedAuthor.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
      ? fallbackAuthor : sanitizedAuthor
    let boundedAuthor = Self.scalarBoundedPrefix(of: normalizedAuthor, maxScalars: 128)
    author =
      boundedAuthor.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
      ? fallbackAuthor : boundedAuthor
    let body = message.bodyText
    let context =
      body.isEmpty
      ? (message.attachments ?? []).map(\.name).joined(separator: ", ") : body
    text = Self.scalarBoundedPrefix(of: context, maxScalars: 512)
  }

  init?(row: [String: Any]) {
    guard let id = row["message_id"] as? String, !id.isEmpty, id.utf8.count <= 512,
      !id.unicodeScalars.contains(where: { $0.value < 32 || $0.value == 127 }),
      let author = row["author"] as? String,
      !author.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
      author.unicodeScalars.count <= 128,
      !author.unicodeScalars.contains(where: { $0.value < 32 || $0.value == 127 }),
      let text = row["text"] as? String, text.unicodeScalars.count <= 512
    else { return nil }
    self.messageID = id
    self.author = author
    self.text = text
  }

  /// Truncate on whole extended grapheme clusters while keeping Unicode scalar
  /// count within `bus-demux.py` `validate_user_reply_reference` limits.
  private static func scalarBoundedPrefix(of value: String, maxScalars: Int) -> String {
    guard maxScalars > 0 else { return "" }
    if value.unicodeScalars.count <= maxScalars { return value }
    var usedScalars = 0
    var end = value.startIndex
    for character in value {
      let count = character.unicodeScalars.count
      if usedScalars + count > maxScalars { break }
      usedScalars += count
      end = value.index(after: end)
    }
    return String(value[..<end])
  }

  private static func stripControlScalars(from value: String) -> String {
    let filtered = value.unicodeScalars.filter { $0.value >= 32 && $0.value != 127 }
    return String(String.UnicodeScalarView(filtered))
  }
}

struct OverlayConversationMessage: Codable, Equatable, Identifiable, Sendable {
  enum Kind: String, Codable, Sendable { case user, reply }
  enum CaptureState: String, Codable, Sendable { case receiving, finished }
  let id: String
  let kind: Kind
  var text: String
  let order: UInt64
  let emittedAt: String
  let owner: OverlayConversationOwner?
  var recipients: [OverlayConversationRecipient]
  let deliveryID: String?
  let replyTo: String?
  let unsolicited: Bool
  var playback: OverlayReplyPlayback?
  var busPath: String
  var replyToOccurrenceID: String? = nil
  var occurrenceIDs: [String]? = nil
  var sourceChannel: String? = nil
  var sourceRevision: UInt64? = nil
  /// Who a channel-0 text was sent to, frozen by its publisher: the copy's
  /// `target.recipients` stamp, outranked by the publisher's `sent.json`.
  /// Nil means the reach is unknown (an older row, or a channel take whose
  /// admitted owners are its `recipients`), never "everyone".
  var sentTo: [OverlayConversationReach]? = nil
  /// This message's receipt files as last composed
  /// (`OverlayReceiptState.compose`); nil until the reader listed them.
  var receipt: OverlayMessageReceipt? = nil
  /// File pointers the typed message carried; the bus never carries the bytes.
  var attachments: [OverlayMessageAttachment]? = nil
  var userReplyTo: OverlayUserReplyReference? = nil
  /// Capture lifecycle comes from bus terminals, independently of recipient ACKs.
  var captureState: CaptureState? = nil
  var showsDeliveryReceipts: Bool { captureState != .receiving }
  var holdUntil: Date? = nil
  var controlIdentity: String? = nil
  var captureSessionID: String? = nil
  var captureAudience: String? = nil
  var captureSampleRateHz: Double? = nil
  var captureAttachmentSegments: [OverlayCaptureAttachmentSegment]? = nil
  var activeCaptureAttachmentOccurrenceID: String? = nil
  var activeCaptureAttachmentSourceSequence: UInt64? = nil
  var activeCaptureAttachmentReducerRevision: UInt64? = nil
  var deliverySealed: Bool? = nil
  var revisedText: String? = nil
  var withdrawn: Bool? = nil
  var holdReleased: Bool? = nil
  /// Paint derived by the existing bus poll, not a local delivery queue.
  var holdSecondsRemaining: Int? = nil

  /// Exact live evidence the attachment action will ask the bus to revalidate.
  /// The target carries capture-time recipients and segment coordinates; callers
  /// must not reconstruct it from the latest transcript or current bindings.
  var activeCaptureAttachmentTarget: OverlayCaptureAttachmentTarget? {
    guard kind == .user, sourceChannel != nil, captureState == .receiving,
      deliverySealed != true, withdrawn != true,
      let captureSessionID, !captureSessionID.isEmpty,
      let activeCaptureAttachmentOccurrenceID,
      let activeCaptureAttachmentSourceSequence,
      let activeCaptureAttachmentReducerRevision,
      let segment = captureAttachmentSegments?.first(where: {
        $0.occurrenceID == activeCaptureAttachmentOccurrenceID
          && $0.sourceSequence == activeCaptureAttachmentSourceSequence
          && $0.reducerRevision == activeCaptureAttachmentReducerRevision
      }),
      !segment.recipientIDs.isEmpty, !busPath.isEmpty
    else { return nil }
    return OverlayCaptureAttachmentTarget(
      captureSessionID: captureSessionID,
      captureEpoch: segment.captureEpoch,
      sourceSequence: segment.sourceSequence,
      occurrenceID: segment.occurrenceID,
      occurrenceSessionID: segment.occurrenceSessionID,
      sampleStart: segment.sampleStart,
      sampleEnd: segment.sampleEnd,
      documentIndex: segment.documentIndex,
      audience: segment.audience,
      recipientIDs: segment.recipientIDs,
      busPath: busPath)
  }

  func holdingSeconds(at now: Date) -> Int? {
    guard kind == .user, deliverySealed == true, showsDeliveryReceipts,
      withdrawn != true, holdReleased != true, let holdUntil,
      !recipients.contains(where: { $0.queued || $0.accepted || $0.acknowledged })
    else { return nil }
    let remaining = holdUntil.timeIntervalSince(now)
    return remaining > 0 ? Int(ceil(min(remaining, Double(Int32.max)))) : nil
  }

  func deliveryStage(at now: Date) -> OverlayChannelDelivery.Stage? {
    guard kind == .user, showsDeliveryReceipts, withdrawn != true else { return nil }
    if holdingSeconds(at: now) != nil { return .held }
    if recipients.contains(where: \.acknowledged) { return .received }
    if recipients.contains(where: { $0.queued || $0.accepted }) { return .queued }
    return .sent
  }

  func projectingDelivery(at now: Date) -> Self {
    var result = self
    result.holdSecondsRemaining = holdingSeconds(at: now)
    return result
  }

  /// The contract's count (`receipt_state` in bus-demux.py, "One contract,
  /// two implementations" in `docs/TRANSCRIPT_BUS.md`) over the composed
  /// receipt files alone: the admitted list plus every agent delivered to or
  /// read outside it, minus every agent whose copy was skipped. Until a list
  /// is known (`sent.json`, the files' stamp, the copy's stamp) the copies
  /// themselves are the list, today's denominator. A listed agent with no
  /// copy and no file is `unreceipted`; a copy without a file keeps its row.
  var readReceipt: OverlayReadReceipt {
    let summary = (receipt ?? OverlayMessageReceipt(messageID: "")).summary(
      listed: sentTo ?? recipients.map(\.reach))
    let present = Set(recipients.map(\.reach))
    return OverlayReadReceipt(
      read: summary.read, denominator: summary.denominator,
      unreceipted: summary.recipients.compactMap { recipient -> OverlayConversationReach? in
        recipient.state == .noReceipt ? recipient.reach : nil
      }.filter { !present.contains($0) })
  }
  /// An agent-authored row can resolve a voice when Play is requested.
  var supportsSpeechPlayback = false
  /// Durable row coordinates let historical replies play after their lease ends.
  var provider: String? = nil
  var providerSessionID: String? = nil
  var playbackProvider: String? { provider ?? owner?.provider }
  var playbackProviderSessionID: String? { providerSessionID ?? owner?.providerSessionID }
  var replyID: String? { kind == .reply ? String(id.dropFirst("reply:".count)) : nil }
  /// The bubble body. `cs-bus --send-text` appends one generated
  /// `[attachment] name (type, bytes): /path` line per pointer to the published
  /// text for agents; the bubble shows the pointers as chips and hides exactly
  /// those generated lines. Anything the user typed, including a sentence that
  /// starts with `[attachment]`, stays. The full text remains for copying.
  var bodyText: String {
    guard let attachments, !attachments.isEmpty else { return text }
    let generated = Set(attachments.map(\.publishedLine))
    var lines = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
    lines.removeAll { generated.contains($0) }
    while lines.last?.isEmpty == true { lines.removeLast() }
    return lines.joined(separator: "\n")
  }
}

struct OverlayCaptureAttachmentSegment: Codable, Equatable, Sendable {
  let occurrenceID: String
  let occurrenceSessionID: String
  let captureEpoch: UInt64
  let sampleStart: UInt64
  let sampleEnd: UInt64
  let sourceSequence: UInt64
  let reducerRevision: UInt64
  let documentIndex: String
  let audience: String
  let recipientIDs: [String]
  var sampleRateHz: Double? = nil
}

struct OverlayCaptureAttachmentSession: Equatable, Hashable, Sendable {
  let id: String
  let channel: String
  let openedAt: String
  let recipientIDs: [String]

  func includes(_ conversation: OverlayConversation) -> Bool {
    if conversation.channel == "0" { return channel == "0" && conversation.owner == nil }
    return conversation.owner.map { recipientIDs.contains($0.id) } == true
  }
}

struct OverlayCaptureAttachmentTarget: Equatable, Sendable {
  let captureSessionID: String
  let captureEpoch: UInt64
  let sourceSequence: UInt64
  let occurrenceID: String
  let occurrenceSessionID: String
  let sampleStart: UInt64
  let sampleEnd: UInt64
  let documentIndex: String
  let audience: String
  let recipientIDs: [String]
  let busPath: String
}

private struct OverlayCaptureAttachmentReceipt: Codable, Equatable, Sendable {
  let stageID: String
  let captureSessionID: String
  let captureEpoch: UInt64
  let sourceSequence: UInt64
  let occurrenceSessionID: String
  let sampleStart: UInt64
  let sampleEnd: UInt64
  let documentIndex: UInt64
  let reducerRevision: UInt64
  let audience: String
  let recipientIDs: [String]
  let recipientBusPaths: [String]
  let commitBus: String
  let attachments: [OverlayMessageAttachment]

  let occurrenceID: String
}

private struct OverlayCaptureAttachmentCommit: Codable, Equatable, Sendable {
  let stageID: String
  let commitBus: String
}

/// One file pointer of a typed message (`attachments` on
/// `codescribe.agent-user-message.v1`): name, absolute path, media type, size.
struct OverlayMessageAttachment: Codable, Equatable, Hashable, Sendable {
  let name: String
  let path: String
  let mediaType: String
  let bytes: Int
  var attachedAtSample: UInt64? = nil
  var attachedAtSeconds: Double? = nil

  var attachmentTimeLabel: String? {
    guard let attachedAtSeconds, attachedAtSeconds.isFinite, attachedAtSeconds >= 0 else {
      return nil
    }
    let seconds = Int(min(attachedAtSeconds, Double(Int32.max)).rounded(.down))
    return "\(seconds / 60):" + String(format: "%02d", seconds % 60)
  }

  /// The pointer line published to the agent, byte for byte.
  var publishedLine: String {
    let marker = attachmentTimeLabel.map { "[attachment @ \($0)]" } ?? "[attachment]"
    return "\(marker) \(name) (\(mediaType), \(bytes) bytes): \(path)"
  }

  static func pointers(_ value: Any?) -> [OverlayMessageAttachment]? {
    guard let entries = value as? [[String: Any]] else { return nil }
    let pointers = entries.compactMap { entry -> OverlayMessageAttachment? in
      guard let path = entry["path"] as? String, path.hasPrefix("/"),
        !path.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
      else { return nil }
      let name = entry["name"] as? String
      return OverlayMessageAttachment(
        name: name?.isEmpty == false ? name! : (path as NSString).lastPathComponent,
        path: path, mediaType: entry["media_type"] as? String ?? "",
        bytes: (entry["bytes"] as? NSNumber)?.intValue ?? 0,
        attachedAtSample: (entry["attached_at_sample"] as? NSNumber).flatMap {
          UInt64($0.stringValue)
        },
        attachedAtSeconds: (entry["attached_at_seconds"] as? NSNumber).flatMap {
          $0.doubleValue.isFinite && $0.doubleValue >= 0 ? $0.doubleValue : nil
        })
    }
    return pointers.isEmpty ? nil : pointers
  }
}

struct OverlayConversation: Equatable, Identifiable, Sendable {
  let id: String
  let channel: String
  let name: String
  let owner: OverlayConversationOwner?
  let messages: [OverlayConversationMessage]
  /// Archive metadata can advertise an owner before its bus has been read.
  /// An unopened history is not a verified empty conversation.
  var historyLoaded = true

  /// The human mailbox has no agent lease, microphone or numeric shortcut.
  /// Its internal route remains channel 10; only the presentation is named.
  var isHumanInbox: Bool { id == "10" && channel == "10" && owner == nil }
  var displayTitle: String {
    isHumanInbox || channel.isEmpty ? name : "\(channel) · \(name)"
  }

  static func humanInbox(messages: [OverlayConversationMessage] = []) -> Self {
    Self(
      id: "10", channel: "10",
      name: String(
        localized: "Your Inbox",
        comment: "The human mailbox beside agent conversations; no channel number is displayed"),
      owner: nil, messages: messages)
  }

  var replyIDs: [String] { messages.filter { $0.kind == .reply }.map(\.id) }
  func projectingDelivery(at now: Date) -> Self {
    var result = Self(
      id: id, channel: channel, name: name, owner: owner,
      messages: messages.map { $0.projectingDelivery(at: now) })
    result.historyLoaded = historyLoaded
    return result
  }
  /// One of this conversation's replies is coming out of the speaker now.
  /// Synthesis and the wait for the shared player are not speech yet; the
  /// player serializes agents, so at most one conversation speaks at a time.
  var replySpeaking: Bool {
    messages.contains { $0.kind == .reply && $0.playback?.state == "playing" }
  }
}

struct OverlayPanelFocusResult: Equatable, Sendable, Codable {
  let requestID: String
  let provider: String
  let providerSessionID: String
  let leaseID: String
  let status: String
  let exitCode: Int?
  let elapsedMs: Int?
  let emittedAt: String
  let frame: AgentFrameBinding?
  /// `frame`, `iterm2` or `terminal`: where the follower aimed. Nil before a
  /// target was known, and from followers older than terminal focus.
  let target: String?
  let terminal: AgentTerminalBinding?

  var isSuccess: Bool {
    status == "focused" || status == "already_focused"
  }

  init(
    requestID: String,
    provider: String,
    providerSessionID: String,
    leaseID: String,
    status: String,
    exitCode: Int? = nil,
    elapsedMs: Int? = nil,
    emittedAt: String,
    frame: AgentFrameBinding? = nil,
    target: String? = nil,
    terminal: AgentTerminalBinding? = nil
  ) {
    self.requestID = requestID
    self.provider = provider
    self.providerSessionID = providerSessionID
    self.leaseID = leaseID
    self.status = status
    self.exitCode = exitCode
    self.elapsedMs = elapsedMs
    self.emittedAt = emittedAt
    self.frame = frame
    self.target = target
    self.terminal = terminal
  }

  init?(row: [String: Any]) {
    guard row["schema"] as? String == "codescribe.agent-panel-focus-result.v1",
      row["kind"] as? String == "agent_panel_focus_result",
      let requestID = row["request_id"] as? String,
      requestID.count == 24,
      requestID.allSatisfy({ "0123456789abcdef".contains($0) }),
      let provider = row["provider"] as? String, !provider.isEmpty,
      let providerSessionID = row["provider_session_id"] as? String, !providerSessionID.isEmpty,
      let leaseID = row["lease_id"] as? String,
      leaseID.count == 32,
      leaseID.allSatisfy({ "0123456789abcdef".contains($0) }),
      let status = row["status"] as? String,
      [
        "focused", "already_focused", "pane_missing", "terminal_missing", "automation_denied",
        "no_frame", "stale", "error",
      ].contains(status),
      let emittedAt = row["emitted_at"] as? String, !emittedAt.isEmpty
    else { return nil }
    self.requestID = requestID
    self.provider = provider
    self.providerSessionID = providerSessionID
    self.leaseID = leaseID
    self.status = status
    self.exitCode = (row["exit_code"] as? NSNumber)?.intValue
    self.elapsedMs = (row["elapsed_ms"] as? NSNumber)?.intValue
    self.emittedAt = emittedAt
    self.frame = (row["frame"] as? [String: Any]).flatMap(AgentFrameBinding.init(row:))
    self.target = (row["target"] as? String).flatMap {
      ["frame", "iterm2", "terminal"].contains($0) ? $0 : nil
    }
    self.terminal = (row["terminal"] as? [String: Any]).flatMap(AgentTerminalBinding.init(row:))
  }
}

struct OverlayChannelDeliverySnapshot: Equatable, Sendable {
  let deliveries: [OverlayChannelDelivery]
  let conversations: [OverlayConversation]
  var activeCaptureAttachmentSessions: [OverlayCaptureAttachmentSession] = []
  var archivedOwners: Set<OverlayConversationOwner> = []
  /// First-load archive replies are history, including their broadcast mirrors.
  var historyReplyIDs: Set<String> = []
  var panelFocusResults: [String: OverlayPanelFocusResult] = [:]
}

/// One delivery as the receipt files describe it (`recipients[]` of
/// `receipt_state`): the farthest file wins, `read` over `delivered`,
/// `skipped` on its own; or an agent on the admitted list with no file at
/// all (`no_receipt`, no delivery id).
struct OverlayReceiptRecipient: Codable, Equatable, Sendable {
  let deliveryID: String?
  let name: String?
  let channel: String?
  let state: OverlayReceiptState.State
  let wake: String?
  let deliveredAt: String?
  let at: String?

  var reach: OverlayConversationReach? {
    guard let name, let channel else { return nil }
    return OverlayConversationReach(name: name, channel: channel)
  }

  /// One agent by name and channel, however many deliveries reached it; a
  /// nameless file counts by its delivery.
  var identity: String {
    if let name { return name + "\u{0}" + (channel ?? "") }
    return (deliveryID ?? "") + "\u{0}\u{0}"
  }
}

/// One `playback.<ticket>.json`: the first terminal state of one playback of
/// a reply, by the player named in the file.
struct OverlayReceiptPlayback: Codable, Equatable, Sendable {
  let ticket: String
  let state: String
  let reason: String?
  let spoken: Bool
  let at: String?
  let owner: OverlayConversationOwner?
}

struct OverlayReceiptWithdrawal: Codable, Equatable, Sendable {
  let deliveryID: String
  let controlID: String?
  let at: String?
}

/// `summary(listed:)` of a composed receipt: the recipients with their
/// `no_receipt` entries, X and Y.
struct OverlayReceiptSummary: Equatable, Sendable {
  let recipients: [OverlayReceiptRecipient]
  let read: Int
  let denominator: Int
}

/// The receipt files of one message directory, composed (`receipt_state` in
/// bus-demux.py): what the files say before any list fallback. `sentTo` is
/// the publisher's `sent.json`, else the stamp the recipients' files carried;
/// `deliveries` has one entry per `delivery_id`, sorted by time; `spoken` is
/// the playback that completed, else the latest terminal one.
struct OverlayMessageReceipt: Codable, Equatable, Sendable {
  let messageID: String
  var sentTo: [OverlayConversationReach]? = nil
  var deliveries: [OverlayReceiptRecipient] = []
  var withdrawn: [OverlayReceiptWithdrawal] = []
  var playbacks: [OverlayReceiptPlayback] = []

  var spoken: OverlayReceiptPlayback? { playbacks.first(where: \.spoken) ?? playbacks.last }

  /// The contract's recipients and count over a list: `sentTo` when the files
  /// carry one, else `fallback` (the helper has none; the overlay passes the
  /// copies it sees). Every listed agent without a file is a `no_receipt`
  /// entry and counts; a skipped one is shown, never counted.
  func summary(listed fallback: [OverlayConversationReach]) -> OverlayReceiptSummary {
    func identity(_ reach: OverlayConversationReach) -> String {
      reach.name + "\u{0}" + reach.channel
    }
    let listed = (sentTo ?? fallback).map(identity)
    let reached = deliveries.filter { $0.state == .delivered || $0.state == .read }
      .map(\.identity)
    let skipped = deliveries.filter { $0.state == .skipped }.map(\.identity)
    var recipients = deliveries
    for reach in sentTo ?? fallback {
      let key = identity(reach)
      guard !reached.contains(key), !deliveries.contains(where: { $0.identity == key }) else {
        continue
      }
      recipients.append(
        OverlayReceiptRecipient(
          deliveryID: nil, name: reach.name, channel: reach.channel, state: .noReceipt,
          wake: nil, deliveredAt: nil, at: nil))
    }
    return OverlayReceiptSummary(
      recipients: recipients,
      read: Set(deliveries.filter { $0.state == .read }.map(\.identity)).count,
      denominator: OverlayReceiptState.denominator(
        listed: listed, reached: reached, skipped: skipped))
  }

  /// The helper's JSON line (`cs-bus --receipts`) for this directory, with
  /// `NSNull` for null, so a model directory's `expected.json` compares
  /// exactly. The helper knows no copies, so its list fallback is empty.
  func jsonObject(listed fallback: [OverlayConversationReach] = []) -> [String: Any] {
    func json(_ value: Any?) -> Any { value ?? NSNull() }
    let summary = summary(listed: fallback)
    return [
      "message_id": messageID,
      "sent_to": json(sentTo.map { $0.map { ["name": $0.name, "channel": $0.channel] } }),
      "recipients": summary.recipients.map { recipient -> [String: Any] in
        [
          "delivery_id": json(recipient.deliveryID), "name": json(recipient.name),
          "channel": json(recipient.channel), "state": recipient.state.rawValue,
          "wake": json(recipient.wake), "delivered_at": json(recipient.deliveredAt),
          "at": json(recipient.at),
        ]
      },
      "denominator": summary.denominator,
      "withdrawn": withdrawn.map { withdrawal -> [String: Any] in
        [
          "delivery_id": withdrawal.deliveryID, "control_id": json(withdrawal.controlID),
          "at": json(withdrawal.at),
        ]
      },
      "spoken": json(
        spoken.map { playback -> [String: Any] in
          [
            "state": playback.state, "reason": json(playback.reason), "spoken": playback.spoken,
            "at": json(playback.at), "playback_ticket": playback.ticket,
          ]
        }),
    ]
  }
}

/// The receipts contract as the overlay computes it (`docs/TRANSCRIPT_BUS.md`,
/// "Receipts" and "One contract, two implementations"): one file per fact
/// under `runtime/receipts/<message_id>/`, a delivery's state is its farthest
/// file (`read` over `delivered`, `skipped` on its own), and the denominator
/// is one formula. The helper's `receipt_state` is the other implementation;
/// both are pinned to the model directories under
/// `scripts/tests/fixtures/receipts/<case>/` by their own test suites. The
/// composer here is the overlay's one implementation: the reader hands it
/// the files of a message directory and the projection keeps what it
/// returns. The overlay computes only this: no roster, no policies, no reach
/// of its own.
enum OverlayReceiptState {
  static let schema = "codescribe.message-receipt.v1"

  enum State: String, Codable, Equatable, Sendable {
    case delivered, read, skipped
    case noReceipt = "no_receipt"
  }

  /// The admitted list a channel-0 text copy carries (`broadcast_reach` in
  /// bus-demux.py): `target.recipients`, frozen by the publisher. Nil without
  /// the stamp: the reach is unknown, never "everyone".
  static func stampedReach(_ row: [String: Any]) -> [OverlayConversationReach]? {
    guard let target = row["target"] as? [String: Any],
      target["mode"] as? String == "broadcast",
      let rows = target["recipients"] as? [[String: Any]]
    else { return nil }
    return reach(rows)
  }

  static func reach(_ rows: [[String: Any]]) -> [OverlayConversationReach] {
    rows.compactMap { row in
      guard let name = row["name"] as? String, let channel = row["channel"] as? String else {
        return nil
      }
      return OverlayConversationReach(name: name, channel: channel)
    }
  }

  /// The one count: everyone on the admitted list, plus anyone delivered to
  /// or read outside it, minus everyone whose copy was skipped. An agent is
  /// one entry by name and channel, however many deliveries reached it; a
  /// member of the list whose copy was skipped leaves the count (the skip
  /// wins), and a member without any file stays in it (`no_receipt`).
  static func denominator<Agent: Hashable>(listed: [Agent], reached: [Agent], skipped: [Agent])
    -> Int
  {
    Set(listed).union(reached).subtracting(skipped).count
  }

  /// `receipt_state(root, message_id)` of bus-demux.py over the files of one
  /// message directory. `files` are the directory's entries by name; a name
  /// starting with `.` (a write in progress), a name and content that
  /// disagree, a foreign schema and a foreign message id are ignored, never
  /// repaired.
  static func compose(files: [(name: String, row: [String: Any])], messageID: String)
    -> OverlayMessageReceipt
  {
    var rows: [String: [State: [String: Any]]] = [:]
    var order: [String] = []
    var result = OverlayMessageReceipt(messageID: messageID)
    var stamped: [OverlayConversationReach]? = nil
    var published: [OverlayConversationReach]? = nil
    for (name, row) in files.sorted(by: { $0.name < $1.name }) {
      guard !name.hasPrefix("."), name.hasSuffix(".json"),
        row["schema"] as? String == schema, row["message_id"] as? String == messageID,
        let kind = row["kind"] as? String
      else { continue }
      let stem = String(name.dropLast(".json".count))
      let delivery = row["delivery_id"] as? String
      if let state = State(rawValue: kind), state != .noReceipt, let delivery,
        stem == delivery + "." + kind
      {
        if rows[delivery] == nil { order.append(delivery) }
        rows[delivery, default: [:]][state] = row
        if stamped == nil, let list = row["sent_to"] as? [[String: Any]] {
          stamped = reach(list)
        }
      } else if kind == "sent", stem == "sent", let list = row["sent_to"] as? [[String: Any]] {
        published = reach(list)
      } else if kind == "withdrawn", let delivery, stem == "withdrawn." + delivery {
        result.withdrawn.append(
          OverlayReceiptWithdrawal(
            deliveryID: delivery, controlID: row["control_id"] as? String,
            at: row["at"] as? String))
      } else if kind == "playback", let ticket = row["playback_ticket"] as? String,
        stem == "playback." + ticket, let state = row["state"] as? String
      {
        result.playbacks.append(
          OverlayReceiptPlayback(
            ticket: ticket, state: state, reason: row["reason"] as? String,
            spoken: row["spoken"] as? Bool == true, at: row["at"] as? String,
            owner: OverlayConversationOwner(row: row)))
      }
    }
    for delivery in order {
      let kinds = rows[delivery] ?? [:]
      let state: State =
        kinds[.read] != nil ? .read : kinds[.delivered] != nil ? .delivered : .skipped
      guard let row = kinds[state] else { continue }
      let delivered = kinds[.delivered]
      // Name and channel from the farthest file, completed from the others:
      // a reader that acknowledged under a lease bound later may not have
      // known its digit when it wrote `read`.
      let others = [State.delivered, .read, .skipped].compactMap { kinds[$0] }
      result.deliveries.append(
        OverlayReceiptRecipient(
          deliveryID: delivery,
          name: row["name"] as? String ?? others.compactMap({ $0["name"] as? String }).first,
          channel: row["channel"] as? String
            ?? others.compactMap({ $0["channel"] as? String }).first,
          state: state, wake: delivered?["wake"] as? String,
          deliveredAt: delivered?["at"] as? String,
          at: (state == .read ? row["read_at"] : row["at"]) as? String))
    }
    result.deliveries.sort {
      ($0.at ?? "", $0.deliveryID ?? "") < ($1.at ?? "", $1.deliveryID ?? "")
    }
    result.withdrawn.sort { ($0.at ?? "", $0.deliveryID) < ($1.at ?? "", $1.deliveryID) }
    result.playbacks.sort { ($0.at ?? "", $0.ticket) < ($1.at ?? "", $1.ticket) }
    result.sentTo = published ?? stamped
    return result
  }
}
