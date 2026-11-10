import XCTest

@testable import Codescribe

@MainActor
final class AppPreferencesMigrationTests: XCTestCase {
  func testMigrationCopiesMissingPreferencesAndPreservesX9busValues() throws {
    let source: [String: Any] = [
      "OverlayPlacement.anchor": "topLeft",
      "TextScale.value": 1.25,
      AppPreferencesMigration.migrationMarker: true,
    ]
    let destination: [String: Any] = [
      "OverlayPlacement.anchor": "topRight",
      "AgentChat.sidebarExpanded.v1": false,
    ]

    let merged = try XCTUnwrap(
      AppPreferencesMigration.mergedPreferences(source: source, destination: destination))

    XCTAssertEqual(merged["OverlayPlacement.anchor"] as? String, "topRight")
    XCTAssertEqual(merged["TextScale.value"] as? Double, 1.25)
    XCTAssertEqual(merged["AgentChat.sidebarExpanded.v1"] as? Bool, false)
    XCTAssertEqual(merged[AppPreferencesMigration.migrationMarker] as? Bool, true)
  }

  func testMigrationMarkerMakesTheCopyOneTime() {
    let destination: [String: Any] = [AppPreferencesMigration.migrationMarker: true]
    XCTAssertNil(
      AppPreferencesMigration.mergedPreferences(
        source: ["newlyChangedOldPreference": "ignored"], destination: destination))
  }
}
