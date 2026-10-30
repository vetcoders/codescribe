import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// The recipient contract of 2026-10-30 (Maciej, 13:58–14:3x): the open
/// conversation is always the recipient; further recipients are the agents
/// whose drafts are linked to it; the inbox and 0 · All are sources that
/// compose to the agents linked to them, and the roster chain is that link.
@MainActor
final class OverlayRecipientRoutingTests: XCTestCase {
  private struct Publication: Equatable, Comparable {
    let owner: String?
    let text: String
    let alsoSentTo: [String]

    static func < (lhs: Publication, rhs: Publication) -> Bool {
      (lhs.owner ?? "") < (rhs.owner ?? "")
    }
  }

  @MainActor
  private final class Fixture {
    let state = OverlayState()
    let four: OverlayConversation
    let six: OverlayConversation
    let seven: OverlayConversation
    let all = OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let inbox = OverlayConversation(
      id: "10", channel: "10", name: "Your Inbox", owner: nil, messages: [])
    var publications: [Publication] = []
    /// Owner ids whose publication the fixture refuses.
    var refusing: Set<String> = []

    init() throws {
      four = try Self.agent("4", session: "routing-four")
      six = try Self.agent("6", session: "routing-six")
      seven = try Self.agent("7", session: "routing-seven")
      apply([all, inbox, four, six, seven])
      state.publishConversationText = { [weak self] owner, text, _, peers in
        guard let self else { return }
        publications.append(
          Publication(owner: owner?.id, text: text, alsoSentTo: peers.map(\.id).sorted()))
        if let id = owner?.id, refusing.contains(id) { throw CocoaError(.fileWriteNoPermission) }
      }
    }

    static func agent(_ digit: String, session: String) throws -> OverlayConversation {
      let owner = try XCTUnwrap(
        OverlayConversationOwner(row: [
          "provider": "codex", "provider_session_id": session,
          "lease_id": AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: session),
          "channel": digit, "name": "Agent \(digit)",
        ]))
      return .init(id: owner.id, channel: digit, name: owner.name, owner: owner, messages: [])
    }

    func apply(_ conversations: [OverlayConversation]) {
      state.applyChannelRoster(
        conversations.filter { $0.owner != nil || $0.channel == "0" }.map {
          CsChannelRosterState(
            channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
            providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
            autosealDeadlineUnixMs: nil, followerAlive: $0.owner == nil ? nil : true,
            unreadSinceUnixMs: nil)
        })
      state.applyConversationSnapshot(.init(deliveries: [], conversations: conversations))
    }

    /// Typing: the only path that mirrors words into linked drafts.
    func typeText(_ text: String, in id: String) {
      let before = ConversationDraft.split(state.conversationDraft(for: id)).text
      state.applyConversationProseEdit(
        OverlayComposerProseEdit(
          before: before, after: before + text,
          replacements: [
            .init(range: NSRange(location: before.utf16.count, length: 0), replacement: text)
          ]), for: id)
    }

    func draft(_ conversation: OverlayConversation) -> String {
      state.conversationDraft(for: conversation.id)
    }

    func members(of conversation: OverlayConversation) -> Set<String> {
      state.draftGroupIndex(for: conversation.id).map { state.conversationDraftGroups[$0].members }
        ?? []
    }

    func selectedChoices(for conversation: OverlayConversation) -> Set<String> {
      Set(state.draftSyncChoices(for: conversation).filter(\.selected).map(\.id))
    }

    func roster() -> OverlayChannelStatusView {
      OverlayChannelStatusView.intercomRoster(state: state, palette: .dark, animates: false)
    }
  }

  // MARK: The open conversation is the recipient

  func testSendingFromAnAgentsConversationPublishesToThatAgentOnly() async throws {
    let f = try Fixture()
    f.state.selectConversation(f.four.id)
    f.state.setConversationDraft("For six", for: f.six.id)
    f.state.setConversationDraft("Four's own words", for: f.four.id)
    XCTAssertEqual(f.state.resolvedComposerTarget(viewing: f.six)?.id, f.six.id)
    XCTAssertEqual(f.state.resolvedComposerTarget(viewing: f.four)?.id, f.four.id)
    await f.state.sendConversationText(f.six)
    XCTAssertEqual(
      f.publications, [Publication(owner: f.six.id, text: "For six", alsoSentTo: [])])
    XCTAssertEqual(f.draft(f.six), "")
    XCTAssertEqual(f.draft(f.four), "Four's own words", "nothing routed six's words to four")
    XCTAssertFalse(f.state.hasSynchronizedDraft(f.six.id))
    XCTAssertEqual(f.state.selectedConversationID, f.four.id)
  }

  func testLinkedAgentsEachReceiveTheirOwnDraftAndAreToldOfTheOther() async throws {
    let f = try Fixture()
    f.state.selectConversation(f.six.id)
    XCTAssertTrue(f.state.toggleDraftSync(from: f.six.id, with: f.four.id))
    f.typeText("Shared words", in: f.six.id)
    XCTAssertEqual(f.draft(f.four), "Shared words", "typing mirrors into the linked draft")
    XCTAssertEqual(f.draft(f.seven), "")
    await f.state.sendConversationText(f.six)
    XCTAssertEqual(
      f.publications.sorted(),
      [
        Publication(owner: f.four.id, text: "Shared words", alsoSentTo: [f.six.id]),
        Publication(owner: f.six.id, text: "Shared words", alsoSentTo: [f.four.id]),
      ].sorted())
    XCTAssertEqual(f.draft(f.six), "")
    XCTAssertEqual(f.draft(f.four), "")
    XCTAssertEqual(f.members(of: f.six), [f.six.id, f.four.id], "the link outlives the send")
  }

  // MARK: The inbox is a source

  func testInboxIsASourceNeverADestinationAndOffersAgentsOnly() throws {
    let f = try Fixture()
    XCTAssertTrue(f.inbox.isDraftSyncSource)
    XCTAssertTrue(f.all.isDraftSyncSource)
    XCTAssertFalse(f.four.isDraftSyncSource)
    XCTAssertFalse(f.state.availableComposerRecipients.contains { $0.id == f.inbox.id })
    XCTAssertTrue(f.state.availableComposerRecipients.contains { $0.id == f.all.id })
    XCTAssertNil(f.state.resolvedComposerTarget(viewing: f.inbox))
    XCTAssertFalse(f.state.composerTargetIsAvailable(f.inbox))
    let agents: Set<String> = [f.four.id, f.six.id, f.seven.id]
    XCTAssertEqual(Set(f.state.draftSyncChoices(for: f.inbox).map(\.id)), agents)
    XCTAssertEqual(Set(f.state.draftSyncChoices(for: f.all).map(\.id)), agents)
    for agent in [f.four, f.six, f.seven] {
      XCTAssertEqual(
        Set(f.state.draftSyncChoices(for: agent).map(\.id)), agents.subtracting([agent.id]),
        "an agent is offered the other agents, never the inbox or 0 · All")
    }
    XCTAssertTrue(f.state.draftSyncChoices(for: f.inbox).allSatisfy(\.enabled))
  }

  func testInboxSendPublishesEachLinkedAgentsDraftAndClearsWhenAllHoldReceipts() async throws {
    let f = try Fixture()
    f.state.selectConversation(f.inbox.id)
    XCTAssertTrue(f.state.toggleDraftSync(from: f.inbox.id, with: f.four.id))
    XCTAssertTrue(f.state.toggleDraftSync(from: f.inbox.id, with: f.seven.id))
    XCTAssertEqual(f.selectedChoices(for: f.inbox), [f.four.id, f.seven.id])
    f.typeText("Hello both", in: f.inbox.id)
    XCTAssertEqual(f.draft(f.four), "Hello both")
    XCTAssertEqual(f.draft(f.seven), "Hello both")
    XCTAssertEqual(f.draft(f.six), "")
    await f.state.sendConversationText(f.inbox)
    XCTAssertEqual(
      f.publications.sorted(),
      [
        Publication(owner: f.four.id, text: "Hello both", alsoSentTo: [f.seven.id]),
        Publication(owner: f.seven.id, text: "Hello both", alsoSentTo: [f.four.id]),
      ].sorted(), "the inbox itself is never published to")
    XCTAssertEqual(f.draft(f.inbox), "")
    XCTAssertEqual(f.draft(f.four), "")
    XCTAssertEqual(f.draft(f.seven), "")
    XCTAssertEqual(f.members(of: f.inbox), [f.inbox.id, f.four.id, f.seven.id])
    XCTAssertTrue(f.state.pendingTextMessages.isEmpty)
    XCTAssertNil(f.state.draftSyncSendError(for: f.inbox.id))
  }

  func testInboxDraftStaysWhileOneLinkedAgentFailedAndClearsOnRetry() async throws {
    let f = try Fixture()
    f.state.selectConversation(f.inbox.id)
    XCTAssertTrue(f.state.toggleDraftSync(from: f.inbox.id, with: f.four.id))
    XCTAssertTrue(f.state.toggleDraftSync(from: f.inbox.id, with: f.seven.id))
    f.typeText("Hello both", in: f.inbox.id)
    f.refusing = [f.seven.id]
    await f.state.sendConversationText(f.inbox)
    XCTAssertEqual(f.publications.count, 2)
    XCTAssertEqual(f.draft(f.four), "", "four holds its receipt")
    XCTAssertEqual(f.draft(f.seven), "Hello both", "seven's publication failed")
    XCTAssertEqual(f.draft(f.inbox), "Hello both", "the inbox keeps the words for a retry")
    XCTAssertNotNil(f.state.textMessageErrors[f.seven.id])
    XCTAssertNotNil(f.state.draftSyncSendError(for: f.inbox.id))
    f.refusing = []
    f.publications = []
    await f.state.sendConversationText(f.inbox)
    XCTAssertEqual(
      f.publications, [Publication(owner: f.seven.id, text: "Hello both", alsoSentTo: [])],
      "only the missing agent is retried")
    XCTAssertEqual(f.draft(f.inbox), "")
    XCTAssertEqual(f.draft(f.seven), "")
    XCTAssertNil(f.state.draftSyncSendError(for: f.inbox.id))
  }

  func testInboxWithNoLinkedAgentPublishesNothing() async throws {
    let f = try Fixture()
    f.state.setConversationDraft("Nobody yet", for: f.inbox.id)
    await f.state.sendConversationText(f.inbox)
    XCTAssertTrue(f.publications.isEmpty)
    XCTAssertEqual(f.draft(f.inbox), "Nobody yet")
  }

  // MARK: 0 · All narrows to a set and keeps it

  func testBroadcastWithoutALinkIsTheOrdinaryChannelZeroSend() async throws {
    let f = try Fixture()
    f.state.setConversationDraft("To everyone", for: f.all.id)
    await f.state.sendConversationText(f.all)
    XCTAssertEqual(f.publications, [Publication(owner: nil, text: "To everyone", alsoSentTo: [])])
    XCTAssertEqual(f.draft(f.all), "")
  }

  func testBroadcastLinkedWithAnAgentSendsOnlyToItAndTheSetPersists() async throws {
    let f = try Fixture()
    f.state.selectConversation(f.all.id)
    XCTAssertTrue(f.state.toggleDraftSync(from: f.all.id, with: f.four.id))
    f.typeText("Only four", in: f.all.id)
    XCTAssertEqual(f.draft(f.four), "Only four")
    await f.state.sendConversationText(f.all)
    XCTAssertEqual(
      f.publications, [Publication(owner: f.four.id, text: "Only four", alsoSentTo: [])])
    XCTAssertEqual(f.draft(f.all), "")
    XCTAssertEqual(f.draft(f.four), "")
    // "wyślij nie jest one shot" (Maciej, 2026-10-30 14:3x): the set stays.
    XCTAssertTrue(f.state.hasSynchronizedDraft(f.all.id))
    XCTAssertEqual(f.selectedChoices(for: f.all), [f.four.id])
    f.publications = []
    f.typeText("Again", in: f.all.id)
    await f.state.sendConversationText(f.all)
    XCTAssertEqual(f.publications, [Publication(owner: f.four.id, text: "Again", alsoSentTo: [])])
    // Only the menu or the chain changes it: unlinked, the next message is a broadcast.
    f.state.stopDraftSync(for: f.all.id)
    XCTAssertFalse(f.state.hasSynchronizedDraft(f.all.id))
    f.publications = []
    f.state.setConversationDraft("Everyone again", for: f.all.id)
    await f.state.sendConversationText(f.all)
    XCTAssertEqual(
      f.publications, [Publication(owner: nil, text: "Everyone again", alsoSentTo: [])])
  }

  // MARK: The roster chain

  func testRosterChainLinksTheRowWithTheOpenConversationAndUnlinksOnItsOwnRow() throws {
    let f = try Fixture()
    f.state.selectConversation(f.six.id)
    XCTAssertTrue(f.roster().syncedConversationIDs.isEmpty)
    let toggle = try XCTUnwrap(f.roster().onToggleDraftSync)
    toggle(f.four.id)
    XCTAssertEqual(f.members(of: f.six), [f.six.id, f.four.id])
    XCTAssertEqual(f.roster().syncedConversationIDs, [f.six.id, f.four.id])
    try XCTUnwrap(f.roster().onToggleDraftSync)(f.seven.id)
    XCTAssertEqual(f.members(of: f.six), [f.six.id, f.four.id, f.seven.id])
    try XCTUnwrap(f.roster().onToggleDraftSync)(f.four.id)
    XCTAssertEqual(f.members(of: f.six), [f.six.id, f.seven.id], "a second press unlinks")
    try XCTUnwrap(f.roster().onToggleDraftSync)(f.six.id)
    XCTAssertFalse(f.state.hasSynchronizedDraft(f.six.id), "the open row's chain unlinks it")
    XCTAssertFalse(f.state.hasSynchronizedDraft(f.seven.id))
    XCTAssertTrue(f.roster().syncedConversationIDs.isEmpty)
  }

  func testRosterChainLinksWithTheInboxAndWithBroadcastWhenTheyAreOpen() throws {
    let f = try Fixture()
    f.state.selectConversation(f.inbox.id)
    try XCTUnwrap(f.roster().onToggleDraftSync)(f.seven.id)
    XCTAssertEqual(f.members(of: f.inbox), [f.inbox.id, f.seven.id])
    XCTAssertTrue(f.roster().syncedConversationIDs.contains(f.seven.id))
    f.state.selectConversation(f.all.id)
    try XCTUnwrap(f.roster().onToggleDraftSync)(f.four.id)
    XCTAssertEqual(f.members(of: f.all), [f.all.id, f.four.id])
    XCTAssertEqual(f.members(of: f.inbox), [f.inbox.id, f.seven.id], "sets are independent")
    f.state.selectConversation(nil)
    try XCTUnwrap(f.roster().onToggleDraftSync)(f.six.id)
    XCTAssertFalse(f.state.hasSynchronizedDraft(f.six.id), "nothing open, nothing to link to")
  }

  func testRosterRowMarksSitOnAgentRowsOnlyAndThePanelButtonFollowsReachability() throws {
    let f = try Fixture()
    let allRow = OverlayChannelDelivery(
      channel: "0", agent: "All", deliveryID: nil, stage: nil, isOpen: false)
    XCTAssertNil(f.roster().rowMarks(for: allRow, conversation: f.all), "nothing on the All row")
    XCTAssertNil(f.roster().rowMarks(for: nil, conversation: f.four))
    let fourRow = try XCTUnwrap(f.state.visibleChannelRows.first { $0.channel == "4" })
    XCTAssertNotNil(f.roster().rowMarks(for: fourRow, conversation: f.four))
    XCTAssertNil(f.roster().panelControl(for: fourRow), "no panel evidence, no button")

    let owner = try XCTUnwrap(f.four.owner)
    var focused: [OverlayConversationOwner] = []
    func makeView(followerAlive: Bool, chain: Bool) -> OverlayChannelStatusView {
      var view = OverlayChannelStatusView(
        channels: [fourRow], unavailable: false, palette: .dark, animates: false,
        hudStates: [
          "4": OverlayChannelHudProjection(
            open: false, loud: false, autosealDeadline: nil, followerAlive: followerAlive,
            provider: owner.provider, providerSessionID: owner.providerSessionID,
            frameDetected: true)
        ],
        conversations: [f.four], selectedConversationID: f.four.id, unreadCounts: [:])
      view.archiveCandidates = ["4": owner]
      view.onArchiveAgent = { _ in }
      view.panelFocusAvailable = true
      view.onFocusAgentPanel = { focused.append($0) }
      if chain { view.onToggleDraftSync = { _ in } }
      return view
    }
    let live = makeView(followerAlive: true, chain: false)
    XCTAssertTrue(live.showsPanelFocusButton(for: fourRow))
    let control = try XCTUnwrap(live.panelControl(for: fourRow))
    XCTAssertEqual(control.symbol, live.panelFocusSymbol(nil, target: .frame))
    XCTAssertEqual(control.hint, live.panelFocusHelp(for: fourRow, state: nil))
    XCTAssertTrue(control.enabled)
    XCTAssertNotNil(live.rowMarks(for: fourRow, conversation: f.four))
    control.action()
    XCTAssertEqual(focused, [owner], "one press reaches the frozen owner")

    let dead = makeView(followerAlive: false, chain: false)
    XCTAssertFalse(dead.showsPanelFocusButton(for: fourRow))
    XCTAssertNil(dead.panelControl(for: fourRow), "hidden when the button would not show")
    XCTAssertNil(dead.rowMarks(for: fourRow, conversation: f.four), "no chain, no button, no marks")
    XCTAssertNotNil(
      makeView(followerAlive: false, chain: true).rowMarks(for: fourRow, conversation: f.four),
      "the chain alone still earns the marks")
  }

  // MARK: Recipients live in the chevron beside Send, on sources only

  private func overlaySource(_ file: String) throws -> String {
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent()
    return try String(
      contentsOf: root.appendingPathComponent("Codescribe/Screens/Overlay/\(file)"),
      encoding: .utf8
    ).split(whereSeparator: \.isWhitespace).joined(separator: " ")
  }

  /// The bar above the composer is gone (Maciej, 2026-10-30 on build 2981:
  /// "ta druga tafla ponad komposerem to jest taka wiocha. Nie możemy tego
  /// zrobić jako dziubek w przycisku wysyłki?"): the inbox and 0 · All pick
  /// their set from a chevron beside Send, the "+" stays the plain attach
  /// button, an agent's conversation gets neither.
  func testRecipientsAreAChevronBesideSendOnSourcesAndTheBarIsGone() throws {
    let window = try overlaySource("DictationOverlayView.swift")
    XCTAssertFalse(window.contains("composerRouting"), "no bar above the composer")
    XCTAssertFalse(window.contains("\"overlay-composer-routing\""))
    XCTAssertFalse(window.contains("\"overlay-composer-recipient\""))
    XCTAssertFalse(window.contains("Voice channels"))
    XCTAssertFalse(window.contains("chooseComposerTarget"), "no recipient override remains")
    XCTAssertTrue(
      window.contains(
        "draftSyncChoices: conversation.isDraftSyncSource ? state.draftSyncChoices(for: conversation) : []"
      ), "only a source offers choices to the composer")
    XCTAssertTrue(
      window.contains(
        "onToggleDraftSync: conversation.isDraftSyncSource ? { state.toggleDraftSync(from: conversation.id, with: $0) } : nil"
      ))
    XCTAssertTrue(
      window.contains(
        "onStopDraftSync: conversation.isDraftSyncSource ? { state.stopDraftSync(for: conversation.id) } : nil"
      ))

    let view = try overlaySource("OverlayConversationView.swift")
    XCTAssertFalse(view.contains("composerRouting"))
    XCTAssertTrue(view.contains("recipientsOnSend: conversation.isDraftSyncSource"))
    // The sticky set is named in the placeholder, since the chevron does not show it.
    XCTAssertTrue(view.contains("let names = draftSyncNames.filter { $0 != conversation.name }"))
    XCTAssertTrue(
      view.contains(
        "if conversation.isHumanInbox { return names.isEmpty ? String(localized: \"Choose recipients\") : String(localized: \"To:\") + \" \" + names.joined(separator: \", \") }"
      ))
    XCTAssertTrue(
      view.contains(
        "return names.isEmpty ? String(localized: \"Send to everyone\") : String(localized: \"Only to: \\(names.joined(separator: \", \"))\")"
      ))
    XCTAssertTrue(view.contains("return String(localized: \"Message the Agent\")"))
    let catalog = try String(
      contentsOf: URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Codescribe/Resources/Localization/Localizable.xcstrings"),
      encoding: .utf8)
    XCTAssertTrue(catalog.contains("\"Only to: %@\""), "the new placeholder is in the catalog")
    XCTAssertTrue(catalog.contains("\"Tylko do: %@\""), "with its Polish")

    let composer = try overlaySource("OverlayConversationComposer.swift")
    XCTAssertTrue(composer.contains("var recipientsOnSend = false"))
    XCTAssertTrue(
      composer.contains(
        "private var offersDraftSyncHere: Bool { !recipientsOnSend && (onToggleDraftSync != nil || onActivateDraftSync != nil) }"
      ), "with the chevron, the \"+\" is a plain attach button")
    XCTAssertTrue(
      composer.contains(
        "private var showsRecipientMenu: Bool { recipientsOnSend && onToggleDraftSync != nil && !draftSyncChoices.isEmpty }"
      ))
    XCTAssertTrue(composer.contains("if showsRecipientMenu { recipientMenu(corners) }"))
    XCTAssertTrue(composer.contains("\"overlay-conversation-recipients\""))
    XCTAssertTrue(
      composer.contains(
        "if draftSyncActive, let onStopDraftSync { Divider() "
          + "Button(action: onStopDraftSync) { Text(verbatim: clearRecipientsTitle) } }"),
      "a chosen set can be dropped from the same menu")
    // Clearing means different things: 0 · All goes back to everyone, the
    // inbox back to nobody, so the view names the item.
    XCTAssertTrue(
      view.contains(
        "clearRecipientsTitle: conversation.isHumanInbox ? String(localized: \"Stop syncing\") "
          + ": String(localized: \"Send to everyone\")"))
    XCTAssertTrue(composer.contains("\"overlay-conversation-plus\""))
    XCTAssertTrue(composer.contains("\"overlay-conversation-attach\""))
  }

  /// The chevron is a narrower action beside Send; the trailing exclusion of
  /// the text grows by its width plus one spacing when it shows, and by
  /// nothing otherwise. The layout type is private to the composer, so the
  /// arithmetic is pinned at the source.
  func testTheRecipientChevronWidensTheTrailingCornerByItsWidthAndOneSpacing() throws {
    let composer = try overlaySource("OverlayConversationComposer.swift")
    XCTAssertTrue(composer.contains("let recipientShare: CGFloat = showsRecipientMenu ? 0.6 : 0"))
    XCTAssertTrue(
      composer.contains(
        "recipientWidth = showsRecipientMenu ? ceil(actionWidth * recipientShare) : 0"))
    XCTAssertTrue(
      composer.contains(
        "trailingWidth = trailingActions * actionWidth + recipientWidth "
          + "+ (trailingActions - 1 + (showsRecipientMenu ? 1 : 0)) * spacing"))
    XCTAssertTrue(
      composer.contains(
        "x: viewport.width - trailingWidth - clearance - textOrigin.x, y: y, width: trailingWidth + clearance"
      ), "the trailing exclusion is the trailing width")
    XCTAssertTrue(
      composer.contains(".frame(width: corners.recipientWidth, height: corners.actionHeight)"),
      "the chevron is drawn at the width the layout reserved")
  }
}
