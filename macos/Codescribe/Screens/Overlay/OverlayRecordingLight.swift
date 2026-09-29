import AppKit
import SwiftUI

/// The overlay's status light: one state, one colour, one name. The header dot,
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
    case .holdToTalk: "Recording"
    case .handsFree: "Recording hands-free"
    case .silence: "Silence"
    case .processing: "Transcribing"
    case .agent: "Recording for the agent"
    }
  }

  var colorName: String {
    switch self {
    case .holdToTalk: "Red"
    case .handsFree: "Pulsing red"
    case .silence: "Yellow"
    case .processing: "Orange"
    case .agent: "Violet"
    }
  }

  /// One sentence: what the colour means right now.
  var meaning: String {
    switch self {
    case .holdToTalk: "Capture is live while the shortcut is held."
    case .handsFree: "Capture stays live until you stop it."
    case .silence:
      "Capture is live, but nothing has reached speaking level for over a second."
    case .processing: "Capture has ended and the engine is transcribing the take."
    case .agent: "Capture is live and the words go to the agent."
    }
  }

  var tooltip: String { "\(colorName) — \(name). \(meaning)" }

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

/// Paints the status light. Colour, pulse, tooltip and VoiceOver text come from
/// `OverlayRecordingLight`; this view adds geometry only.
struct OverlayRecordingLightView: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.csTextScale) private var textScale
  @Environment(\.displayScale) private var displayScale

  let light: OverlayRecordingLight
  let palette: OverlayAppearancePalette
  /// False while the panel is hidden or occluded: a pulse nobody sees must not
  /// keep the render loop awake.
  let animates: Bool

  var body: some View {
    let diameter = OverlayRecordingLight.diameter(textScale: textScale)
    Group {
      if light.pulses && animates && !reduceMotion {
        TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
          dot.opacity(
            OverlayRecordingLight.pulseOpacity(
              at: timeline.date.timeIntervalSinceReferenceDate))
        }
      } else {
        dot
      }
    }
    .frame(width: diameter, height: diameter)
    .padding(4)
    .contentShape(Rectangle())
    .help(light.tooltip)
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(light.name)
    .accessibilityHint(light.meaning)
    .accessibilityIdentifier("overlay-recording-light")
  }

  /// Hairline ring keeps the yellow and orange hues legible on paper glass.
  private var dot: some View {
    Circle()
      .fill(light.color)
      .overlay {
        Circle()
          .strokeBorder(palette.border.color, lineWidth: 1 / max(displayScale, 1))
          .accessibilityHidden(true)
      }
  }
}
