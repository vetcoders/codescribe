import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class Div0LatestAdmissionTests: XCTestCase {
  func testOneAgentsPressFromMiniReachesTheOnlyConversation() {
    let state = OverlayState()
    let conversation = OverlayConversation(
      id: "admission-single", channel: "2", name: "Lena", owner: nil, messages: [])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
    state.showTranscription()
    state.setPresentationMode(.mini)

    state.toggleAgentSidebar()

    XCTAssertEqual(state.selectedConversationID, conversation.id)
    XCTAssertFalse(state.showsAgentMonitor)
    XCTAssertFalse(state.isCollapsed)
  }

  func testHeaderControlPointDoesNotStartResizeButTheTopEdgeDoes() {
    let bounds = NSRect(x: 0, y: 0, width: 500, height: 300)
    let control = NSPoint(x: 450, y: bounds.maxY - 11)
    XCTAssertNil(OverlayResizeHit.edge(at: control, in: bounds))
    XCTAssertFalse(OverlayResizeHit.cursorRects(in: bounds).contains { $0.0.contains(control) })
    XCTAssertEqual(
      OverlayResizeHit.edge(at: NSPoint(x: 450, y: bounds.maxY - 1), in: bounds), .top)
  }

  func testCalibrationNoticeShowsTheFailureWithoutTheBridgeEnumDump() async {
    var engine = MockSettingsEngine()
    engine.admissionReadiness = .sampleMissing
    engine.calibrateEnergyObserver = { _ in throw CsError.Recording(msg: "measurement busy") }
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe(.allGranted))

    await model.runCalibration()

    XCTAssertTrue(model.calibrationNotice?.contains("measurement busy") == true)
    XCTAssertFalse(model.calibrationNotice?.contains("Recording(msg:") == true)
  }
}
