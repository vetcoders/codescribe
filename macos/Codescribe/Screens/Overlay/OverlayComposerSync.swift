import Foundation

struct OverlayComposerTextReplacement: Equatable {
  let range: NSRange
  let replacement: String
}

/// Native proposals use UTF-16 coordinates in `before`, never a guessed diff.
struct OverlayComposerProseEdit: Equatable {
  let before: String
  let after: String
  let replacements: [OverlayComposerTextReplacement]

  static func identical(_ lhs: String, _ rhs: String) -> Bool {
    lhs.utf16.elementsEqual(rhs.utf16)
  }

  /// Descending application keeps every proposal in its original coordinates.
  func validatedComposerReplacements() -> [OverlayComposerTextReplacement]? {
    let ordered = replacements.sorted { $0.range.location > $1.range.location }
    var previous: NSRange?
    var result = before
    for replacement in ordered {
      let range = replacement.range
      guard range.location >= 0, range.location <= before.utf16.count,
        range.length >= 0, range.length <= before.utf16.count - range.location,
        Range(range, in: before) != nil
      else { return nil }
      if let previous {
        guard range.location < previous.location,
          range.location + range.length <= previous.location
        else { return nil }
      }
      guard let indices = Range(range, in: result) else { return nil }
      result.replaceSubrange(indices, with: replacement.replacement)
      previous = range
    }
    return Self.identical(result, after) ? ordered : nil
  }
}

struct OverlayDraftSyncMember: Equatable {
  let owner: OverlayConversationOwner
  var proseAnchor: Int
  var privateAttachments: Set<String>

  func owns(_ conversation: OverlayConversation) -> Bool {
    conversation.id == owner.id && conversation.owner?.id == owner.id
      && conversation.channel == owner.channel && conversation.owner?.channel == owner.channel
  }
}

/// Membership and epoch metadata only. Every exact owner keeps its full draft.
struct OverlayDraftSyncGroup: Equatable {
  let id: String
  var participants: [String: OverlayDraftSyncMember]
  var sharedAttachments: Set<String> = []
  var members: Set<String> { Set(participants.keys) }
}

/// A receipt freezes one owner's payload, reply and edit generations.
struct OverlayComposerDraftPublication: Equatable {
  let id: String
  let owner: OverlayConversationOwner
  let recipient: OverlayState.ComposerTarget
  let payload: String
  let reply: OverlayUserReplyReference?
  let draftRevision: UUID?
  let replyRevision: UUID?

  static func == (lhs: Self, rhs: Self) -> Bool {
    lhs.id == rhs.id && lhs.owner.id == rhs.owner.id && lhs.owner.channel == rhs.owner.channel
      && lhs.recipient == rhs.recipient
      && OverlayComposerProseEdit.identical(lhs.payload, rhs.payload) && lhs.reply == rhs.reply
      && lhs.draftRevision == rhs.draftRevision && lhs.replyRevision == rhs.replyRevision
  }
}

struct OverlayComposerSyncChoice: Identifiable {
  let id: String
  let title: String
  let selected: Bool
  let enabled: Bool
}

extension OverlayState {
  var draftSyncConversations: [OverlayConversation] {
    OverlayChannelStatusView.currentConversations(
      conversations: conversations, archivedOwners: archivedAgentOwners,
      hudStates: channelHudStates
    ).filter { conversation in
      guard let owner = conversation.owner else { return false }
      return owner.id == conversation.id && owner.channel == conversation.channel
        && (1...9).contains(Int(conversation.channel) ?? 0)
    }.sorted { (Int($0.channel) ?? 0) < (Int($1.channel) ?? 0) }
  }

  func draftGroupIndex(for id: String) -> Int? {
    conversationDraftGroups.firstIndex { $0.members.contains(id) }
  }

  func conversationDraft(for id: String) -> String { conversationDrafts[id] ?? "" }

  /// Attachment-only edits share pointers. Unproven prose stays private and
  /// starts a fresh epoch at every member's current prose end.
  func setConversationDraft(_ text: String, for id: String) {
    reconcileDraftSyncOwners()
    let before = conversationDraft(for: id)
    guard !OverlayComposerProseEdit.identical(before, text) else { return }
    let old = ConversationDraft.split(before)
    let new = ConversationDraft.split(text)
    conversationDrafts[id] = text
    guard let index = draftGroupIndex(for: id) else { return }
    if OverlayComposerProseEdit.identical(old.text, new.text) {
      applyDraftAttachments(from: old.attachments, to: new.attachments, for: id, group: index)
    } else {
      resetDraftSyncEpoch(at: index)
    }
  }

  func applyConversationProseEdit(_ edit: OverlayComposerProseEdit, for id: String) {
    reconcileDraftSyncOwners()
    let source = ConversationDraft.split(conversationDraft(for: id))
    guard OverlayComposerProseEdit.identical(source.text, edit.before),
      let replacements = edit.validatedComposerReplacements()
    else {
      // Malformed/stale provenance must not discard the user's input.
      setConversationDraft(
        ConversationDraft.compose(text: edit.after, attachments: source.attachments), for: id)
      if let index = draftGroupIndex(for: id) { resetDraftSyncEpoch(at: index) }
      return
    }
    guard let index = draftGroupIndex(for: id) else {
      conversationDrafts[id] = ConversationDraft.compose(
        text: edit.after, attachments: source.attachments)
      return
    }
    var group = conversationDraftGroups[index]
    var prose: [String: String] = [:]
    var attachments: [String: [String]] = [:]
    var touched: Set<String> = []
    var shared: String?
    for (member, metadata) in group.participants {
      let parts = ConversationDraft.split(conversationDraft(for: member))
      guard let suffix = sharedDraftProse(parts.text, anchor: metadata.proseAnchor),
        shared.map({ OverlayComposerProseEdit.identical($0, suffix) }) ?? true
      else {
        setConversationDraft(
          ConversationDraft.compose(text: edit.after, attachments: source.attachments), for: id)
        resetDraftSyncEpoch(at: index)
        return
      }
      shared = suffix
      prose[member] = parts.text
      attachments[member] = parts.attachments
    }
    for replacement in replacements {
      guard let sourceMember = group.participants[id] else { return }
      let range = replacement.range
      let anchor = sourceMember.proseAnchor
      let end = range.location + range.length
      // Crossing replacement inserts privately. Only its overlapping shared
      // deletion travels to the other owners.
      let sharedRange: NSRange?
      let sharedReplacement: String
      if range.location >= anchor {
        sharedRange = NSRange(location: range.location - anchor, length: range.length)
        sharedReplacement = replacement.replacement
      } else if end > anchor {
        sharedRange = NSRange(location: 0, length: end - anchor)
        sharedReplacement = ""
      } else {
        sharedRange = nil
        sharedReplacement = ""
      }
      for member in group.members {
        guard var text = prose[member], var metadata = group.participants[member] else { return }
        let appliedRange: NSRange
        let inserted: String
        if member == id {
          appliedRange = range
          inserted = replacement.replacement
          if range.location < anchor {
            metadata.proseAnchor = end > anchor
              ? range.location + inserted.utf16.count
              : anchor + inserted.utf16.count - range.length
          }
        } else if let sharedRange {
          appliedRange = NSRange(
            location: metadata.proseAnchor + sharedRange.location, length: sharedRange.length)
          inserted = sharedReplacement
        } else {
          continue
        }
        guard let indices = Range(appliedRange, in: text) else {
          setConversationDraft(
            ConversationDraft.compose(text: edit.after, attachments: source.attachments), for: id)
          resetDraftSyncEpoch(at: index)
          return
        }
        text.replaceSubrange(indices, with: inserted)
        prose[member] = text
        group.participants[member] = metadata
        touched.insert(member)
      }
    }
    // Prepare all candidates before mutating any peer's canonical draft.
    conversationDraftGroups[index] = group
    for member in group.members {
      guard let text = prose[member], let paths = attachments[member] else { continue }
      conversationDrafts[member] = ConversationDraft.compose(text: text, attachments: paths)
    }
    for member in touched {
      conversationDraftRevisions[member] = UUID()
      conversationDraftReceipts.removeValue(forKey: member)
    }
  }

  private func sharedDraftProse(_ text: String, anchor: Int) -> String? {
    guard anchor >= 0, anchor <= text.utf16.count,
      let range = Range(NSRange(location: anchor, length: text.utf16.count - anchor), in: text)
    else { return nil }
    return String(text[range])
  }

  private func applyDraftAttachments(
    from old: [String], to new: [String], for id: String, group index: Int
  ) {
    var group = conversationDraftGroups[index]
    let added = new.filter { !old.contains($0) }
    let removed = Set(old).subtracting(new).intersection(group.sharedAttachments)
      .subtracting(group.participants[id]?.privateAttachments ?? [])
    if var metadata = group.participants[id] {
      // Once removed, a private pointer is no longer a retained baseline.
      // Pasting that path again is future input, like any other new pointer.
      metadata.privateAttachments.formIntersection(new)
      group.participants[id] = metadata
    }
    group.sharedAttachments.formUnion(added)
    group.sharedAttachments.subtract(removed)
    for (peer, metadata) in group.participants where peer != id {
      let parts = ConversationDraft.split(conversationDraft(for: peer))
      var paths = parts.attachments.filter {
        !removed.contains($0) || metadata.privateAttachments.contains($0)
      }
      paths.append(contentsOf: added.filter { !paths.contains($0) })
      // Reuse canonical pointers; saving clipboard bytes is the paste owner's job.
      if paths != parts.attachments {
        conversationDrafts[peer] = ConversationDraft.compose(text: parts.text, attachments: paths)
      }
    }
    conversationDraftGroups[index] = group
  }

  func resetDraftSyncEpoch(at index: Int) {
    var group = conversationDraftGroups[index]
    for member in group.members {
      let parts = ConversationDraft.split(conversationDraft(for: member))
      group.participants[member]?.proseAnchor = parts.text.utf16.count
      group.participants[member]?.privateAttachments = Set(parts.attachments)
    }
    group.sharedAttachments = []
    conversationDraftGroups[index] = group
  }

  func draftSyncNames(for id: String) -> [String] {
    guard let index = draftGroupIndex(for: id) else { return [] }
    let group = conversationDraftGroups[index]
    return draftSyncConversations.filter { group.participants[$0.id]?.owns($0) == true }.map(\.name)
  }

  func draftSyncChoices(for conversation: OverlayConversation) -> [OverlayComposerSyncChoice] {
    let current = draftSyncConversations
    guard current.contains(where: { $0.id == conversation.id && $0.owner == conversation.owner })
    else { return [] }
    let group = draftGroupIndex(for: conversation.id).map { conversationDraftGroups[$0] }
    return current.filter { $0.id != conversation.id }.map { target in
      let selected = group?.participants[target.id]?.owns(target) == true
      return OverlayComposerSyncChoice(
        id: target.id, title: target.name, selected: selected,
        enabled: selected
          ? !groupIsBusy(group?.members ?? [])
          : canLinkDraft(conversation.id, with: target.id))
    }
  }

  /// A draft microphone take freezes its audience at request. Membership
  /// cannot change while its words are still being spoken or projected.
  private func groupIsBusy(_ members: Set<String>) -> Bool {
    !pendingTextMessages.isDisjoint(with: members)
      || !pendingCaptureAttachmentSends.isDisjoint(with: members)
      || members.contains { captureAttachmentDrafts[$0] != nil }
      || (draftCapture.hasRequest
        && draftCapture.conversationID.map { members.contains($0) } == true)
      || pendingDraftProjections.values.contains { projection in
        draftCapture.destination(for: projection.sessionId).map { members.contains($0) } == true
      }
  }

  private func canLinkDraft(_ source: String, with target: String) -> Bool {
    let eligible = Set(draftSyncConversations.map(\.id))
    guard source != target, eligible.contains(source), eligible.contains(target) else { return false }
    let sourceMembers = draftGroupIndex(for: source).map { conversationDraftGroups[$0].members }
      ?? [source]
    let targetMembers = draftGroupIndex(for: target).map { conversationDraftGroups[$0].members }
      ?? [target]
    return !groupIsBusy(sourceMembers.union(targetMembers))
  }

  func canActivateDraftSync(for id: String) -> Bool {
    guard draftSyncConversations.contains(where: { $0.id == id }) else { return false }
    if let armed = armedDraftSyncConversationID {
      return armed == id || canLinkDraft(armed, with: id)
    }
    let members = draftGroupIndex(for: id).map { conversationDraftGroups[$0].members } ?? [id]
    return !groupIsBusy(members)
  }

  func activateDraftSync(for id: String) {
    reconcileDraftSyncOwners()
    guard canActivateDraftSync(for: id) else { return }
    if let armed = armedDraftSyncConversationID {
      if armed == id || toggleDraftSync(from: armed, with: id) {
        armedDraftSyncConversationID = nil
      }
    } else if hasSynchronizedDraft(id) {
      stopDraftSync(for: id)
    } else {
      armedDraftSyncConversationID = id
    }
  }

  /// Joining starts a new epoch without moving any existing words or files.
  @discardableResult
  func toggleDraftSync(from source: String, with target: String) -> Bool {
    reconcileDraftSyncOwners()
    if let index = draftGroupIndex(for: source),
      conversationDraftGroups[index].members.contains(target)
    {
      guard !groupIsBusy(conversationDraftGroups[index].members) else { return false }
      unlinkConversationDraft(target)
      armedDraftSyncConversationID = nil
      return true
    }
    guard canLinkDraft(source, with: target) else { return false }
    let sourceGroup = draftGroupIndex(for: source).map { conversationDraftGroups[$0] }
    let targetGroup = draftGroupIndex(for: target).map { conversationDraftGroups[$0] }
    let members = (sourceGroup?.members ?? [source]).union(targetGroup?.members ?? [target])
    var participants: [String: OverlayDraftSyncMember] = [:]
    for conversation in draftSyncConversations where members.contains(conversation.id) {
      guard let owner = conversation.owner else { return false }
      let parts = ConversationDraft.split(conversationDraft(for: conversation.id))
      participants[conversation.id] = OverlayDraftSyncMember(
        owner: owner, proseAnchor: parts.text.utf16.count, privateAttachments: Set(parts.attachments))
    }
    guard Set(participants.keys) == members else { return false }
    conversationDraftGroups.removeAll { !$0.members.isDisjoint(with: members) }
    conversationDraftGroups.append(OverlayDraftSyncGroup(
      id: "draft-sync:\(UUID().uuidString)", participants: participants))
    armedDraftSyncConversationID = nil
    return true
  }

  func stopDraftSync(for id: String) {
    if armedDraftSyncConversationID == id { armedDraftSyncConversationID = nil }
    guard let index = draftGroupIndex(for: id),
      !groupIsBusy(conversationDraftGroups[index].members)
    else { return }
    unlinkConversationDraft(id)
  }

  private func unlinkConversationDraft(_ id: String) {
    guard let index = draftGroupIndex(for: id) else { return }
    conversationDraftGroups[index].participants.removeValue(forKey: id)
    if conversationDraftGroups[index].members.count < 2 {
      conversationDraftGroups.remove(at: index)
    } else {
      resetDraftSyncEpoch(at: index)
    }
  }

  /// Retirement changes membership only; successors never inherit old drafts.
  func reconcileDraftSyncOwners() {
    let current = draftSyncConversations
    if let armed = armedDraftSyncConversationID, !current.contains(where: { $0.id == armed }) {
      armedDraftSyncConversationID = nil
    }
    let stale = conversationDraftGroups.flatMap { group in
      group.participants.compactMap { id, metadata in
        current.contains(where: { metadata.owns($0) }) ? nil : id
      }
    }
    for id in stale { unlinkConversationDraft(id) }
  }

  func draftSyncSendError(for id: String) -> String? {
    if let own = textMessageErrors[id] { return own }
    guard let index = draftGroupIndex(for: id),
      conversationDraftGroups[index].members.contains(where: { textMessageErrors[$0] != nil })
    else { return nil }
    return String(localized: "Some linked agents did not receive this message. Send again to retry them.")
  }

  func hasSynchronizedDraft(_ id: String) -> Bool { draftGroupIndex(for: id) != nil }
}
