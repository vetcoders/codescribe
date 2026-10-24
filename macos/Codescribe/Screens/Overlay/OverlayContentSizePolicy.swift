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
  /// The palette shows seven controls at once; its remaining commands scroll.
  static let visibleSlotBudget = 7
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

  /// The whole command inventory remains mounted, including disabled slots.
  static var columnHeight: CGFloat {
    height(forFirst: groupControlCounts.reduce(0, +))
  }

  /// Height of the first seven slots, respecting the same group boundaries
  /// as the full stack. End at the seventh control so the next one cannot
  /// peek through the full stack's bottom padding.
  static var viewportHeight: CGFloat {
    height(forFirst: visibleSlotBudget) - contentVerticalPadding
  }

  private static func height(forFirst slotCount: Int) -> CGFloat {
    var remaining = max(0, slotCount)
    var height = 2 * contentVerticalPadding
    for (index, count) in groupControlCounts.enumerated() {
      let visible = min(remaining, count)
      guard visible > 0 else { break }
      if index > 0 { height += 2 * groupSpacing + separatorHeight }
      height += CGFloat(visible) * controlHeight + CGFloat(visible - 1) * itemSpacing
      remaining -= visible
    }
    return height
  }

  /// The header the column hangs below: `OverlayHeaderDensity.rowHeight`
  /// plus its 10 pt of vertical padding on each side.
  static var headerHeight: CGFloat { OverlayHeaderDensity.rowHeight + 20 }

  /// The expanded floor pays for seven visible slots, not the full inventory.
  /// The scroll view keeps every command available below that viewport.
  static var minimumExpandedHeight: CGFloat {
    headerHeight + columnTopGap + viewportHeight + columnBottomPadding
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
