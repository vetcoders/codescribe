import AppKit
import SwiftUI

// Typography — system UI + JetBrains Mono (mono/eyebrows/code).
// Mono is used ONLY for time, versions, keyboard shortcuts, code/logs and
// eyebrows. Buttons, statuses and sentences speak in the system face, and
// SF Mono is never used (Founder, 2026-10-18).

enum CSFont {
  // Display / UI — native system face
  static func ui(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
    Font.system(size: size, weight: weight)
  }
  // Code / eyebrows / meta — JetBrains Mono
  static func mono(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
    Font.custom(FontLoader.jetBrainsMono, size: size).weight(weight)
  }
  // AppKit twin of `mono` for NSTextView-backed surfaces (oversized bubble
  // payloads); same face, graceful system-mono fallback.
  static func nsMono(_ size: CGFloat) -> NSFont {
    NSFont(name: FontLoader.jetBrainsMono, size: size)
      ?? .monospacedSystemFont(ofSize: size, weight: .regular)
  }

  // Named ramps from the handoff
  // -.03/-.04em tracking applied at call site
  static func hero(_ size: CGFloat = 64) -> Font { ui(size, .bold) }
  static let title = ui(15, .bold)
  static let bodyLg = ui(18, .regular)
  static let body = ui(14, .regular)
  static let eyebrow = mono(11, .semibold)  // tracking .18–.24em at call site
  static let metaMono = mono(11, .medium)
}

// Eyebrow label: mono, uppercase, wide tracking, olive — the section marker.
// A caller that speaks in one family (the setup wizard, Founder 2026-10-26)
// passes the system face; the default stays the mono eyebrow.
struct EyebrowLabel: View {
  let text: String
  var color: Color = CSColor.eyebrowOlive
  var font: Font = CSFont.eyebrow
  var body: some View {
    Text(text)
      .textCase(.uppercase)
      .font(font)
      .tracking(2.2)
      .foregroundStyle(color)
  }
}
