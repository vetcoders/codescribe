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

  /// Maciej, 2026-10-23: one agent keeps one open fiszka. The desktop finds
  /// the open window for a card identity; a closed record is a saved window
  /// and never counts, mini still counts, and another owner behind the same
  /// name is another conversation.
  func testOneOpenInstancePerConversation() throws {
    let lena = try identity("lena", channel: "2", lease: "a")
    let usurper = try identity("lena", channel: "2", lease: "b")
    var desktop = OverlayIntercomDesktopState()
    XCTAssertNil(desktop.openInstance(for: lena), "nothing detached yet")

    let saved = OverlayIntercomInstance(
      identity: lena, frame: CGRect(x: 100, y: 100, width: 380, height: 360), closed: true)
    desktop.upsert(saved)
    XCTAssertNil(
      desktop.openInstance(for: lena), "a closed record is a saved window, not an open one")

    let mini = OverlayIntercomInstance(
      identity: lena, frame: CGRect(x: 900, y: 500, width: 320, height: 240), minimized: true)
    desktop.upsert(mini)
    XCTAssertEqual(desktop.openInstance(for: lena)?.id, mini.id, "mini is still an open window")
    XCTAssertNil(
      desktop.openInstance(for: usurper),
      "another owner behind the same name and digit is another conversation")

    var closed = mini
    closed.closed = true
    desktop.upsert(closed)
    XCTAssertEqual(desktop.instances.count, 2, "closing records state, it does not forget")
    XCTAssertNil(desktop.openInstance(for: lena), "closing frees the row for a fresh detach")
  }

  /// Restore is a UI operation: geometry, mini and closed come back exactly,
  /// and an instance follows its exact owner, never a digit or a name.
  func testDesktopSurvivesReopeningAndKeepsTheExactOwner() throws {
    let name = "CodescribeTests.intercom-desktop.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    let lena = try identity("lena", channel: "3", lease: "a")
    let usurper = try identity("lena", channel: "3", lease: "b")

    var desktop = OverlayIntercomDesktopState(
      groupWidth: 520, groupRowHeight: 84, groupTier: .actions)
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
  func testLoadDropsInstancesWithoutACanonicalIdentity() throws {
    let name = "CodescribeTests.intercom-desktop.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }

    let orphan = OverlayIntercomInstance(
      identity: OverlayIntercomCardIdentity(digit: "4", conversation: nil),
      frame: CGRect(x: 0, y: 0, width: 300, height: 240))
    var desktop = OverlayIntercomDesktopState()
    desktop.upsert(orphan)
    OverlayIntercomDesktopStore(defaults: defaults).save(desktop)

    let restored = OverlayIntercomDesktopStore(defaults: defaults).load()
    XCTAssertTrue(restored.instances.isEmpty)
  }

  /// Maciej, 2026-10-23: fiszki must take channel 0 too. The broadcast
  /// conversation is the one legitimate ownerless identity: its instance
  /// persists across a reload and matches the live "0 · All" conversation,
  /// while an ownerless instance of any other digit still drops on load.
  func testBroadcastChannelZeroInstancePersistsAndMatchesWithoutAnOwner() throws {
    let name = "CodescribeTests.intercom-desktop.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }

    let broadcast = OverlayConversation(
      id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let identity = OverlayIntercomCardIdentity(digit: "0", conversation: broadcast)
    XCTAssertNil(identity.ownerID, "the broadcast conversation has no owner")

    var desktop = OverlayIntercomDesktopState()
    desktop.upsert(
      OverlayIntercomInstance(
        identity: identity, frame: CGRect(x: 10, y: 20, width: 360, height: 300)))
    OverlayIntercomDesktopStore(defaults: defaults).save(desktop)

    let restored = OverlayIntercomDesktopStore(defaults: defaults).load()
    let back = try XCTUnwrap(restored.instances.first, "channel 0 survives the reload")
    XCTAssertTrue(back.matches(broadcast))
    XCTAssertEqual(back.digit, "0")

    let agentConversation = OverlayConversation(
      id: "0", channel: "0", name: "All",
      owner: OverlayConversationOwner(
        row: [
          "provider": "codex", "provider_session_id": "x-session",
          "lease_id": String(repeating: "c", count: 32), "channel": "0", "name": "x",
        ]), messages: [])
    XCTAssertFalse(
      back.matches(agentConversation), "an owned conversation is not the broadcast instance")
  }

  func testEveryContentTierAndItsDimensionsSurviveStoreRoundTrip() throws {
    let name = "CodescribeTests.intercom-desktop.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    let store = OverlayIntercomDesktopStore(defaults: defaults)

    for tier in IntercomContentTier.allCases {
      let desktop = OverlayIntercomDesktopState(
        groupWidth: 412, groupRowHeight: 72, groupTier: tier)
      store.save(desktop)
      XCTAssertEqual(
        store.load(), desktop, "the selected \(tier) tier restores with its dimensions")
    }
  }

  func testDesktopSavedBeforeContentTiersPreservesInstancesAndIgnoresItsScale() throws {
    let name = "CodescribeTests.intercom-desktop.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    let instance = OverlayIntercomInstance(
      identity: try identity("lena", channel: "2", lease: "a"),
      frame: CGRect(x: 40, y: 60, width: 400, height: 320), minimized: true)
    let instances = try JSONSerialization.jsonObject(with: JSONEncoder().encode([instance]))
    let data = try JSONSerialization.data(withJSONObject: [
      "groupScale": 2.5, "instances": instances,
    ])
    defaults.set(data, forKey: OverlayIntercomDesktopStore.key)
    let store = OverlayIntercomDesktopStore(defaults: defaults)

    let restored = store.load()
    XCTAssertEqual(restored.instances, [instance])
    XCTAssertNil(restored.groupWidth)
    XCTAssertNil(restored.groupRowHeight)
    XCTAssertEqual(restored.groupTier, .actions)

    store.save(restored)
    let savedData = try XCTUnwrap(defaults.data(forKey: OverlayIntercomDesktopStore.key))
    let saved = try XCTUnwrap(JSONSerialization.jsonObject(with: savedData) as? [String: Any])
    XCTAssertNil(saved["groupScale"], "saving removes the obsolete zoom value")
    XCTAssertEqual(saved["groupTier"] as? String, "actions")
  }

  func testInvalidSavedDimensionsDoNotDiscardTheTierOrInstances() throws {
    let name = "CodescribeTests.intercom-desktop.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    let instance = OverlayIntercomInstance(
      identity: try identity("lena", channel: "2", lease: "a"),
      frame: CGRect(x: 40, y: 60, width: 400, height: 320))
    let instances = try XCTUnwrap(String(data: JSONEncoder().encode([instance]), encoding: .utf8))

    for invalid in ["0", "-12", "1e400", "\"NaN\"", "\"Infinity\"", "\"-Infinity\"", "null"] {
      let data = Data(
        """
        {"groupWidth":\(invalid),"groupRowHeight":\(invalid),"groupTier":"name",\
        "instances":\(instances)}
        """.utf8)
      defaults.set(data, forKey: OverlayIntercomDesktopStore.key)
      let restored = OverlayIntercomDesktopStore(defaults: defaults).load()
      XCTAssertNil(restored.groupWidth, invalid)
      XCTAssertNil(restored.groupRowHeight, invalid)
      XCTAssertEqual(restored.groupTier, .name, invalid)
      XCTAssertEqual(restored.instances, [instance], invalid)
    }
  }

  func testDecodedNonfiniteDimensionsBecomeUnspecified() throws {
    let decoder = JSONDecoder()
    decoder.nonConformingFloatDecodingStrategy = .convertFromString(
      positiveInfinity: "Infinity", negativeInfinity: "-Infinity", nan: "NaN")

    for invalid in ["NaN", "Infinity", "-Infinity"] {
      let data = Data("{\"groupWidth\":\"\(invalid)\",\"groupRowHeight\":\"\(invalid)\"}".utf8)
      let restored = try decoder.decode(OverlayIntercomDesktopState.self, from: data)
      XCTAssertNil(restored.groupWidth, invalid)
      XCTAssertNil(restored.groupRowHeight, invalid)
      XCTAssertEqual(restored.groupTier, .actions)
      XCTAssertTrue(restored.instances.isEmpty)
    }
  }
}
