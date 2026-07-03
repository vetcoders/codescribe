import SwiftUI

// Motion constants from the handoff keyframes. Short, breathing, never show-off.
enum CSMotion {
    // expanding mic ring
    static let ripple = Animation.easeOut(duration: 2.5).repeatForever(autoreverses: false)
    // opacity .6→1 breathing
    static let softpulse = Animation.easeInOut(duration: 2.4).repeatForever(autoreverses: true)
    // waveform bar scaleY .35→1 (per-bar duration varies; base here)
    static func eq(_ duration: Double) -> Animation {
        Animation.easeInOut(duration: duration).repeatForever(autoreverses: true)
    }
    // word/element rise + fade
    static let floatIn = Animation.easeOut(duration: 0.35)
    // cursor blink (1s steps)
    static let blink = Animation.linear(duration: 1).repeatForever(autoreverses: true)
    // hero glow breathe 6–7s
    static let breathe = Animation.easeInOut(duration: 6.5).repeatForever(autoreverses: true)
}
