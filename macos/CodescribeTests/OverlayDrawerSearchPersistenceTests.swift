import SwiftUI
import XCTest

@testable import Codescribe

/// The agent drawer's saved-conversation search must keep its query across
/// overlay body passes. `DictationOverlayView.channelStatusView` rebuilds the
/// `OverlayChannelStatusView` value on every pass and installs only its
/// `monitorBody(...)`, so a `@State` declared on that value never persists:
/// each HUD tick handed the drawer a fresh, empty model and the query the user
/// was typing vanished. The overlay state owns the model instead.
@MainActor
final class OverlayDrawerSearchPersistenceTests: XCTestCase {
  func testDrawerUsesTheOverlayStatesSearchModelOnEveryBodyPass() {
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(
      .init(
        deliveries: [],
        conversations: [
          OverlayConversation(
            id: "saved-1", channel: "1", name: "klaudiusz", owner: nil,
            messages: [
              OverlayConversationMessage(
                id: "reply:saved-1-0", kind: .reply, text: "Zapisane jako dwa punkty.",
                order: 0, emittedAt: "2026-10-14T12:20:00Z", owner: nil,
                recipients: [], deliveryID: nil, replyTo: nil, unsolicited: false,
                playback: nil, busPath: "")
            ])
        ]))
    let overlay = DictationOverlayView(state: state)

    let first = overlay.channelStatusView.activeSearchModel
    XCTAssertTrue(first === state.savedConversationSearch)
    first.query = "piszę coś"

    // A channel status change re-evaluates the overlay body and rebuilds the
    // drawer value; the search model it receives must be the same object.
    state.toggleAgentSidebar()
    let second = overlay.channelStatusView.activeSearchModel
    XCTAssertTrue(first === second, "drawer rebuilt with a different search model")
    XCTAssertEqual(second.query, "piszę coś")
    XCTAssertEqual(state.savedConversationSearch.query, "piszę coś")
  }

  func testDrawerValueWithoutInjectedModelDoesNotPersistItsQuery() {
    // Documents why the state owns the model: a bare drawer value's internal
    // model lives only as long as that value.
    func drawer() -> OverlayChannelStatusView {
      OverlayChannelStatusView(
        channels: [], unavailable: false, palette: .dark, animates: false)
    }
    let a = drawer().activeSearchModel
    a.query = "piszę coś"
    XCTAssertFalse(a === drawer().activeSearchModel)
  }
}
