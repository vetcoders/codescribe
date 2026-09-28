import AppKit

/// Content-to-window sizing policy. It measures projected text independently
/// of `NSHostingView.fittingSize`, preserving the window-owned sizing contract.
enum OverlayContentSizePolicy {
  static let maximumScreenFraction: CGFloat = 0.60
  private static let chromeHeight: CGFloat = 100
  private static let bodyHorizontalInsets: CGFloat = 40
  private static let bodyMinimumHeight: CGFloat = 130

  static func preferredHeight(
    for text: @autoclosure () -> String,
    width: CGFloat,
    textScale: CGFloat,
    screen: NSScreen?,
    currentHeight: CGFloat = 0
  ) -> CGFloat {
    let maximum = screen.map {
      max(
        DictationOverlayWindow.minSize.height, floor($0.visibleFrame.height * maximumScreenFraction)
      )
    }
    // The panel only grows. Once capped, neither materialize nor lay out the
    // transcript on every projection; its native scroll view owns the overflow.
    if let maximum, currentHeight + 0.5 >= maximum { return maximum }
    let font = NSFont.systemFont(ofSize: 15 * textScale, weight: .medium)
    let paragraph = NSMutableParagraphStyle()
    paragraph.lineSpacing = 5
    let measured = (text() as NSString).boundingRect(
      with: NSSize(
        width: max(1, width - bodyHorizontalInsets),
        height: .greatestFiniteMagnitude
      ),
      options: [.usesLineFragmentOrigin, .usesFontLeading],
      attributes: [.font: font, .paragraphStyle: paragraph]
    )
    let desired = max(
      DictationOverlayWindow.minSize.height,
      chromeHeight + max(bodyMinimumHeight, ceil(measured.height))
    )
    guard let maximum else { return desired }
    return min(desired, maximum)
  }
}
