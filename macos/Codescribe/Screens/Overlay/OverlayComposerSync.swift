import Foundation

/// Membership and publication receipts; the existing draft dictionary owns the text.
struct OverlayDraftSyncGroup: Equatable {
  let id: String
  var members: Set<String>
  var revision: UInt64 = 0
  var sent: Set<String> = []
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
    ).filter { $0.owner != nil && (1...9).contains(Int($0.channel) ?? 0) }
      .sorted { (Int($0.channel) ?? 0) < (Int($1.channel) ?? 0) }
  }

  func draftGroupIndex(for id: String) -> Int? {
    conversationDraftGroups.firstIndex { $0.members.contains(id) }
  }

  func conversationDraft(for id: String) -> String {
    let key = draftGroupIndex(for: id).map { conversationDraftGroups[$0].id } ?? id
    return conversationDrafts[key] ?? ""
  }

  func setConversationDraft(_ text: String, for id: String) {
    let index = draftGroupIndex(for: id)
    let key = index.map { conversationDraftGroups[$0].id } ?? id
    guard conversationDrafts[key] != text else { return }
    conversationDrafts[key] = text
    if let index {
      conversationDraftGroups[index].revision &+= 1
      conversationDraftGroups[index].sent = []
    }
  }

  func draftSyncNames(for id: String) -> [String] {
    guard let index = draftGroupIndex(for: id) else { return [] }
    let members = conversationDraftGroups[index].members
    return draftSyncConversations.filter { members.contains($0.id) }.map(\.name)
  }

  func draftSyncChoices(for conversation: OverlayConversation) -> [OverlayComposerSyncChoice] {
    let current = draftSyncConversations
    guard current.contains(where: { $0.id == conversation.id }) else { return [] }
    let group = draftGroupIndex(for: conversation.id).map { conversationDraftGroups[$0] }
    return current.filter { $0.id != conversation.id }.map { target in
      let selected = group?.members.contains(target.id) == true
      return OverlayComposerSyncChoice(
        id: target.id, title: target.name, selected: selected,
        enabled: selected
          ? !groupIsBusy(group?.members ?? [])
          : canLinkDraft(conversation.id, with: target.id))
    }
  }

  private func groupIsBusy(_ members: Set<String>) -> Bool {
    !pendingTextMessages.isDisjoint(with: members)
      || members.contains { captureAttachmentDrafts[$0] != nil }
  }

  private func canLinkDraft(_ source: String, with target: String) -> Bool {
    let eligible = Set(draftSyncConversations.map(\.id))
    guard source != target, eligible.contains(source), eligible.contains(target) else { return false }
    let sourceMembers = draftGroupIndex(for: source).map { conversationDraftGroups[$0].members }
      ?? [source]
    let targetMembers = draftGroupIndex(for: target).map { conversationDraftGroups[$0].members }
      ?? [target]
    guard !groupIsBusy(sourceMembers.union(targetMembers)) else { return false }
    let sourceText = conversationDraft(for: source)
    let targetText = conversationDraft(for: target)
    return sourceText.isEmpty || targetText.isEmpty || sourceText == targetText
  }

  /// A different nonempty draft is never overwritten when joining a group.
  @discardableResult
  func toggleDraftSync(from source: String, with target: String) -> Bool {
    if let index = draftGroupIndex(for: source),
      conversationDraftGroups[index].members.contains(target)
    {
      guard !groupIsBusy(conversationDraftGroups[index].members) else { return false }
      unlinkConversationDraft(target)
      return true
    }
    guard canLinkDraft(source, with: target) else { return false }
    let sourceGroup = draftGroupIndex(for: source).map { conversationDraftGroups[$0] }
    let targetGroup = draftGroupIndex(for: target).map { conversationDraftGroups[$0] }
    let sourceText = conversationDraft(for: source)
    let targetText = conversationDraft(for: target)
    let text = sourceText.isEmpty ? targetText : sourceText
    let members = (sourceGroup?.members ?? [source]).union(targetGroup?.members ?? [target])
    let oldGroups = [sourceGroup, targetGroup].compactMap { $0 }
    let sent = oldGroups.filter { conversationDrafts[$0.id] == text }
      .reduce(into: Set<String>()) { $0.formUnion($1.sent) }
    conversationDraftGroups.removeAll { $0.members.contains(source) || $0.members.contains(target) }
    for key in members.union(Set(oldGroups.map(\.id))) {
      conversationDrafts.removeValue(forKey: key)
    }
    let group = OverlayDraftSyncGroup(id: "draft-sync:\(UUID().uuidString)", members: members, sent: sent)
    conversationDrafts[group.id] = text
    conversationDraftGroups.append(group)
    return true
  }

  func stopDraftSync(for id: String) {
    guard let index = draftGroupIndex(for: id),
      !groupIsBusy(conversationDraftGroups[index].members)
    else { return }
    unlinkConversationDraft(id)
  }

  /// Leaving retains the text and attachment pointers in each remaining owner.
  private func unlinkConversationDraft(_ id: String) {
    guard let index = draftGroupIndex(for: id) else { return }
    var group = conversationDraftGroups[index]
    let text = conversationDrafts[group.id] ?? ""
    conversationDrafts[id] = text
    group.members.remove(id)
    group.sent.remove(id)
    if group.members.count < 2 {
      for member in group.members { conversationDrafts[member] = text }
      conversationDrafts.removeValue(forKey: group.id)
      conversationDraftGroups.remove(at: index)
    } else {
      conversationDraftGroups[index] = group
    }
  }

  /// An archived/disconnected owner cannot remain a recipient of future edits.
  func reconcileDraftSyncOwners() {
    let eligible = Set(draftSyncConversations.map(\.id))
    let stale = conversationDraftGroups.flatMap { $0.members.subtracting(eligible) }
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
