import AppKit
import SwiftUI

/// The recording control's status: one state, one colour, one name. Its tint,
/// its tooltip and its VoiceOver text all read this table, so no colour can
/// appear on screen without a name the user can hover for.
///
/// Red, pulsing red, orange and violet are the hues of the cursor hold badge
/// (`app/os/hold_badge.rs`) and the menu-bar dot (`TrayStatusStore`), so the
/// overlay is where those colours get their names. Yellow is the overlay's own
/// silence state: capture is live, but nothing reaches speaking level.
enum OverlayRecordingLight: CaseIterable, Equatable, Sendable {
  case holdToTalk
  case handsFree
  case silence
  case processing
  case agent

  /// Diameter at text scale 1. Larger than the 7 pt close dot so the light
  /// reads as information, not as the brand mark next to the wordmark.
  static let baseDiameter: CGFloat = 10

  /// Follows the overlay text scale (⌘+ / ⌘-) like every CSFont.ui label.
  static func diameter(textScale: CGFloat) -> CGFloat {
    baseDiameter * textScale
  }

  /// Canonical recording semantics decide the light; nothing here reads text.
  /// A terminal take has no light: the outcome body and footer speak for it.
  static func resolve(
    mode: OverlayMode,
    terminal: Bool,
    recording: Bool,
    transcribing: Bool,
    indicatorMode: CsIndicatorMode,
    silent: Bool
  ) -> OverlayRecordingLight? {
    guard !terminal else { return nil }
    if mode == .finalizing || transcribing || indicatorMode == .processing {
      return .processing
    }
    guard recording, mode == .listening else { return nil }
    // The agent hue names the destination; silence does not hide it.
    if indicatorMode == .assistive { return .agent }
    if silent { return .silence }
    return indicatorMode == .toggle ? .handsFree : .holdToTalk
  }

  var name: String {
    switch self {
    case .holdToTalk: String(localized: "Recording", comment: "Recording light state")
    case .handsFree: String(localized: "Recording hands-free", comment: "Recording light state")
    case .silence: String(localized: "Silence", comment: "Recording light state")
    case .processing: String(localized: "Transcribing", comment: "Recording light state")
    case .agent:
      String(localized: "Recording for the agent", comment: "Recording light state")
    }
  }

  var colorName: String {
    switch self {
    case .holdToTalk: String(localized: "Red", comment: "Recording light colour")
    case .handsFree: String(localized: "Pulsing red", comment: "Recording light colour")
    case .silence: String(localized: "Yellow", comment: "Recording light colour")
    case .processing: String(localized: "Orange", comment: "Recording light colour")
    case .agent: String(localized: "Violet", comment: "Recording light colour")
    }
  }

  /// One sentence: what the colour means right now.
  var meaning: String {
    switch self {
    case .holdToTalk: String(localized: "Capture is live while the shortcut is held.")
    case .handsFree: String(localized: "Capture stays live until you stop it.")
    case .silence:
      String(
        localized: "Capture is live, but nothing has reached speaking level for over a second.")
    case .processing:
      String(localized: "Capture has ended and the engine is transcribing the take.")
    case .agent: String(localized: "Capture is live and the words go to the agent.")
    }
  }

  var tooltip: String {
    String(
      localized: "overlay.recordingLight.tooltip",
      defaultValue: "\(colorName) — \(name). \(meaning)",
      comment: "Recording light tooltip: colour name, state name, then one sentence of meaning")
  }

  /// Hold and hands-free share the recording red; the pulse tells them apart,
  /// exactly as the cursor badge does.
  var pulses: Bool {
    switch self {
    case .handsFree, .processing: true
    case .holdToTalk, .silence, .agent: false
    }
  }

  var nsColor: NSColor {
    switch self {
    case .holdToTalk, .handsFree: CSPalette.indicatorRecording
    case .silence: CSPalette.indicatorSilence
    case .processing: CSPalette.modeProcessing
    case .agent: CSPalette.assistive
    }
  }

  var color: Color { Color(nsColor: nsColor) }

  /// Opacity of a pulsing light at `time`: a raised cosine between 0.45 and 1
  /// with a 1.2 s period, computed from the clock so a paused timeline stops it.
  static func pulseOpacity(at time: TimeInterval) -> Double {
    0.725 + 0.275 * cos(time * 2 * .pi / 1.2)
  }
}
