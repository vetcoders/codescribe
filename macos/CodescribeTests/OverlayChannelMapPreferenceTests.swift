import AppKit
import Foundation
import Observation
import XCTest

@testable import Codescribe

@MainActor
final class OverlayChannelMapPreferenceTests: XCTestCase {
  func testSavedBetaChoiceFollowsCompiledChannelWithoutRewritingPreferences() throws {
    let name = "CodescribeTests.channel-lane.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    defaults.set(true, forKey: OverlayChannelMapPreference.key)
    defaults.set("vertical", forKey: OverlayChannelMapPreference.layoutKey)
    let preference = OverlayChannelMapPreference(defaults: defaults)
    #if CHANNEL_nightly
      XCTAssertTrue(preference.enabled)
      XCTAssertEqual(preference.mode, .vertical)
      preference.selectMode(.horizontal)
      XCTAssertEqual(preference.mode, .horizontal)
    #else
      XCTAssertFalse(preference.enabled)
      XCTAssertEqual(preference.mode, .classic)
      let original = defaults.dictionaryRepresentation() as NSDictionary
      preference.select(true)
      preference.selectLayout(.horizontal)
      preference.selectMode(.vertical)
      XCTAssertEqual(defaults.dictionaryRepresentation() as NSDictionary, original)
      XCTAssertFalse(preference.enabled)
      XCTAssertEqual(OverlayChannelMapPreference(defaults: defaults).mode, .classic)
    #endif
  }

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
    var reveals = 0
    state.onRevealIntercom = { reveals += 1 }
    for mode: OverlayPresentationMode in [.mini, .midi, .expanded] {
      state.setPresentationMode(mode)
      XCTAssertNil(
        DictationOverlayView(state: state).channelMap,
        "Fiszki mode removes the duplicate channelMap counter from the main overlay")
      state.toggleAgentSidebar()
      XCTAssertEqual(state.presentationMode, mode, "Reveal must not switch overlay density")
    }
    XCTAssertEqual(reveals, 3)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertEqual(state.selectedConversationID, selected)
    XCTAssertEqual(state.channelHudStates, hud)
    XCTAssertEqual(state.conversationDrafts["layout-draft"], "unfinished message")
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
    state.channelMapPreference.selectLayout(.horizontal)
    XCTAssertTrue(state.channelMapInHeader)
    XCTAssertEqual(DictationOverlayView(state: state).channelMap?.stacked, false)
    state.setPresentationMode(.midi)
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

  /// Mini with the map off is its controls plus the measured brand word
  /// after its 6 pt gap (bus brief 2026-11-04 §3): never narrower than its
  /// controls, so the chevron is never clipped at the capsule's edge
  /// (Maciej, 2026-10-30 14:24).
  func testMiniBrandWidthIsTheMeasuredWordRowAndNeverNarrowerThanItsControls() {
    typealias W = DictationOverlayWindow
    let font = NSFont.systemFont(ofSize: W.miniWordSize, weight: .bold)
    let word =
      ceil(
        ("codescribe" as NSString).size(withAttributes: [.font: font, .kern: W.miniWordTracking])
          .width) + 1
    let row = W.miniFloorWidth + W.miniBrandGap + word
    XCTAssertEqual(W.miniBrandWidth, ceil(row))
    XCTAssertGreaterThanOrEqual(W.miniBrandWidth, row, "the chevron is never clipped")
    XCTAssertGreaterThan(W.miniBrandWidth, W.miniFloorWidth)
    XCTAssertEqual(W.miniBrandWidth, W.miniBrandWidth.rounded(.up), "whole points")
  }

  /// The map setting picks mini's resting width, the brand word or the dot
  /// alone; a live take adds its measured width on top and nothing remembers
  /// it (bus brief 2026-11-04 §3, §6). Mini is 36 pt tall either way.
  func testPanelMiniSizeFollowsTheMapSettingAndTheTake() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let panel = FloatingOverlayPanel(
      contentRect: .zero, styleMask: [.borderless], backing: .buffered, defer: false)
    defer { panel.invalidatePresence() }
    panel.setChannelMapPresentation(enabled: false)
    XCTAssertEqual(
      panel.miniSize,
      NSSize(
        width: DictationOverlayWindow.miniBrandWidth, height: DictationOverlayWindow.miniHeight))
    panel.setChannelMapPresentation(enabled: true)
    XCTAssertEqual(
      panel.miniSize,
      NSSize(
        width: DictationOverlayWindow.miniFloorWidth, height: DictationOverlayWindow.miniHeight))
    let take = DictationOverlayWindow.miniTakeWidth(live: true, openDigits: ["2", "5"])
    panel.setMiniTake(width: take)
    XCTAssertEqual(panel.miniTakeWidth, take)
    XCTAssertEqual(panel.miniSize.width, DictationOverlayWindow.miniFloorWidth + take)
    XCTAssertEqual(
      panel.miniSize.height, DictationOverlayWindow.miniHeight, "a take adds no height")
    panel.setMiniTake(width: 0)
    XCTAssertEqual(panel.miniSize.width, DictationOverlayWindow.miniFloorWidth, "back to rest")
    // Without the horizontal map the bar is the classic 410 × 46 strip.
    panel.setMidiBarGeometry(.classic)
    XCTAssertEqual(panel.midiPresentationWidth, 410)
    XCTAssertEqual(
      panel.midiSize, NSSize(width: 410, height: DictationOverlayWindow.collapsedHeight))
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
