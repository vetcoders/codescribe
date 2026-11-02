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
  /// nil for the two ownerless sources, the human inbox and 0 · All: neither
  /// has a lease to publish to. Their typing mirrors into the linked agents'
  /// drafts and Send publishes each of those (Maciej, 2026-10-30 13:58 and
  /// 14:24: the recipient menu is the sync mechanism moved out of "+").
  let owner: OverlayConversationOwner?
  var proseAnchor: Int
  var privateAttachments: Set<String>

  func owns(_ conversation: OverlayConversation) -> Bool {
    guard let owner else { return conversation.isDraftSyncSource }
    return conversation.id == owner.id && conversation.owner?.id == owner.id
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
  /// Why this row is not a free checkbox: another sync, a live take, or a
  /// seat whose owner no longer matches the channel. Nil when the row can
  /// change with no extra fact to show.
  var note: String? = nil
}

extension OverlayState {
  var draftSyncConversations: [OverlayConversation] {
    OverlayChannelStatusView.currentConversations(
      conversations: conversations, archivedOwners: archivedAgentOwners,
      hudStates: channelHudStates
    ).filter { conversation in
      if conversation.isDraftSyncSource { return true }
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

  /// Exact owners of one group. The roster badge, the recipient menu, the
  /// tooltip and Send all mean this set — never raw membership of every group.
  private func owningDraftSyncMembers(_ group: OverlayDraftSyncGroup) -> [OverlayConversation] {
    draftSyncConversations.filter { group.participants[$0.id]?.owns($0) == true }
  }

  func draftSyncNames(for id: String) -> [String] {
    guard let index = draftGroupIndex(for: id) else { return [] }
    return owningDraftSyncMembers(conversationDraftGroups[index]).map(\.name)
  }

  /// Owners of the open conversation's group. An agent linked only somewhere
  /// else stays dark here: the badge answers the same question as the menu.
  func draftSyncRosterIDs() -> Set<String> {
    guard let selected = selectedConversation else { return [] }
    return draftSyncLinkedIDs(anchoredAt: selected.id)
  }

  func draftSyncLinkedIDs(anchoredAt id: String) -> Set<String> {
    guard let index = draftGroupIndex(for: id) else { return [] }
    return Set(owningDraftSyncMembers(conversationDraftGroups[index]).map(\.id))
  }

  /// Live owners of every group except the open conversation's.
  func draftSyncOccupiedElsewhereIDs() -> Set<String> {
    guard let selected = selectedConversation else { return [] }
    let home = draftGroupIndex(for: selected.id)
    var occupied: Set<String> = []
    for (index, group) in conversationDraftGroups.enumerated() where index != home {
      occupied.formUnion(owningDraftSyncMembers(group).map(\.id))
    }
    return occupied
  }

  func draftSyncChoices(for conversation: OverlayConversation) -> [OverlayComposerSyncChoice] {
    let current = draftSyncConversations
    guard current.contains(where: { $0.id == conversation.id && $0.owner == conversation.owner })
    else { return [] }
    let home = draftGroupIndex(for: conversation.id)
    let group = home.map { conversationDraftGroups[$0] }
    let homeMembers = group?.members ?? [conversation.id]
    let homeLock = draftSyncBlockReason(homeMembers)
    // Agents only: the inbox and 0 · All are chosen from, never offered.
    return current.filter { $0.id != conversation.id && $0.owner != nil }.map { target in
      let seat = group?.participants[target.id]
      let selected = seat?.owns(target) == true
      let staleSeat = seat != nil && !selected
      let elsewhere = draftGroupIndex(for: target.id)
      let occupiedElsewhere = !selected && !staleSeat && elsewhere != nil && elsewhere != home
      let peers = elsewhere.map {
        owningDraftSyncMembers(conversationDraftGroups[$0]).filter { $0.id != target.id }.map(\.name)
      } ?? []
      let linkBlocked = draftSyncBlockReason(
        homeMembers.union(elsewhere.map { conversationDraftGroups[$0].members } ?? [target.id]))
      let enabled: Bool
      let note: String?
      if staleSeat {
        // A successor on this channel is a different owner. The seat must not
        // look selectable, and the click must not retarget the old group.
        enabled = false
        note = String(localized: "This link no longer matches the agent on this channel. The message will not go to them.")
      } else if selected {
        enabled = homeLock == nil
        if draftMembershipIsRecording(homeMembers) {
          note = String(localized: "Included in this dictation. Stop it before changing recipients.")
        } else {
          note = homeLock
        }
      } else if let linkBlocked {
        enabled = false
        note = linkBlocked
      } else if occupiedElsewhere {
        enabled = canLinkDraft(conversation.id, with: target.id)
        let list = peers.joined(separator: " · ")
        note = list.isEmpty
          ? String(localized: "Already in another sync. Choosing them joins the two syncs.")
          : String(localized: "Already in a sync with \(list). Choosing them joins the two syncs.")
      } else {
        enabled = canLinkDraft(conversation.id, with: target.id)
        note = nil
      }
      return OverlayComposerSyncChoice(
        id: target.id, title: target.name, selected: selected, enabled: enabled, note: note)
    }
  }

  /// Who a live composer take will actually reach. Nil when this conversation
  /// is not the take's destination or one of the recipients frozen at start.
  func dictationAudienceLabel(for conversation: OverlayConversation) -> String? {
    guard let audience = draftCapture.liveAudience, audience.contains(conversation.id) else {
      return nil
    }
    let names = draftSyncConversations.filter { audience.contains($0.id) }.map(\.name)
    guard !names.isEmpty else { return nil }
    return String(localized: "This dictation goes to: \(names.joined(separator: " · "))")
  }

  /// Why the open conversation cannot change sync right now, if it cannot.
  func draftSyncLockReason(anchoredAt id: String) -> String? {
    let members = draftGroupIndex(for: id).map { conversationDraftGroups[$0].members } ?? [id]
    return draftSyncBlockReason(members)
  }

  /// A draft microphone take freezes its audience at request. Membership
  /// cannot change while its words are still being spoken or projected.
  private func groupIsBusy(_ members: Set<String>) -> Bool {
    draftSyncBlockReason(members) != nil
  }

  private func draftMembershipIsRecording(_ members: Set<String>) -> Bool {
    (draftCapture.hasRequest && draftCapture.conversationID.map { members.contains($0) } == true)
      || pendingDraftProjections.values.contains { projection in
        draftCapture.destination(for: projection.sessionId).map { members.contains($0) } == true
      }
  }

  private func draftSyncBlockReason(_ members: Set<String>) -> String? {
    if draftMembershipIsRecording(members) {
      return String(localized: "Dictation is in progress. This take keeps the recipients it started with. Stop it to add or remove someone.")
    }
    if !pendingTextMessages.isDisjoint(with: members)
      || !pendingCaptureAttachmentSends.isDisjoint(with: members)
      || members.contains(where: { captureAttachmentDrafts[$0] != nil })
    {
      return String(localized: "A delivery is still going out. Wait for it to finish before changing recipients.")
    }
    return nil
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
      guard conversation.owner != nil || conversation.isDraftSyncSource else { return false }
      let parts = ConversationDraft.split(conversationDraft(for: conversation.id))
      participants[conversation.id] = OverlayDraftSyncMember(
        owner: conversation.owner, proseAnchor: parts.text.utf16.count,
        privateAttachments: Set(parts.attachments))
    }
    guard Set(participants.keys) == members else { return false }
    conversationDraftGroups.removeAll { !$0.members.isDisjoint(with: members) }
    conversationDraftGroups.append(OverlayDraftSyncGroup(
      id: "draft-sync:\(UUID().uuidString)", participants: participants))
    armedDraftSyncConversationID = nil
    // Linking changes who receives the open draft: a held edit started before
    // this must not steal focus on return.
    composerTargetRevision = UUID()
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
    composerTargetRevision = UUID()
    conversationDraftGroups[index].participants.removeValue(forKey: id)
    if conversationDraftGroups[index].members.count < 2 {
      conversationDraftGroups.remove(at: index)
    } else {
      resetDraftSyncEpoch(at: index)
    }
  }

  /// Retirement changes membership only. A new session or lease is a new
  /// `OverlayConversationOwner.id` (provider, session, lease). The same name
  /// on the same channel does not inherit the group: Send must not reach
  /// whoever took the channel. The seat leaves the badge, the menu and the
  /// group together on the next snapshot, edit or toggle.
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
