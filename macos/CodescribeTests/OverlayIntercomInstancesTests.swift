import Foundation
import XCTest

@testable import Codescribe

@MainActor
final class OverlayIntercomInstancesTests: XCTestCase {
  private func identity(
    _ name: String, channel: String, lease: String
  ) throws -> OverlayIntercomCardIdentity {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "\(name)-session",
        "lease_id": String(repeating: lease, count: 32), "channel": channel, "name": name,
      ]))
    return OverlayIntercomCardIdentity(
      digit: channel,
      conversation: OverlayConversation(
        id: owner.id, channel: channel, name: name, owner: owner, messages: []))
  }

  /// Maciej, 2026-10-22: the same agent can keep two fiszki in two places.
  /// Each instance owns its geometry and window state; closing one leaves the
  /// other standing.
  func testTwoInstancesOfTheSameAgentLiveAndCloseIndependently() throws {
    let lena = try identity("lena", channel: "2", lease: "a")
    var desktop = OverlayIntercomDesktopState()
    let first = OverlayIntercomInstance(
      identity: lena, frame: CGRect(x: 100, y: 100, width: 380, height: 360))
    let second = OverlayIntercomInstance(
      identity: lena, frame: CGRect(x: 900, y: 500, width: 320, height: 240), minimized: true)
    desktop.upsert(first)
    desktop.upsert(second)

    XCTAssertNotEqual(first.id, second.id, "a view instance is not the conversation")
    XCTAssertEqual(desktop.instances.count, 2)

    var closed = second
    closed.closed = true
    desktop.upsert(closed)
    XCTAssertEqual(desktop.instances.count, 2, "closing records state, it does not forget")
    XCTAssertFalse(desktop.instances[0].closed)
    XCTAssertTrue(desktop.instances[1].closed)
    XCTAssertTrue(desktop.instances[1].minimized, "mini survives beside closed")

    desktop.remove(first.id)
    XCTAssertEqual(desktop.instances.map(\.id), [second.id])
  }

  /// Restore is a UI operation: geometry, mini and closed come back exactly,
  /// and an instance follows its exact owner, never a digit or a name.
  func testDesktopSurvivesReopeningAndKeepsTheExactOwner() throws {
    let name = "CodescribeTests.intercom-desktop.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    let lena = try identity("lena", channel: "3", lease: "a")
    let usurper = try identity("lena", channel: "3", lease: "b")

    var desktop = OverlayIntercomDesktopState(groupScale: 1.6)
    let instance = OverlayIntercomInstance(
      identity: lena, frame: CGRect(x: 40, y: 60, width: 400, height: 320), minimized: true)
    desktop.upsert(instance)
    OverlayIntercomDesktopStore(defaults: defaults).save(desktop)

    let restored = OverlayIntercomDesktopStore(defaults: defaults).load()
    XCTAssertEqual(restored, desktop)
    let back = try XCTUnwrap(restored.instances.first)
    XCTAssertEqual(back.frame, CGRect(x: 40, y: 60, width: 400, height: 320))
    XCTAssertTrue(back.minimized)

    let lenaConversation = OverlayConversation(
      id: back.conversationID, channel: "3", name: "lena",
      owner: OverlayConversationOwner(
        row: [
          "provider": "codex", "provider_session_id": "lena-session",
          "lease_id": String(repeating: "a", count: 32), "channel": "3", "name": "lena",
        ]), messages: [])
    XCTAssertTrue(back.matches(lenaConversation))
    XCTAssertEqual(usurper.digit, back.digit)
    XCTAssertEqual(usurper.name, back.name)
    XCTAssertNotEqual(usurper.ownerID, back.ownerID, "same digit and name is another lease")
  }

  /// A stored instance that lost its exact conversation identity is dropped
  /// on load instead of being guessed onto a new owner.
  func testLoadDropsInstancesWithoutACanonicalIdentityAndClampsTheScale() throws {
    let name = "CodescribeTests.intercom-desktop.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }

    let orphan = OverlayIntercomInstance(
      identity: OverlayIntercomCardIdentity(digit: "4", conversation: nil),
      frame: CGRect(x: 0, y: 0, width: 300, height: 240))
    var desktop = OverlayIntercomDesktopState(groupScale: 9)
    desktop.upsert(orphan)
    OverlayIntercomDesktopStore(defaults: defaults).save(desktop)

    let restored = OverlayIntercomDesktopStore(defaults: defaults).load()
    XCTAssertTrue(restored.instances.isEmpty)
    XCTAssertEqual(restored.groupScale, OverlayIntercomDesktopState.maximumGroupScale)
    XCTAssertEqual(OverlayIntercomDesktopStore(defaults: defaults).load().groupScale, 2.5)
  }

  /// The group resizes like a window: the drag ratio becomes the shared
  /// proportional scale, clamped to the sane range.
  func testGroupScaleFollowsTheDragRatioAndStaysInRange() {
    XCTAssertEqual(
      OverlayIntercomDesktopState.groupScale(current: 1, currentWidth: 200, requestedWidth: 300),
      1.5)
    XCTAssertEqual(
      OverlayIntercomDesktopState.groupScale(current: 1.5, currentWidth: 300, requestedWidth: 200),
      1.0)
    XCTAssertEqual(
      OverlayIntercomDesktopState.groupScale(current: 2, currentWidth: 200, requestedWidth: 2000),
      OverlayIntercomDesktopState.maximumGroupScale)
    XCTAssertEqual(
      OverlayIntercomDesktopState.groupScale(current: 1, currentWidth: 200, requestedWidth: 20),
      OverlayIntercomDesktopState.minimumGroupScale)
    XCTAssertEqual(
      OverlayIntercomDesktopState.groupScale(current: 1, currentWidth: 0, requestedWidth: 300), 1,
      "a degenerate width changes nothing")
  }
}
