import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayDividerFoldTests: XCTestCase {
  private let scales: [CGFloat] = [1, 1.6]

  func testDrawerWidthUsesTheRowMinimumWhenTheDividerIsNotDragging() {
    for scale in scales {
      for preferred in [CGFloat(0), 120, 239] {
        XCTAssertEqual(
          OverlayAgentDrawerLayout.drawerWidth(
            textScale: scale, preferredWidth: preferred, windowWidth: 1_200 * scale),
          240 * scale, accuracy: 0.001)
      }
      XCTAssertEqual(
        OverlayAgentDrawerLayout.drawerWidth(textScale: scale, preferredWidth: 120),
        240 * scale, accuracy: 0.001)
    }
  }

  func testAWideDividerReleaseStillLeavesTheConversationMinimum() {
    for scale in scales {
      let width = OverlayAgentDrawerLayout.drawerWidth(
        textScale: scale, preferredWidth: 900, windowWidth: 1_200 * scale)
      XCTAssertEqual(width, 880 * scale, accuracy: 0.001)
      XCTAssertEqual(1_200 * scale - width, 320 * scale, accuracy: 0.001)
    }
  }

  func testFoldThresholdSeparatesTheRailFromTheRowsAtEachTextScale() {
    for scale in scales {
      let rail = OverlayAgentDrawerLayout.railWidth(textScale: scale)
      let fold = OverlayAgentDrawerLayout.railFoldWidth(textScale: scale)
      let rows = OverlayAgentDrawerLayout.contentMinimumWidth(textScale: scale)
      let conversation = OverlayAgentDrawerLayout.minimumConversationWidth(textScale: scale)
      XCTAssertEqual(fold, 160 * scale, accuracy: 0.001)
      XCTAssertEqual(rows, 240 * scale, accuracy: 0.001)
      XCTAssertLessThan(rail, fold)
      XCTAssertLessThan(fold, rows)
      XCTAssertEqual(
        OverlayAgentDrawerLayout.minimumSplitWindowWidth(textScale: scale),
        rows + conversation, accuracy: 0.001)
      XCTAssertEqual(
        OverlayAgentDrawerLayout.minimumSplitWindowWidth(textScale: scale),
        560 * scale, accuracy: 0.001)
    }
    XCTAssertEqual(OverlayAgentDrawerLayout.railUnfoldDrag, 40)
  }

  func testRosterMessageScaleMatchesTheConversationBodyFont() {
    XCTAssertEqual(OverlayConversationMessageContent.bodyFontSize, 13, accuracy: 0.001)
  }

  @MainActor
  func testRosterPreviewSessionBrowsesLastFiveMessagesWithoutWrappingAndFreezesWhileOpen() {
    let makeMessage = { (id: String, text: String) in
      OverlayConversationMessage(
        id: id, kind: .reply, text: text, order: 1,
        emittedAt: "2026-11-04T02:00:00Z", owner: nil, recipients: [],
        deliveryID: id, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
    }
    let emptyConversation = OverlayConversation(
      id: "2", channel: "2", name: "Tola", owner: nil, messages: [])
    let emptySession = OverlayRosterPreviewSession(conversation: emptyConversation)
    XCTAssertTrue(emptySession.ids.isEmpty)
    XCTAssertFalse(emptySession.canStepOlder)
    XCTAssertFalse(emptySession.canStepNewer)

    let messages = (1...6).map { makeMessage("m\($0)", "Body \($0)") }
    let conversation = OverlayConversation(
      id: "2", channel: "2", name: "Tola", owner: nil, messages: messages)
    let session = OverlayRosterPreviewSession(conversation: conversation)
    session.openLatest(ownerID: "2")
    XCTAssertEqual(session.ids, ["m2", "m3", "m4", "m5", "m6"])
    XCTAssertEqual(session.index, 4)
    XCTAssertTrue(session.canStepOlder)
    XCTAssertFalse(session.canStepNewer)

    session.stepNewer()
    XCTAssertEqual(session.index, 4, "must not wrap past newest")

    session.stepOlder()
    session.stepOlder()
    XCTAssertEqual(session.index, 2)
    XCTAssertEqual(session.ids[session.index], "m4")

    // A new message arriving while browsing must not shift the viewed slot.
    let updatedMessages = messages + [makeMessage("m7", "Body 7")]
    let updatedConversation = OverlayConversation(
      id: "2", channel: "2", name: "Tola", owner: nil, messages: updatedMessages)
    session.update(updatedConversation, withdrawnIDs: ["m3"])
    XCTAssertEqual(session.ids, ["m2", "m3", "m4", "m5", "m6"])
    XCTAssertEqual(session.index, 2)
    XCTAssertEqual(session.message(for: "m4")?.text, "Body 4")
    XCTAssertTrue(session.isWithdrawn("m3"))
    XCTAssertNil(session.message(for: "m3"))

    session.stepOlder()
    session.stepOlder()
    XCTAssertEqual(session.index, 0)
    XCTAssertFalse(session.canStepOlder)
    session.stepOlder()
    XCTAssertEqual(session.index, 0, "must not wrap past oldest")

    session.close()
    XCTAssertEqual(session.ids, ["m3", "m4", "m5", "m6", "m7"])
    XCTAssertEqual(session.index, 4)
  }
}
