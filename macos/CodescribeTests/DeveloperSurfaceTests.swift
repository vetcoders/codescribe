import XCTest

@testable import Codescribe

final class DeveloperSurfaceTests: XCTestCase {
  func testMissingPlistKeyIsOff() {
    XCTAssertFalse(DeveloperSurface.parse(nil))
    XCTAssertFalse(DeveloperSurface.parse(""))
    XCTAssertFalse(DeveloperSurface.parse("0"))
  }

  func testKeyedInstallAppBakesOn() {
    XCTAssertTrue(DeveloperSurface.parse("1"))
    XCTAssertTrue(DeveloperSurface.parse("true"))
    XCTAssertTrue(DeveloperSurface.parse(NSNumber(value: 1)))
  }
}
