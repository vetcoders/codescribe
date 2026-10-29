import AppKit
import SwiftUI

/// The pointer dot's palette as the core paints it. The core owns the colours
/// (`IndicatorState::color` in `app/os/hold_badge.rs`) and hands them over
/// through `indicatorPalette()`; the Settings legend and the recording lamp read
/// them here, so no Swift surface keeps its own copy of the dot's colours
/// (Founder smoke S5, 2026-10-26: the legend showed colours the dot never drew).
enum IndicatorPalette {
  /// Every state the dot shows, in legend order, read once from the core.
  static let swatches: [CsIndicatorSwatch] = indicatorPalette()

  /// The swatch the core paints for `state`. The core's palette covers every
  /// state (pinned by its own tests), so a missing one is a broken build.
  static func swatch(_ state: CsIndicatorState) -> CsIndicatorSwatch {
    guard let swatch = swatches.first(where: { $0.state == state }) else {
      preconditionFailure("indicator_palette has no \(state)")
    }
    return swatch
  }

  /// The dot's hue for `state`, opaque: a lamp or a menu-bar dot draws the
  /// same colour as the pointer dot without its translucency.
  static func opaqueColor(_ state: CsIndicatorState) -> NSColor {
    swatch(state).nsColor.withAlphaComponent(1)
  }
}

extension CsIndicatorSwatch {
  /// The colour exactly as the pointer dot paints it, translucency included.
  var nsColor: NSColor {
    NSColor(srgbRed: red, green: green, blue: blue, alpha: alpha)
  }

  /// The rim's colour, for the dot that needs one over a white page.
  var rimColor: NSColor? {
    rim.map { IndicatorPalette.opaqueColor($0) }
  }
}

extension CsIndicatorState {
  /// The state's name in the Settings legend.
  var legendName: String {
    switch self {
    case .recording: String(localized: "Recording")
    case .wordsOwed: String(localized: "Words still arriving")
    case .processing: String(localized: "Processing")
    case .agent: String(localized: "Agent")
    case .agentQuiet: String(localized: "Agent between phrases")
    case .refused: String(localized: "Shortcut refused")
    }
  }
}
