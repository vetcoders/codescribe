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

  func testPowerModeCaptionDescribesActiveLabMode() {
    XCTAssertEqual(DeveloperSurface.powerModeCaption, "Voice Lab power mode")
  }

  func testPowerModeRequiresBothDeveloperBuildAndEnabledVoiceLab() {
    for surface in [false, true] {
      for labMode in [false, true] {
        XCTAssertEqual(
          DeveloperSurface.isPowerModeEnabled(labMode: labMode, surfaceEnabled: surface),
          surface && labMode)
      }
    }
  }

  func testLabSectionIsHiddenOnProductionBundle() {
    XCTAssertEqual(SettingsSection.lab.availability, .hidden)
    XCTAssertFalse(SettingsSection.matching(query: "").contains(.lab))
    XCTAssertTrue(SettingsSection.matching(query: "").contains(.agent))
  }

  func testLabModeDoesNotHideOverlayWhenTrayWantsIt() {
    let defaults = UserDefaults(suiteName: UUID().uuidString)!
    defaults.set(true, forKey: DictationOverlayGate.labModeDefaultsKey)
    XCTAssertTrue(DictationOverlayGate.isLabModeOn(defaults: defaults, surfaceEnabled: true))
    XCTAssertTrue(
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

  func testVoiceLabSkipsAForeignPortInsteadOfRefusing() async {
    let candidates = Array(8765...8768)
    let screenscribeOn8765: (Int) async -> VoiceLabRuntime.PortState = {
      $0 == 8765 ? .foreign : .free
    }
    let first = await VoiceLabRuntime.choosePort(
      remembered: nil, candidates: candidates, probe: screenscribeOn8765)
    XCTAssertEqual(first?.port, 8766)
    XCTAssertEqual(first?.running, false)

    let labOn8767: (Int) async -> VoiceLabRuntime.PortState = {
      switch $0 {
      case 8765: .foreign
      case 8767: .voiceLab
      default: .free
      }
    }
    let remembered = await VoiceLabRuntime.choosePort(
      remembered: 8767, candidates: candidates, probe: labOn8767)
    XCTAssertEqual(remembered?.port, 8767)
    XCTAssertEqual(remembered?.running, true)

    let everythingTaken = await VoiceLabRuntime.choosePort(
      remembered: nil, candidates: candidates, probe: { _ in .foreign })
    XCTAssertNil(everythingTaken)
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
