import SwiftUI

// Motion constants from the handoff keyframes. Short, breathing, never show-off.
enum CSMotion {
  /// Tempo tokens. Values are the app's current ones, named; nothing was
  /// retimed (overlay under heat observation, founder 2026-10-26).
  enum Tempo {
    static let hover: TimeInterval = 0.18  // hover / press feedback
    static let enter: TimeInterval = 0.35  // enter / reveal
    static let lampPulse: TimeInterval = 1.2  // recording light pulse period
    static let processingArc: TimeInterval = 1.2  // one turn of the transcribing arc
    static let pulse: TimeInterval = 2.4  // slow breathing pulse
    static let ripple: TimeInterval = 2.5  // ripple period
  }
  // expanding mic ring
  static let ripple = Animation.easeOut(duration: Tempo.ripple).repeatForever(autoreverses: false)
  // opacity .6→1 breathing
  static let softpulse = Animation.easeInOut(duration: Tempo.pulse).repeatForever(
    autoreverses: true)
  // waveform bar scaleY .35→1 (per-bar duration varies; base here)
  static func eq(_ duration: Double) -> Animation {
    Animation.easeInOut(duration: duration).repeatForever(autoreverses: true)
  }
  // word/element rise + fade
  static let floatIn = Animation.easeOut(duration: Tempo.enter)
  // cursor blink (1s steps)
  static let blink = Animation.linear(duration: 1).repeatForever(autoreverses: true)
}
