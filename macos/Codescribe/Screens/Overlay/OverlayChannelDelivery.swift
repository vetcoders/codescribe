import CryptoKit
import Foundation

/// Read-only delivery evidence. Receipt means the agent acknowledged the envelope,
/// not that it executed the request. No transcript text participates in identity.
struct OverlayChannelDelivery: Equatable, Identifiable, Sendable {
  enum Stage: String, Codable, Sendable { case sent, queued, received }

  let channel: String
  let agent: String
  let deliveryID: String?
  let stage: Stage?
  let isOpen: Bool
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
    private var receiptCursor: Int = 0
    private var messageRevisions: [String: UInt64] = [:]
    private var deliveryOrigins: [String: DeliveryOrigin] = [:]
    private var sessionOpenedAt: [String: String] = [:]
    private struct DeliveryOrigin: Codable {
      let captureSession: String
      let channel: String
      let documentIndex: String
      let audience: String
      let terminal: Bool
    }
    private enum CodingKeys: String, CodingKey {
      case seals, sessions, endedSessions, receipts, sealOrder, seenSeals, messages, messageOrder
      case captureRecipients, historicalOwners, playbackByReply, retiredPlaybackTickets, messageSequence, messageRevisions, deliveryOrigins, revision, sessionOpenedAt
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
      if schema == "codescribe.agent-ack.v1", row["kind"] as? String == "agent_ack",
        let id = row["delivery_id"] as? String,
        let channel = row["channel"] as? String, let agent = row["agent"] as? String
      {
        receipts[id] = Receipt(channel: channel, agent: agent)
        return
      }
      if schema == "codescribe.channel-session.v1", row["kind"] as? String == "channel_session",
        let channel = row["channel"] as? String,
        let provider = row["provider"] as? String,
        let providerSession = row["provider_session_id"] as? String,
        let sessionID = row["session_id"] as? String,
        let openedAt = row["opened_at"] as? String,
        let state = row["state"] as? String
      {
        // A delayed close from an earlier take cannot close its successor.
        if state != "open", let current = sessions[channel], current.openedAt != openedAt {
          return
        }
        guard state == "open" || state == "sealed" else { return }
        if state == "open" && !admitsOpening(channel: channel, session: sessionID, openedAt: openedAt) {
          return
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
      let owners = (row["recipients"] as? [[String: Any]])?
        .compactMap(OverlayConversationOwner.init(row:)) ?? captureRecipients[sessionID] ?? []
      seals[audience] = Seal(phaseID: phaseID, audience: audience, order: sealOrder,
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
      let deliveryID = seal.map {
        Self.identity(["native_bus_demux", leaseID, $0.phaseID, "seal", Self.routedAudience($0.audience)])
      }
      var stage: Stage? = deliveryID == nil ? nil : .sent
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
      }
      return OverlayChannelDelivery(
        channel: channel, agent: agent, deliveryID: deliveryID, stage: stage, isOpen: isOpen)
    }

    mutating func observeLease(_ lease: [String: Any], channel: String, binding: [String: Any]) {
      guard let owner = OverlayConversationOwner(lease: lease, channel: channel,
        name: binding["audience"] as? String ?? ""),
        lease["schema"] as? String == "codescribe.agent-bridge.lease.v1",
        owner.provider == binding["provider"] as? String,
        owner.providerSessionID == binding["provider_session_id"] as? String
      else { return }
      if historicalOwners[owner.id] == nil || historicalOwners[owner.id]?.channel.isEmpty == true {
        // Completing missing roster metadata leaves each stored message's owner label intact.
        historicalOwners[owner.id] = owner
        revision &+= 1
      }
      let pending = lease["pending"] as? [[String: Any]] ?? []
      for envelope in pending {
        guard let delivery = envelope["delivery_id"] as? String,
          envelope["provider"] as? String == owner.provider,
          envelope["provider_session_id"] as? String == owner.providerSessionID,
          envelope["lease_id"] as? String == owner.leaseID,
          envelope["bus"] as? String == lease["bus"] as? String else { continue }
        associateEnvelope(envelope, owner: owner, delivery: delivery)
        updateRecipient(delivery: delivery, owner: owner, queued: true, accepted: false,
          acknowledged: false)
      }
      pruneHistory()
    }

    mutating func observeAcceptance(_ receipt: [String: Any]) {
      guard receipt["schema"] as? String == "codescribe.native-queue.receipt.v1",
        receipt["disposition"] as? String == "provider_accepted",
        let delivery = receipt["delivery_id"] as? String,
        let owner = OverlayConversationOwner(row: receipt)
      else { return }
      updateRecipient(delivery: delivery, owner: owner, queued: false, accepted: true,
        acknowledged: false)
    }

    mutating func receiptCoordinates() -> [(OverlayConversationOwner, String)] {
      var result: [(OverlayConversationOwner, String)] = []
      var seen: Set<String> = []
      for id in messageOrder {
        guard let message = messages[id] else { continue }
        for recipient in message.recipients {
          if let delivery = recipient.deliveryID, !recipient.accepted || !recipient.acknowledged,
            seen.insert(recipient.owner.id + "\0" + delivery).inserted {
            result.append((recipient.owner, delivery))
          }
        }
      }
      guard !result.isEmpty else { receiptCursor = 0; return [] }
      let start = receiptCursor % result.count
      let count = min(result.count, 128)
      receiptCursor = (start + count) % result.count
      return (0..<count).map { result[(start + $0) % result.count] }
    }

    func conversations(busPath: String) -> [OverlayConversation] {
      let ordered = messageOrder.compactMap { messages[$0] }.map { message in
        var result = message
        result.busPath = busPath
        return result
      }
      var result = [OverlayConversation(id: "0", channel: "0", name: "All", owner: nil,
        messages: ordered)]
      for owner in historicalOwners.values.sorted(by: { $0.id < $1.id }) {
        let rows = ordered.filter { row in
          row.owner?.id == owner.id || row.recipients.contains { $0.owner.id == owner.id }
        }
        result.append(OverlayConversation(id: owner.id, channel: owner.channel,
          name: owner.name, owner: owner, messages: rows))
      }
      return result
    }

    mutating func observeAcknowledgment(_ receipt: [String: Any], owner: OverlayConversationOwner,
      delivery: String, busPath: String) {
      guard receipt["lease_id"] as? String == owner.leaseID,
        receipt["delivery_id"] as? String == delivery,
        receipt["bus"] as? String == busPath,
        let envelope = receipt["envelope"] as? [String: Any],
        envelope["delivery_id"] as? String == delivery,
        envelope["lease_id"] as? String == owner.leaseID,
        envelope["provider"] as? String == owner.provider,
        envelope["provider_session_id"] as? String == owner.providerSessionID
      else { return }
      associateEnvelope(envelope, owner: owner, delivery: delivery)
      updateRecipient(delivery: delivery, owner: owner, queued: false, accepted: false,
        acknowledged: true)
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
      messages[key] = message
    }

    private mutating func consumeConversation(_ row: [String: Any]) {
      let schema = row["schema"] as? String
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
        guard messages[key] == nil else { return }
        let recipients = owners.map { owner in
          historicalOwners[owner.id] = historicalOwners[owner.id] ?? owner
          return OverlayConversationRecipient(
            owner: owner,
            deliveryID: Self.identity([
              "native_bus_demux", owner.leaseID, identity, "message",
              Self.routedAudience(audience),
            ]), queued: false, accepted: false, acknowledged: false)
        }
        store(
          OverlayConversationMessage(
            id: key, kind: .user, text: text, order: nextOrder(),
            emittedAt: row["emitted_at"] as? String ?? "", owner: nil, recipients: recipients,
            deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: ""))
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
          {
            guard let previous = sessionOpenedAt[origin.captureSession],
              let earlier = Self.openingDate(previous), earlier < opened
            else { continue }
            finalizeRefused(session: origin.captureSession)
          }
        } else {
          // A predecessor close can settle its own receipt, never its successor's.
          finalizeRefused(session: session)
        }
      }
      if schema == "codescribe.transcript.v1" || schema == "codescribe.transcript-evidence.v1",
        row["status"] as? String == "session_ended"
          || row["reducer_action"] as? String == "session_ended",
        let session = row["session_id"] as? String
      {
        finalizeRefused(session: session)
      }
      if schema == "codescribe.channel-recipients.v1",
        let session = row["session_id"] as? String,
        let entries = row["recipients"] as? [[String: Any]]
      {
        captureRecipients[session] = entries.compactMap(OverlayConversationOwner.init(row:))
        for owner in captureRecipients[session] ?? [] { historicalOwners[owner.id] = owner }
        return
      }
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
        let state = row["state"] as? String,
        let message = messages["reply:" + replyID], message.owner?.id == owner.id
      {
        let playback = OverlayReplyPlayback(
          replyID: replyID, ticket: ticket, state: state,
          reason: row["reason"] as? String ?? row["tts_error"] as? String,
          spoken: row["spoken"] as? Bool == true,
          emittedAt: row["emitted_at"] as? String ?? "")
        if retiredPlaybackTickets[replyID]?.contains(ticket) == true { return }
        // Delayed observations cannot rewind playback or finish another ticket.
        if let current = playbackByReply[replyID] {
          if playback.emittedAt < current.emittedAt { return }
          if current.ticket == ticket, current.state != "waiting" && current.state != "playing",
            state == "waiting" || state == "playing"
          {
            return
          }
        }
        if let current = playbackByReply[replyID], current.ticket != ticket,
          state != "waiting" && state != "playing"
        {
          return
        }
        if let current = playbackByReply[replyID], current.ticket != ticket {
          var retired = retiredPlaybackTickets[replyID] ?? []
          retired.append(current.ticket)
          retiredPlaybackTickets[replyID] = Array(retired.suffix(64))
        }
        playbackByReply[replyID] = playback
        messages["reply:" + replyID]?.playback = playback
        return
      }
      if schema == "codescribe.agent-reply.v1", let replyID = row["reply_id"] as? String,
        let owner = OverlayConversationOwner(row: row), let text = row["text"] as? String
      {
        let key = "reply:" + replyID
        if let previous = messages[key], previous.owner?.id != owner.id { return }
        historicalOwners[owner.id] = historicalOwners[owner.id] ?? owner
        let addressed =
          row["association"] as? String == "addressed"
          && row["delivery_id"] is String
        let order = messages[key]?.order ?? nextOrder()
        var message = OverlayConversationMessage(
          id: key, kind: .reply,
          text: text, order: order,
          emittedAt: row["emitted_at"] as? String ?? "", owner: historicalOwners[owner.id],
          recipients: [], deliveryID: addressed ? row["delivery_id"] as? String : nil,
          replyTo: addressed ? row["delivery_id"] as? String : nil,
          unsolicited: !addressed, playback: playbackByReply[replyID], busPath: "")
        if addressed, let occurrenceSession = row["occurrence_session_id"] as? String,
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
      } else {
        owners = captureRecipients[session] ?? []
      }
      // An audience label alone cannot choose a current provider session.
      guard !owners.isEmpty else { return }
      let text =
        row["label"] as? String ?? row["text"] as? String
        ?? row["rendered_text"] as? String
      guard let text, !text.isEmpty else { return }
      let evidence = schema == "codescribe.transcript-evidence.v1"
      if !evidence && !(row["utterance_id"] is NSNumber || row["utterance_id"] is String) { return }
      if evidence
        && (row["occurrence_session_id"] as? String == nil
          || !(row["capture_epoch"] is NSNumber) || !(row["sample_start"] is NSNumber)
          || !(row["sample_end"] is NSNumber))
      {
        return
      }
      let occurrence =
        evidence
        ? Self.identity([
          "occurrence", row["occurrence_session_id"] as? String ?? session,
          Self.coordinate(row["capture_epoch"]), Self.coordinate(row["sample_start"]),
          Self.coordinate(row["sample_end"]),
        ])
        : Self.identity(["utterance", session, Self.coordinate(row["utterance_id"])])
      let key = "utterance:" + occurrence
      let revision =
        (row[evidence ? "reducer_revision" : "sequence"] as? NSNumber)?.uint64Value ?? 0
      guard revision >= (messageRevisions[key] ?? 0) else { return }
      messageRevisions[key] = revision
      let phase: String
      let terminal =
        evidence
        ? row["reducer_action"] as? String == "record_ledger_terminal_seal"
        : row["status"] as? String == "transcript_sealed"
      if evidence {
        let components = session.split(separator: "-")
        let channel =
          components.count > 2 && components[0] == "agent" && components[1] == "channel"
          ? String(components[2]) : ""
        deliveryOrigins[key] = DeliveryOrigin(
          captureSession: session, channel: channel,
          documentIndex: Self.coordinate(row["document_index"]), audience: audience,
          terminal: terminal)
      }
      if evidence && terminal {
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
        terminal
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
      store(
        OverlayConversationMessage(
          id: key, kind: .user,
          text: text, order: order,
          emittedAt: row["emitted_at"] as? String ?? "", owner: nil, recipients: recipients,
          deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: ""))
    }

    private func admitsOpening(channel: String, session: String, openedAt: String) -> Bool {
      guard !endedSessions.contains(session), let incoming = Self.openingDate(openedAt) else { return false }
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

    private mutating func finalizeRefused(session: String) {
      for (id, origin) in deliveryOrigins where origin.captureSession == session && !origin.terminal {
        guard var message = messages[id] else { continue }
        let phase = Self.identity(["coverage-refused-seal", session, origin.documentIndex])
        for index in message.recipients.indices {
          let owner = message.recipients[index].owner
          let delivery = Self.identity(["native_bus_demux", owner.leaseID, phase, "seal", Self.routedAudience(origin.audience)])
          if message.recipients[index].deliveryID != delivery {
            message.recipients[index] = OverlayConversationRecipient(owner: owner, deliveryID: delivery,
              queued: false, accepted: false, acknowledged: false)
          }
        }
        messages[id] = message
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

    private mutating func updateRecipient(delivery: String, owner: OverlayConversationOwner,
      queued: Bool, accepted: Bool, acknowledged: Bool) {
      var changed = false
      for id in messageOrder {
        guard var message = messages[id] else { continue }
        var rowChanged = false
        for index in message.recipients.indices
        where message.recipients[index].owner.id == owner.id
          && message.recipients[index].deliveryID == delivery {
          let before = message.recipients[index]
          if (queued && !before.queued) || (accepted && !before.accepted)
            || (acknowledged && !before.acknowledged) {
            message.recipients[index].queued = before.queued || queued
            message.recipients[index].accepted = before.accepted || accepted
            message.recipients[index].acknowledged = before.acknowledged || acknowledged
            rowChanged = true
          }
        }
        if rowChanged { messages[id] = message; changed = true }
      }
      if changed { revision &+= 1 }
    }

    private mutating func pruneHistory() {
      var bytes = messages.values.reduce(0) { $0 + $1.text.utf8.count }
      while messageOrder.count > Self.historyLimit || (bytes > Self.historyByteLimit && messageOrder.count > 1) {
        let id = messageOrder.removeFirst()
        bytes -= messages.removeValue(forKey: id)?.text.utf8.count ?? 0
        messageRevisions.removeValue(forKey: id)
        deliveryOrigins.removeValue(forKey: id)
      }
      let replies = Set(messages.values.compactMap { $0.playback?.replyID })
      playbackByReply = playbackByReply.filter { replies.contains($0.key) }
      retiredPlaybackTickets = retiredPlaybackTickets.filter { replies.contains($0.key) }
      let ownerIDs = Set(messages.values.flatMap { row in
        row.recipients.map { $0.owner.id } + (row.owner.map { [$0.id] } ?? [])
      })
      if historicalOwners.count > Self.historyLimit {
        historicalOwners = historicalOwners.filter { ownerIDs.contains($0.key) }
      }
      if captureRecipients.count > Self.historyLimit {
        for key in captureRecipients.keys.sorted().prefix(captureRecipients.count - Self.historyLimit) {
          captureRecipients.removeValue(forKey: key)
        }
      }
      if sessionOpenedAt.count > Self.historyLimit {
        let retained = Set(deliveryOrigins.values.map(\.captureSession) + sessions.values.map(\.sessionID))
        sessionOpenedAt = sessionOpenedAt.filter { retained.contains($0.key) }
      }
      if receipts.count > Self.historyLimit * 2 {
        let deliveryIDs = Set(messages.values.flatMap { $0.recipients.compactMap(\.deliveryID) })
        receipts = receipts.filter { deliveryIDs.contains($0.key) }
      }
      if seals.count > Self.historyLimit {
        let keep = Set(seals.values.sorted { $0.order > $1.order }.prefix(Self.historyLimit).map(\.audience))
        seals = seals.filter { keep.contains($0.key) }
      }
      if sessions.count > Self.historyLimit {
        let keep = Set(sessions.values.sorted { $0.openedAt > $1.openedAt }.prefix(Self.historyLimit).map(\.sessionID))
        sessions = sessions.filter { keep.contains($0.value.sessionID) }
      }
      if seenSeals.count > Self.historyLimit * 2 {
        seenSeals = Set(seals.values.map { $0.audience + "\0" + $0.phaseID })
      }
      if endedSessions.count > Self.historyLimit * 2 {
        let retained = Set(sessions.values.map(\.sessionID))
        endedSessions = endedSessions.intersection(retained)
      }
    }

    private static func coordinate(_ value: Any?) -> String {
      if let string = value as? String { return string }
      if let number = value as? NSNumber { return number.stringValue }
      return ""
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
      lease.allSatisfy({ "0123456789abcdef".contains($0) }) else { return nil }
    self.provider = provider
    providerSessionID = session
    leaseID = lease
    name = row["name"] as? String ?? row["agent"] as? String ?? row["audience"] as? String ?? provider
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
  var queued: Bool
  var accepted: Bool
  var acknowledged: Bool
}

struct OverlayReplyPlayback: Codable, Equatable, Sendable {
  let replyID: String
  let ticket: String
  let state: String
  let reason: String?
  let spoken: Bool
  let emittedAt: String
}

struct OverlayConversationMessage: Codable, Equatable, Identifiable, Sendable {
  enum Kind: String, Codable, Sendable { case user, reply }
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
  var replyID: String? { kind == .reply ? String(id.dropFirst("reply:".count)) : nil }
}

struct OverlayConversation: Equatable, Identifiable, Sendable {
  let id: String
  let channel: String
  let name: String
  let owner: OverlayConversationOwner?
  let messages: [OverlayConversationMessage]
  var replyIDs: [String] { messages.filter { $0.kind == .reply }.map(\.id) }
}

struct OverlayChannelDeliverySnapshot: Equatable, Sendable {
  let deliveries: [OverlayChannelDelivery]
  let conversations: [OverlayConversation]
}
