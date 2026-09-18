import AppKit

/// Content-to-window sizing policy. It measures projected text independently
/// of `NSHostingView.fittingSize`, preserving the window-owned sizing contract.
enum OverlayContentSizePolicy {
  static let maximumScreenFraction: CGFloat = 0.60
  private static let chromeHeight: CGFloat = 100
  private static let bodyHorizontalInsets: CGFloat = 40
  private static let bodyMinimumHeight: CGFloat = 130

  static func preferredHeight(
    for text: String,
    width: CGFloat,
    textScale: CGFloat,
    screen: NSScreen?
  ) -> CGFloat {
    let font = NSFont.systemFont(ofSize: 15 * textScale, weight: .medium)
    let paragraph = NSMutableParagraphStyle()
    paragraph.lineSpacing = 5
    let measured = (text as NSString).boundingRect(
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
    guard let visibleHeight = screen?.visibleFrame.height else { return desired }
    let maximum = max(
      DictationOverlayWindow.minSize.height,
      floor(visibleHeight * maximumScreenFraction)
    )
    return min(desired, maximum)
  }
}
