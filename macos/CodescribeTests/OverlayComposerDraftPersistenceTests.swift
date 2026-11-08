import XCTest

@testable import Codescribe

/// Maciej, 2026-11-08: a crash took a roughly 1500-character All draft that
/// lived only in memory. Unsent drafts now outlive the process, each in its
/// own exact conversation, and never in another owner's.
@MainActor
final class OverlayComposerDraftPersistenceTests: XCTestCase {
  private var root: URL!

  override func setUp() {
    super.setUp()
    root = FileManager.default.temporaryDirectory
      .appendingPathComponent("overlay-composer-drafts-\(UUID().uuidString)", isDirectory: true)
  }

  override func tearDown() {
    try? FileManager.default.removeItem(at: root)
    super.tearDown()
  }

  private func agentBus(lease: Character) -> OverlayChannelDelivery.Bus {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume([
      "schema": "codescribe.agent-reply.v1", "kind": "agent_reply",
      "reply_id": String(repeating: lease == "a" ? "1" : "2", count: 24), "text": "Gotowe",
      "provider": "codex", "provider_session_id": "lena-\(lease)",
      "lease_id": String(repeating: lease, count: 32), "channel": "2", "name": "Lena",
      "association": "unsolicited",
    ])
    return bus
  }

  private func snapshot(_ bus: OverlayChannelDelivery.Bus) -> OverlayChannelDeliverySnapshot {
    .init(deliveries: [], conversations: bus.conversations(busPath: "/fixture/bus"))
  }

  private func agent(_ snapshot: OverlayChannelDeliverySnapshot) throws -> OverlayConversation {
    try XCTUnwrap(snapshot.conversations.first { $0.channel == "2" })
  }

  private func stored() -> [String: OverlayComposerDraftStore.Entry] {
    OverlayComposerDraftStore.load(cacheRoot: root) ?? [:]
  }

  func testUnsentDraftsReturnToTheirExactConversationsAfterRelaunch() throws {
    let bus = agentBus(lease: "a")
    let lena = try agent(snapshot(bus))
    let first = OverlayState()
    first.restoreComposerDrafts(cacheRoot: root)
    first.applyConversationSnapshot(snapshot(bus))
    let reply = OverlayUserReplyReference(message: try XCTUnwrap(lena.messages.last))
    let long = String(repeating: "Każde słowo tego szkicu ma przetrwać. ", count: 40)
    first.setConversationDraft(long, for: "0")
    first.setConversationDraft("Do skrzynki", for: "10")
    first.setConversationDraft("Lena, sprawdź to", for: lena.id)
    first.conversationReplies[lena.id] = reply
    first.persistComposerDrafts()

    let path = OverlayComposerDraftStore.url(cacheRoot: root).path
    let attributes = try FileManager.default.attributesOfItem(atPath: path)
    XCTAssertEqual((attributes[.posixPermissions] as? NSNumber)?.intValue, 0o600)
    XCTAssertEqual(
      OverlayComposerDraftStore.url(cacheRoot: root),
      root.appendingPathComponent("overlay-composer-drafts.v1.json"))

    // The process dies here: no shutdown hook runs. A new one reads the file.
    let relaunched = OverlayState()
    relaunched.restoreComposerDrafts(cacheRoot: root)
    relaunched.applyConversationSnapshot(snapshot(bus))
    XCTAssertEqual(relaunched.conversationDraft(for: "0"), long)
    XCTAssertEqual(relaunched.conversationDraft(for: "10"), "Do skrzynki")
    XCTAssertEqual(relaunched.conversationDraft(for: lena.id), "Lena, sprawdź to")
    XCTAssertEqual(relaunched.conversationReplies[lena.id], reply)
    XCTAssertTrue(relaunched.founderComposing, "restored words protect the app like typed ones")
  }

  func testTypingReachesTheFileWithoutAnExplicitSave() async throws {
    let state = OverlayState()
    state.restoreComposerDrafts(cacheRoot: root)
    state.applyConversationSnapshot(snapshot(agentBus(lease: "a")))
    state.setConversationDraft("Pisane w biegu", for: "0")
    XCTAssertNil(stored()["0"], "typing is coalesced, not written per keystroke")
    try await Task.sleep(for: .milliseconds(800))
    XCTAssertEqual(stored()["0"]?.text, "Pisane w biegu")
  }

  func testAReplacementAgentOnTheSameChannelNeverReceivesTheDraft() throws {
    let original = agentBus(lease: "a")
    let lena = try agent(snapshot(original))
    let first = OverlayState()
    first.restoreComposerDrafts(cacheRoot: root)
    first.applyConversationSnapshot(snapshot(original))
    first.setConversationDraft("Tylko dla tej sesji", for: lena.id)
    first.persistComposerDrafts()

    let replacement = agentBus(lease: "b")
    let successor = try agent(snapshot(replacement))
    XCTAssertEqual(successor.channel, lena.channel)
    XCTAssertNotEqual(successor.id, lena.id)
    let relaunched = OverlayState()
    relaunched.restoreComposerDrafts(cacheRoot: root)
    relaunched.applyConversationSnapshot(snapshot(replacement))
    XCTAssertEqual(relaunched.conversationDraft(for: successor.id), "")
    XCTAssertFalse(
      relaunched.founderComposing, "an absent owner's draft cannot hold every idle restart")
    relaunched.setConversationDraft("Coś innego", for: "0")
    relaunched.persistComposerDrafts()
    XCTAssertEqual(stored()[lena.id]?.text, "Tylko dla tej sesji", "kept for its own owner")

    let returned = OverlayState()
    returned.restoreComposerDrafts(cacheRoot: root)
    returned.applyConversationSnapshot(snapshot(original))
    XCTAssertEqual(returned.conversationDraft(for: lena.id), "Tylko dla tej sesji")
  }

  func testANewerDraftTypedDuringSendSurvivesAndASentDraftLeavesTheFileAtOnce() async throws {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "lena-session",
        "lease_id": String(repeating: "c", count: 32), "channel": "1", "name": "Lena",
      ]))
    let conversation = OverlayConversation(
      id: owner.id, channel: "1", name: "Lena", owner: owner, messages: [])
    let state = OverlayState()
    state.restoreComposerDrafts(cacheRoot: root)
    state.applyChannelRoster([
      CsChannelRosterState(
        channel: "1", audience: "Lena", provider: "codex", providerSessionId: "lena-session",
        open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
        unreadSinceUnixMs: nil)
    ])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
    XCTAssertTrue(state.composerTargetIsAvailable(conversation))
    state.setConversationDraft("Pierwsza wersja", for: conversation.id)

    let (release, releaser) = AsyncStream<Void>.makeStream()
    let entered = expectation(description: "publisher entered")
    state.publishConversationText = { _, text, _, _ in
      XCTAssertEqual(text, "Pierwsza wersja")
      entered.fulfill()
      for await _ in release { break }
      return AgentBusTextPublication(messageID: String(repeating: "d", count: 24))
    }
    let send = Task { await state.sendConversationText(conversation) }
    await fulfillment(of: [entered], timeout: 2)
    state.setConversationDraft("Nowsza wersja", for: conversation.id)
    releaser.yield(())
    await send.value
    XCTAssertEqual(state.conversationDraft(for: conversation.id), "Nowsza wersja")
    state.persistComposerDrafts()
    XCTAssertEqual(stored()[conversation.id]?.text, "Nowsza wersja")

    state.publishConversationText = { _, _, _, _ in
      AgentBusTextPublication(messageID: String(repeating: "e", count: 24))
    }
    await state.sendConversationText(conversation)
    XCTAssertEqual(state.conversationDraft(for: conversation.id), "")
    // No coalescing window: a crash right after Send cannot bring the words back.
    XCTAssertNil(stored()[conversation.id])
  }

  func testWordsTypedBeforeTheConversationArrivesWinOverTheFile() throws {
    try OverlayComposerDraftStore.save(
      cacheRoot: root, drafts: ["0": .init(text: "Z pliku", reply: nil, savedAt: Date())])
    let state = OverlayState()
    state.restoreComposerDrafts(cacheRoot: root)
    state.setConversationDraft("Nowe", for: "0")
    state.applyConversationSnapshot(snapshot(agentBus(lease: "a")))
    XCTAssertEqual(state.conversationDraft(for: "0"), "Nowe")
  }

  func testADraftPastRetentionIsLetGo() throws {
    let old = Date(timeIntervalSinceNow: -OverlayComposerDraftStore.retention - 60)
    try OverlayComposerDraftStore.save(
      cacheRoot: root, drafts: ["0": .init(text: "Stary szkic", reply: nil, savedAt: old)])
    let state = OverlayState()
    state.restoreComposerDrafts(cacheRoot: root)
    state.applyConversationSnapshot(snapshot(agentBus(lease: "a")))
    XCTAssertEqual(state.conversationDraft(for: "0"), "")
    XCTAssertFalse(state.founderComposing)
  }
}
