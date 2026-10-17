import Foundation
import XCTest

@testable import Codescribe

/// Founder 2026-09-09 16:15: a refused agent turn rendered as
/// `Codescribe.CsError.Agent(msg: "Provider stream error: … {\n "error": …")`.
/// A person reads the message the core composed, never the enum reflection.
final class CsErrorPresentationTests: XCTestCase {
  func testBridgeErrorsSurfaceTheirPayloadOnly() {
    let message = "Provider stream error: Agent SSE HTTP 401 Unauthorized: insufficient permissions"
    let error: Error = CsError.Agent(msg: message)
    XCTAssertEqual(error.userFacingMessage, message)
    XCTAssertTrue(
      error.localizedDescription.contains("CsError"),
      "the generated description is the enum dump this seam exists to hide")
    XCTAssertFalse(error.userFacingMessage.contains("CsError"))
    XCTAssertEqual(CsError.Config(msg: "no key").userFacingMessage, "no key")
  }

  /// Founder, 2026-10-15: the AirPods calibration refusal reached Settings in
  /// English. A refusal crosses as a code; each sentence is catalog copy.
  func testCalibrationRefusalsReadCatalogSentencesInTheInterfaceLanguage() {
    let polish = Locale(identifier: "pl")
    let silence = CsCalibrationRefusal.digitalSilence(
      deviceName: "AirPods Pro Macieja", samples: 238_560)
    XCTAssertEqual(
      CalibrationRefusalCopy.sentence(silence, locale: polish),
      "Z „AirPods Pro Macieja” nie dociera sygnał dźwiękowy. Sprawdź, czy urządzenie przekazuje dźwięk do tego Maca, i wybierz mikrofon w Centrum sterowania."
    )
    XCTAssertEqual(
      CalibrationRefusalCopy.sentence(
        .insufficientSpeech(measuredSeconds: 0.4, requiredSeconds: 1), locale: polish),
      "Usłyszano tylko 0,4 s mowy, a potrzeba co najmniej 1,0 s. Mów normalnie przez cały pomiar.")
    for refusal in [
      silence, .noActiveSpeech, .insufficientSpeech(measuredSeconds: 0.4, requiredSeconds: 1),
      .insufficientSeparation(thresholdDbfs: -48, noiseFloorDbfs: -50), .unusable,
    ] {
      let english = CalibrationRefusalCopy.sentence(refusal, locale: Locale(identifier: "en"))
      XCTAssertNotEqual(
        CalibrationRefusalCopy.sentence(refusal, locale: polish), english,
        "\(refusal) has a Polish sentence")
      let error: Error = CsError.CalibrationRefused(refusal: refusal)
      XCTAssertFalse(error.userFacingMessage.contains("CalibrationRefused"))
      XCTAssertFalse(error.userFacingMessage.isEmpty)
    }
  }

  func testForeignErrorsKeepTheSystemDescription() {
    let error: Error = CocoaError(.fileNoSuchFile)
    XCTAssertEqual(error.userFacingMessage, error.localizedDescription)
  }
}
