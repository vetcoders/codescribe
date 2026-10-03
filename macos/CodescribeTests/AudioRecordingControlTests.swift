import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// XCTest does not expose SwiftUI AX buttons through AppKit's child graph.
/// Exercise the real admission/action and actual observation consumer here.
/// Native button/AX acceptance remains a separate required gate.
@MainActor
final class AudioRecordingControlTests: XCTestCase {
  private struct RenderRead: Equatable {
    let starting: Bool
    let recording: Bool
    let finalPass: Bool
  }
  private final class RenderReads { var values: [RenderRead] = [] }

  private func model(state: OverlayState, tray: TrayViewModel) -> SettingsViewModel {
    SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe(.allGranted),
      audioRecordingControlProvider: { (state: state, tray: tray) })
  }

  func testFakeAndNilModelsStayDetachedUnlessOwnersAreExplicitlyInjected() {
    let fake = SettingsViewModel(engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe())
    XCTAssertNil(fake.audioRecordingControls())
    let detached = SettingsViewModel(permissionProbe: MockPermissionProbe())
    XCTAssertNil(detached.audioRecordingControls())
    let state = OverlayState(autoSendEnabled: { false }, micAccessProvider: { true })
    let tray = TrayViewModel(engine: MockTrayEngine())
    let injected = model(state: state, tray: tray)
    XCTAssertTrue(injected.audioRecordingControls()?.state === state)
    XCTAssertTrue(injected.audioRecordingControls()?.tray === tray)
  }

  func testFinalPassRemainsProcessingBeforeReducerFinalizingProjection() async {
    let state = OverlayState(autoSendEnabled: { false }, micAccessProvider: { true })
    let tray = TrayViewModel(engine: MockTrayEngine())
    let model = model(state: state, tray: tray)
    await model.refreshAdmission()
    let panel = AudioPanel(model: model)
    XCTAssertTrue(panel.canStartRecording(state, tray: tray))
    state.handleRecordingStarted()
    state.applySessionFinalised()
    XCTAssertEqual(state.mode, .listening)
    XCTAssertFalse(state.transcribing)
    XCTAssertTrue(state.isFinalPass)
    XCTAssertTrue(panel.recordingProcessing(state))
    XCTAssertFalse(panel.canStartRecording(state, tray: tray))
    state.finishControllerRecording()
    XCTAssertFalse(panel.recordingProcessing(state))
    XCTAssertTrue(panel.canStartRecording(state, tray: tray))
  }

  func testAudioRetryInvokesSharedAdmissionOnceInsteadOfFencedDirectStart() async {
    let state = OverlayState(autoSendEnabled: { false }, micAccessProvider: { true })
    defer { state.finishControllerRecording() }
    state.prepareForExternalStart()
    state.handleError(message: "synthetic-start-failure")
    XCTAssertNotNil(state.elapsedCaptureSeconds(), "failed capture retains its clock fence")
    XCTAssertFalse(state.recording)
    XCTAssertFalse(state.warmingUp)
    let engine = MockTrayEngine()
    let tray = TrayViewModel(engine: engine)
    var admitted = 0
    tray.onDictationStartRequested = {
      admitted += 1
      state.prepareForExternalStart()
    }
    let model = model(state: state, tray: tray)
    await model.refreshAdmission()
    let panel = AudioPanel(model: model)
    XCTAssertTrue(panel.canStartRecording(state, tray: tray))
    panel.startRecording(state, tray: tray)
    panel.startRecording(state, tray: tray)
    XCTAssertEqual(admitted, 1)
    XCTAssertTrue(state.recording, "shared admission reserves capture before engine start")
    await awaitCondition { engine.recording }
    XCTAssertEqual(admitted, 1)
    XCTAssertTrue(engine.recording)
  }

  func testStartWhileTrayBusyNeverUsesToggleAsStop() async {
    let state = OverlayState(autoSendEnabled: { false }, micAccessProvider: { true })
    let engine = MockTrayEngine(recording: true)
    let tray = TrayViewModel(engine: engine, isRecording: true)
    let model = model(state: state, tray: tray)
    await model.refreshAdmission()
    let panel = AudioPanel(model: model)
    XCTAssertFalse(panel.canStartRecording(state, tray: tray))
    panel.startRecording(state, tray: tray)
    for _ in 0..<3 { await Task.yield() }
    XCTAssertTrue(engine.recording)
    tray.isRecording = false
    tray.isStartingDictation = true
    XCTAssertFalse(panel.canStartRecording(state, tray: tray))
    panel.startRecording(state, tray: tray)
    for _ in 0..<3 { await Task.yield() }
    XCTAssertTrue(engine.recording)
  }

  func testActualConsumerRerendersForTrayOnlyAndOverlayOnlyChanges() async throws {
    let reads = RenderReads()
    let state = OverlayState(autoSendEnabled: { false }, micAccessProvider: { true })
    let tray = TrayViewModel(engine: MockTrayEngine())
    let host = NSHostingView(
      rootView: AudioReadinessObserver(recordingState: state, tray: tray) { state, tray, _ in
        let read = RenderRead(starting: tray.isStartingDictation, recording: tray.isRecording,
          finalPass: state.isFinalPass)
        reads.values.append(read)
        return Text(verbatim: "\(read.starting) \(read.recording) \(read.finalPass)")
      })
    let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 300, height: 100),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    window.orderFrontRegardless()
    defer { window.contentView = nil; window.close() }
    host.layoutSubtreeIfNeeded()
    await awaitCondition { reads.values.last == RenderRead(starting: false, recording: false, finalPass: false) }
    tray.isStartingDictation = true
    await awaitCondition { reads.values.last == RenderRead(starting: true, recording: false, finalPass: false) }
    XCTAssertEqual(reads.values.last, RenderRead(starting: true, recording: false, finalPass: false))
    tray.isStartingDictation = false
    tray.isRecording = true
    await awaitCondition { reads.values.last == RenderRead(starting: false, recording: true, finalPass: false) }
    XCTAssertEqual(reads.values.last, RenderRead(starting: false, recording: true, finalPass: false))
    state.isFinalPass = true
    await awaitCondition { reads.values.last == RenderRead(starting: false, recording: true, finalPass: true) }
    XCTAssertEqual(reads.values.last, RenderRead(starting: false, recording: true, finalPass: true))
  }
}
