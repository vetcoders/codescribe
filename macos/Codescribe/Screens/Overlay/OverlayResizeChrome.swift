import SwiftUI

/// Chrome geometry uses AppKit's bottom-left coordinates. The grip lives in
/// the existing resize band; Actions sits entirely above that band's hit area.
enum OverlayResizeChrome {
  static let gripSize = CGSize(width: 38, height: 4)
  static let gripBottomInset: CGFloat = 6
  static let actionsBottomInset = OverlayResizeHit.band + 2
  static let actionsHeight: CGFloat = 24

  static func actionsWidth(narrow: Bool) -> CGFloat { narrow ? 34 : 88 }

  static func gripRect(in bounds: CGRect) -> CGRect {
    CGRect(
      x: bounds.midX - gripSize.width / 2, y: bounds.minY + gripBottomInset,
      width: gripSize.width, height: gripSize.height)
  }

  static func actionsRect(in bounds: CGRect, hovering: Bool = false) -> CGRect {
    let width = actionsWidth(narrow: !hovering)
    return CGRect(
      x: bounds.midX - width / 2, y: bounds.minY + actionsBottomInset,
      width: width, height: actionsHeight)
  }

  static func sideIndicatorOpacity(pointerInside: Bool) -> Double {
    pointerInside ? 0.35 : 0
  }

  static func sideIndicatorAnimation(reduceMotion: Bool) -> Animation? {
    reduceMotion ? nil : .easeOut(duration: 0.15)
  }
}
