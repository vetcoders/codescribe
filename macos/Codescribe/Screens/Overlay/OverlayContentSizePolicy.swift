import AppKit

/// Trailing transcript-action column. Transcription owns this width. It is not
/// the agents drawer preference, and a collapsed mini/midi hides the column.
///
/// The column fits one control, its insets and the panel's resize band. The
/// band stays inside the column's width and the controls end before it
/// starts, so every button remains fully clickable while the native edge
/// resize keeps the outer `OverlayResizeHit.band` points. There is no second
/// empty strip beyond the band: the control's own chrome is unscaled
/// (`TextScale` multiplies text only), so the width is the same at every
/// supported text scale.
enum OverlayTranscriptActionsLayout {
  /// One rail control: a 24 pt glyph in `OverlayHoverControl`'s 2 pt padding.
  static let controlHeight: CGFloat = 28
  /// Horizontal room around a control inside the sidebar material.
  static let controlInset: CGFloat = 4
  static let itemSpacing: CGFloat = 2
  static let groupSpacing: CGFloat = 8
  static let separatorHeight: CGFloat = 1
  static let contentVerticalPadding: CGFloat = 4
  /// Controls per group, top to bottom, matching the rail's fixed order:
  /// history/versions/undo/redo, previous take, the document commands, the
  /// engine commands and the delivery command.
  static let groupControlCounts = [4, 1, 4, 2, 1]
  /// Gap between the header and the column, and the column's bottom padding,
  /// as installed by `transcriptActionsColumn`.
  static let columnTopGap: CGFloat = 8
  static let columnBottomPadding: CGFloat = 12

  static var contentWidth: CGFloat { controlHeight + 2 * controlInset }

  static func columnWidth(textScale: CGFloat) -> CGFloat {
    contentWidth + OverlayResizeHit.band
  }

  /// Height of the whole fixed column: every control, the spacing inside and
  /// between its groups, its separators and its vertical padding. Nothing in
  /// it scales with the text size, so it holds at every supported scale.
  static var columnHeight: CGFloat {
    let controls = groupControlCounts.reduce(0, +)
    let innerSpacing = groupControlCounts.map { max(0, $0 - 1) }.reduce(0, +)
    let separators = groupControlCounts.count - 1
    let stackItems = groupControlCounts.count + separators
    return CGFloat(controls) * controlHeight
      + CGFloat(innerSpacing) * itemSpacing
      + CGFloat(separators) * separatorHeight
      + CGFloat(max(0, stackItems - 1)) * groupSpacing
      + 2 * contentVerticalPadding
  }

  /// The header the column hangs below: `OverlayHeaderDensity.rowHeight`
  /// plus its 10 pt of vertical padding on each side.
  static var headerHeight: CGFloat { OverlayHeaderDensity.rowHeight + 20 }

  /// Smallest expanded height in which the full action column is visible at
  /// once: the header clearance `transcriptActionsColumn` pays with its top
  /// padding (the header floats over the canvas), the whole column and its
  /// bottom padding. Anything less and the scroll view clips the last
  /// controls instead of showing them.
  static var minimumExpandedHeight: CGFloat {
    headerHeight + columnTopGap + columnHeight + columnBottomPadding
  }
}

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
    // Callers pass the window width. The actions column is inside that width
    // on the expanded transcription surface, so the measured lines are the
    // text column that remains.
    let textWidth = max(
      1,
      width - bodyHorizontalInsets
        - OverlayTranscriptActionsLayout.columnWidth(textScale: textScale))
    let measured = (text() as NSString).boundingRect(
      with: NSSize(
        width: textWidth,
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
