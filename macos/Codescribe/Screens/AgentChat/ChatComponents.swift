import SwiftUI

// Screen-local helpers for Agent Chat. Off-token shades from the mock that the
// shared CSColor palette does not carry verbatim live here (and ONLY here).

enum ChatPalette {
    static let nameInactive = Color(hex: 0xC7CABF)   // inactive thread name / segmented body
    static let nameActive = Color(hex: 0xF0EEE7)     // active thread name / titles / you-bubble text
    static let activeThreadSub = Color(hex: 0x9A7A6A) // "active · restored" subtitle
    static let toolBody = Color(hex: 0x9AA093)        // tool-activity detail text
    static let thinking = Color(hex: 0x8A8D87)        // "thinking…" label
    static let sendGlyph = Color(hex: 0x0A0A0A)       // ↑ glyph on terracotta button
}

/// Expanding terracotta ring + solid dot — the composer mic affordance.
struct RippleMic: View {
    @State private var animate = false
    var body: some View {
        ZStack {
            Circle()
                .strokeBorder(CSColor.terracotta, lineWidth: 1)
                .frame(width: 12, height: 12)
                .scaleEffect(animate ? 2.7 : 0.5)
                .opacity(animate ? 0 : 0.7)
            Circle()
                .fill(CSColor.terracotta)
                .frame(width: 6, height: 6)
        }
        .frame(width: 12, height: 12)
        .onAppear { withAnimation(CSMotion.ripple) { animate = true } }
    }
}

/// Blinking terracotta caret shown while a turn streams.
struct BlinkCaret: View {
    @State private var on = true
    var body: some View {
        Rectangle()
            .fill(CSColor.terracotta)
            .frame(width: 7, height: 15)
            .opacity(on ? 1 : 0)
            .onAppear { withAnimation(CSMotion.blink) { on = false } }
    }
}

/// Renders body text with `backtick` code spans coloured olive + mono.
struct CodeSpanText: View {
    let raw: String
    var size: CGFloat = 14
    var bodyColor: Color = CSColor.textBodyAlt

    var body: some View {
        segments
            .lineSpacing(5)             // ≈ line-height 1.6 on 14px
            .fixedSize(horizontal: false, vertical: true)
    }

    private var segments: Text {
        let parts = raw.components(separatedBy: "`")
        var out = Text("")
        for (i, part) in parts.enumerated() {
            if i.isMultiple(of: 2) {
                out = out + Text(part)
                    .font(CSFont.ui(size))
                    .foregroundColor(bodyColor)
            } else {
                out = out + Text(part)
                    .font(CSFont.mono(size - 1))
                    .foregroundColor(CSColor.oliveLight)
            }
        }
        return out
    }
}
