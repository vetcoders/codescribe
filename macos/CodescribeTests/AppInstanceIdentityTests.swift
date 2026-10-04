import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class AppInstanceIdentityTests: XCTestCase {
  func testDebugHostHasDistinctBundleIdentity() {
    XCTAssertEqual(Bundle.main.bundleIdentifier, "com.vetcoders.codescribe.dev")
  }

  func testDebugAndReleaseDoNotCountAsEachOthersDuplicate() {
    for (current, candidate) in [
      ("com.vetcoders.codescribe.dev", "com.vetcoders.codescribe"),
      ("com.vetcoders.codescribe", "com.vetcoders.codescribe.dev"),
    ] {
      XCTAssertFalse(
        AppDelegate.isOtherInstance(
          bundleIdentifier: current, currentPID: 1,
          candidateBundleIdentifier: candidate, candidatePID: 2, isTerminated: false
        ))
    }
  }

  func testOnlyAnotherLiveProcessOfSameBundleCounts() {
    let identity = "com.vetcoders.codescribe"
    XCTAssertTrue(
      AppDelegate.isOtherInstance(
        bundleIdentifier: identity, currentPID: 1,
        candidateBundleIdentifier: identity, candidatePID: 2, isTerminated: false
      ))
    XCTAssertFalse(
      AppDelegate.isOtherInstance(
        bundleIdentifier: identity, currentPID: 1,
        candidateBundleIdentifier: identity, candidatePID: 1, isTerminated: false
      ))
    XCTAssertFalse(
      AppDelegate.isOtherInstance(
        bundleIdentifier: identity, currentPID: 1,
        candidateBundleIdentifier: identity, candidatePID: 2, isTerminated: true
      ))
  }
}
