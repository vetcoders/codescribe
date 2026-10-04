import CryptoKit
import Foundation

/// Read-only delivery evidence. Receipt means the agent acknowledged the envelope,
/// not that it executed the request. No transcript text participates in identity.
struct OverlayChannelDelivery: Equatable, Identifiable, Sendable {
  enum Stage: String, Sendable { case sent, queued, received }

  let channel: String
  let agent: String
  let deliveryID: String?
  let stage: Stage?
  let isOpen: Bool
  var id: String { channel }

  /// Metadata projection of the bus; never a document reducer or delivery writer.
  struct Bus {
    private var seals: [String: Seal] = [:]
    private var sessions: [String: Session] = [:]
    private var endedSessions: Set<String> = []
    private var receipts: [String: Receipt] = [:]
    private var sealOrder: UInt64 = 0
    private var seenSeals: Set<String> = []

    private struct Seal {
      let phaseID: String
      let audience: String
      let order: UInt64
    }
    private struct Session {
      let provider: String
      let providerSession: String
      let sessionID: String
      let openedAt: String
      let isOpen: Bool
    }
    private struct Receipt {
      let channel: String
      let agent: String
    }

    mutating func consume(_ row: [String: Any]) {
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
      seals[audience] = Seal(phaseID: phaseID, audience: audience, order: sealOrder)
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
      let seal = [seals[agent], seals["*"]].compactMap { $0 }.max { $0.order < $1.order }
      let deliveryID = seal.map {
        Self.identity(["native_bus_demux", leaseID, $0.phaseID, "seal", $0.audience])
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
      }
      return OverlayChannelDelivery(
        channel: channel, agent: agent, deliveryID: deliveryID, stage: stage, isOpen: isOpen)
    }

    private static func coordinate(_ value: Any?) -> String {
      if let string = value as? String { return string }
      if let number = value as? NSNumber { return number.stringValue }
      return ""
    }

    // Exact bus-demux.py _identity contract, including its NUL separator and
    // terminal phase coalescing. Fixed Python-produced vectors guard drift.
    private static func identity(_ parts: [String]) -> String {
      SHA256.hash(data: Data(parts.joined(separator: "\0").utf8))
        .prefix(12).map { String(format: "%02x", $0) }.joined()
    }
  }
}
