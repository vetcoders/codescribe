import AppKit
import XCTest

@testable import Codescribe

/// The recorder's chord model must agree with core `WidgetShortcut`: the same
/// wire ids, the same glyph labels, the same refusals, the same keycodes.
final class WidgetShortcutChordTests: XCTestCase {
  func testDefaultIsControlShiftBAndRoundTrips() {
    let chord = WidgetShortcutChord.default
    XCTAssertEqual(chord.wireId, "ctrl+shift+b")
    XCTAssertEqual(chord.label, "⌃⇧B")
    XCTAssertEqual(WidgetShortcutChord(wireId: "ctrl+shift+b"), chord)
    XCTAssertEqual(WidgetShortcutChord(wireId: "Control + Shift + B"), chord, "aliases and case")
  }

  func testCustomChordKeepsTheCoreWireOrderAndGlyphOrder() {
    let chord = WidgetShortcutChord(wireId: "alt+cmd+space")
    XCTAssertEqual(chord?.wireId, "option+cmd+space")
    XCTAssertEqual(chord?.label, "⌥⌘Space")
    XCTAssertEqual(WidgetShortcutChord(wireId: "ctrl+option+k")?.label, "⌃⌥K")
  }

  func testRefusesWhatTheCoreRefuses() {
    XCTAssertNil(WidgetShortcutChord(wireId: "b"), "a bare letter types")
    XCTAssertNil(WidgetShortcutChord(wireId: "shift+b"), "Shift alone types")
    XCTAssertNil(WidgetShortcutChord(wireId: "ctrl+shift"), "no key")
    XCTAssertNil(WidgetShortcutChord(wireId: "ctrl+shift+1"), "digits are channels")
    XCTAssertNil(WidgetShortcutChord(wireId: "cmd+shift+space"), "the Agent summon")
    XCTAssertNotNil(WidgetShortcutChord(wireId: "ctrl+cmd+shift+space"))
  }

  func testAKeyPressBecomesTheChordByPhysicalKey() {
    let recorded = WidgetShortcutChord.from(modifierFlags: [.control, .shift], keyCode: 11)
    XCTAssertEqual(recorded, .default, "keycode 11 is the B key")
    XCTAssertEqual(
      WidgetShortcutChord.from(modifierFlags: [.option, .command], keyCode: 49)?.wireId,
      "option+cmd+space")
    XCTAssertNil(WidgetShortcutChord.from(modifierFlags: [.control], keyCode: 18), "the 1 key")
    XCTAssertNil(WidgetShortcutChord.from(modifierFlags: [.control], keyCode: 36), "Return")
    XCTAssertEqual(
      WidgetShortcutChord.from(modifierFlags: [.shift], keyCode: 11)?.refusal,
      .needsCommandModifier)
    XCTAssertEqual(
      WidgetShortcutChord.from(modifierFlags: [.command, .shift], keyCode: 49)?.refusal,
      .agentSummon)
  }
}
