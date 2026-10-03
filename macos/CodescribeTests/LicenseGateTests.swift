import XCTest

@testable import Codescribe

@MainActor
final class LicenseGateTests: XCTestCase {
  func testUnlicensedBlocksAgenticAndActiveUnlocksIt() async {
    let service = LicenseService(
      keychain: nil,
      autoload: true,
      now: { Date(timeIntervalSince1970: 1_775_304_000) }
    )
    let store = AgentChatStore(
      threads: [ChatThread(title: "Gate", meta: "now")],
      licenseService: service
    )
    store.draft = "paid agent turn"
    XCTAssertTrue(store.isAgenticLocked)
    XCTAssertFalse(store.canSend)
    store.send()
    XCTAssertEqual(store.draft, "paid agent turn")
    XCTAssertTrue(store.queuedTurns.isEmpty)

    await awaitCondition { !service.isBusy }
    let activated = await service.activate(LicenseTestFixture.devKey)
    XCTAssertTrue(activated)
    XCTAssertFalse(store.isAgenticLocked)
    XCTAssertTrue(store.canSend)
  }
}
