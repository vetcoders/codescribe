import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayFastHintTests: XCTestCase {
  /// A hint must not enlarge a compact button, cover its click target, or
  /// create a window when its control has no visible owning window.
  func testHoverPreservesControlGeometryAndClickThroughTracking() throws {
    for leading in [false, true] {
      for enabled in [false, true] {
        let host = NSHostingView(
          rootView: Button("Copy") {}.buttonStyle(.plain)
            .frame(width: 32, height: 24)
            .overlayFastHint(
              "A long action description that exceeds the width of a narrow rail",
              palette: .dark, leading: leading, enabled: enabled))
        host.frame = NSRect(x: 0, y: 0, width: 32, height: 24)
        host.layoutSubtreeIfNeeded()
        let region = try XCTUnwrap(
          descendants(host).compactMap { $0 as? OverlayMenuCursorRegionView }.first)
        XCTAssertNotNil(region.onHover)
        XCTAssertNil(region.hitTest(NSPoint(x: 1, y: 1)), "The button owns clicks")
        let before = host.fittingSize
        region.onHover?(true)
        host.layoutSubtreeIfNeeded()
        XCTAssertEqual(host.fittingSize.width, before.width, accuracy: 0.5)
        XCTAssertEqual(host.fittingSize.height, before.height, accuracy: 0.5)
        XCTAssertEqual(host.fittingSize.width, 32, accuracy: 0.5)
        XCTAssertEqual(host.fittingSize.height, 24, accuracy: 0.5)
        XCTAssertNil(host.window)
        XCTAssertNil(OverlayHintPanel.shared.presentedPanel)
        region.onHover?(false)
      }
    }
  }

  /// Exercise the compiled product catalog, including unavailable states:
  /// VoiceOver and visual hints must resolve actual Polish copy, not keys.
  func testActionAndUnavailableHintsResolveFromBothProductLocalizations() throws {
    let polish = try XCTUnwrap(
      Bundle.main.path(forResource: "pl", ofType: "lproj").flatMap(Bundle.init(path:)))
    let expected = [
      "Microphone unavailable": "Mikrofon niedostępny",
      "No conversation on this channel": "Na tym kanale nie ma rozmowy",
      "Nothing to copy": "Nie ma czego skopiować",
      "Playback unavailable": "Odtwarzanie niedostępne",
      "Reply to this message": "Odpowiedz na tę wiadomość",
      "Show the message this replies to": "Pokaż wiadomość, na którą to jest odpowiedź",
    ]
    let english = try XCTUnwrap(
      Bundle.main.path(forResource: "en", ofType: "lproj").flatMap(Bundle.init(path:)))
    for (key, value) in expected {
      XCTAssertEqual(polish.localizedString(forKey: key, value: nil, table: "Localizable"), value)
      XCTAssertEqual(english.localizedString(forKey: key, value: nil, table: "Localizable"), key)
    }
  }

  private func descendants(_ view: NSView) -> [NSView] {
    [view] + view.subviews.flatMap(descendants)
  }
}
