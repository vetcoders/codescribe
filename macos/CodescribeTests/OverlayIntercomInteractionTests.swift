import AppKit
import XCTest

@testable import Codescribe

/// Exercise the real owner while visible remains false. No window is ordered,
/// and state-commit evidence never certifies native presentation or focus.
@MainActor
final class OverlayIntercomInteractionTests: XCTestCase {
  @MainActor
  private final class Fixture {
    let initialWindows = Set(NSApp.windows.map(ObjectIdentifier.init))
    let suite = "CodescribeTests.intercom-interactions.\(UUID().uuidString)"
    let defaults: UserDefaults
    let directory: URL
    let store: OverlayIntercomDesktopStore
    let state = OverlayState()
    let log: DiagnosticLog
    let controller: OverlayIntercomWidgetController

    init(desktop: OverlayIntercomDesktopState = .init()) throws {
      defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
      directory = FileManager.default.temporaryDirectory.appendingPathComponent(suite)
      store = OverlayIntercomDesktopStore(defaults: defaults)
      store.save(desktop)
      log = DiagnosticLog(url: DiagnosticLog.url(churnDirectory: directory.path))
      controller = OverlayIntercomWidgetController(
        state: state, textScale: TextScaleController(key: suite),
        desktopStore: store, diagnosticLog: log)
      state.publishConversationText = { _, _, _, _ in XCTFail("Window actions must not send") }
    }

    func cleanup() {
      XCTAssertEqual(controller.interactionWindowCounts.visible, 0)
      XCTAssertEqual(controller.interactionWindowCounts.key, 0)
      let panels = NSApp.windows.filter {
        $0 is OverlayIntercomPanel && !initialWindows.contains(ObjectIdentifier($0))
      }
      XCTAssertEqual(panels.count, controller.interactionWindowCounts.owned)
      controller.invalidate()
      XCTAssertEqual(controller.interactionWindowCounts.owned, 0)
      for panel in panels {
        XCTAssertNil(panel.contentView)
        XCTAssertNil(panel.delegate)
        XCTAssertFalse(panel.isVisible)
        XCTAssertFalse(panel.isKeyWindow)
      }
      defaults.removePersistentDomain(forName: suite)
      XCTAssertTrue(defaults.persistentDomain(forName: suite)?.isEmpty ?? true)
      XCTAssertNoThrow(try FileManager.default.removeItem(at: directory))
      XCTAssertFalse(FileManager.default.fileExists(atPath: directory.path))
    }
  }

  private func conversation(_ session: String, channel: String = "3") throws -> OverlayConversation
  {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session,
        "lease_id": AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: session),
        "channel": channel, "name": "private-agent-name",
      ]))
    return .init(id: owner.id, channel: channel, name: owner.name, owner: owner, messages: [])
  }

  private func identity(_ session: String, channel: String = "3") throws
    -> OverlayIntercomCardIdentity
  {
    .init(digit: channel, conversation: try conversation(session, channel: channel))
  }

  private func completions(_ fixture: Fixture, count: Int) throws -> [[String: Any]] {
    let rows = try fixture.log.lines().map { line in
      let marker = try XCTUnwrap(line.range(of: " interaction "))
      return try XCTUnwrap(
        JSONSerialization.jsonObject(with: Data(line[marker.upperBound...].utf8)) as? [String: Any])
    }
    XCTAssertEqual(rows.count, count * 2)
    let groups = Dictionary(grouping: rows) { $0["actionID"] as? String }
    XCTAssertEqual(groups.count, count)
    for events in groups.values {
      XCTAssertEqual(events.map { $0["phase"] as? String }, ["started", "completed"])
      XCTAssertEqual(events.first?["action"] as? String, events.last?["action"] as? String)
      XCTAssertEqual(events.first?["runID"] as? String, events.last?["runID"] as? String)
    }
    let allowed: Set<String> = [
      "schema", "runID", "actionID", "action", "surface", "phase", "durationMilliseconds",
      "outcome", "effect", "cleanup", "reason",
    ]
    for row in rows {
      XCTAssertTrue(Set(row.keys).isSubset(of: allowed))
      XCTAssertNotNil(UUID(uuidString: try XCTUnwrap(row["actionID"] as? String)))
    }
    let text = try String(contentsOf: fixture.log.url, encoding: .utf8)
    for privateValue in [
      "private-agent-name", "private draft sentinel", "private-session-original",
    ] {
      XCTAssertFalse(text.contains(privateValue))
    }
    let completed = rows.filter { $0["phase"] as? String == "completed" }
    for row in completed {
      XCTAssertNotEqual(row["outcome"] as? String, "succeeded", "No native result was witnessed")
      let duration = try XCTUnwrap(row["durationMilliseconds"] as? Double)
      XCTAssertTrue(duration.isFinite && duration >= 0)
    }
    return completed
  }

  func testGroupFoldAndUnfoldPreserveDetachedRecordsAndConversationTruth() throws {
    let card = try identity("private-session-original")
    var desktop = OverlayIntercomDesktopState()
    desktop.upsert(.init(identity: card, frame: CGRect(x: 60, y: 70, width: 380, height: 320)))
    let f = try Fixture(desktop: desktop)
    defer { f.cleanup() }
    let conversationID = try XCTUnwrap(card.conversationID)
    f.state.conversationDrafts = [conversationID: "private draft sentinel"]
    let drafts = f.state.conversationDrafts
    let generation = f.state.captureGeneration
    let folded = f.controller.presentation.folded
    f.controller.toggleGroup()
    XCTAssertEqual(f.controller.presentation.folded, !folded)
    XCTAssertEqual(f.store.load(), desktop)
    f.controller.toggleGroup()
    XCTAssertEqual(f.controller.presentation.folded, folded)
    XCTAssertEqual(f.store.load(), desktop)
    XCTAssertEqual(f.state.conversationDrafts, drafts)
    XCTAssertEqual(f.state.captureGeneration, generation)
    XCTAssertTrue(f.state.pendingTextMessages.isEmpty)
    let events = try completions(f, count: 2)
    XCTAssertEqual(Set(events.compactMap { $0["action"] as? String }).count, 2)
    XCTAssertTrue(events.allSatisfy { $0["outcome"] as? String == "unverified" })
  }

  func testGlobalRestoreKeepsExactSavedGeometryAndWindowsCreatedWhileDocked() throws {
    var desktop = OverlayIntercomDesktopState(
      groupWidth: 520, groupRowHeight: 80, groupTier: .actions)
    for (index, session) in ["original", "mini", "closed"].enumerated() {
      desktop.upsert(
        .init(
          identity: try identity(session),
          frame: CGRect(x: 70 + 20 * index, y: 80, width: 380, height: 320),
          minimized: index == 1, closed: index == 2))
    }
    let f = try Fixture(desktop: desktop)
    defer { f.cleanup() }
    let presentation = f.controller.presentation
    f.controller.dockAll()
    var docked = desktop
    for index in docked.instances.indices { docked.instances[index].closed = true }
    XCTAssertEqual(f.store.load(), docked)
    XCTAssertTrue(f.controller.canRestoreDockedLayout)
    let newCard = try identity("new-owner", channel: "4")
    XCTAssertTrue(f.controller.detach(newCard))
    let newWindow = try XCTUnwrap(f.store.load().openInstance(for: newCard))
    f.controller.restoreDockedLayout()
    var restored = desktop
    restored.upsert(newWindow)
    XCTAssertEqual(f.store.load(), restored)
    XCTAssertEqual(f.controller.presentation, presentation)
    XCTAssertFalse(f.controller.canRestoreDockedLayout)
    XCTAssertEqual(f.controller.rowOffset, 0)
    _ = try completions(f, count: 3)
  }

  func testRepeatedDetachDoesNotDuplicateAndDockReopenKeepTheExactOwner() throws {
    let card = try identity("private-session-original")
    let f = try Fixture()
    defer { f.cleanup() }
    XCTAssertTrue(f.controller.detach(card))
    let saved = try XCTUnwrap(f.store.load().openInstance(for: card))
    XCTAssertFalse(f.controller.detach(card))
    XCTAssertEqual(f.store.load().instances, [saved])
    f.controller.closeInstance(saved.id)
    var closed = saved
    closed.closed = true
    XCTAssertEqual(f.store.load().instances, [closed])
    XCTAssertEqual(f.store.load().groupRows(of: [card]), [card])
    let successor = try conversation("successor")
    f.state.applyConversationSnapshot(.init(deliveries: [], conversations: [successor]))
    f.controller.reopenInstance(saved.id)
    XCTAssertEqual(f.store.load().instances, [saved])
    XCTAssertNotEqual(saved.ownerID, successor.owner?.id)
    XCTAssertEqual(f.state.conversations.map(\.id), [successor.id])
    XCTAssertTrue(f.state.pendingTextMessages.isEmpty)
    let events = try completions(f, count: 4)
    XCTAssertEqual(Set(events.compactMap { $0["action"] as? String }).count, 4)
  }

  func testMissingSavedUUIDRefusesWithoutChangingDesktop() throws {
    let f = try Fixture()
    defer { f.cleanup() }
    let before = f.store.load()
    f.controller.closeInstance(UUID())
    f.controller.reopenInstance(UUID())
    XCTAssertEqual(f.store.load(), before)
    let events = try completions(f, count: 2)
    for event in events {
      XCTAssertEqual(event["outcome"] as? String, "refused")
      XCTAssertEqual(event["effect"] as? String, "noChange")
      XCTAssertEqual(event["cleanup"] as? String, "notRequired")
      XCTAssertEqual(event["reason"] as? String, "notFound")
    }
  }

  func testInvalidatedDesktopDockRecordsRefusalAndOwnsNoWindows() throws {
    let f = try Fixture()
    defer { f.cleanup() }
    f.controller.invalidate()
    let before = f.store.load()
    f.controller.dockAll()
    XCTAssertEqual(f.store.load(), before)
    XCTAssertFalse(f.controller.canRestoreDockedLayout)
    let event = try XCTUnwrap(try completions(f, count: 1).first)
    XCTAssertEqual(event["outcome"] as? String, "refused")
    XCTAssertEqual(event["effect"] as? String, "noChange")
  }
}
