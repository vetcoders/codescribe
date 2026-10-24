import SwiftUI

/// The overlay's view of `CSPalette`: every role resolved to one opaque value
/// per appearance. `CSPalette` owns the values — none is restated here — and
/// the projection exists only because the sheet composites text over glass and
/// proves its contrast from exact sRGB components, which a dynamic `NSColor`
/// cannot answer. Native glass owns translucency; desktopBackground is the
/// opaque surface used when Reduce Transparency is enabled.
struct OverlayAppearancePalette: Equatable, Sendable {
  let appearance: OverlayAppearance
  let desktopBackground: OverlayColorToken
  let border: OverlayColorToken
  let primaryText: OverlayColorToken
  let bodyText: OverlayColorToken
  let mutedText: OverlayColorToken
  let listeningStatus: OverlayColorToken
  let processingStatus: OverlayColorToken
  let successStatus: OverlayColorToken
  let errorStatus: OverlayColorToken
  /// The assistive violet (voice routed to an agent), raised for contrast on
  /// either canvas; the Agents control's hover hue.
  let agentAccent: OverlayColorToken
  /// A6 uncertain-word text: the processing orange at label strength. Distinct
  /// from `processingStatus`, which wears the amber of engine completeness
  /// warnings; this role means "the engine was unsure of THIS word".
  let uncertainWord: OverlayColorToken
  /// Lexicon marker for `surface_rewritten` words (d5): the dictionary owns
  /// this surface, so it gets the steel blue of `CSPalette.lexicon`, never the
  /// uncertainty orange.
  let lexiconMarker: OverlayColorToken
  let shadowOpacity: Double

  static var light: OverlayAppearancePalette { OverlayAppearancePalette(.light) }
  static var dark: OverlayAppearancePalette { OverlayAppearancePalette(.dark) }

  private init(_ appearance: OverlayAppearance) {
    self.appearance = appearance
    desktopBackground = OverlayColorToken(CSPalette.sheet, in: appearance)
    border = OverlayColorToken(CSPalette.panelEdge, in: appearance)
    primaryText = OverlayColorToken(CSPalette.textHigh, in: appearance)
    bodyText = OverlayColorToken(CSPalette.textBodyAlt, in: appearance)
    mutedText = OverlayColorToken(CSPalette.textMuted, in: appearance)
    listeningStatus = OverlayColorToken(CSPalette.terracottaLight, in: appearance)
    processingStatus = OverlayColorToken(CSPalette.amberLight, in: appearance)
    successStatus = OverlayColorToken(CSPalette.oliveStatus, in: appearance)
    errorStatus = OverlayColorToken(CSPalette.dangerLight, in: appearance)
    agentAccent = OverlayColorToken(CSPalette.assistiveLight, in: appearance)
    uncertainWord = OverlayColorToken(CSPalette.modeProcessingLight, in: appearance)
    lexiconMarker = OverlayColorToken(CSPalette.lexicon, in: appearance)
    shadowOpacity = appearance == .dark ? 0.20 : 0.16
  }

  static func resolve(_ colorScheme: ColorScheme) -> OverlayAppearancePalette {
    resolve(OverlayAppearance(colorScheme: colorScheme))
  }

  static func resolve(_ appearance: OverlayAppearance) -> OverlayAppearancePalette {
    appearance == .dark ? .dark : .light
  }
}
