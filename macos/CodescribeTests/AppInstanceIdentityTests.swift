import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class AppInstanceIdentityTests: XCTestCase {
  func testDebugHostHasDistinctBundleIdentity() {
    XCTAssertEqual(Bundle.main.bundleIdentifier, "com.vetcoders.x9bus.dev")
  }

  func testDebugAndReleaseDoNotCountAsEachOthersDuplicate() {
    for (current, candidate) in [
      ("com.vetcoders.x9bus.dev", "com.vetcoders.x9bus"),
      ("com.vetcoders.x9bus", "com.vetcoders.x9bus.dev"),
    ] {
      XCTAssertFalse(
        AppDelegate.isOtherInstance(
          bundleIdentifier: current, currentPID: 1,
          candidateBundleIdentifier: candidate, candidatePID: 2, isTerminated: false
        ))
    }
  }

  func testOnlyAnotherLiveProcessOfSameBundleCounts() {
    let identity = "com.vetcoders.x9bus"
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

  func testX9busDoesNotCountLegacyCodescribeAsDuplicate() {
    XCTAssertFalse(
      AppDelegate.isOtherInstance(
        bundleIdentifier: "com.vetcoders.x9bus", currentPID: 1,
        candidateBundleIdentifier: "com.vetcoders.codescribe", candidatePID: 2, isTerminated: false
      ))
  }
}
