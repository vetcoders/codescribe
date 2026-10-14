import AppKit
import XCTest

@testable import Codescribe

/// The Dock menu carries the tray panel's four primary actions, in its order,
/// with the dictation entry worded from the live state (Founder, 2026-10-14:
/// "Agent, nakładka, mikrofon prosto z Docka, Ustawienia").
@MainActor
final class DockMenuTests: XCTestCase {
  private func state(
    agentAvailable: Bool = true, starting: Bool = false, recording: Bool = false,
    assistive: Bool = false, startInAssistive: Bool = false
  ) -> DockMenuState {
    DockMenuState(
      agentAvailable: agentAvailable, isStartingDictation: starting, isRecording: recording,
      assistiveRecording: assistive, startInAssistive: startInAssistive)
  }

  func testFourEntriesInTrayOrderWithTrayWording() {
    let menu = DockMenu().menu(for: state(), handlers: [:])
    XCTAssertEqual(menu.items.count, 4)
    XCTAssertEqual(
      menu.items.map(\.title),
      [
        String(localized: "Agent"), String(localized: "Show widget"),
        String(localized: "Start transcription", comment: "Tray row: start a new transcription"),
        String(localized: "Settings…"),
      ])
    XCTAssertFalse(menu.autoenablesItems)
    XCTAssertTrue(menu.items.allSatisfy(\.isEnabled))
  }

  func testDictationEntryFollowsRecordingState() {
    let stop = DockMenu().menu(for: state(recording: true), handlers: [:]).items[2]
    XCTAssertEqual(
      stop.title,
      String(localized: "Stop transcription", comment: "Tray row: stop the current transcription"))
    XCTAssertTrue(stop.isEnabled)

    let agentStop = DockMenu().menu(for: state(recording: true, assistive: true), handlers: [:])
      .items[2]
    XCTAssertEqual(
      agentStop.title,
      String(
        localized: "Stop transcription in Agent mode",
        comment: "Tray row: stop the current transcription in Agent mode"))

    let agentStart = DockMenu().menu(for: state(startInAssistive: true), handlers: [:]).items[2]
    XCTAssertEqual(
      agentStart.title,
      String(
        localized: "Start transcription in Agent mode",
        comment: "Tray row: start a new transcription in Agent mode"))

    let starting = DockMenu().menu(for: state(starting: true), handlers: [:]).items[2]
    XCTAssertEqual(
      starting.title,
      String(localized: "Starting…", comment: "Tray row: a recording start is in flight"))
    XCTAssertFalse(starting.isEnabled, "a start in flight takes no second click")
  }

  func testAgentEntryDisabledWhenAgentUnavailable() {
    let menu = DockMenu().menu(for: state(agentAvailable: false), handlers: [:])
    XCTAssertFalse(menu.items[0].isEnabled)
    XCTAssertTrue(menu.items[1].isEnabled)
  }

  func testEachEntryPerformsItsOwnHandler() {
    var fired: [DockMenuEntry] = []
    let dock = DockMenu()
    let menu = dock.menu(
      for: state(),
      handlers: [
        .agent: { fired.append(.agent) }, .widget: { fired.append(.widget) },
        .dictation: { fired.append(.dictation) }, .settings: { fired.append(.settings) },
      ])
    for index in menu.items.indices.reversed() {
      menu.performActionForItem(at: index)
    }
    XCTAssertEqual(fired, [.settings, .dictation, .widget, .agent])
  }
}
