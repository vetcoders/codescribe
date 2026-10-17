import Sparkle
import XCTest

@testable import Codescribe

@MainActor
final class ReleaseUpdatePolicyTests: XCTestCase {
  func testHistoricalCounterWouldOfferTheOlderRelease() {
    XCTAssertEqual(
      SUStandardVersionComparator.default.compareVersion("1838", toVersion: "479"),
      .orderedDescending
    )
    XCTAssertFalse(
      ReleaseUpdatePolicy.accepts(
        version: "1838", displayVersion: "0.15.2", currentVersion: "0.16.0"
      )
    )
  }

  func testReleaseVersionOrdersPatchMinorAndMajorUpdates() {
    for next in ["0.17.1", "0.18.0", "1.0.0"] {
      XCTAssertTrue(
        ReleaseUpdatePolicy.accepts(
          version: next, displayVersion: next, currentVersion: "0.17.0"
        ), next
      )
    }
  }

  func testEqualAndOlderReleasesNeverUpdate() {
    for previous in ["0.15.2", "0.16.0", "0.17.0"] {
      XCTAssertFalse(
        ReleaseUpdatePolicy.accepts(
          version: previous, displayVersion: previous, currentVersion: "0.17.0"
        ), previous
      )
    }
  }

  func testMalformedOrContradictoryFeedCannotOrderUpdates() {
    for version in ["2001", "0.18", "0.018.0", "0.18.0-beta", "0.18.0 ", "0..18"] {
      XCTAssertFalse(
        ReleaseUpdatePolicy.accepts(
          version: version, displayVersion: version, currentVersion: "0.17.0"
        ), version
      )
    }
    XCTAssertFalse(
      ReleaseUpdatePolicy.accepts(
        version: "0.18.0", displayVersion: "0.15.2", currentVersion: "0.17.0"
      )
    )
  }
}
