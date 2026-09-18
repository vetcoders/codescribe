import SwiftUI

/// Explicit appearance input for the floating sheet. Keeping the choice as
/// plain data makes a live SwiftUI `colorScheme` change repaint every token and
/// keeps the contrast proof independent from AppKit vibrancy.
enum OverlayAppearance: Equatable, Sendable {
  case light
  case dark

  init(colorScheme: ColorScheme) {
    self = colorScheme == .dark ? .dark : .light
  }
}
