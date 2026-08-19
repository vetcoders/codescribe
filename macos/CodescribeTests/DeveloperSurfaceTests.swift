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

  func testPowerModeCaptionIsTheVisibleInstallTag() {
    XCTAssertEqual(DeveloperSurface.powerModeCaption, "You use dev power mode")
  }

  func testLabSectionIsHiddenOnProductionBundle() {
    XCTAssertEqual(SettingsSection.lab.availability, .hidden)
    XCTAssertFalse(SettingsSection.matching(query: "").contains(.lab))
    XCTAssertTrue(SettingsSection.matching(query: "").contains(.agent))
  }

  func testLabModeOnDeveloperSurfaceHidesOverlayWithoutTouchingTray() {
    let defaults = UserDefaults(suiteName: UUID().uuidString)!
    defaults.set(true, forKey: DictationOverlayGate.labModeDefaultsKey)
    XCTAssertTrue(DictationOverlayGate.isLabModeOn(defaults: defaults, surfaceEnabled: true))
    XCTAssertFalse(
      DictationOverlayGate.shouldShowOverlay(
        trayEnabled: true,
        defaults: defaults,
        surfaceEnabled: true
      )
    )
  }

  func testLabModeOnProductionSurfaceDoesNotHideOverlay() {
    let defaults = UserDefaults(suiteName: UUID().uuidString)!
    defaults.set(true, forKey: DictationOverlayGate.labModeDefaultsKey)
    XCTAssertFalse(DictationOverlayGate.isLabModeOn(defaults: defaults, surfaceEnabled: false))
    XCTAssertTrue(
      DictationOverlayGate.shouldShowOverlay(
        trayEnabled: true,
        defaults: defaults,
        surfaceEnabled: false
      )
    )
  }

  func testVoiceLabSpawnIsCSOnlyAndIdleWhenAlreadyUp() {
    XCTAssertTrue(
      VoiceLabRuntime.shouldSpawn(
        surfaceEnabled: true,
        runningTests: false,
        alreadyListening: false,
        serverExists: true
      )
    )
    XCTAssertFalse(
      VoiceLabRuntime.shouldSpawn(
        surfaceEnabled: false,
        runningTests: false,
        alreadyListening: false,
        serverExists: true
      )
    )
    XCTAssertFalse(
      VoiceLabRuntime.shouldSpawn(
        surfaceEnabled: true,
        runningTests: true,
        alreadyListening: false,
        serverExists: true
      )
    )
    XCTAssertFalse(
      VoiceLabRuntime.shouldSpawn(
        surfaceEnabled: true,
        runningTests: false,
        alreadyListening: true,
        serverExists: true
      )
    )
    XCTAssertFalse(
      VoiceLabRuntime.shouldSpawn(
        surfaceEnabled: true,
        runningTests: false,
        alreadyListening: false,
        serverExists: false
      )
    )
  }

  func testTrayOffHidesOverlayEvenWhenLabModeIsOff() {
    let defaults = UserDefaults(suiteName: UUID().uuidString)!
    XCTAssertFalse(
      DictationOverlayGate.shouldShowOverlay(
        trayEnabled: false,
        defaults: defaults,
        surfaceEnabled: true
      )
    )
  }
}
