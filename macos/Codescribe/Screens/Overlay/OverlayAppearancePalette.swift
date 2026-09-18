import SwiftUI

/// Overlay-only contrast and material roles. They preserve the established
/// warm paper palette while avoiding the app-wide dark-only `CSColor` surface
/// tokens. The material remains the physical sheet; the 18% tint only steadies
/// it over unusually bright or dark desktop content.
struct OverlayAppearancePalette: Equatable, Sendable {
  let appearance: OverlayAppearance
  let desktopBackground: OverlayColorToken
  let surfaceTint: OverlayColorToken
  let border: OverlayColorToken
  let primaryText: OverlayColorToken
  let bodyText: OverlayColorToken
  let mutedText: OverlayColorToken
  let listeningStatus: OverlayColorToken
  let processingStatus: OverlayColorToken
  let successStatus: OverlayColorToken
  let neutralStatus: OverlayColorToken
  let errorStatus: OverlayColorToken
  let shadowOpacity: Double

  static let light = OverlayAppearancePalette(
    appearance: .light,
    desktopBackground: OverlayColorToken(0xF6F3EE),
    surfaceTint: OverlayColorToken(0xFFFFFF, alpha: 0.18),
    border: OverlayColorToken(0x5F5A52, alpha: 0.20),
    primaryText: OverlayColorToken(0x1C1B18),
    bodyText: OverlayColorToken(0x5F5A52),
    mutedText: OverlayColorToken(0x6E675F),
    listeningStatus: OverlayColorToken(0x9B4528),
    processingStatus: OverlayColorToken(0x8A5B00),
    successStatus: OverlayColorToken(0x4D5E2D),
    neutralStatus: OverlayColorToken(0x5F5A52),
    errorStatus: OverlayColorToken(0xA2302B),
    shadowOpacity: 0.16
  )

  static let dark = OverlayAppearancePalette(
    appearance: .dark,
    desktopBackground: OverlayColorToken(0x191919),
    surfaceTint: OverlayColorToken(0x202020, alpha: 0.18),
    border: OverlayColorToken(0xF3F0EA, alpha: 0.16),
    primaryText: OverlayColorToken(0xF3F0EA),
    bodyText: OverlayColorToken(0xC8C1B8),
    mutedText: OverlayColorToken(0x918A82),
    listeningStatus: OverlayColorToken(0xE08A64),
    processingStatus: OverlayColorToken(0xE2BE5B),
    successStatus: OverlayColorToken(0xB5C98D),
    neutralStatus: OverlayColorToken(0xA9A39B),
    errorStatus: OverlayColorToken(0xFFAAA5),
    shadowOpacity: 0.20
  )

  static func resolve(_ colorScheme: ColorScheme) -> OverlayAppearancePalette {
    resolve(OverlayAppearance(colorScheme: colorScheme))
  }

  static func resolve(_ appearance: OverlayAppearance) -> OverlayAppearancePalette {
    appearance == .dark ? .dark : .light
  }

  /// Refused coverage reuses the caution amber rather than gaining a token of
  /// its own. The choice is deliberate and it is a constraint, not a
  /// preference: every token here is held to a measured contrast floor by
  /// `OverlayAppearanceTests`, and a new pair of hex values authored under the
  /// compile embargo would be two unverified colours shipped on the strength
  /// of an agent's eye. Amber is already proven in both appearances and it is
  /// the honest hue for "settled, but not complete".
  ///
  /// What matters far more than which warm colour it is: it must never be
  /// `successStatus`. Green here would be the palette agreeing with a seal
  /// that was refused.
  func statusToken(for mode: OverlayMode) -> OverlayColorToken {
    switch mode {
    case .listening: listeningStatus
    case .finalizing: processingStatus
    case .coverageRefused: processingStatus
    case .formatted: successStatus
    case .noSpeech: neutralStatus
    case .error: errorStatus
    }
  }
}
