import CoreGraphics
import Foundation

/// The Agent channel chord's modifier in effect (`AGENT_CHANNEL_MODIFIER`
/// after the core's one-key-one-role rule). The channel map names it in every
/// shortcut hint, so a hint never claims Ctrl while the chord is Fn, and names
/// no chord at all when both keys record dictation (`"off"`).
enum OverlayChannelChordModifier: Equatable, Sendable {
  case control
  case function
  /// Both keys record dictation: no digit chord opens a channel.
  case unbound

  /// `"fn"` is Fn, `"off"` is no chord, every other token, Command included,
  /// is Ctrl (`HotkeysEngine.normalizeChannelModifier`).
  init(setting: String) {
    switch setting.lowercased() {
    case "fn": self = .function
    case "off": self = .unbound
    default: self = .control
    }
  }

  /// The key's name as the Settings picker prints it, verbatim in every
  /// language there as well; `nil` without a chord.
  var keyName: String? {
    switch self {
    case .control: "Ctrl"
    case .function: "Fn"
    case .unbound: nil
    }
  }

  /// The written chord in a hint: `Ctrl+5`; `nil` without a chord.
  func shortcut(digit: String) -> String? { keyName.map { $0 + "+" + digit } }

  /// The chord as VoiceOver reads it: `Ctrl 5`; `nil` without a chord.
  func spokenShortcut(digit: String) -> String? { keyName.map { $0 + " " + digit } }
}

/// One channel of the collapsed overlay's map: a bound digit and its agent,
/// or channel 0 while the microphone broadcasts on it. Display values only;
/// the conversation and its exact owner are resolved by the roster when the
/// chip is clicked.
struct OverlayChannelMapChip: Equatable, Identifiable, Sendable {
  let digit: String
  /// The agent's name as the map shows it: the lease name with a capital
  /// first letter (bus intercom spec §13.5). Channel 0 has none.
  let name: String
  /// The follower behind the channel is gone: the chip dims like the
  /// roster's Disconnected row. The digit stays, because the chord still
  /// reaches the channel.
  let disconnected: Bool
  /// The microphone feeds this channel now: the digit carries the badge.
  let microphoneOpen: Bool
  let unread: Bool
  /// A current conversation exists for the digit, so a click can select it.
  let selectable: Bool
  /// The selected conversation is on this channel: the circle wears the
  /// muted violet fill of the viewed conversation.
  let selected: Bool

  var id: String { digit }
  var isBroadcast: Bool { digit == "0" }

  /// The floating hint: `5 · Kuba · Ctrl+5`, or `0 · Ctrl+0`; without a
  /// chord `5 · Kuba` or `0`.
  func hint(modifier: OverlayChannelChordModifier) -> String {
    var parts = isBroadcast ? [digit] : [digit, name]
    if let shortcut = modifier.shortcut(digit: digit) { parts.append(shortcut) }
    return parts.joined(separator: " · ")
  }

  func accessibilityLabel(modifier: OverlayChannelChordModifier) -> String {
    guard let shortcut = modifier.spokenShortcut(digit: digit) else {
      return accessibilityLabelWithoutShortcut
    }
    // Channel 0 is on the map only while the microphone broadcasts on it.
    if isBroadcast {
      return String(
        localized: "Channel \(digit), shortcut \(shortcut), microphone open",
        comment:
          "Collapsed overlay channel 0 chip while broadcasting: digit, the channel chord (e.g. Ctrl 0)"
      )
    }
    return microphoneOpen
      ? String(
        localized: "Channel \(digit), \(name), shortcut \(shortcut), microphone open",
        comment:
          "Compact overlay channel chip: digit, agent name, the channel chord (e.g. Ctrl 5); the microphone feeds this channel"
      )
      : String(
        localized: "Channel \(digit), \(name), shortcut \(shortcut)",
        comment: "Compact overlay channel chip: digit, agent name, the channel chord (e.g. Ctrl 5)")
  }

  /// VoiceOver label while both keys record dictation and no chord exists.
  private var accessibilityLabelWithoutShortcut: String {
    if isBroadcast {
      return String(
        localized: "Channel \(digit), microphone open",
        comment:
          "Collapsed overlay channel 0 chip while broadcasting, when no key plus digit opens a channel"
      )
    }
    return microphoneOpen
      ? String(
        localized: "Channel \(digit), \(name), microphone open",
        comment:
          "Compact overlay channel chip when no key plus digit opens a channel: digit, agent name; the microphone feeds this channel"
      )
      : String(
        localized: "Channel \(digit), \(name)",
        comment:
          "Compact overlay channel chip when no key plus digit opens a channel: digit, agent name")
  }

  /// A lease name as the map prints it: the first grapheme upper-cased for
  /// the current locale (`igor` → `Igor`). Leases, bindings and identifiers
  /// keep the name as the bus has it.
  static func displayName(_ name: String) -> String {
    guard let first = name.first else { return name }
    return String(first).uppercased(with: .current) + String(name.dropFirst())
  }
}

/// The collapsed overlay's channel map (bus intercom spec §5.1, §6, §13): a
/// projection of the same roster snapshot the Agents canvas reads. It holds
/// no state, polls nothing and never names a session; a click resolves the
/// conversation through the roster's existing selection path.
struct OverlayChannelMapModel: Equatable, Sendable {
  /// Channel 0 while the microphone broadcasts on it, then the agents.
  let chips: [OverlayChannelMapChip]

  /// The bound digits 1–9 in order, the agents; channel 0 leads only while
  /// the microphone is open on it (spec §13.2). A digit is bound when the
  /// roster snapshot carries it (`hudStates`); delivery history alone does
  /// not put a channel on the map. Channel 10 and other non-chord channels
  /// stay off it, since no chord reaches them.
  init(
    channels: [OverlayChannelDelivery],
    hudStates: [String: OverlayChannelHudProjection],
    selectedDigit: String? = nil,
    selectableDigits: Set<String>,
    unreadDigits: Set<String>
  ) {
    var agents: [OverlayChannelMapChip] = []
    var placed: Set<String> = []
    let ordered = channels.sorted { (Int($0.channel) ?? .max) < (Int($1.channel) ?? .max) }
    for channel in ordered {
      let digit = channel.channel
      guard let number = Int(digit), (1...9).contains(number), !placed.contains(digit),
        let hud = hudStates[digit]
      else { continue }
      placed.insert(digit)
      agents.append(
        OverlayChannelMapChip(
          digit: digit, name: OverlayChannelMapChip.displayName(channel.agent),
          disconnected: hud.followerAlive == false, microphoneOpen: hud.open,
          unread: unreadDigits.contains(digit), selectable: selectableDigits.contains(digit),
          selected: digit == selectedDigit))
    }
    guard hudStates["0"]?.open == true else {
      chips = agents
      return
    }
    let broadcast = OverlayChannelMapChip(
      digit: "0", name: "", disconnected: false, microphoneOpen: true,
      unread: unreadDigits.contains("0"), selectable: selectableDigits.contains("0"),
      selected: selectedDigit == "0")
    chips = [broadcast] + agents
  }

  /// The bound agents, without channel 0's broadcast chip.
  var agents: [OverlayChannelMapChip] { chips.filter { !$0.isBroadcast } }

  /// The counter counts agents; channel 0 is not one.
  var count: Int { agents.count }

  /// Digits whose microphone is open, in map order.
  var openDigits: [String] { chips.filter(\.microphoneOpen).map(\.digit) }

  var microphoneOpen: Bool { !openDigits.isEmpty }

  var anyUnread: Bool { chips.contains(where: \.unread) }

  /// What the counter stands for: the number of agents, or the open digits
  /// while a channel records.
  var counterFigure: String {
    microphoneOpen ? openDigits.joined(separator: ", ") : String(count)
  }

  /// The agents in one line, for the counter's hint:
  /// `3 Igor · 4 Hania · 6 Konfig`.
  var counterHint: String {
    agents.map { $0.digit + " " + $0.name }.joined(separator: " · ")
  }

  var counterAccessibilityValue: String {
    guard microphoneOpen else { return counterHint }
    let open = String(
      localized: "Microphone open on channel \(counterFigure)",
      comment: "Compact overlay channel map counter: the channel digits the microphone feeds")
    return counterHint.isEmpty ? open : open + ". " + counterHint
  }

  /// The recipients of the current utterance in one line, for the hint of
  /// mini's Agents slot while it shows their digits: `1 · As, 4 · Hania`;
  /// channel 0 reads `0 · All`.
  var recipientsHint: String {
    chips.filter(\.microphoneOpen).map { chip in
      chip.digit + " · "
        + (chip.isBroadcast
          ? String(localized: "All")
          : chip.name)
    }.joined(separator: ", ")
  }
}

/// How the expanded bar places its agent map at one window width (Monika's
/// bar brief, 2026-10-30). A pure function of measured widths: the window
/// owner sizes the bar with it and the header draws the same answer, so the
/// two never disagree about the row count.
///
/// Narrowing the bar walks one ladder: full names under their circles with
/// the gaps shrinking to their floor; then two rows, the tools on top and
/// the map below with each name beside its circle; then one row again with
/// the names truncated; then the circles overlapping. Every name keeps its own
/// slot and gap, so names never run together and every slot stays a hit area.
struct OverlayChannelMapLayout: Equatable, Sendable {
  enum Arrangement: Equatable, Sendable {
    /// One row: a name under its circle.
    case oneRow
    /// Two rows: the map on its own row, a name beside its circle.
    case twoRows
  }

  let arrangement: Arrangement
  /// The gap between two items.
  let spacing: CGFloat
  /// Each item's slot: in one row its width (the wider of circle and name,
  /// narrower than the circle once circles overlap); in two rows the width
  /// the name may take beside its circle.
  let slots: [CGFloat]
  /// The width each name may take; a name wider than this is truncated.
  let nameWidths: [CGFloat]
  /// The map's width.
  let width: CGFloat

  /// Circles overlap: a slot is narrower than the circle.
  var overlaps: Bool { arrangement == .oneRow && slots.contains { $0 + 0.5 < circle } }
  let circle: CGFloat

  /// Each item's leading edge in the one-row map.
  var origins: [CGFloat] {
    var x: CGFloat = 0
    return slots.map { slot in
      defer { x += slot + spacing }
      return x
    }
  }
}

/// The measured parts of the expanded bar: everything but the map in one row
/// (`baseWidth`), the bar's side padding, the map's full name widths and the
/// circle. `DictationOverlayWindow.midiBarGeometry` measures them; this type
/// only decides.
struct OverlayBarGeometry: Equatable, Sendable {
  /// Padding, brand, the meter while recording, the tools and their gaps.
  let baseWidth: CGFloat
  /// Both side paddings: the second row spans the bar minus these.
  let horizontalPadding: CGFloat
  /// Full name width per map item, 0 for channel 0, which has no name.
  let nameWidths: [CGFloat]
  let circle: CGFloat
  /// The gap between a circle and the name beside it (two rows).
  let inlineGap: CGFloat
  /// The bar never grows past this for one row; a wider team takes two.
  var oneRowBudget: CGFloat = Self.defaultOneRowBudget

  static let groupGap: CGFloat = 10
  static let oneRowNaturalGap: CGFloat = 8
  /// Names stay this far apart at every density.
  static let oneRowMinimumGap: CGFloat = 3
  static let twoRowNaturalGap: CGFloat = 10
  static let twoRowMinimumGap: CGFloat = 6
  /// The densest slot: an initial under circles that overlap by a third.
  static let densestSlot: CGFloat = 10
  static let defaultOneRowBudget: CGFloat = 520
  /// The second row's height and its distance from the first.
  static let secondRowHeight: CGFloat = 20
  static let rowGap: CGFloat = 4

  private var count: Int { nameWidths.count }

  private func gaps(_ gap: CGFloat) -> CGFloat { gap * CGFloat(max(0, count - 1)) }

  private func oneRowWidth(caps: [CGFloat], gap: CGFloat) -> CGFloat {
    caps.reduce(0) { $0 + max(circle, $1) } + gaps(gap)
  }

  private func twoRowWidth(gap: CGFloat) -> CGFloat {
    nameWidths.reduce(0) { $0 + circle + ($1 > 0 ? inlineGap + $1 : 0) } + gaps(gap)
  }

  /// The bar with its map in one row at a gap; without agents, the base.
  private func oneRowBar(caps: [CGFloat], gap: CGFloat) -> CGFloat {
    count == 0 ? baseWidth : baseWidth + Self.groupGap + oneRowWidth(caps: caps, gap: gap)
  }

  private var oneRowNatural: CGFloat {
    oneRowBar(caps: nameWidths, gap: Self.oneRowNaturalGap)
  }

  private var twoRowMinimum: CGFloat {
    max(baseWidth, horizontalPadding + twoRowWidth(gap: Self.twoRowMinimumGap))
  }

  /// The width at which one row with full names still holds. A team whose
  /// natural row passes the budget takes two rows until the user widens the
  /// bar to that natural row.
  private var oneRowThreshold: CGFloat {
    oneRowNatural <= oneRowBudget
      ? oneRowBar(caps: nameWidths, gap: Self.oneRowMinimumGap) : oneRowNatural
  }

  /// The width the bar takes by itself: one row with full names, or two
  /// rows when that row passes the budget. Never wider than the content.
  var naturalWidth: CGFloat {
    guard count > 0 else { return baseWidth }
    if oneRowNatural <= oneRowBudget { return oneRowNatural }
    return max(baseWidth, horizontalPadding + twoRowWidth(gap: Self.twoRowNaturalGap))
  }

  /// The narrowest bar: the densest layout the ladder reaches.
  var floorWidth: CGFloat {
    guard count > 0 else { return baseWidth }
    let densest =
      baseWidth + Self.groupGap + Self.densestSlot * CGFloat(count)
      + gaps(Self.oneRowMinimumGap)
    return min(naturalWidth, densest, twoRowMinimum)
  }

  /// The height the bar adds at a width: the second row's own height and
  /// its gap, nothing in one row.
  func extraHeight(width: CGFloat) -> CGFloat {
    layout(width: width).arrangement == .twoRows ? Self.rowGap + Self.secondRowHeight : 0
  }

  /// The map at a bar width. Half a point absorbs fractional rounding.
  func layout(width: CGFloat) -> OverlayChannelMapLayout {
    guard count > 0 else {
      return OverlayChannelMapLayout(
        arrangement: .oneRow, spacing: 0, slots: [], nameWidths: [], width: 0, circle: circle)
    }
    let available = width - baseWidth - Self.groupGap
    if width + 0.5 >= oneRowThreshold {
      let fixed = oneRowWidth(caps: nameWidths, gap: 0)
      let gap =
        count > 1
        ? min(
          Self.oneRowNaturalGap,
          max(Self.oneRowMinimumGap, (available - fixed) / CGFloat(count - 1)))
        : Self.oneRowNaturalGap
      return oneRow(caps: nameWidths, gap: gap)
    }
    if width + 0.5 >= twoRowMinimum {
      let fixed = twoRowWidth(gap: 0)
      let gap =
        count > 1
        ? min(
          Self.twoRowNaturalGap,
          max(Self.twoRowMinimumGap, (width - horizontalPadding - fixed) / CGFloat(count - 1)))
        : Self.twoRowNaturalGap
      return OverlayChannelMapLayout(
        arrangement: .twoRows, spacing: gap, slots: nameWidths, nameWidths: nameWidths,
        width: fixed + gaps(gap), circle: circle)
    }
    let gap = Self.oneRowMinimumGap
    // Truncate: the widest names give way first, never below the circle.
    if oneRowWidth(caps: nameWidths.map { min($0, circle) }, gap: gap) <= available + 0.5 {
      var low = circle
      var high = nameWidths.max() ?? circle
      for _ in 0..<24 {
        let mid = (low + high) / 2
        if oneRowWidth(caps: nameWidths.map { min($0, mid) }, gap: gap) <= available {
          low = mid
        } else {
          high = mid
        }
      }
      return oneRow(caps: nameWidths.map { min($0, floor(low)) }, gap: gap)
    }
    // Overlap: every slot narrows below the circle to the densest slot.
    let slot = max(
      Self.densestSlot, floor((available - gaps(gap)) / CGFloat(count)))
    return OverlayChannelMapLayout(
      arrangement: .oneRow, spacing: gap, slots: Array(repeating: slot, count: count),
      nameWidths: nameWidths.map { min($0, slot) },
      width: slot * CGFloat(count) + gaps(gap), circle: circle)
  }

  private func oneRow(caps: [CGFloat], gap: CGFloat) -> OverlayChannelMapLayout {
    let slots = caps.map { max(circle, $0) }
    return OverlayChannelMapLayout(
      arrangement: .oneRow, spacing: gap, slots: slots, nameWidths: caps,
      width: slots.reduce(0, +) + gaps(gap), circle: circle)
  }
}
