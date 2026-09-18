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
    XCTAssertTrue(error.localizedDescription.contains("CsError"), "the generated description is the enum dump this seam exists to hide")
    XCTAssertFalse(error.userFacingMessage.contains("CsError"))
    XCTAssertEqual(CsError.Config(msg: "no key").userFacingMessage, "no key")
  }

  func testForeignErrorsKeepTheSystemDescription() {
    let error: Error = CocoaError(.fileNoSuchFile)
    XCTAssertEqual(error.userFacingMessage, error.localizedDescription)
  }
}
