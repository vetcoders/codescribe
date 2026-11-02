import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// The provider sheet must contain its actual native input controls. A fixed
/// outer width alone can hide an oversized child by clipping its leading edge.
@MainActor
final class CustomProviderFormLayoutTests: XCTestCase {
  func testNativeInputsStayInsideTheSheetInEnglishAndPolish() throws {
    for language in ["en", "pl"] {
      for dark in [false, true] {
        let model = SettingsViewModel.preview(.keys)
        let root = CustomProviderForm(model: model, target: .add)
          .environment(\.locale, Locale(identifier: language))
          .environment(\.colorScheme, dark ? .dark : .light)
        let host = NSHostingView(rootView: root)
        host.sizingOptions = []
        host.frame = NSRect(x: 0, y: 0, width: 480, height: 600)
        let window = NSWindow(
          contentRect: NSRect(x: -10000, y: -10000, width: 480, height: 600),
          styleMask: .borderless, backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        window.contentView = host
        defer { window.close() }
        BackgroundTestWindow.present(window)
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        host.layoutSubtreeIfNeeded()

        let controls = nativeControls(in: host)
        XCTAssertGreaterThanOrEqual(controls.count, 3, "Inspect real name, endpoint and key fields")
        for control in controls {
          let frame = control.convert(control.bounds, to: host)
          XCTAssertGreaterThanOrEqual(
            frame.minX, -0.5,
            "\(language), dark=\(dark): \(type(of: control)) starts outside the sheet: \(frame)")
          XCTAssertLessThanOrEqual(
            frame.maxX, host.bounds.maxX + 0.5,
            "\(language), dark=\(dark): \(type(of: control)) ends outside the sheet: \(frame)")
        }
      }
    }
  }

  private func nativeControls(in root: NSView) -> [NSControl] {
    var result: [NSControl] = []
    if let control = root as? NSControl,
      !control.isHiddenOrHasHiddenAncestor,
      control.bounds.width > 1, control.bounds.height > 1
    {
      result.append(control)
    }
    return result + root.subviews.flatMap { nativeControls(in: $0) }
  }
}
