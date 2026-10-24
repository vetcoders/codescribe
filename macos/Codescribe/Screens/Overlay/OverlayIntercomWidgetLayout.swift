import AppKit

enum IntercomContentTier: String, Codable, CaseIterable, Sendable {
  case icon, name, actions
}

/// Content revealed inside the existing group card, independently of whether
/// its full conversation window has been opened. These are presentation gates;
/// drafts, messages and commands remain owned by the shared conversation.
enum IntercomCardContent: Equatable, Sendable {
  case header, context, preview, composer
}

/// What a one-line row shows beside its digit, dot and name, counted from its
/// right edge as on the agents list (Maciej, 2026-10-23 seal: in a narrow row
/// the microphone comes first, then the speaker that mutes an agent talking
/// to us; the antenna and moon stay, "ale dopiero po poszerzaniu"; and later:
/// "od prawej jak w oryginale … może być na jednym miejscu z mikrofonem, ale
/// niech się mieści").
struct IntercomRowControls: Equatable {
  /// 0: none. 1: one place the microphone and speaker share. 2: the speaker
  /// and the microphone. 3: the panel focus too.
  let slots: Int
  let marks: Bool

  static let all = IntercomRowControls(slots: 3, marks: true)
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

  init(names: [String], digits: [String], scale: CGFloat) {
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
    nameWidth = ceil(max(iconSide, name + digit + 36 * scale + 45))
    let shortName = ("Abc…" as NSString).size(withAttributes: [.font: font]).width
    minimumNameWidth = min(nameWidth, ceil(max(iconSide, shortName + digit + 36 * scale + 45)))
    // The focus, speaker and microphone are three row-height slots. Their
    // fixed gaps already belong to the name row's 45pt chrome budget.
    actionsWidth = nameWidth + 3 * OverlayIntercomWidgetLayout.controlHeight(scale: scale)
  }

  /// The antenna and moon: two 14 pt marks 3 pt apart, plus the row's 6 pt
  /// spacing. The name budgets above include them.
  var marksWidth: CGFloat { ceil(31 * nameFontSize / 11 + 6) }

  /// The name truncates before a control drops; the marks drop first and
  /// return only when the whole name, the marks and all three controls fit.
  func rowControls(width: CGFloat) -> IntercomRowControls {
    if width >= actionsWidth { return .all }
    let slot = OverlayIntercomWidgetLayout.controlHeight(scale: nameFontSize / 11)
    let free = width - (minimumNameWidth - marksWidth)
    return IntercomRowControls(
      slots: min(3, max(0, Int((free / slot).rounded(.down)))), marks: false)
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

  func cardContent(width: CGFloat, cardHeight: CGFloat) -> IntercomCardContent {
    let scale = max(1, nameFontSize / 11)
    guard width >= minimumNameWidth else { return .header }
    if width >= 320 * scale && cardHeight >= 240 * scale { return .composer }
    if width >= 220 * scale && cardHeight >= 140 * scale { return .preview }
    let header = OverlayIntercomWidgetLayout.controlHeight(scale: scale)
    if cardHeight >= header + 24 * scale { return .context }
    return .header
  }

  /// A card stretches with the gesture, from one row up to the space below the
  /// header, revealing context, messages and then the composer (Maciej,
  /// 2026-10-23: the stretched tiles of Codex's tiers "to było w sumie fajne",
  /// while the one-or-two-row snap that replaced them "nie robi nic"). Stacked
  /// controls appear once the card holds two rows.
  func fittedCardHeight(_ requested: CGFloat, count: Int, available: CGFloat) -> CGFloat {
    guard count > 0 else { return iconSide }
    // The group can extend below the display. Dividing by the roster count
    // prevented a 240 pt composer with four cards on a 900 pt screen, even
    // after an explicit stretch. Bound each card, keeping room for the header.
    let maximum =
      available - OverlayIntercomWidgetLayout.controlHeight(scale: nameFontSize / 11)
      - OverlayIntercomWidgetLayout.gap
    return max(iconSide, min(requested, maximum))
  }

  func resizedGroupFrame(
    _ frame: CGRect, delta: CGSize, edge: OverlayIntercomWidgetLayout.ResizeEdge,
    count: Int, visibleFrame: CGRect, minimumWidth: CGFloat = 0
  ) -> CGRect {
    var minimum = minimumGroupSize(count: count)
    minimum.width = max(minimum.width, minimumWidth)
    let maximumHeight =
      OverlayIntercomWidgetLayout.controlHeight(scale: nameFontSize / 11)
      + CGFloat(max(0, count))
      * (fittedCardHeight(.greatestFiniteMagnitude, count: count, available: visibleFrame.height)
        + OverlayIntercomWidgetLayout.gap)
    let bounds = CGRect(
      x: visibleFrame.minX, y: visibleFrame.maxY - max(visibleFrame.height, maximumHeight),
      width: visibleFrame.width, height: max(visibleFrame.height, maximumHeight))
    var result = OverlayIntercomWidgetLayout.resizedFrame(
      frame, delta: delta, edge: edge, minimum: minimum,
      visibleFrame: bounds)
    let width = max(
      min(minimumWidth, visibleFrame.width),
      fittedWidth(result.width, available: visibleFrame.width))
    if !edge.anchorsLeft { result.origin.x = result.maxX - width }
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
  /// Tight enough that the stack reads as one list, not a column of buttons.
  static let gap: CGFloat = 4

  static func controlHeight(scale: CGFloat) -> CGFloat { rowHeight * max(1, scale) }

  /// One corner for the header, every row and a mini fiszka. A radius of half
  /// the row turned a two-line card into a stadium (Maciej, 2026-10-23:
  /// "przaśnie", then "strasznie dupne"); the open conversation keeps 16.
  static func rowCornerRadius(scale: CGFloat) -> CGFloat { 10 * max(1, scale) }
  static let conversationCornerRadius: CGFloat = 16

  static func height(count: Int, scale: CGFloat = 1) -> CGFloat {
    controlHeight(scale: scale) * CGFloat(count + 1) + CGFloat(count) * gap
  }

  /// Full-size controls remain reachable on short displays by paging only the
  /// visible native capsules, rather than shrinking names or covering desktop gaps.
  static func visibleRange(
    count: Int, offset: Int, visibleHeight: CGFloat, scale: CGFloat,
    cardHeight: CGFloat? = nil
  ) -> Range<Int> {
    let header = controlHeight(scale: scale)
    let row = cardHeight ?? header
    let capacity = max(1, Int(max(0, visibleHeight - header) / (max(header, row) + gap)))
    let shown = min(max(0, count), capacity)
    let start = min(max(0, offset), max(0, count - shown))
    return start..<(start + shown)
  }

  /// Agent text is the measured digit and name.
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
    _ frame: CGRect, count _: Int, visibleFrame: CGRect, scale: CGFloat = 1,
    cardHeight _: CGFloat? = nil
  ) -> CGRect {
    var result = frame
    result.size.width = min(frame.width, visibleFrame.width)
    result.size.height = min(controlHeight(scale: scale), visibleFrame.height)
    return movedFrame(result, visibleFrame: visibleFrame, headerHeight: result.height)
  }

  /// Moving keeps the grab strip reachable, not the body below it. The same
  /// rule owns the group header and a detached conversation's title row.
  /// Initial placement and resizing retain their separate size constraints.
  static func movedFrame(
    _ frame: CGRect, visibleFrame: CGRect, headerHeight: CGFloat
  ) -> CGRect {
    var result = frame
    let gripHeight = min(max(0, headerHeight), frame.height, visibleFrame.height)
    result.origin.x = min(
      max(frame.minX, visibleFrame.minX), max(visibleFrame.minX, visibleFrame.maxX - frame.width))
    result.origin.y = min(
      max(frame.minY, visibleFrame.minY + gripHeight - frame.height),
      visibleFrame.maxY - frame.height)
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

  enum ResizeEdge: Equatable {
    case left, right, top, bottom, bottomLeft, bottomRight

    /// The left edge stays put while this edge moves.
    var anchorsLeft: Bool { self == .right || self == .bottomRight }
    /// The edge changes the height, not only the width.
    var resizesHeight: Bool { self != .left && self != .right }
  }

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
    case .right, .bottomRight:
      result.size.width = min(
        max(frame.width + delta.width, minWidth), visibleFrame.maxX - frame.minX)
    case .top, .bottom:
      break
    }
    switch edge {
    case .bottom, .bottomLeft, .bottomRight:
      result.origin.y = min(
        max(frame.minY + delta.height, visibleFrame.minY), frame.maxY - minHeight)
      result.size.height = frame.maxY - result.minY
    case .top:
      result.size.height = min(
        max(frame.height + delta.height, minHeight), visibleFrame.maxY - frame.minY)
    case .left, .right:
      break
    }
    return result
  }

  /// How far the whole group moves per point a row's edge is dragged, so the
  /// edge under the pointer follows it: dragging the bottom of row `index`
  /// stretches it and every row above it, dragging its top stretches it and
  /// every row below while the group's bottom stays put. Side edges and the
  /// header's edges move one to one.
  static func rowEdgeFactor(edge: ResizeEdge, index: Int?, count: Int) -> CGFloat {
    guard let index, count > 0, (0..<count).contains(index) else { return 1 }
    switch edge {
    case .bottom, .bottomLeft, .bottomRight: return CGFloat(count) / CGFloat(index + 1)
    case .top: return CGFloat(count) / CGFloat(count - index)
    case .left, .right: return 1
    }
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

  var isHumanInbox: Bool {
    digit == "10" && conversationID == "10" && ownerID == nil
  }
  var displayDigit: String { isHumanInbox ? "" : digit }

  /// Only the two canonical mailboxes may present without an agent owner.
  /// A known digit with a fabricated conversation ID is still a placeholder.
  var canExpand: Bool {
    guard conversationID != nil else { return false }
    return ownerID != nil || isHumanInbox || (digit == "0" && conversationID == "0")
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

/// Which conversation the group opened and whether it is folded. Hover moves
/// no window: a row that grew under the pointer jutted out of the column and
/// danced on its own edge; the preview is the click (Maciej, 2026-10-22
/// 14:27Z, as Olga and the journal relay it: "podgląd dopiero na klik").
/// Opening selects no capture destination; durable drafts stay in OverlayState.
struct OverlayIntercomPresentation: Equatable {
  /// A visible card already presents its own draft capture. A main-overlay
  /// composer, or a draft with no visible card, keeps the existing show path.
  static func shouldShowMainOverlayForDraftCapture(
    verticalIntercomActive: Bool, mainPanelVisible: Bool, conversationPresentedInIntercom: Bool
  ) -> Bool {
    !verticalIntercomActive || mainPanelVisible || !conversationPresentedInIntercom
  }

  var expanded: Set<OverlayIntercomCardIdentity> = []
  var folded = false

  func frontOrder(_ identities: [OverlayIntercomCardIdentity]) -> [OverlayIntercomCardIdentity] {
    identities.filter { !expanded.contains($0) } + identities.filter { expanded.contains($0) }
  }

  /// One conversation opens inside the group at a time. A second place for
  /// the same or another agent is a durable detached instance, not a second
  /// group expansion.
  mutating func expand(_ identity: OverlayIntercomCardIdentity) {
    guard identity.canExpand else { return }
    expanded = [identity]
  }

  mutating func close(_ identity: OverlayIntercomCardIdentity) {
    expanded.remove(identity)
  }
}
