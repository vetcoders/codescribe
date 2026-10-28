import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class OverlayForwardSyncProvenanceTests: XCTestCase {
  func testRepeatedTextDistinguishesPrivateInsertionFromSharedAppend() throws {
    let (state, agents) = try fixture(["aa", "X"])
    append("a", to: agents[0], state: state)
    edit(agents[0], state: state, range: NSRange(location: 1, length: 0), replacement: "a")
    XCTAssertEqual(drafts(state, agents), ["aaaa", "Xa"])
    append("a", to: agents[0], state: state)
    XCTAssertEqual(drafts(state, agents), ["aaaaa", "Xaa"])
  }

  func testCrossBoundaryReplacementSharesDeletionButKeepsReplacementPrivate() throws {
    let (state, agents) = try fixture(["private", "X"])
    append("shared", to: agents[0], state: state)
    edit(agents[0], state: state, range: NSRange(location: 5, length: 5), replacement: "Z")
    XCTAssertEqual(drafts(state, agents), ["privaZred", "Xred"])
    edit(agents[0], state: state, range: NSRange(location: 0, length: 6), replacement: "other")
    XCTAssertEqual(drafts(state, agents), ["otherred", "Xred"])
    append("!", to: agents[1], state: state)
    XCTAssertEqual(drafts(state, agents), ["otherred!", "Xred!"])
  }

  func testUnicodeRangesRetainDifferentPrivatePrefixes() throws {
    let (state, agents) = try fixture(["🦊", "P"])
    append("👩🏽‍💻x", to: agents[0], state: state)
    edit(agents[0], state: state, range: NSRange(location: 2, length: 7), replacement: "🐈")
    XCTAssertEqual(drafts(state, agents), ["🦊🐈x", "P🐈x"])
  }

  func testNativeMultiRangeBatchUsesOriginalCoordinates() throws {
    let (state, agents) = try fixture(["private", "X"])
    append("a•b", to: agents[0], state: state)
    state.applyConversationProseEdit(
      .init(
        before: "privatea•b", after: "privateA•B",
        replacements: [
          .init(range: NSRange(location: 7, length: 1), replacement: "A"),
          .init(range: NSRange(location: 9, length: 1), replacement: "B"),
        ]), for: agents[0].id)
    XCTAssertEqual(drafts(state, agents), ["privateA•B", "XA•B"])
  }

  func testUnknownBindingEditRemainsPrivateAndStartsFreshEpoch() throws {
    let (state, agents) = try fixture(["aa", "X"])
    append("a", to: agents[0], state: state)
    state.setConversationDraft("unproven", for: agents[0].id)
    XCTAssertEqual(drafts(state, agents), ["unproven", "Xa"])
    append(" future", to: agents[1], state: state)
    XCTAssertEqual(drafts(state, agents), ["unproven future", "Xa future"])
  }

  func testInvalidSurrogateAndStaleProposalsRetainInputPrivately() throws {
    let (state, agents) = try fixture(["🦊", "X"])
    state.applyConversationProseEdit(
      .init(
        before: "🦊", after: "🦊private",
        replacements: [.init(range: NSRange(location: 1, length: 0), replacement: "private")]),
      for: agents[0].id)
    XCTAssertEqual(drafts(state, agents), ["🦊private", "X"])
    state.applyConversationProseEdit(
      .init(
        before: "stale", after: "retained",
        replacements: [.init(range: NSRange(location: 5, length: 0), replacement: "!")]),
      for: agents[0].id)
    XCTAssertEqual(drafts(state, agents), ["retained", "X"])
    append("?", to: agents[1], state: state)
    XCTAssertEqual(drafts(state, agents), ["retained?", "X?"])
  }

  func testLateJoinDoesNotInheritAnEarlierSharedEpoch() throws {
    let (state, agents) = try fixture(["A", "B", "C"], link: false)
    XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: agents[1].id))
    append(" early", to: agents[0], state: state)
    XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: agents[2].id))
    XCTAssertEqual(drafts(state, agents), ["A early", "B early", "C"])
    append(" new", to: agents[2], state: state)
    XCTAssertEqual(drafts(state, agents), ["A early new", "B early new", "C new"])
    state.stopDraftSync(for: agents[1].id)
    append("!", to: agents[0], state: state)
    XCTAssertEqual(drafts(state, agents), ["A early new!", "B early new", "C new!"])
  }

  func testPrimaryGestureArmsCancelsPairsMergesAndUnlinks() throws {
    let (state, agents) = try fixture(["A", "B", "C"], link: false)
    state.activateDraftSync(for: agents[0].id)
    XCTAssertEqual(state.armedDraftSyncConversationID, agents[0].id)
    state.activateDraftSync(for: agents[0].id)
    XCTAssertNil(state.armedDraftSyncConversationID)
    state.activateDraftSync(for: agents[0].id)
    state.activateDraftSync(for: agents[1].id)
    XCTAssertTrue(state.hasSynchronizedDraft(agents[0].id))
    state.activateDraftSync(for: agents[2].id)
    state.activateDraftSync(for: agents[0].id)
    XCTAssertEqual(state.conversationDraftGroups.first?.members, Set(agents.map(\.id)))
    state.activateDraftSync(for: agents[1].id)
    XCTAssertFalse(state.hasSynchronizedDraft(agents[1].id))
    XCTAssertTrue(state.hasSynchronizedDraft(agents[2].id))
    XCTAssertEqual(drafts(state, agents), ["A", "B", "C"])
  }

  func testSharedAttachmentRemovalPreservesPeerPrivateBaseline() throws {
    let (state, agents) = try fixture(["A", "B"], link: false)
    let root = ConversationAttachmentPointer.configDirectory.appendingPathComponent("attachments")
    let own = root.appendingPathComponent("private-a-\(UUID().uuidString).txt").path
    let peer = root.appendingPathComponent("private-b-\(UUID().uuidString).txt").path
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    for path in [own, peer] { try Data("Admitted file".utf8).write(to: URL(fileURLWithPath: path)) }
    defer { for path in [own, peer] { try? FileManager.default.removeItem(atPath: path) } }
    state.setConversationDraft(
      ConversationDraft.compose(text: "A", attachments: [own]), for: agents[0].id)
    state.setConversationDraft(
      ConversationDraft.compose(text: "B", attachments: [peer]), for: agents[1].id)
    XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: agents[1].id))
    state.setConversationDraft(
      ConversationDraft.compose(text: "A", attachments: [own, peer]), for: agents[0].id)
    state.setConversationDraft(
      ConversationDraft.compose(text: "A", attachments: [own]), for: agents[0].id)
    XCTAssertEqual(
      try attachmentIdentities(
        ConversationDraft.split(state.conversationDraft(for: agents[1].id)).attachments),
      try attachmentIdentities([peer]))
    state.setConversationDraft("A", for: agents[0].id)
    state.setConversationDraft(
      ConversationDraft.compose(text: "A", attachments: [own]), for: agents[0].id)
    XCTAssertEqual(
      try attachmentIdentities(
        ConversationDraft.split(state.conversationDraft(for: agents[1].id)).attachments),
      try attachmentIdentities([peer, own]))
    state.setConversationDraft(
      ConversationDraft.compose(text: "B", attachments: [peer]), for: agents[1].id)
    XCTAssertEqual(
      ConversationDraft.split(state.conversationDraft(for: agents[0].id)).attachments, [])
  }

  func testDistinctPrivatePayloadsHaveNoFalseCoAddressAndRetryOnlyFailure() async throws {
    let (state, agents) = try fixture(["private A", "private B"])
    append(" shared", to: agents[0], state: state)
    var calls: [(String, String)] = []
    var fail = true
    state.publishConversationText = { owner, text, _, peers in
      let owner = try XCTUnwrap(owner)
      XCTAssertTrue(
        peers.isEmpty, "Different complete messages cannot claim identical co-address context")
      calls.append((owner.id, text))
      if owner.id == agents[1].id && fail { throw CocoaError(.fileWriteNoPermission) }
    }
    await state.sendConversationText(agents[0])
    XCTAssertEqual(calls.map { $0.1 }, ["private A shared", "private B shared"])
    XCTAssertEqual(drafts(state, agents), ["", "private B shared"])
    fail = false
    await state.sendConversationText(agents[1])
    XCTAssertEqual(calls.map { $0.0 }, [agents[0].id, agents[1].id, agents[1].id])
    XCTAssertEqual(drafts(state, agents), ["", ""])
  }

  func testDraftABADuringPublicationCannotClearTheNewGeneration() async throws {
    let (state, agents) = try fixture(["", ""])
    append("stable", to: agents[0], state: state)
    var targets: [String] = []
    state.publishConversationText = { owner, _, _, _ in
      targets.append(try XCTUnwrap(owner).id)
      if targets.count == 1 {
        state.setConversationDraft("temporary", for: agents[0].id)
        state.setConversationDraft("stable", for: agents[0].id)
      }
    }
    await state.sendConversationText(agents[0])
    XCTAssertEqual(targets, agents.map(\.id))
    XCTAssertEqual(drafts(state, agents), ["stable", ""])
    await state.sendConversationText(agents[0])
    XCTAssertEqual(targets, [agents[0].id, agents[1].id, agents[0].id])
    XCTAssertEqual(drafts(state, agents), ["", ""])
  }

  func testFutureInputDuringSendDoesNotRecallFrozenRecipientsOrClearNewText() async throws {
    let (state, agents) = try fixture(["", ""])
    append("old", to: agents[0], state: state)
    var texts: [String] = []
    state.publishConversationText = { _, text, _, peers in
      texts.append(text)
      XCTAssertEqual(
        peers.count, 1, "CC describes the frozen identical selection, not current draft text")
      if texts.count == 1 { self.append(" new", to: agents[0], state: state) }
    }
    await state.sendConversationText(agents[0])
    XCTAssertEqual(texts, ["old", "old"])
    XCTAssertEqual(drafts(state, agents), ["old new", "old new"])
  }

  func testRetiringFirstRecipientCannotRecallSurvivingFrozenRecipient() async throws {
    let (state, agents) = try fixture(["A", "B"])
    append(" shared", to: agents[0], state: state)
    var targets: [String] = []
    state.publishConversationText = { owner, _, _, _ in
      targets.append(try XCTUnwrap(owner).id)
      if targets.count == 1 {
        state.applyChannelRoster([
          .init(
            channel: agents[1].channel, audience: agents[1].name, provider: "codex",
            providerSessionId: agents[1].owner?.providerSessionID ?? "", open: false, loud: false,
            autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
        ])
        state.applyConversationSnapshot(.init(deliveries: [], conversations: [agents[1]]))
      }
    }
    await state.sendConversationText(agents[0])
    XCTAssertEqual(targets, agents.map(\.id))
    XCTAssertEqual(state.conversationDraft(for: agents[1].id), "")
    XCTAssertEqual(state.conversationDraft(for: agents[0].id), "A shared")
  }

  func testPrimaryAvailabilityReflectsAnInFlightGroupDelivery() async throws {
    let (state, agents) = try fixture(["A", "B"])
    var publications = 0
    state.publishConversationText = { _, _, _, _ in
      publications += 1
      XCTAssertFalse(state.canActivateDraftSync(for: agents[0].id))
      XCTAssertFalse(state.canActivateDraftSync(for: agents[1].id))
      state.activateDraftSync(for: agents[0].id)
      XCTAssertTrue(
        state.hasSynchronizedDraft(agents[0].id), "A busy group cannot change membership")
    }
    await state.sendConversationText(agents[0])
    XCTAssertEqual(publications, 2)
    XCTAssertTrue(state.canActivateDraftSync(for: agents[0].id))
  }

  func testArmedCancelRemainsAvailableDuringItsDelivery() async throws {
    let (state, agents) = try fixture(["A", "B"], link: false)
    state.activateDraftSync(for: agents[0].id)
    var publications = 0
    state.publishConversationText = { _, _, _, _ in
      publications += 1
      XCTAssertTrue(
        state.canActivateDraftSync(for: agents[0].id), "Cancel never needs a new sharing admission")
      XCTAssertFalse(state.canActivateDraftSync(for: agents[1].id))
      state.activateDraftSync(for: agents[0].id)
      XCTAssertNil(state.armedDraftSyncConversationID)
      XCTAssertFalse(state.canActivateDraftSync(for: agents[0].id))
    }
    await state.sendConversationText(agents[0])
    XCTAssertEqual(publications, 1)
  }

  private func attachmentIdentities(_ paths: [String]) throws -> [String] {
    try paths.map { path in
      let attributes = try FileManager.default.attributesOfItem(atPath: path)
      let device = try XCTUnwrap(attributes[.systemNumber] as? NSNumber)
      let file = try XCTUnwrap(attributes[.systemFileNumber] as? NSNumber)
      return "\(device.uint64Value):\(file.uint64Value)"
    }
  }

  private func drafts(_ state: OverlayState, _ agents: [OverlayConversation]) -> [String] {
    agents.map { state.conversationDraft(for: $0.id) }
  }

  private func append(_ text: String, to agent: OverlayConversation, state: OverlayState) {
    let before = ConversationDraft.split(state.conversationDraft(for: agent.id)).text
    state.applyConversationProseEdit(
      .init(
        before: before, after: before + text,
        replacements: [
          .init(range: NSRange(location: before.utf16.count, length: 0), replacement: text)
        ]), for: agent.id)
  }

  private func edit(
    _ agent: OverlayConversation, state: OverlayState, range: NSRange, replacement: String
  ) {
    let before = ConversationDraft.split(state.conversationDraft(for: agent.id)).text
    let after = (before as NSString).replacingCharacters(in: range, with: replacement)
    state.applyConversationProseEdit(
      .init(
        before: before, after: after,
        replacements: [.init(range: range, replacement: replacement)]), for: agent.id)
  }

  private func fixture(_ privateText: [String], link: Bool = true) throws -> (
    OverlayState, [OverlayConversation]
  ) {
    let agents = try privateText.indices.map { index in
      let channel = String(index + 1)
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": "provenance-\(channel)",
          "lease_id": String(repeating: channel, count: 32), "channel": channel,
          "name": "Agent \(channel)",
        ]))
      return OverlayConversation(
        id: owner.id, channel: channel, name: owner.name, owner: owner, messages: [])
    }
    let state = OverlayState()
    state.applyChannelRoster(
      agents.map {
        .init(
          channel: $0.channel, audience: $0.name, provider: "codex",
          providerSessionId: $0.owner?.providerSessionID ?? "", open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
      })
    state.applyConversationSnapshot(.init(deliveries: [], conversations: agents))
    for (agent, text) in zip(agents, privateText) {
      state.setConversationDraft(text, for: agent.id)
    }
    if link {
      for agent in agents.dropFirst() {
        XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: agent.id))
      }
    }
    return (state, agents)
  }
}
