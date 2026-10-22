import AppKit

/// Geometry only. Every pill has its own compact window, so the desktop owns
/// the gaps between pills; there is no transparent window covering the column.
enum OverlayIntercomWidgetLayout {
  static let headerHeight: CGFloat = 30
  static let rowHeight: CGFloat = 44
  static let gap: CGFloat = 8
  static let previewSize = CGSize(width: 320, height: 188)

  static func height(count: Int) -> CGFloat {
    headerHeight + CGFloat(count) * (rowHeight + gap)
  }

  static func clampedHeader(_ frame: CGRect, count: Int, visibleFrame: CGRect) -> CGRect {
    var result = frame
    result.size.width = min(frame.width, visibleFrame.width)
    result.origin.x = min(max(frame.minX, visibleFrame.minX), visibleFrame.maxX - result.width)
    let totalHeight = min(height(count: count), visibleFrame.height)
    result.origin.y = min(
      max(frame.minY, visibleFrame.minY + totalHeight - headerHeight),
      visibleFrame.maxY - headerHeight)
    return result
  }

  static func rowFrame(index: Int, header: CGRect, count: Int, visibleFrame: CGRect) -> CGRect {
    // Nine owners fit a normal desktop. On a shorter screen, reduce only the
    // gaps, preserving the controls' readable geometry.
    let availableGap =
      count > 0
      ? (visibleFrame.height - headerHeight - CGFloat(count) * rowHeight) / CGFloat(count) : gap
    let spacing = min(gap, max(0, availableGap))
    return CGRect(
      x: header.minX,
      y: header.minY - CGFloat(index + 1) * (rowHeight + spacing),
      width: header.width, height: rowHeight)
  }

  static func previewFrame(beside row: CGRect, visibleFrame: CGRect) -> CGRect {
    let width = min(previewSize.width, visibleFrame.width)
    let height = min(previewSize.height, visibleFrame.height)
    let left = row.minX - width - gap
    let x = left >= visibleFrame.minX ? left : row.maxX + gap
    return CGRect(
      x: min(max(x, visibleFrame.minX), visibleFrame.maxX - width),
      y: min(max(row.maxY - height, visibleFrame.minY), visibleFrame.maxY - height),
      width: width, height: height)
  }
}
