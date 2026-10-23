import AppKit

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

  /// Content measurements include only the text; the rest mirrors the drawn
  /// controls. The header reserves its channel-0 audio slot (controlHeight)
  /// plus the HStack gap it introduced (Maciej, 2026-10-23: the first row is
  /// the broadcast's microphone), so a short roster never compresses the row.
  static func restingWidth(headerText: CGFloat, agentText: CGFloat, scale: CGFloat) -> CGFloat {
    let unit = max(1, scale)
    return max(headerText + 56 * unit + 68, agentText + 30 * unit + 47)
  }

  static func clampedHeader(
    _ frame: CGRect, count: Int, visibleFrame: CGRect, scale: CGFloat = 1
  ) -> CGRect {
    var result = frame
    result.size.width = min(frame.width, visibleFrame.width)
    result.size.height = min(controlHeight(scale: scale), visibleFrame.height)
    result.origin.x = min(max(frame.minX, visibleFrame.minX), visibleFrame.maxX - result.width)
    let totalHeight = min(height(count: count, scale: scale), visibleFrame.height)
    result.origin.y = min(
      max(frame.minY, visibleFrame.minY + totalHeight - result.height),
      visibleFrame.maxY - result.height)
    return result
  }

  static func rowFrame(
    index: Int, header: CGRect, count: Int, visibleFrame: CGRect, scale: CGFloat = 1
  ) -> CGRect {
    let row = controlHeight(scale: scale)
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
