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
/// the map below with each name beside its circle; then shortened names, in
/// whichever of one or two rows reaches the narrower floor. A shortened name
/// keeps at least three characters and `…`, and a name of four characters or
/// fewer is never shortened. Circles never overlap: Monika rejected one-letter
/// names under overlapping circles on 2026-10-30 ("ten widok odpada
/// całkowicie"). Every name keeps its own slot and gap, so names never run
/// together and every slot stays a hit area.
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
  /// never narrower than the circle); in two rows the width the name may
  /// take beside its circle.
  let slots: [CGFloat]
  /// The width each name may take; a name wider than this is truncated.
  let nameWidths: [CGFloat]
  /// The map's width.
  let width: CGFloat

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
  /// The narrowest each name may become: three characters and `…`, or the
  /// full name when it has four characters or fewer
  /// (`OverlayChannelPill.shortestNameWidth`).
  let minimumNameWidths: [CGFloat]
  let circle: CGFloat
  /// The gap between a circle and the name beside it (two rows).
  let inlineGap: CGFloat
  /// The bar never grows past this for one row; a wider team takes two.
  var oneRowBudget: CGFloat = Self.defaultOneRowBudget
  /// How far a one-row map's hanging names reach below the bar's lower
  /// padding at the current text scale: 0 at scale 1, more with larger
  /// text. The bar grows by exactly this instead of clipping the names
  /// (Monika's brief: text is never shrunk, height grows only by what is
  /// needed). `DictationOverlayWindow.midiBarGeometry` measures it.
  var oneRowOverhang: CGFloat = 0

  static let groupGap: CGFloat = 10
  static let oneRowNaturalGap: CGFloat = 8
  /// Names stay this far apart at every density.
  static let oneRowMinimumGap: CGFloat = 3
  static let twoRowNaturalGap: CGFloat = 10
  static let twoRowMinimumGap: CGFloat = 6
  static let defaultOneRowBudget: CGFloat = 520
  /// The second row's height and its distance from the first.
  static let secondRowHeight: CGFloat = 20
  static let rowGap: CGFloat = 4

  private var count: Int { nameWidths.count }

  private func gaps(_ gap: CGFloat) -> CGFloat { gap * CGFloat(max(0, count - 1)) }

  private func oneRowWidth(caps: [CGFloat], gap: CGFloat) -> CGFloat {
    caps.reduce(0) { $0 + max(circle, $1) } + gaps(gap)
  }

  private func twoRowWidth(caps: [CGFloat], gap: CGFloat) -> CGFloat {
    caps.reduce(0) { $0 + circle + ($1 > 0 ? inlineGap + $1 : 0) } + gaps(gap)
  }

  /// The bar with its map in one row at a gap; without agents, the base.
  private func oneRowBar(caps: [CGFloat], gap: CGFloat) -> CGFloat {
    count == 0 ? baseWidth : baseWidth + Self.groupGap + oneRowWidth(caps: caps, gap: gap)
  }

  /// The bar with its map on the second row at a gap.
  private func twoRowBar(caps: [CGFloat], gap: CGFloat) -> CGFloat {
    max(baseWidth, horizontalPadding + twoRowWidth(caps: caps, gap: gap))
  }

  private var oneRowNatural: CGFloat {
    oneRowBar(caps: nameWidths, gap: Self.oneRowNaturalGap)
  }

  private var twoRowMinimum: CGFloat {
    twoRowBar(caps: nameWidths, gap: Self.twoRowMinimumGap)
  }

  /// The densest each arrangement gets: every name at its shortest.
  private var oneRowDensest: CGFloat {
    oneRowBar(caps: minimumNameWidths, gap: Self.oneRowMinimumGap)
  }

  private var twoRowDensest: CGFloat {
    twoRowBar(caps: minimumNameWidths, gap: Self.twoRowMinimumGap)
  }

  /// Shortened names take the arrangement whose floor is narrower.
  private var shortenedArrangement: OverlayChannelMapLayout.Arrangement {
    twoRowDensest < oneRowDensest ? .twoRows : .oneRow
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
    return twoRowBar(caps: nameWidths, gap: Self.twoRowNaturalGap)
  }

  /// The narrowest bar: the densest layout the ladder reaches, every name
  /// at its shortest in the narrower arrangement. The user cannot drag the
  /// bar below it.
  var floorWidth: CGFloat {
    guard count > 0 else { return baseWidth }
    return min(naturalWidth, oneRowDensest, twoRowDensest)
  }

  /// The height the bar adds at a width: the second row's own height and
  /// its gap; in one row, only the names' overhang past the lower padding.
  func extraHeight(width: CGFloat) -> CGFloat {
    guard count > 0 else { return 0 }
    return layout(width: width).arrangement == .twoRows
      ? Self.rowGap + Self.secondRowHeight : oneRowOverhang
  }

  /// The map at a bar width. Half a point absorbs fractional rounding.
  func layout(width: CGFloat) -> OverlayChannelMapLayout {
    guard count > 0 else {
      return OverlayChannelMapLayout(
        arrangement: .oneRow, spacing: 0, slots: [], nameWidths: [], width: 0, circle: circle)
    }
    let oneRowAvailable = width - baseWidth - Self.groupGap
    let twoRowAvailable = width - horizontalPadding
    if width + 0.5 >= oneRowThreshold {
      let fixed = oneRowWidth(caps: nameWidths, gap: 0)
      return oneRow(
        caps: nameWidths,
        gap: spacing(
          available: oneRowAvailable, fixed: fixed, natural: Self.oneRowNaturalGap,
          minimum: Self.oneRowMinimumGap))
    }
    if width + 0.5 >= twoRowMinimum {
      let fixed = twoRowWidth(caps: nameWidths, gap: 0)
      return twoRows(
        caps: nameWidths,
        gap: spacing(
          available: twoRowAvailable, fixed: fixed, natural: Self.twoRowNaturalGap,
          minimum: Self.twoRowMinimumGap))
    }
    // Shorten: the widest names give way first, each never below its
    // shortest form, in the arrangement with the narrower floor.
    switch shortenedArrangement {
    case .oneRow:
      let gap = Self.oneRowMinimumGap
      let caps = shortenedCaps(available: oneRowAvailable) {
        oneRowWidth(caps: $0, gap: gap)
      }
      return oneRow(caps: caps, gap: gap)
    case .twoRows:
      let gap = Self.twoRowMinimumGap
      let caps = shortenedCaps(available: twoRowAvailable) {
        twoRowWidth(caps: $0, gap: gap)
      }
      return twoRows(caps: caps, gap: gap)
    }
  }

  /// The gap that fills the room between the natural and the minimum gap.
  private func spacing(
    available: CGFloat, fixed: CGFloat, natural: CGFloat, minimum: CGFloat
  ) -> CGFloat {
    guard count > 1 else { return natural }
    return min(natural, max(minimum, (available - fixed) / CGFloat(count - 1)))
  }

  /// The largest common cap whose row fits, each name kept between its
  /// shortest and its full width. At the floor every name is at its shortest.
  private func shortenedCaps(
    available: CGFloat, rowWidth: ([CGFloat]) -> CGFloat
  ) -> [CGFloat] {
    func caps(_ cap: CGFloat) -> [CGFloat] {
      zip(nameWidths, minimumNameWidths).map { full, shortest in
        max(shortest, min(full, cap))
      }
    }
    var low: CGFloat = 0
    var high = nameWidths.max() ?? 0
    for _ in 0..<24 {
      let mid = (low + high) / 2
      if rowWidth(caps(mid)) <= available {
        low = mid
      } else {
        high = mid
      }
    }
    return caps(floor(low))
  }

  private func oneRow(caps: [CGFloat], gap: CGFloat) -> OverlayChannelMapLayout {
    let slots = caps.map { max(circle, $0) }
    return OverlayChannelMapLayout(
      arrangement: .oneRow, spacing: gap, slots: slots, nameWidths: caps,
      width: slots.reduce(0, +) + gaps(gap), circle: circle)
  }

  private func twoRows(caps: [CGFloat], gap: CGFloat) -> OverlayChannelMapLayout {
    OverlayChannelMapLayout(
      arrangement: .twoRows, spacing: gap, slots: caps, nameWidths: caps,
      width: twoRowWidth(caps: caps, gap: gap), circle: circle)
  }
}
