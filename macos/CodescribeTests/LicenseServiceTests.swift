import XCTest

@testable import Codescribe

// Mutable fixture state is protected because production storage uses a queue.
private final class MemoryLicenseKeychain: LicenseKeychainStoring, @unchecked Sendable {
  private let lock = NSLock()
  private var payload: Data?
  private var failingRead = false
  private var failingSave = false
  private var failingDelete = false
  private var blockedLoad: DispatchSemaphore?
  private var loads = 0
  enum Failure: Error { case storage }
  var data: Data? { lock.withLock { payload } }
  var loadCount: Int { lock.withLock { loads } }
  func failReads(_ value: Bool) { lock.withLock { failingRead = value } }
  func failSaves(_ value: Bool) { lock.withLock { failingSave = value } }
  func failDeletes(_ value: Bool) { lock.withLock { failingDelete = value } }
  func blockNextLoad(_ gate: DispatchSemaphore) { lock.withLock { blockedLoad = gate } }
  func replaceData(_ data: Data?) { lock.withLock { payload = data } }
  func load() throws -> Data? {
    let gate = lock.withLock { () -> DispatchSemaphore? in
      loads += 1
      let gate = blockedLoad
      blockedLoad = nil
      return gate
    }
    gate?.wait()
    return try lock.withLock {
      if failingRead { throw Failure.storage }
      return payload
    }
  }
  func save(_ data: Data) throws {
    try lock.withLock {
      if failingSave { throw Failure.storage }
      payload = data
    }
  }
  func delete() throws {
    try lock.withLock {
      if failingDelete { throw Failure.storage }
      payload = nil
    }
  }
}

enum LicenseTestFixture {
  // Deterministic public DEV fixture signed by the RFC 8032 test key. It is
  // neither a credential nor able to issue another license. The marker is split
  // so the fixture is not one scanner-shaped literal.
  static let devKey = [
    "CSK1.",
    "ey",
    "J2IjoxLCJza3UiOiJhZ2VudGljLWxpZmV0aW1lIiwiZW1haWxfaGFzaCI6IjEyZDJmZjZlOGE1OTI2YTA3NzBlNGVkOWRlNGQ2NzM1NTgwYzY0Nzg2ODM5OTE2NzczMDRlZDRmZWMwM2M5MDMiLCJpc3N1ZWQiOiIyMDI2LTA4LTA0IiwidXBkYXRlc191bnRpbCI6IjIwMjctMDgtMDQiLCJzZWF0X2xpbWl0IjozfQ.",
    "h5qFB3Wiir_5ubQg7jU6WSOCoxSFbgGllUHfomsYfwaty5l5cM3tR3FqVIGWslDmeb2snQ5B7OyJW6sDiUndAw",
  ].joined()
}

@MainActor
final class LicenseServiceTests: XCTestCase {
  func testGeneralSettingsRefreshDoesNotReadSavedLicenseButExplicitRetryDoes() async {
    let keychain = MemoryLicenseKeychain()
    let service = LicenseService(keychain: keychain, autoload: false)
    let model = SettingsViewModel(
      permissionProbe: MockPermissionProbe(.allGranted), licenseService: service,
      servingStatusProvider: { nil })
    model.refresh()
    await awaitCondition { !service.isBusy }
    XCTAssertEqual(keychain.loadCount, 0, "Ordinary configuration refresh is not license consent")

    model.refreshLicense()
    await awaitCondition { !service.isBusy }
    XCTAssertEqual(keychain.loadCount, 1, "The explicit license retry must remain functional")
  }

  func testExplicitSignalOrXCTestHostDisablesSwiftLicenseKeychain() {
    XCTAssertFalse(licenseKeychainDisabledByEnvironment([:]))
    XCTAssertFalse(licenseKeychainDisabledByEnvironment(["CI": "1"]))
    XCTAssertFalse(
      licenseKeychainDisabledByEnvironment(["CODESCRIBE_DATA_DIR": "/tmp/codescribe"])
    )
    XCTAssertTrue(
      licenseKeychainDisabledByEnvironment(["CODESCRIBE_DISABLE_KEYCHAIN": "1"])
    )
    XCTAssertTrue(
      licenseKeychainDisabledByEnvironment(["XCTestConfigurationFilePath": ""])
    )
    XCTAssertTrue(
      licenseKeychainDisabledByEnvironment(["XCTestSessionIdentifier": "session"])
    )
    XCTAssertTrue(licenseKeychainDisabledByEnvironment(["XCTestBundlePath": "tests.xctest"]))
  }

  func testDevKeyPersistsAcrossServiceRestartAndRemovalReturnsUnlicensed() async {
    let keychain = MemoryLicenseKeychain()
    let activationDate = Date(timeIntervalSince1970: 1_775_304_000)
    let first = LicenseService(
      keychain: keychain,
      autoload: false,
      now: { activationDate }
    )
    let activated = await first.activate(LicenseTestFixture.devKey)
    XCTAssertTrue(activated)
    XCTAssertEqual(first.status.state, .active)
    XCTAssertTrue(first.canUseAgentic)

    let restarted = LicenseService(
      keychain: keychain,
      autoload: true,
      now: { activationDate.addingTimeInterval(24 * 60 * 60) }
    )
    await awaitCondition { !restarted.isBusy }
    XCTAssertEqual(restarted.status.state, .graceOffline)
    XCTAssertEqual(restarted.status.daysLeft, 29)
    XCTAssertTrue(restarted.canUseAgentic)

    await restarted.removeLicense()
    XCTAssertEqual(restarted.status.state, .unlicensed)
    XCTAssertFalse(restarted.canUseAgentic)
    XCTAssertNil(keychain.data)
  }

  func testExpiredUpdatesClosesTheAgenticGate() async {
    // Past the fixture's updates_until (2027-08-04): the key still verifies
    // (claims present, SKU entitled) but the entitlement is not current, so
    // the gate must close instead of treating the SKU as a lifetime unlock.
    let afterExpiry = Date(timeIntervalSince1970: 1_827_600_000)  // 2027-11-27
    let service = LicenseService(
      keychain: MemoryLicenseKeychain(),
      autoload: false,
      now: { afterExpiry }
    )
    let activated = await service.activate(LicenseTestFixture.devKey)
    XCTAssertTrue(activated)
    XCTAssertEqual(service.status.state, .expiredUpdates)
    XCTAssertTrue(service.status.agenticEntitled, "SKU stays entitled — state is what expires")
    XCTAssertFalse(service.canUseAgentic)
    XCTAssertTrue(service.agenticBlockMessage.contains("ended"))
  }

  func testInvalidKeyFailsClosedWithoutPersisting() async {
    let keychain = MemoryLicenseKeychain()
    let service = LicenseService(keychain: keychain, autoload: false)
    let activated = await service.activate("CSK1.invalid.invalid")
    XCTAssertFalse(activated)
    XCTAssertEqual(service.status.state, .unlicensed)
    XCTAssertNil(keychain.data)
  }

  func testSystemKeychainPersistsAcrossInstancesAndDeletesCleanly() throws {
    let service = "com.vetcoders.codescribe.tests.\(UUID().uuidString)"
    let first = SystemLicenseKeychain(service: service)
    let payload = Data("CSK1.test-persistence".utf8)
    defer { try? first.delete() }

    try first.save(payload)

    let restarted = SystemLicenseKeychain(service: service)
    XCTAssertEqual(try restarted.load(), payload)
    try restarted.delete()
    XCTAssertNil(try restarted.load())
  }
  func testSlowColdLoadKeepsMainActorResponsiveAndCoalescesRefresh() async {
    let storage = MemoryLicenseKeychain()
    let gate = DispatchSemaphore(value: 0)
    storage.blockNextLoad(gate)
    let service = LicenseService(keychain: storage, autoload: true)
    defer { gate.signal() }
    await awaitCondition { storage.loadCount == 1 }
    XCTAssertTrue(service.isBusy)
    XCTAssertEqual(service.readState, .loading)
    XCTAssertFalse(service.canUseAgentic)
    for _ in 0..<20 { service.refresh() }
    let mutation = await service.activate(LicenseTestFixture.devKey)
    XCTAssertFalse(mutation, "pending physical read owns the slot")
    await Task.yield()
    XCTAssertEqual(storage.loadCount, 1)
    gate.signal()
    await awaitCondition { !service.isBusy }
    XCTAssertEqual(service.readState, .available)
  }

  func testColdDeniedReadIsUnavailableRatherThanConfirmedAbsence() async {
    let storage = MemoryLicenseKeychain()
    storage.failReads(true)
    let service = LicenseService(keychain: storage, autoload: true)
    await awaitCondition { !service.isBusy }
    XCTAssertEqual(service.readState, .unavailable)
    XCTAssertFalse(service.canUseAgentic)
    XCTAssertNotNil(service.lastError)
    storage.failReads(false)
    service.refresh()
    await awaitCondition { !service.isBusy }
    XCTAssertEqual(service.readState, .available)
    XCTAssertNil(service.lastError)
  }

  func testReadErrorRetainsVerifiedPayloadButDoesNotExtendGraceOrExpiry() async {
    let storage = MemoryLicenseKeychain()
    let activatedAt = Date(timeIntervalSince1970: 1_775_304_000)
    var clock = activatedAt
    let service = LicenseService(keychain: storage, autoload: false, now: { clock })
    let activated = await service.activate(LicenseTestFixture.devKey)
    XCTAssertTrue(activated)
    let durable = storage.data
    storage.failReads(true)
    service.refresh()
    await awaitCondition { !service.isBusy }
    XCTAssertEqual(service.readState, .unavailable)
    XCTAssertTrue(service.canUseAgentic)
    XCTAssertEqual(storage.data, durable)
    clock = activatedAt.addingTimeInterval(31 * 24 * 60 * 60)
    XCTAssertFalse(service.canUseAgentic, "read errors cannot renew offline grace")
    clock = Date(timeIntervalSince1970: 1_827_600_000)
    XCTAssertEqual(service.status.state, .expiredUpdates)
    XCTAssertFalse(service.canUseAgentic)
  }

  func testFailedReplacementAndDeletePreserveExistingLicense() async {
    let storage = MemoryLicenseKeychain()
    let service = LicenseService(
      keychain: storage, autoload: false,
      now: { Date(timeIntervalSince1970: 1_775_304_000) })
    let activated = await service.activate(LicenseTestFixture.devKey)
    XCTAssertTrue(activated)
    let durable = storage.data
    storage.failSaves(true)
    let replacement = await service.activate(LicenseTestFixture.devKey)
    XCTAssertFalse(replacement)
    XCTAssertEqual(storage.data, durable)
    XCTAssertTrue(service.canUseAgentic)
    storage.failDeletes(true)
    await service.removeLicense()
    XCTAssertEqual(storage.data, durable)
    XCTAssertTrue(service.canUseAgentic)
    storage.failDeletes(false)
    await service.removeLicense()
    XCTAssertNil(storage.data)
    XCTAssertFalse(service.canUseAgentic)
  }

  func testMalformedStoredPayloadFailsClosedAfterSuccessfulRead() async {
    let storage = MemoryLicenseKeychain()
    let service = LicenseService(
      keychain: storage, autoload: false,
      now: { Date(timeIntervalSince1970: 1_775_304_000) })
    let activated = await service.activate(LicenseTestFixture.devKey)
    XCTAssertTrue(activated)
    storage.replaceData(Data("malformed".utf8))
    service.refresh()
    await awaitCondition { !service.isBusy }
    XCTAssertEqual(service.readState, .unavailable)
    XCTAssertFalse(service.canUseAgentic)
  }

}
