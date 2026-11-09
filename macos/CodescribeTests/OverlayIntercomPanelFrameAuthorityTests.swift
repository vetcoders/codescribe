import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Three SIGABRTs on 2026-11-07 (04:44, 14:36, 16:09; builds 3638e11e8 and
/// 50147d049, both carrying b433db1d7's `sizingOptions = []`):
///
///     NSGenericException: The window has been marked as needing another Update
///     Constraints in Window pass, but it has already had more Update Constraints
///     in Window passes than there are views in the window.
///     <Codescribe.OverlayIntercomPanel: …> {{0, -4462}, {16296, 4462}}
///
/// The AppKit DisplayCycle log shows the loop: inside one `NSDisplayCycleFlush`,
/// `NSHostingView.windowDidLayout()` calls `updateAnimatedWindowSize(_:)`, which
/// sets the card's window frame to the content's ideal size (a long conversation
/// laid out on one line) with the top-left corner pinned, the frame change
/// invalidates the hosting view's safe area, that marks the window for another
/// layout pass, the controller fits the card back, and the pass calls
/// `windowDidLayout()` again — 90 times in 50 ms, then AppKit throws. Empty
/// `sizingOptions` does not switch that path off: on the unfixed tree the first
/// layout after presenting the fixture below already turned a 320×120 panel into
/// `(40, -3840, 135228, 4000)`.
final class OverlayIntercomPanelFrameAuthorityTests: XCTestCase {
  /// Content whose ideal size dwarfs every screen, as a long conversation does.
  private var oversizedContent: some View {
    Text(String(repeating: "lorem ipsum dolor ", count: 1200))
      .fixedSize()
      .frame(height: 4000)
  }

  @MainActor
  private func presentedCard(frame: CGRect) -> OverlayIntercomPanel {
    let panel = OverlayIntercomPanel()
    let hosting = NSHostingView(rootView: oversizedContent)
    hosting.sizingOptions = []
    panel.contentView = hosting
    panel.resize(to: frame, display: false)
    BackgroundTestWindow.present(panel)
    return panel
  }

  @MainActor
  private func pump(_ panel: NSWindow, seconds: TimeInterval) {
    let deadline = Date(timeIntervalSinceNow: seconds)
    while Date() < deadline {
      RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.02))
      panel.layoutIfNeeded()
    }
  }

  @MainActor
  func testPresentingACardNeverLetsSwiftUIResizeIt() {
    let start = CGRect(x: 40, y: 40, width: 320, height: 120)
    let panel = presentedCard(frame: start)
    defer {
      panel.orderOut(nil)
      panel.close()
    }
    panel.layoutIfNeeded()
    pump(panel, seconds: 0.2)
    print(
      "frame-authority-witness frame=\(panel.frame) refused=\(panel.refusedForeignSizes)")
    XCTAssertEqual(panel.frame, start, "a presented card keeps the frame it was given")
  }

  @MainActor
  func testAnAnimatedSlideKeepsTheSizeAndLandsOnItsRow() {
    let start = CGRect(x: 40, y: 40, width: 320, height: 120)
    let panel = presentedCard(frame: start)
    defer {
      panel.orderOut(nil)
      panel.close()
    }
    panel.layoutIfNeeded()
    // The production slide: a new row target with the same size (positionCards).
    let slid = start.offsetBy(dx: 0, dy: -60)
    NSAnimationContext.runAnimationGroup { context in
      context.duration = 0.1
      context.timingFunction = CAMediaTimingFunction(name: .easeOut)
      panel.animator().setFrame(slid, display: true)
    }
    pump(panel, seconds: 0.6)
    XCTAssertEqual(panel.frame.size, start.size, "only the controller sizes a card")
    XCTAssertEqual(panel.frame.origin, slid.origin, "the slide still lands on its row")
  }

  @MainActor
  func testOnlyTheControllerChangesACardsSize() {
    let panel = OverlayIntercomPanel()
    defer { panel.close() }
    let given = CGRect(x: -10000, y: -10000, width: 200, height: 80)
    panel.resize(to: given, display: false)
    XCTAssertEqual(panel.frame, given)

    panel.setFrame(given.insetBy(dx: -50, dy: -50), display: false)
    panel.setFrame(CGRect(origin: given.origin, size: CGSize(width: 900, height: 900)),
      display: false, animate: false)
    panel.setContentSize(CGSize(width: 640, height: 480))
    XCTAssertEqual(panel.frame, given, "foreign sizes are dropped")
    XCTAssertEqual(panel.refusedForeignSizes, 3)

    let moved = given.offsetBy(dx: 30, dy: -30)
    panel.setFrame(moved, display: false)
    XCTAssertEqual(panel.frame, moved, "a move at the same size passes untouched")

    let grown = CGRect(origin: moved.origin, size: CGSize(width: 260, height: 140))
    panel.resize(to: grown, display: false)
    XCTAssertEqual(panel.frame, grown, "the controller's resize is honoured")
    XCTAssertEqual(panel.refusedForeignSizes, 3)
  }
}
