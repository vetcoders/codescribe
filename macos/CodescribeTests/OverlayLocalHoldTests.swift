import XCTest

@testable import Codescribe

/// Maciej, 2026-11-08 09:13Z: what happens in the UI stays in the UI until the
/// hold ends; the backend gets the message only afterwards. Time is driven by
/// hand: the clock never moves and the hold loop never wakes on its own.
@MainActor
final class OverlayLocalHoldTests: XCTestCase {
  private var root: URL!
  private var clock = Date(timeIntervalSince1970: 1_800_000_000)

  override func setUp() async throws {
    try await super.setUp()
    await MainActor.run {
      root = FileManager.default.temporaryDirectory
        .appendingPathComponent("overlay-local-hold-\(UUID().uuidString)", isDirectory: true)
    }
  }

  override func tearDown() async throws {
    await MainActor.run {
      try? FileManager.default.removeItem(at: root)
    }
    try await super.tearDown()
  }

  /// Cancel/Edit hands back the frozen draft whole: the prose, every file the
  /// held row painted as an attachment, and the reply reference, so the files
  /// return as composer pills and newer typing stays in front.
  func testEditReturnsTheFrozenProseAndAttachmentsAsOneDraft() throws {
    let configDirectory = CodescribeConfig().configDir()
    guard configDirectory != NSHomeDirectory() + "/.codescribe" else {
      throw XCTSkip("needs an isolated CODESCRIBE_DATA_DIR, as make test-swift provides")
    }
    let store = ConversationAttachmentPointer.configDirectory
      .appendingPathComponent("attachments/\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: store, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: store) }
    let files = try ["Zrzut ekranu 2026-11-09 o 08.17.png", "notatka żółć.txt"].map {
      name -> String in
      let url = store.appendingPathComponent(name)
      try Data(name.utf8).write(to: url)
      return url.resolvingSymlinksInPath().path
    }
    let prose = "Popraw to proszę.\n\nDrugi akapit."
    let frozen =
      prose + "\n" + files.map { ConversationAttachmentPointer.prefix + $0 }.joined(separator: "\n")
    let owner = try XCTUnwrap(agent("2").owner)
    let reply = try XCTUnwrap(
      OverlayUserReplyReference(
        row: ["message_id": "reply-1", "author": "lena", "text": "Poprzednia odpowiedź"]))
    let send = OverlayHeldSend(
      id: OverlayHeldSend.newID(), conversationID: owner.id, owner: owner, text: frozen,
      reply: reply, alsoSentTo: [], acceptedAt: clock,
      deadline: clock.addingTimeInterval(5), phase: .holding)
    let row = send.row(order: 1, now: clock)
    XCTAssertEqual(row.text, prose)
    XCTAssertEqual(row.attachments?.map { $0.path }, files)
    XCTAssertEqual(row.userReplyTo, reply)

    let returned = OverlayConversationView.heldDraft(row)
    XCTAssertEqual(returned, frozen)
    let parts = ConversationDraft.split(returned)
    XCTAssertEqual(parts.text, prose)
    XCTAssertEqual(parts.attachments, files)

    let target = try agent("2")
    let state = state(agents: [target]) { _, _, _, _ in
      AgentBusTextPublication(messageID: "unused", holdUntil: nil, recipients: [])
    }
    state.setConversationDraft("Nowy dopisek", for: target.id)
    let revision = state.prepareHeldComposerEdit(target)
    _ = state.returnHeldComposerText(
      returned, to: target, revision: revision, attachments: files, reply: row.userReplyTo)
    let mergedParts = ConversationDraft.split(state.conversationDraft(for: target.id))
    XCTAssertEqual(mergedParts.text, "Nowy dopisek\n\n" + prose)
    XCTAssertEqual(mergedParts.attachments, files)
    XCTAssertEqual(state.conversationReplies[target.id], reply)
    for path in files { XCTAssertTrue(FileManager.default.fileExists(atPath: path)) }
  }

  private static let all = OverlayConversation(
    id: "0", channel: "0", name: "All", owner: nil, messages: [])

  private func agent(_ channel: String) throws -> OverlayConversation {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "hold-\(channel)",
        "lease_id": String(repeating: channel, count: 32), "channel": channel,
        "name": "Agent \(channel)",
      ]))
    return OverlayConversation(
      id: owner.id, channel: channel, name: owner.name, owner: owner, messages: [])
  }

  private func state(
    agents: [OverlayConversation], seconds: UInt32 = 10, extra: [OverlayConversation] = [],
    dispatch:
      @escaping @MainActor (
        OverlayConversationOwner?, String, OverlayUserReplyReference?, [OverlayConversationOwner]
      ) async throws -> AgentBusTextPublication
  ) -> OverlayState {
    let state = OverlayState()
    state.heldSendSeconds = { seconds }
    state.heldSendNow = { [unowned self] in self.clock }
    state.heldSendSleep = { _ in try? await Task.sleep(for: .seconds(3600)) }
    state.dispatchConversationText = dispatch
    state.restoreComposerDrafts(cacheRoot: root, now: clock)
    state.applyChannelRoster(
      agents.map { agent in
        .init(
          channel: agent.channel, audience: agent.name, provider: "codex",
          providerSessionId: agent.owner?.providerSessionID ?? "", open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
      })
    state.applyConversationSnapshot(
      .init(deliveries: [], conversations: [Self.all] + extra + agents), now: clock)
    return state
  }

  private func heldRow(_ state: OverlayState, in id: String) -> OverlayConversationMessage? {
    state.conversations.first { $0.id == id }?.messages.last { OverlayHeldSend.isLocal($0.id) }
  }

  private func advance(_ seconds: TimeInterval, _ state: OverlayState) {
    clock = clock.addingTimeInterval(seconds)
    state.tickHeldSends()
  }

  private func settle(_ condition: @MainActor () -> Bool) async {
    for _ in 0..<400 where !condition() { try? await Task.sleep(for: .milliseconds(5)) }
  }

  private static let published = AgentBusTextPublication(
    messageID: String(repeating: "e", count: 24))

  func testEnterKeepsTheMessageInTheAppUntilItsDeadline() async throws {
    let lena = try agent("1")
    let owner = try XCTUnwrap(lena.owner)
    var calls: [String] = []
    let state = state(agents: [lena]) { recipient, text, _, peers in
      XCTAssertEqual(recipient, owner)
      XCTAssertEqual(peers, [])
      calls.append(text)
      return Self.published
    }
    state.setConversationDraft("Pierwsza wiadomość", for: lena.id)
    await state.sendConversationText(lena)

    XCTAssertEqual(calls, [], "Enter publishes nothing while the hold runs")
    XCTAssertEqual(state.conversationDraft(for: lena.id), "", "the composer is free at once")
    XCTAssertTrue(state.pendingTextMessages.isEmpty)
    let held = try XCTUnwrap(heldRow(state, in: lena.id))
    XCTAssertEqual(held.text, "Pierwsza wiadomość")
    XCTAssertEqual(held.holdSecondsRemaining, 10)
    XCTAssertEqual(held.deliveryStage(at: clock), .held)
    XCTAssertEqual(held.sourceChannel, "1")
    XCTAssertEqual(held.recipients.map(\.owner), [owner])
    XCTAssertEqual(OverlayHeldSendStore.load(cacheRoot: root).map(\.text), ["Pierwsza wiadomość"])
    XCTAssertTrue(state.founderComposing, "a held message blocks the idle restart")

    advance(9.5, state)
    XCTAssertEqual(calls, [])
    XCTAssertEqual(heldRow(state, in: lena.id)?.holdSecondsRemaining, 1)

    advance(0.5, state)
    await settle { state.heldSends.allSatisfy { $0.phase == .dispatched } }
    XCTAssertEqual(calls, ["Pierwsza wiadomość"], "exactly one publication at the deadline")
    let sent = try XCTUnwrap(heldRow(state, in: lena.id))
    XCTAssertNil(sent.holdSecondsRemaining, "a published message promises no cancel")
    XCTAssertNil(sent.controlIdentity)
    XCTAssertEqual(OverlayHeldSendStore.load(cacheRoot: root), [])
    XCTAssertFalse(state.founderComposing)

    // The reader's copy replaces the local row.
    var copy = OverlayConversationMessage(
      id: "typed:" + Self.published.messageID, kind: .user, text: "Pierwsza wiadomość",
      order: 1, emittedAt: "", owner: nil,
      recipients: [.init(owner: owner, queued: true, accepted: false, acknowledged: false)],
      deliveryID: nil, replyTo: nil, unsolicited: false, busPath: "/hold/bus")
    copy.controlIdentity = Self.published.messageID
    copy.deliverySealed = true
    let delivered = OverlayConversation(
      id: lena.id, channel: "1", name: lena.name, owner: owner, messages: [copy])
    state.applyConversationSnapshot(
      .init(deliveries: [], conversations: [Self.all, delivered]), now: clock)
    XCTAssertNil(heldRow(state, in: lena.id))
    XCTAssertEqual(state.conversations.first { $0.id == lena.id }?.messages.count, 1)
    XCTAssertTrue(state.heldSends.isEmpty)
  }

  func testCancelEditDuringTheHoldMeansZeroPublications() async throws {
    let lena = try agent("1")
    var calls = 0
    let state = state(agents: [lena]) { _, _, _, _ in
      calls += 1
      return Self.published
    }
    state.setConversationDraft("Jednak nie", for: lena.id)
    await state.sendConversationText(lena)
    let held = try XCTUnwrap(heldRow(state, in: lena.id))
    try await state.controlDelivery(.withdraw, message: held, owner: lena.owner, text: nil)
    XCTAssertNil(heldRow(state, in: lena.id))
    XCTAssertTrue(state.heldSends.isEmpty)
    XCTAssertEqual(OverlayHeldSendStore.load(cacheRoot: root), [])
    XCTAssertFalse(state.founderComposing)

    advance(30, state)
    await settle { false }
    XCTAssertEqual(calls, 0, "a cancelled hold never reaches the helper")
    do {
      try await state.controlDelivery(.withdraw, message: held, owner: lena.owner, text: nil)
      XCTFail("a message that left the clock cannot be withdrawn twice")
    } catch {
      XCTAssertEqual(error as? OverlayHeldSendError, .alreadySending)
    }
  }

  func testSendNowAndTheDeadlineRaceToOnePublicationWithoutASecondHold() async throws {
    let lena = try agent("1")
    var calls = 0
    let entered = expectation(description: "publication started")
    let (release, releaser) = AsyncStream<Void>.makeStream()
    let state = state(agents: [lena]) { _, _, _, _ in
      calls += 1
      // A second publication is reported by `calls`, not by an XCTest
      // over-fulfilment exception, and it never waits for `release`: the
      // test must fail on the count, not hang on a publication it never
      // expected.
      guard calls == 1 else { return Self.published }
      entered.fulfill()
      for await _ in release { break }
      return Self.published
    }
    state.setConversationDraft("Teraz", for: lena.id)
    await state.sendConversationText(lena)
    let held = try XCTUnwrap(heldRow(state, in: lena.id))
    let sendNow = Task {
      try await state.controlDelivery(.release, message: held, owner: lena.owner, text: nil)
    }
    await fulfillment(of: [entered], timeout: 2)
    XCTAssertNil(
      heldRow(state, in: lena.id)?.holdSecondsRemaining, "a handed-over row shows no clock")
    XCTAssertTrue(state.founderComposing, "words still in the app's hands block a restart")
    do {
      try await state.controlDelivery(.withdraw, message: held, owner: lena.owner, text: nil)
      XCTFail("Cancel/Edit after Send now must refuse")
    } catch {
      XCTAssertEqual(error as? OverlayHeldSendError, .alreadySending)
    }
    advance(11, state)
    try await state.controlDelivery(.release, message: held, owner: lena.owner, text: nil)
    releaser.yield(())
    try await sendNow.value
    await settle { state.heldSends.allSatisfy { $0.phase == .dispatched } }
    XCTAssertEqual(calls, 1, "Send now, a second Send now and the deadline publish once")

    var holds: [UInt32] = []
    _ = try await OverlayState.dispatchHeldText(lena.owner, "x", nil, []) { _, _, hold, _, _ in
      holds.append(hold)
      return Self.published
    }
    XCTAssertEqual(holds, [0], "the helper must not hold a message the app already held")
  }

  func testARefusedPublicationReturnsTheWordsBehindNewerTyping() async throws {
    let lena = try agent("1")
    let reply = try XCTUnwrap(
      OverlayUserReplyReference(row: ["message_id": "parent", "author": "Lena", "text": "Pytanie"]))
    let state = state(agents: [lena]) { _, _, _, _ in throw CocoaError(.fileWriteNoPermission) }
    state.setConversationDraft("Stara", for: lena.id)
    state.conversationReplies[lena.id] = reply
    await state.sendConversationText(lena)
    XCTAssertNil(state.conversationReplies[lena.id], "the reply froze with the held message")
    state.setConversationDraft("Nowa", for: lena.id)
    let held = try XCTUnwrap(heldRow(state, in: lena.id))
    try await state.controlDelivery(.release, message: held, owner: lena.owner, text: nil)
    XCTAssertEqual(state.conversationDraft(for: lena.id), "Nowa\n\nStara")
    XCTAssertEqual(state.conversationReplies[lena.id], reply)
    XCTAssertNotNil(state.textMessageErrors[lena.id])
    XCTAssertTrue(state.heldSends.isEmpty)
    XCTAssertEqual(OverlayHeldSendStore.load(cacheRoot: root), [])
  }

  func testASlowPublicationNeverBlocksTheNextMessage() async throws {
    let lena = try agent("1")
    var calls: [String] = []
    let entered = expectation(description: "first publication started")
    let (release, releaser) = AsyncStream<Void>.makeStream()
    let state = state(agents: [lena]) { _, text, _, _ in
      calls.append(text)
      if text == "A" {
        entered.fulfill()
        for await _ in release { break }
      }
      return Self.published
    }
    state.setConversationDraft("A", for: lena.id)
    await state.sendConversationText(lena)
    advance(10, state)
    await fulfillment(of: [entered], timeout: 2)
    state.setConversationDraft("B", for: lena.id)
    await state.sendConversationText(lena)
    XCTAssertEqual(state.conversationDraft(for: lena.id), "")
    XCTAssertEqual(calls, ["A"], "B waits for its own deadline, not for A's helper")
    XCTAssertEqual(state.heldSends.map(\.phase), [.dispatching, .holding])
    releaser.yield(())
    await settle { state.heldSends.first?.phase == .dispatched }
    XCTAssertEqual(state.heldSends.last?.phase, .holding)
    XCTAssertEqual(OverlayHeldSendStore.load(cacheRoot: root).map(\.text), ["B"])
  }

  func testAllAndSyncKeepTheirRecipientsRepliesAndCoAddresses() async throws {
    let agents = try ["1", "2"].map(agent)
    var calls: [(String?, String, OverlayUserReplyReference?, Set<String>)] = []
    let state = state(agents: agents) { owner, text, reply, peers in
      calls.append((owner?.id, text, reply, Set(peers.map(\.id))))
      return Self.published
    }
    // 0 · All: one broadcast row in All, published with no owner.
    state.setConversationDraft("Do wszystkich", for: "0")
    await state.sendConversationText(Self.all)
    let broadcast = try XCTUnwrap(heldRow(state, in: "0"))
    XCTAssertEqual(broadcast.sourceChannel, "0")
    XCTAssertNotNil(broadcast.holdSecondsRemaining)

    // Sync: each linked agent gets its own held row and its own reply.
    XCTAssertTrue(state.toggleDraftSync(from: agents[0].id, with: agents[1].id))
    let reply = try XCTUnwrap(
      OverlayUserReplyReference(row: ["message_id": "q", "author": "Agent 2", "text": "?"]))
    state.conversationReplies[agents[1].id] = reply
    let before = ConversationDraft.split(state.conversationDraft(for: agents[0].id)).text
    state.applyConversationProseEdit(
      OverlayComposerProseEdit(
        before: before, after: before + "Razem",
        replacements: [
          .init(range: NSRange(location: before.utf16.count, length: 0), replacement: "Razem")
        ]), for: agents[0].id)
    await state.sendConversationText(agents[0])
    XCTAssertEqual(calls.count, 0)
    for agent in agents {
      XCTAssertNotNil(heldRow(state, in: agent.id)?.holdSecondsRemaining)
      XCTAssertEqual(state.conversationDraft(for: agent.id), "")
    }

    advance(10, state)
    await settle { state.heldSends.allSatisfy { $0.phase == .dispatched } }
    XCTAssertEqual(calls.count, 3)
    let byOwner = Dictionary(uniqueKeysWithValues: calls.map { ($0.0 ?? "0", $0) })
    XCTAssertEqual(byOwner["0"]?.1, "Do wszystkich")
    XCTAssertEqual(byOwner[agents[0].id]?.1, "Razem")
    XCTAssertEqual(byOwner[agents[0].id]?.3, [agents[1].id])
    XCTAssertEqual(byOwner[agents[1].id]?.3, [agents[0].id])
    XCTAssertEqual(byOwner[agents[1].id]?.2, reply)
    XCTAssertNil(byOwner[agents[0].id]?.2)
  }

  func testTheInboxSendsThroughItsLinkedAgentAndThatSendIsHeldToo() async throws {
    let lena = try agent("1")
    let inbox = OverlayConversation(
      id: "10", channel: "10", name: "Inbox", owner: nil, messages: [])
    var calls: [(String?, String)] = []
    let state = state(agents: [lena], extra: [inbox]) { owner, text, _, _ in
      calls.append((owner?.id, text))
      return Self.published
    }
    XCTAssertTrue(state.toggleDraftSync(from: inbox.id, with: lena.id))
    state.applyConversationProseEdit(
      OverlayComposerProseEdit(
        before: "", after: "Ze skrzynki",
        replacements: [.init(range: NSRange(location: 0, length: 0), replacement: "Ze skrzynki")]),
      for: inbox.id)
    await state.sendConversationText(inbox)
    XCTAssertTrue(calls.isEmpty, "a send from the inbox waits out the hold like any other")
    XCTAssertNotNil(heldRow(state, in: lena.id)?.holdSecondsRemaining)
    XCTAssertNil(heldRow(state, in: inbox.id), "the inbox is a source, never a recipient")
    XCTAssertEqual(state.conversationDraft(for: inbox.id), "")
    advance(10, state)
    await settle { !calls.isEmpty }
    XCTAssertEqual(calls.map { $0.0 }, [lena.owner?.id])
    XCTAssertEqual(calls.map { $0.1 }, ["Ze skrzynki"])
  }

  func testARelaunchReturnsHeldMessagesToTheirComposersAndSendsNothing() async throws {
    let lena = try agent("1")
    var calls = 0
    let first = state(agents: [lena]) { _, _, _, _ in
      calls += 1
      return Self.published
    }
    first.setConversationDraft("Przerwana przez pad", for: lena.id)
    await first.sendConversationText(lena)
    XCTAssertEqual(OverlayHeldSendStore.load(cacheRoot: root).count, 1)
    // A killed process never ticks again: its clock stops here, or its own
    // clock task would see the next process's time and publish.
    let death = clock
    first.heldSendNow = { death }

    // The process dies during the hold. A new one reads the same directory.
    let relaunched = state(agents: [lena]) { _, _, _, _ in
      calls += 1
      return Self.published
    }
    XCTAssertEqual(relaunched.conversationDraft(for: lena.id), "Przerwana przez pad")
    XCTAssertNil(relaunched.textMessageErrors[lena.id])
    XCTAssertTrue(relaunched.heldSends.isEmpty)
    advance(60, relaunched)
    await settle { false }
    XCTAssertEqual(calls, 0, "a relaunch never publishes on its own")
    XCTAssertEqual(OverlayHeldSendStore.load(cacheRoot: root), [])
    XCTAssertEqual(
      OverlayComposerDraftStore.load(cacheRoot: root)?[lena.id]?.text, "Przerwana przez pad")

    // One that was being handed over may have arrived: it says so.
    var handing = try XCTUnwrap(first.heldSends.first)
    handing.phase = .dispatching
    try OverlayHeldSendStore.save(cacheRoot: root, sends: [handing])
    let third = state(agents: [lena]) { _, _, _, _ in
      calls += 1
      return Self.published
    }
    XCTAssertEqual(third.conversationDraft(for: lena.id), "Przerwana przez pad")
    XCTAssertNotNil(third.textMessageErrors[lena.id])
    XCTAssertEqual(calls, 0)
  }

  func testWithTheHoldOffEnterPublishesAtOnce() async throws {
    let lena = try agent("1")
    var calls: [String] = []
    let state = state(agents: [lena], seconds: 0) { _, text, _, _ in
      calls.append(text)
      return Self.published
    }
    state.setConversationDraft("Od razu", for: lena.id)
    await state.sendConversationText(lena)
    XCTAssertEqual(calls, ["Od razu"])
    XCTAssertNil(heldRow(state, in: lena.id))
    XCTAssertTrue(state.heldSends.isEmpty)
  }

  func testRefusedHeldSendInDraftSyncReturnsAttachmentsOnlyToFailedPeer() async throws {
    let configDirectory = CodescribeConfig().configDir()
    guard configDirectory != NSHomeDirectory() + "/.codescribe" else {
      throw XCTSkip("needs an isolated CODESCRIBE_DATA_DIR, as make test-swift provides")
    }
    let store = ConversationAttachmentPointer.configDirectory
      .appendingPathComponent("attachments/\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: store, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: store) }
    let files = try ["screen-1.png", "screen-2.png"].map { name -> String in
      let url = store.appendingPathComponent(name)
      try Data(name.utf8).write(to: url)
      return url.resolvingSymlinksInPath().path
    }

    let agents = try ["1", "2", "3"].map(agent)
    let failedID = agents[2].id
    var delivered: [String] = []
    let state = state(agents: agents) { owner, _, _, _ in
      if owner?.id == failedID {
        throw CocoaError(.fileWriteNoPermission)
      }
      if let id = owner?.id {
        delivered.append(id)
      }
      return Self.published
    }
    XCTAssertTrue(state.toggleDraftSync(from: Self.all.id, with: agents[0].id))
    XCTAssertTrue(state.toggleDraftSync(from: Self.all.id, with: agents[1].id))
    XCTAssertTrue(state.toggleDraftSync(from: Self.all.id, with: agents[2].id))

    let attachmentDraft = ConversationDraft.compose(text: "", attachments: files)
    state.setConversationDraft(attachmentDraft, for: Self.all.id)
    for peer in agents {
      XCTAssertEqual(
        ConversationDraft.split(state.conversationDraft(for: peer.id)).attachments.count, 2)
    }

    await state.sendConversationText(Self.all)
    XCTAssertEqual(state.conversationDraft(for: Self.all.id), "")
    for peer in agents {
      XCTAssertEqual(state.conversationDraft(for: peer.id), "")
    }

    advance(10, state)
    await settle {
      state.heldSends.count == 2 && state.heldSends.allSatisfy { $0.phase == .dispatched }
    }

    XCTAssertEqual(Set(delivered), [agents[0].id, agents[1].id])
    XCTAssertEqual(
      state.conversationDraft(for: Self.all.id), "",
      "Returned attachment from one failed peer must not re-populate 0 · All")
    XCTAssertEqual(
      state.conversationDraft(for: agents[0].id), "",
      "Returned attachment from one failed peer must not re-broadcast to delivered peer 1")
    XCTAssertEqual(
      state.conversationDraft(for: agents[1].id), "",
      "Returned attachment from one failed peer must not re-broadcast to delivered peer 2")
    let failedParts = ConversationDraft.split(state.conversationDraft(for: failedID))
    XCTAssertEqual(
      failedParts.attachments.count, 2,
      "Failed peer retains its returned attachments privately for retry")
  }

  func testSnapshotArrivingDuringDispatchingRetiresLocalRowWithoutDoubleBubble() async throws {
    let lena = try agent("1")
    let owner = try XCTUnwrap(lena.owner)
    let entered = expectation(description: "dispatch started")
    let (release, releaser) = AsyncStream<Void>.makeStream()
    var capturedMessageID: String?
    let state = state(agents: [lena]) { _, _, _, _ in
      let assigned = OverlayState.activeHeldDispatchMessageID
      capturedMessageID = assigned
      entered.fulfill()
      for await _ in release { break }
      return AgentBusTextPublication(
        messageID: assigned ?? Self.published.messageID, holdUntil: "", recipients: [owner])
    }

    state.setConversationDraft("Przelecisz przez wszystkie transkrypcje", for: Self.all.id)
    await state.sendConversationText(Self.all)
    let held = try XCTUnwrap(heldRow(state, in: Self.all.id))
    let sendTask = Task {
      try await state.controlDelivery(.release, message: held, owner: nil, text: nil)
    }
    await fulfillment(of: [entered], timeout: 2)

    let preassignedID = try XCTUnwrap(capturedMessageID)
    XCTAssertEqual(preassignedID.count, 24)
    let cliArgs = try RealAgentBridgeInstaller.busTextArguments(
      holdSeconds: 0, owner: nil, root: root, messageID: preassignedID)
    XCTAssertTrue(cliArgs.suffix(2) == ["--message-id", preassignedID])

    // FSEvents wakes the reader before x9bus finishes waking followers and exits.
    var busRow = OverlayConversationMessage(
      id: "typed:" + preassignedID, kind: .user,
      text: "Przelecisz przez wszystkie transkrypcje",
      order: 1, emittedAt: "", owner: nil,
      recipients: [.init(owner: owner, queued: true, accepted: false, acknowledged: false)],
      deliveryID: nil, replyTo: nil, unsolicited: false, busPath: "/hold/bus")
    busRow.controlIdentity = preassignedID
    busRow.deliverySealed = true
    let allDelivered = OverlayConversation(
      id: "0", channel: "0", name: "All", owner: nil, messages: [busRow])
    state.applyConversationSnapshot(
      .init(deliveries: [], conversations: [allDelivered, lena]), now: clock)

    XCTAssertNil(
      heldRow(state, in: Self.all.id),
      "local dispatching row must yield as soon as the bus snapshot carries its message_id")
    XCTAssertEqual(
      state.conversations.first { $0.id == Self.all.id }?.messages.count, 1,
      "0 · All must never show both the canonical Read 0/N row and a duplicate local row")
    XCTAssertTrue(state.heldSends.isEmpty)

    releaser.yield(())
    try await sendTask.value
    XCTAssertEqual(state.conversations.first { $0.id == Self.all.id }?.messages.count, 1)
    XCTAssertTrue(state.heldSends.isEmpty)
  }

  func testDistinctSendsWithIdenticalTextAreNeverCollapsedByTextEquality() async throws {
    let lena = try agent("1")
    let state = state(agents: [lena]) { _, _, _, _ in Self.published }
    state.setConversationDraft("Tak", for: Self.all.id)
    await state.sendConversationText(Self.all)
    state.setConversationDraft("Tak", for: Self.all.id)
    await state.sendConversationText(Self.all)

    let allMessages = try XCTUnwrap(state.conversations.first { $0.id == Self.all.id }?.messages)
    XCTAssertEqual(allMessages.count, 2, "two distinct sends with identical text remain two rows")
    XCTAssertNotEqual(state.heldSends.first?.messageID, state.heldSends.last?.messageID)
  }
}
