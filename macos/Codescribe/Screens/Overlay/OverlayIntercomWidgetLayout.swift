import AppKit

enum IntercomContentTier: String, Codable, CaseIterable, Sendable {
  case icon, name, actions

  var hoverPreviewEnabled: Bool { self == .icon || self == .name }
}

/// Measures the same fonts and slots painted by cardHeader. Resizing selects
/// content; only the text-scale preference changes these typography metrics.
struct IntercomContentMetrics: Equatable {
  let iconSide: CGFloat
  /// The longest name painted whole.
  let nameWidth: CGFloat
  /// The narrowest name row: the digit, marks and dot with a few letters and an
  /// ellipsis. Between this and `nameWidth` names truncate instead of snapping
  /// the whole group to icons (Maciej, 2026-10-23: the breakpoint locked out
  /// every width between the stacked cards and the circles).
  let minimumNameWidth: CGFloat
  let actionsWidth: CGFloat
  let nameFontSize: CGFloat
  let digitFontSize: CGFloat
  var stackedActionsWidth: CGFloat {
    max(
      minimumNameWidth,
      3 * OverlayIntercomWidgetLayout.controlHeight(scale: nameFontSize / 11) + 9)
  }

  init(names: [String], digits: [String], scale: CGFloat, hasDetached: Bool = false) {
    nameFontSize = 11 * scale
    digitFontSize = 10 * scale
    let font = NSFont.systemFont(ofSize: nameFontSize, weight: .semibold)
    let digitFont = CSFont.nsMono(digitFontSize)
    let name = names.map { ($0 as NSString).size(withAttributes: [.font: font]).width }.max() ?? 0
    let digit =
      digits.map { ($0 as NSString).size(withAttributes: [.font: digitFont]).width }.max() ?? 0
    let contentHeight = max(
      font.ascender - font.descender, digitFont.ascender - digitFont.descender)
    iconSide = ceil(
      max(
        OverlayIntercomWidgetLayout.controlHeight(scale: scale), contentHeight + 8,
        digit + 8 + 4 + 8))
    let windowMark: CGFloat = hasDetached ? 9 * scale + 5 : 0
    nameWidth = ceil(max(iconSide, name + digit + windowMark + 36 * scale + 45))
    let shortName = ("Abc…" as NSString).size(withAttributes: [.font: font]).width
    minimumNameWidth = min(
      nameWidth, ceil(max(iconSide, shortName + digit + windowMark + 36 * scale + 45)))
    // The focus, speaker and microphone are three row-height slots. Their
    // fixed gaps already belong to the name row's 45pt chrome budget.
    actionsWidth = nameWidth + 3 * OverlayIntercomWidgetLayout.controlHeight(scale: scale)
  }

  func tier(width: CGFloat, cardHeight: CGFloat) -> IntercomContentTier {
    if width >= actionsWidth || (width >= stackedActionsWidth && cardHeight >= 2 * iconSide) {
      return .actions
    }
    if width >= minimumNameWidth { return .name }
    return .icon
  }

  func fittedWidth(_ requested: CGFloat, available: CGFloat) -> CGFloat {
    let width = min(requested, available)
    return width < minimumNameWidth ? min(iconSide, available) : width
  }

  /// A card is as tall as its content: one row, or two when the controls sit
  /// under the name. Free heights only added an empty bottom (Maciej,
  /// 2026-10-23: generous upward "ale bez treści"), so the gesture picks the
  /// nearer of the two and a single row is always reachable.
  func fittedCardHeight(
    _ requested: CGFloat, width: CGFloat, count: Int, available: CGFloat
  ) -> CGFloat {
    guard count > 0 else { return iconSide }
    let maximum =
      (available - OverlayIntercomWidgetLayout.controlHeight(scale: nameFontSize / 11))
      / CGFloat(count) - OverlayIntercomWidgetLayout.gap
    let stacks = width >= stackedActionsWidth && width < actionsWidth
    return stacks && requested >= 1.5 * iconSide && maximum >= 2 * iconSide
      ? 2 * iconSide : iconSide
  }

  func resizedGroupFrame(
    _ frame: CGRect, delta: CGSize, edge: OverlayIntercomWidgetLayout.ResizeEdge,
    count: Int, visibleFrame: CGRect
  ) -> CGRect {
    var result = OverlayIntercomWidgetLayout.resizedFrame(
      frame, delta: delta, edge: edge, minimum: minimumGroupSize(count: count),
      visibleFrame: visibleFrame)
    let width = fittedWidth(result.width, available: visibleFrame.width)
    if edge != .bottomRight { result.origin.x = result.maxX - width }
    result.size.width = width
    return result
  }

  func minimumGroupSize(count: Int) -> CGSize {
    CGSize(
      width: iconSide,
      height: OverlayIntercomWidgetLayout.controlHeight(scale: nameFontSize / 11)
        + CGFloat(count) * (iconSide + OverlayIntercomWidgetLayout.gap))
  }
}

/// Geometry only. Each card owns a native window; desktop gaps never intercept clicks.
enum OverlayIntercomWidgetLayout {
  static let headerHeight: CGFloat = 30
  static let rowHeight: CGFloat = 30
  static let gap: CGFloat = 8

  static func controlHeight(scale: CGFloat) -> CGFloat { rowHeight * max(1, scale) }

  static func height(count: Int, scale: CGFloat = 1) -> CGFloat {
    controlHeight(scale: scale) * CGFloat(count + 1) + CGFloat(count) * gap
  }

  /// Full-size controls remain reachable on short displays by paging only the
  /// visible native capsules, rather than shrinking names or covering desktop gaps.
  static func visibleRange(
    count: Int, offset: Int, visibleHeight: CGFloat, scale: CGFloat
  ) -> Range<Int> {
    let row = controlHeight(scale: scale)
    let capacity = max(1, Int(max(0, visibleHeight - row) / (row + gap)))
    let shown = min(max(0, count), capacity)
    let start = min(max(0, offset), max(0, count - shown))
    return start..<(start + shown)
  }

  /// Agent text is the measured digit, name and optional detached-window mark.
  /// The row adds reception frames (28 scaled + 3 fixed), the 8pt status dot, three 6pt
  /// identity gaps, 12pt leading padding, a focus slot, two audio slots with
  /// their 6pt gap, and two 3pt outer gaps. These match cardHeader exactly.
  static func restingWidth(headerText: CGFloat, agentText: CGFloat, scale: CGFloat) -> CGFloat {
    let unit = max(1, scale)
    return max(headerText + 70 * unit + 68, agentText + 36 * scale + 90 * unit + 45)
  }

  /// The detached mini header has the same three control slots plus its
  /// fixed close slot, six outer 3pt gaps and 12 + 4pt horizontal padding.
  static func minimizedInstanceWidth(text: CGFloat, scale: CGFloat) -> CGFloat {
    text + 36 * scale + 116 * max(1, scale) + 43
  }

  static func clampedHeader(
    _ frame: CGRect, count: Int, visibleFrame: CGRect, scale: CGFloat = 1,
    cardHeight: CGFloat? = nil
  ) -> CGRect {
    var result = frame
    result.size.width = min(frame.width, visibleFrame.width)
    result.size.height = min(controlHeight(scale: scale), visibleFrame.height)
    result.origin.x = min(max(frame.minX, visibleFrame.minX), visibleFrame.maxX - result.width)
    let totalHeight = min(
      result.height + CGFloat(count) * ((cardHeight ?? controlHeight(scale: scale)) + gap),
      visibleFrame.height)
    result.origin.y = min(
      max(frame.minY, visibleFrame.minY + totalHeight - result.height),
      visibleFrame.maxY - result.height)
    return result
  }

  static func rowFrame(
    index: Int, header: CGRect, count: Int, visibleFrame: CGRect, scale: CGFloat = 1,
    cardHeight: CGFloat? = nil
  ) -> CGRect {
    let row = cardHeight ?? controlHeight(scale: scale)
    let availableGap =
      count > 0
      ? (visibleFrame.height - header.height - CGFloat(count) * row) / CGFloat(count) : gap
    let spacing = min(gap, max(0, availableGap))
    return CGRect(
      x: header.minX, y: header.minY - CGFloat(index + 1) * (row + spacing),
      width: header.width, height: row)
  }

  /// The right edge stays anchored. Hover widens only this card, never its neighbours.
  static func previewFrame(
    beside row: CGRect, visibleFrame: CGRect, scale: CGFloat = 1
  ) -> CGRect {
    cardFrame(
      below: row, size: CGSize(width: max(row.width, 360 * max(1, scale)), height: row.height),
      visibleFrame: visibleFrame)
  }

  static func conversationSize(scale: CGFloat) -> CGSize {
    CGSize(width: 380 * max(1, scale), height: 360 * max(1, scale))
  }

  static func minimumConversationSize(scale: CGFloat) -> CGSize {
    CGSize(width: 320 * max(1, scale), height: 240 * max(1, scale))
  }

  static func conversationFrame(
    below row: CGRect, size: CGSize, visibleFrame: CGRect, scale: CGFloat
  ) -> CGRect {
    let minimum = minimumConversationSize(scale: scale)
    return cardFrame(
      below: row,
      size: CGSize(width: max(size.width, minimum.width), height: max(size.height, minimum.height)),
      visibleFrame: visibleFrame)
  }

  static func cardFrame(below row: CGRect, size: CGSize, visibleFrame: CGRect) -> CGRect {
    let width = min(size.width, visibleFrame.width)
    let height = min(size.height, visibleFrame.height)
    return CGRect(
      x: min(max(row.maxX - width, visibleFrame.minX), visibleFrame.maxX - width),
      y: min(max(row.maxY - height, visibleFrame.minY), visibleFrame.maxY - height),
      width: width, height: height)
  }

  enum ResizeEdge: Equatable { case left, bottomLeft, bottomRight }

  static func resizedFrame(
    _ frame: CGRect, delta: CGSize, edge: ResizeEdge, minimum: CGSize, visibleFrame: CGRect
  ) -> CGRect {
    let minWidth = min(minimum.width, visibleFrame.width)
    let minHeight = min(minimum.height, visibleFrame.height)
    var result = frame
    switch edge {
    case .left, .bottomLeft:
      result.origin.x = min(max(frame.minX + delta.width, visibleFrame.minX), frame.maxX - minWidth)
      result.size.width = frame.maxX - result.minX
    case .bottomRight:
      result.size.width = min(
        max(frame.width + delta.width, minWidth), visibleFrame.maxX - frame.minX)
    }
    if edge != .left {
      result.origin.y = min(
        max(frame.minY + delta.height, visibleFrame.minY), frame.maxY - minHeight)
      result.size.height = frame.maxY - result.minY
    }
    return result
  }
}

/// Exact conversation identity, including the provider/session/lease owner. A
/// digit-only placeholder can paint a roster entry but cannot open a composer.
struct OverlayIntercomCardIdentity: Hashable {
  let digit: String
  let conversationID: String?
  let ownerID: String?
  let name: String

  init(digit: String, conversation: OverlayConversation?, name: String? = nil) {
    self.digit = digit
    conversationID = conversation?.id
    ownerID = conversation?.owner?.id
    self.name = name ?? conversation.map { OverlayChannelMapChip.displayName($0.name) } ?? digit
  }

  static func == (lhs: Self, rhs: Self) -> Bool {
    lhs.digit == rhs.digit && lhs.conversationID == rhs.conversationID && lhs.ownerID == rhs.ownerID
  }

  func hash(into hasher: inout Hasher) {
    hasher.combine(digit)
    hasher.combine(conversationID)
    hasher.combine(ownerID)
  }

  func matches(_ conversation: OverlayConversation) -> Bool {
    conversationID == conversation.id && ownerID == conversation.owner?.id
      && digit == conversation.channel
  }
}

/// Hover is independent of expanded conversations. Neither gesture selects a
/// capture destination; all durable drafts remain in OverlayState.
struct OverlayIntercomPresentation: Equatable {
  /// A visible card already presents its own draft capture. A main-overlay
  /// composer, or a draft with no visible card, keeps the existing show path.
  static func shouldShowMainOverlayForDraftCapture(
    verticalIntercomActive: Bool, mainPanelVisible: Bool, conversationPresentedInIntercom: Bool
  ) -> Bool {
    !verticalIntercomActive || mainPanelVisible || !conversationPresentedInIntercom
  }

  var hovered: OverlayIntercomCardIdentity?
  var expanded: Set<OverlayIntercomCardIdentity> = []
  var folded = false

  mutating func enter(_ identity: OverlayIntercomCardIdentity) {
    hovered = expanded.contains(identity) ? nil : identity
  }

  mutating func leave(_ identity: OverlayIntercomCardIdentity) {
    if hovered == identity { hovered = nil }
  }

  /// One conversation opens inside the group at a time. A second place for
  /// the same or another agent is a durable detached instance, not a second
  /// group expansion.
  mutating func expand(_ identity: OverlayIntercomCardIdentity) {
    guard identity.conversationID != nil, identity.ownerID != nil else { return }
    expanded = [identity]
    hovered = nil
  }

  mutating func close(_ identity: OverlayIntercomCardIdentity) {
    expanded.remove(identity)
    leave(identity)
  }
}
