import XCTest

@testable import Codescribe

private final class MemoryLicenseKeychain: LicenseKeychainStoring {
  var data: Data?
  func load() throws -> Data? { data }
  func save(_ data: Data) throws { self.data = data }
  func delete() throws { data = nil }
}

enum LicenseTestFixture {
  // Deterministic public DEV fixture signed by the RFC 8032 test key. It is
  // neither a JWT nor a secret and cannot issue another license.
  static let devKey =
    // nosemgrep: generic.secrets.security.detected-jwt-token.detected-jwt-token
    "CSK1.eyJ2IjoxLCJza3UiOiJhZ2VudGljLWxpZmV0aW1lIiwiZW1haWxfaGFzaCI6IjEyZDJmZjZlOGE1OTI2YTA3NzBlNGVkOWRlNGQ2NzM1NTgwYzY0Nzg2ODM5OTE2NzczMDRlZDRmZWMwM2M5MDMiLCJpc3N1ZWQiOiIyMDI2LTA4LTA0IiwidXBkYXRlc191bnRpbCI6IjIwMjctMDgtMDQiLCJzZWF0X2xpbWl0IjozfQ.h5qFB3Wiir_5ubQg7jU6WSOCoxSFbgGllUHfomsYfwaty5l5cM3tR3FqVIGWslDmeb2snQ5B7OyJW6sDiUndAw"
}

@MainActor
final class LicenseServiceTests: XCTestCase {
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

  func testDevKeyPersistsAcrossServiceRestartAndRemovalReturnsUnlicensed() {
    let keychain = MemoryLicenseKeychain()
    let activationDate = Date(timeIntervalSince1970: 1_775_304_000)
    let first = LicenseService(
      keychain: keychain,
      autoload: false,
      now: { activationDate }
    )
    XCTAssertTrue(first.activate(LicenseTestFixture.devKey))
    XCTAssertEqual(first.status.state, .active)
    XCTAssertTrue(first.canUseAgentic)

    let restarted = LicenseService(
      keychain: keychain,
      autoload: true,
      now: { activationDate.addingTimeInterval(24 * 60 * 60) }
    )
    XCTAssertEqual(restarted.status.state, .graceOffline)
    XCTAssertEqual(restarted.status.daysLeft, 29)
    XCTAssertTrue(restarted.canUseAgentic)

    restarted.removeLicense()
    XCTAssertEqual(restarted.status.state, .unlicensed)
    XCTAssertFalse(restarted.canUseAgentic)
    XCTAssertNil(keychain.data)
  }

  func testInvalidKeyFailsClosedWithoutPersisting() {
    let keychain = MemoryLicenseKeychain()
    let service = LicenseService(keychain: keychain, autoload: false)
    XCTAssertFalse(service.activate("CSK1.invalid.invalid"))
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
}
