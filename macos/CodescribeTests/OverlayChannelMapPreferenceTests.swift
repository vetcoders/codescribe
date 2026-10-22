import Foundation
import Observation
import XCTest

@testable import Codescribe

@MainActor
final class OverlayChannelMapPreferenceTests: XCTestCase {
  func testAbsentChoiceIsOffAndExplicitChoiceSurvivesReopening() throws {
    let name = "CodescribeTests.channel-map.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    let preference = OverlayChannelMapPreference(defaults: defaults)
    XCTAssertFalse(preference.enabled)
    preference.select(true)
    XCTAssertTrue(OverlayChannelMapPreference(defaults: defaults).enabled)
    preference.select(false)
    XCTAssertFalse(OverlayChannelMapPreference(defaults: defaults).enabled)
  }

  func testLayoutPersistsWithoutEnablingTheMapAndUnknownChoiceKeepsTheStrip() throws {
    let name = "CodescribeTests.intercom-layout.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    let preference = OverlayChannelMapPreference(defaults: defaults)
    XCTAssertEqual(preference.layout, .horizontal)
    preference.selectLayout(.vertical)
    XCTAssertFalse(preference.enabled)
    XCTAssertEqual(OverlayChannelMapPreference(defaults: defaults).layout, .vertical)
    defaults.set("unknown-layout", forKey: OverlayChannelMapPreference.layoutKey)
    XCTAssertEqual(OverlayChannelMapPreference(defaults: defaults).layout, .horizontal)
  }

  /// Maciej, 2026-10-22: Settings offers three peer states. Classic is the
  /// map turned off, and it remembers the previously chosen layout.
  func testModeExposesClassicAsAPeerAndRemembersTheLayout() throws {
    let name = "CodescribeTests.intercom-mode.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    let preference = OverlayChannelMapPreference(defaults: defaults)
    XCTAssertEqual(preference.mode, .classic, "a fresh installation keeps the classic overlay")

    preference.selectMode(.vertical)
    XCTAssertTrue(preference.enabled)
    XCTAssertEqual(preference.layout, .vertical)
    XCTAssertEqual(OverlayChannelMapPreference(defaults: defaults).mode, .vertical)

    preference.selectMode(.classic)
    XCTAssertFalse(preference.enabled)
    XCTAssertEqual(preference.mode, .classic)
    XCTAssertEqual(
      preference.layout, .vertical, "classic does not forget which layout was chosen")

    preference.selectMode(.horizontal)
    XCTAssertEqual(OverlayChannelMapPreference(defaults: defaults).mode, .horizontal)
    XCTAssertEqual(
      OverlayIntercomPresentationMode(enabled: false, layout: .horizontal), .classic,
      "a disabled map is the classic overlay regardless of the stored layout")
  }

  func testSwitchingLayoutKeepsCaptureSelectionAndDraft() throws {
    let state = OverlayState.previewFormatted()
    optIntoChannelMap(state)
    state.applyChannelRoster([
      CsChannelRosterState(
        channel: "3", audience: "astra", provider: "codex", providerSessionId: "layout-fixture",
        open: true, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
        unreadSinceUnixMs: nil)
    ])
    let generation = state.captureGeneration
    let selected = state.selectedConversationID
    let hud = state.channelHudStates
    state.conversationDrafts["layout-draft"] = "unfinished message"
    state.channelMapPreference.selectLayout(.vertical)
    XCTAssertTrue(state.channelMapEnabled)
    XCTAssertFalse(state.channelMapInHeader)
    XCTAssertNil(DictationOverlayView(state: state).channelMap)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertEqual(state.selectedConversationID, selected)
    XCTAssertEqual(state.channelHudStates, hud)
    XCTAssertEqual(state.conversationDrafts["layout-draft"], "unfinished message")
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
    state.channelMapPreference.selectLayout(.horizontal)
    XCTAssertTrue(state.channelMapInHeader)
    XCTAssertNotNil(DictationOverlayView(state: state).channelMap)
  }

  func testOptOutHidesOnlyMapAndKeepsConversationAndCapture() throws {
    let state = OverlayState.previewFormatted()
    optIntoChannelMap(state)
    state.applyChannelRoster([
      CsChannelRosterState(
        channel: "3", audience: "astra", provider: "codex", providerSessionId: "astra-test",
        open: true, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
        unreadSinceUnixMs: nil)
    ])
    let view = DictationOverlayView(state: state)
    XCTAssertNotNil(view.channelMap)
    let generation = state.captureGeneration
    let selected = state.selectedConversationID
    let hud = state.channelHudStates
    state.channelMapPreference.select(false)
    XCTAssertNil(view.channelMap)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertEqual(state.selectedConversationID, selected)
    XCTAssertEqual(state.channelHudStates, hud)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
  }

  func testDisabledPanelUsesEarlierMiniAndCompactWidths() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let panel = FloatingOverlayPanel(
      contentRect: .zero, styleMask: [.borderless], backing: .buffered, defer: false)
    defer { panel.invalidatePresence() }
    panel.setChannelMapPresentation(enabled: false, capturing: false, naturalWidth: 160)
    XCTAssertEqual(panel.miniSize.width, 200)
    panel.setChannelMapPresentation(enabled: false, capturing: true, naturalWidth: 160)
    XCTAssertEqual(panel.miniSize.width, 164)
    panel.setChannelMapPresentation(enabled: true, capturing: true, naturalWidth: 160)
    XCTAssertEqual(panel.miniSize.width, 160)
    panel.setMidiNaturalWidth(DictationOverlayWindow.midiSize.width)
    XCTAssertEqual(panel.midiPresentationWidth, 410)
  }
}

extension XCTestCase {
  /// Map-specific fixtures opt in without changing the person's app defaults.
  @MainActor
  func optIntoChannelMap(_ state: OverlayState) {
    let name = "CodescribeTests.channel-map.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: name)!
    addTeardownBlock { defaults.removePersistentDomain(forName: name) }
    let preference = OverlayChannelMapPreference(defaults: defaults)
    preference.select(true)
    state.channelMapPreference = preference
  }
}
