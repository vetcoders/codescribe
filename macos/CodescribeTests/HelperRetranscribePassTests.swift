import XCTest

@testable import Codescribe

final class HelperRetranscribePassTests: XCTestCase {
  func testLocalPowerUsesCandleHq() {
    XCTAssertEqual(helperRetranscribePass(asrMode: "local_power"), .fullHq)
  }

  func testCloudUsesCloudFilePass() {
    XCTAssertEqual(helperRetranscribePass(asrMode: "cloud"), .cloud)
  }

  func testAppleOnlyHasNoHelper() {
    XCTAssertNil(helperRetranscribePass(asrMode: "apple_only"))
    XCTAssertNil(helperRetranscribePass(asrMode: ""))
  }
}
