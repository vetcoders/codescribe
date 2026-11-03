import Foundation
import XCTest

@testable import Codescribe

/// One key, one role: Settings › Shortcuts never offers a key that records
/// dictation as the Agent channel modifier, and says so in one sentence. The
/// core resolves the modifier in effect; the view model only projects it.
@MainActor
final class ChannelModifierRecordingKeyTests: XCTestCase {
  private func model(
    channelModifier: String, fnRecords: Bool, ctrlRecords: Bool,
    fnBinding: Bool = false, fnTap: Bool = false
  ) -> SettingsViewModel {
    var stored = CsSettings.sample
    stored.channelModifier = channelModifier
    stored.fnRecordsDictation = fnRecords
    stored.fnBindingRecordsDictation = fnBinding
    stored.fnTapTogglesDictation = fnTap
    stored.ctrlRecordsDictation = ctrlRecords
    let engine = MockSettingsEngine(settingsLoader: { stored })
    return SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
  }

  /// The Founder's settings: Hold Fn/Globe and the Fn tap record, the stored
  /// `fn` resolved to Ctrl, and the sentence names both steps that free Fn.
  func testFnRecordingGreysFnAndShowsCtrlInEffect() {
    let model = model(
      channelModifier: "ctrl", fnRecords: true, ctrlRecords: false, fnBinding: true, fnTap: true)
    XCTAssertEqual(model.channelModifier, "ctrl")
    XCTAssertFalse(model.channelModifierFnAvailable)
    XCTAssertTrue(model.channelModifierCtrlAvailable)
    XCTAssertEqual(
      model.channelModifierRecordingNote,
      String(
        localized:
          "Fn records dictation — to use it for channels, assign dictation to another shortcut and turn off the Fn tap."
      ))
  }

  /// Only the parts that apply: the binding alone, or the tap alone.
  func testFnSentenceNamesOnlyWhatRecords() {
    XCTAssertEqual(
      model(channelModifier: "ctrl", fnRecords: true, ctrlRecords: false, fnBinding: true)
        .channelModifierRecordingNote,
      String(
        localized:
          "Fn records dictation — to use it for channels, assign dictation to another shortcut."))
    XCTAssertEqual(
      model(channelModifier: "ctrl", fnRecords: true, ctrlRecords: false, fnTap: true)
        .channelModifierRecordingNote,
      String(localized: "Fn records dictation — to use it for channels, turn off the Fn tap."))
  }

  func testCtrlRecordingGreysCtrlAndShowsFnInEffect() {
    let model = model(channelModifier: "fn", fnRecords: false, ctrlRecords: true)
    XCTAssertEqual(model.channelModifier, "fn")
    XCTAssertTrue(model.channelModifierFnAvailable)
    XCTAssertFalse(model.channelModifierCtrlAvailable)
    XCTAssertNotNil(model.channelModifierRecordingNote)
  }

  /// Both keys record: the disabled Off segment represents the effective
  /// value, and the sentence points at the agents list.
  func testBothKeysRecordingLeaveNoChannelShortcut() {
    let model = model(channelModifier: "off", fnRecords: true, ctrlRecords: true)
    XCTAssertEqual(model.channelModifier, "off")
    XCTAssertFalse(model.channelModifierFnAvailable)
    XCTAssertFalse(model.channelModifierCtrlAvailable)
    XCTAssertEqual(
      model.channelModifierRecordingNote,
      String(
        localized:
          "Ctrl and Fn both record dictation, so open Agent channels from the agents list."))
  }

  func testFreeKeysOfferBothWithoutANote() {
    let model = model(channelModifier: "fn", fnRecords: false, ctrlRecords: false)
    XCTAssertEqual(model.channelModifier, "fn")
    XCTAssertTrue(model.channelModifierFnAvailable)
    XCTAssertTrue(model.channelModifierCtrlAvailable)
    XCTAssertNil(model.channelModifierRecordingNote)
  }

  func testPickerRepresentsTheEffectiveOffValueWithoutOfferingItAsAChoice() throws {
    let path = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens/Settings/ShortcutsPanel.swift")
    let source = try String(contentsOf: path, encoding: .utf8)
    XCTAssertTrue(source.contains("if model.channelModifier == \"off\""))
    XCTAssertTrue(source.contains("Text(\"Off\").tag(\"off\")\n                .disabled(true)"))
  }
}
