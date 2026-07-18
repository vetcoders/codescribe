import SwiftUI

// codescribe design tokens — single source of truth for color.
// Locked palette from the handoff (README-HANDOFF.md · Design Tokens).
// Terracotta = the ONE brand accent (active / voice / primary).
// Assistive violet = voice routed to the agent. Olive/green = healthy status.
// Amber = reasoning/format meta.
// NO macOS system-blue anywhere in the redesigned UI.

extension Color {
    init(hex: UInt32, alpha: Double = 1.0) {
        let r = Double((hex >> 16) & 0xFF) / 255.0
        let g = Double((hex >> 8) & 0xFF) / 255.0
        let b = Double(hex & 0xFF) / 255.0
        self.init(.sRGB, red: r, green: g, blue: b, opacity: alpha)
    }
}

enum CSColor {
    // Surfaces
    static let ink = Color(hex: 0x090A0D)                 // page base (near-black)
    static let glassBase = Color(hex: 0x12141A, alpha: 0.84) // app window material over #0b0c10
    static let glassUnder = Color(hex: 0x0B0C10)
    static func surfaceRaised(_ a: Double = 0.03) -> Color { Color.white.opacity(a) } // .02–.04
    static func hairline(_ a: Double = 0.07) -> Color { Color.white.opacity(a) }      // .06–.09

    // Brand accent — terracotta
    static let terracotta = Color(hex: 0xD97757)          // primary / active / voice
    static let terracottaLight = Color(hex: 0xE9B79F)     // active labels (text on dark accent)
    static let terracottaDeep = Color(hex: 0xC98A6E)      // secondary voice accent
    static let terracottaTintBars = Color(hex: 0xE6A98F)  // every-5th waveform bar

    // Assistive accent — agent-routed voice
    static let assistive = Color(hex: 0x9B72F2)
    static let assistiveLight = Color(hex: 0xC9B7FF)

    // Status — olive / green
    static let olive = Color(hex: 0x5F6B3E)               // healthy base
    static let oliveLight = Color(hex: 0x9DB178)          // idle / granted / success dot
    static let eyebrowOlive = Color(hex: 0x7F8C5E)        // section eyebrows

    // Reasoning — amber
    static let amber = Color(hex: 0xD6B24E)

    // Destructive actions — reserved for explicit danger-zone controls
    static let danger = Color(hex: 0xD84A4A)
    static let dangerLight = Color(hex: 0xFFAAA5)

    // Text
    static let textHigh = Color(hex: 0xF4F2EC)            // headlines
    static let textBody = Color(hex: 0xE9E7E0)
    static let textBodyAlt = Color(hex: 0xDFE2DB)
    static let textMuted = Color(hex: 0x9A9D97)
    static let textMutedAlt = Color(hex: 0x82857F)
    static let textFaint = Color(hex: 0x6F7268)           // mono meta
    static let textFaintAlt = Color(hex: 0x5D6058)        // timestamps
}

enum CSRadius {
    static let input: CGFloat = 9
    static let card: CGFloat = 12
    static let window: CGFloat = 22
    static let tray: CGFloat = 14
    static let pill: CGFloat = 20
}
