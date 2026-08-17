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

  func testRequestBindsArchivedFileAndNeverInventsLastSession() {
    let audio = URL(fileURLWithPath: "/tmp/history/083000_take_raw.wav")
    switch HelperFilePass.request(asrMode: "local_power", archivedAudio: audio) {
    case .success(let (pass, prefixed)):
      XCTAssertEqual(pass, .fullHq)
      XCTAssertEqual(prefixed, "hq:/tmp/history/083000_take_raw.wav")
    case .failure(let reason):
      XCTFail("expected archived HQ path, got \(reason)")
    }

    switch HelperFilePass.request(asrMode: "cloud", archivedAudio: audio) {
    case .success(let (pass, prefixed)):
      XCTAssertEqual(pass, .cloud)
      XCTAssertEqual(prefixed, "cloud:/tmp/history/083000_take_raw.wav")
    case .failure(let reason):
      XCTFail("expected archived cloud path, got \(reason)")
    }

    switch HelperFilePass.request(asrMode: "local_power", archivedAudio: nil) {
    case .failure(.noArchivedAudio):
      break
    default:
      XCTFail("missing archive must refuse, never last_session.wav")
    }

    switch HelperFilePass.request(asrMode: "apple_only", archivedAudio: audio) {
    case .failure(.noHelper):
      break
    default:
      XCTFail("apple_only must not build a file pass")
    }
  }

  func testCompareLeavesDailyUnchanged() {
    let line = HelperFilePass.compare(
      daily: "overlay",
      helper: "overlayu",
      pass: .fullHq
    )
    XCTAssertTrue(line.contains("DAILY"))
    XCTAssertTrue(line.contains("overlayu"))
    XCTAssertTrue(line.contains("unchanged"))
    XCTAssertEqual(
      HelperFilePass.compare(daily: "same", helper: "same", pass: .fullHq),
      "Helper Full HQ file pass matches daily."
    )
  }
}
