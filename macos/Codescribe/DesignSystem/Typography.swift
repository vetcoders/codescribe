import SwiftUI

// Typography — Space Grotesk (display/UI) + JetBrains Mono (mono/eyebrows/code).
// Mono is used ONLY for eyebrows, meta, code, logs — never as the page voice.

enum CSFont {
    // Display / UI — Space Grotesk
    static func ui(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        Font.custom(FontLoader.spaceGrotesk, size: size).weight(weight)
    }
    // Code / eyebrows / meta — JetBrains Mono
    static func mono(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        Font.custom(FontLoader.jetBrainsMono, size: size).weight(weight)
    }

    // Named ramps from the handoff
    static func hero(_ size: CGFloat = 64) -> Font { ui(size, .bold) }        // -.03/-.04em tracking applied at call site
    static let h2 = ui(26, .bold)
    static let title = ui(15, .bold)
    static let bodyLg = ui(18, .regular)
    static let body = ui(14, .regular)
    static let bodyStrong = ui(13, .semibold)
    static let eyebrow = mono(11, .semibold)                                  // tracking .18–.24em at call site
    static let metaMono = mono(11, .medium)
    static let tagMono = mono(10, .semibold)
}

// Eyebrow label: mono, uppercase, wide tracking, olive — the section marker.
struct EyebrowLabel: View {
    let text: String
    var color: Color = CSColor.eyebrowOlive
    var body: some View {
        Text(text.uppercased())
            .font(CSFont.eyebrow)
            .tracking(2.2)
            .foregroundStyle(color)
    }
}
