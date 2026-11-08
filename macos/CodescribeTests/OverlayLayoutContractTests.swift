import XCTest

@testable import Codescribe

/// The per-layout truth table. Each row is a decision owned by one layout's
/// contract; a change here is a product decision for THAT layout, named in
/// the contract's doc comment, never a side effect of another layout's work.
final class OverlayLayoutContractTests: XCTestCase {

  func testClassicKeepsTheProvenStripAndCarriesNoMap() {
    let c = ClassicLayoutContract()
    XCTAssertEqual(c.mode, .classic)
    XCTAssertFalse(c.mapInHeader)
    XCTAssertFalse(c.mapInMini)
    XCTAssertFalse(c.counterStacked)
    XCTAssertFalse(c.takeResizesSurface, "a take never resizes the classic strip")
    XCTAssertTrue(c.meterVisibleAtRest)
  }

  func testBarCarriesTheMapInHeaderAndMini() {
    let c = BarLayoutContract()
    XCTAssertEqual(c.mode, .horizontal)
    XCTAssertTrue(c.mapInHeader)
    XCTAssertTrue(c.mapInMini, "mini trades the brand word for the map (Monika, 2026-11-04 §3)")
    XCTAssertFalse(c.counterStacked)
    XCTAssertFalse(c.takeResizesSurface, "the bar's width never follows a take")
    XCTAssertTrue(c.meterVisibleAtRest)
    XCTAssertFalse(
      c.collapsedClickOpensWindow, "mini and the bar toggle (Monika, 2026-11-04 §12.1)")
    XCTAssertFalse(c.windowHeaderClickRestoresCompactForm, "the window returns to the bar")
  }

  /// Maciej, 2026-11-06: in Klasyczny and Fiszki a click on the collapsed
  /// overlay opens the window, and the window's header returns to the compact
  /// form it replaced.
  func testClassicAndFiszkiOwnTheirHeaderClick() {
    for c in [ClassicLayoutContract() as any OverlayLayoutContract, FiszkiLayoutContract()] {
      XCTAssertTrue(c.collapsedClickOpensWindow, "\(c.mode)")
      XCTAssertTrue(c.windowHeaderClickRestoresCompactForm, "\(c.mode)")
    }
  }

  func testFiszkiKeepTheMapInTheIntercomGroup() {
    let c = FiszkiLayoutContract()
    XCTAssertEqual(c.mode, .vertical)
    XCTAssertFalse(c.mapInHeader)
    XCTAssertFalse(c.mapInMini)
    XCTAssertFalse(c.counterStacked)
    // Decyzja Macieja 2026-11-06: recording start will expand the fiszki
    // overlay (waveform + clock, + transcript view while active). That cut
    // flips takeResizesSurface here, with its own acceptance; until it
    // lands, fiszki keep the shared stability.
    XCTAssertFalse(c.takeResizesSurface)
    XCTAssertTrue(c.meterVisibleAtRest)
  }

  /// The factory is the single switch point: the Settings picker's mode and
  /// nothing else chooses the contract.
  func testEveryModeResolvesToItsOwnContract() {
    for mode in OverlayIntercomPresentationMode.allCases {
      XCTAssertEqual(mode.contract.mode, mode)
    }
    XCTAssertTrue(OverlayIntercomPresentationMode.classic.contract is ClassicLayoutContract)
    XCTAssertTrue(OverlayIntercomPresentationMode.horizontal.contract is BarLayoutContract)
    XCTAssertTrue(OverlayIntercomPresentationMode.vertical.contract is FiszkiLayoutContract)
  }

  /// The contract mirrors the legacy `enabled` + `layout` pair exactly, for
  /// every combination the preference can store.
  func testContractMatchesLegacyPreferenceSemantics() {
    for (enabled, layout) in [(false, OverlayIntercomLayout.horizontal), (false, .vertical),
      (true, .horizontal), (true, .vertical)]
    {
      let mode = OverlayIntercomPresentationMode(enabled: enabled, layout: layout)
      let c = mode.contract
      XCTAssertEqual(c.mode != .classic, enabled)
      XCTAssertEqual(c.mapInHeader, enabled && layout == .horizontal)
      XCTAssertEqual(c.mapInMini, enabled && layout == .horizontal)
      XCTAssertEqual(c.mode == .vertical, enabled && layout == .vertical)
    }
  }
}
