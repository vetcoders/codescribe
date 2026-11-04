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

/// One channel of the overlay's map: a bound digit and its agent,
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
  /// violet selection fill.
  let selected: Bool

  var id: String { digit }
  var isBroadcast: Bool { digit == "0" }

  /// The floating hint: `5 · Kuba · Ctrl+5`, or `0 · Ctrl+0`; without a
  /// chord `5 · Kuba` or `0`. It names the agent in full whatever the bar
  /// cuts its caption to.
  func hint(modifier: OverlayChannelChordModifier) -> String {
    var parts = isBroadcast || name.isEmpty ? [digit] : [digit, name]
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

/// The widths the expanded bar's map gives its chips' names (bus brief
/// 2026-11-04 §5, §11C). Every value is a string measured in the pill's own
/// font (`OverlayChannelPill.nameFont`); channel 0 and an agent with an empty
/// name have zero, so their pill is the circle alone.
struct OverlayChannelMapNameWidths: Equatable, Sendable {
  /// Each name whole, up to `OverlayChannelPill.maxNameWidth`: the widest
  /// the bar grows to when the user widens it.
  let full: [CGFloat]
  /// The default bar's names: whole up to six letters, else five and `…`.
  let preferred: [CGFloat]
  /// A narrowed bar's floor: whole up to five letters, else four and `…`.
  let shortest: [CGFloat]

  init(full: [CGFloat], preferred: [CGFloat], shortest: [CGFloat]) {
    self.full = full
    self.preferred = preferred
    self.shortest = shortest
  }

  @MainActor
  init(chips: [OverlayChannelMapChip], scale: CGFloat) {
    let names = chips.map { $0.isBroadcast ? "" : $0.name }
    full = names.map { $0.isEmpty ? 0 : OverlayChannelPill.nameWidth($0, scale: scale) }
    preferred = names.map { name in
      guard !name.isEmpty else { return 0 }
      return name.count <= OverlayChannelPill.preferredNameLength
        ? OverlayChannelPill.nameWidth(name, scale: scale)
        : OverlayChannelPill.truncatedNameWidth(
          name, prefix: OverlayChannelPill.preferredNamePrefix, scale: scale)
    }
    shortest = names.map { name in
      name.isEmpty
        ? 0
        : OverlayChannelPill.truncatedNameWidth(
          name, prefix: OverlayChannelPill.shortestNamePrefix, scale: scale)
    }
  }

  /// One cap for every name: a name narrower than the cap stays whole, a
  /// wider one is cut to the cap but never below its shortest width.
  func capped(at cap: CGFloat) -> [CGFloat] {
    zip(full, shortest).map { full, shortest in
      full == 0 ? 0 : max(shortest, min(full, cap))
    }
  }

  /// The names a row of `available` points shows: whole when they fit, else
  /// the largest common cap that fits, so the longest names give way first
  /// and short ones keep every letter. Below the shortest widths the row
  /// keeps them; the window's resize minimum never proposes less. A nil
  /// width is the layout's ideal-size probe, which the whole names answer.
  func fitted(available: CGFloat?, rowWidth: ([CGFloat]) -> CGFloat) -> [CGFloat] {
    guard let available, available.isFinite, rowWidth(full) > available + 0.5 else { return full }
    guard rowWidth(shortest) < available + 0.5 else { return shortest }
    var low: CGFloat = 0
    var high = full.max() ?? 0
    // Half a point of cap is below what the ellipsis truncation can show.
    while high - low > 0.5 {
      let cap = (low + high) / 2
      if rowWidth(capped(at: cap)) <= available + 0.5 { low = cap } else { high = cap }
    }
    return capped(at: low)
  }
}

/// The overlay's channel map (bus intercom spec §5.1, §6, §13): a
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

  /// The expanded bar keeps 1–5 agents in its single row, names under the
  /// circles, and gives 6–9 a second row, names beside them (bus brief
  /// 2026-11-04 §4). Measured width decides inside that preference: a single
  /// row the user narrowed below its shortest names takes two rows.
  static let singleRowAgentLimit = 5

  /// A row of the map's pills at the given name widths: each pill's
  /// measured width in its form and the gaps between them.
  @MainActor
  static func rowWidth(
    nameWidths: [CGFloat], form: OverlayChannelPill.Form, scale: CGFloat, spacing: CGFloat
  ) -> CGFloat {
    rowWidth(
      chipWidths: nameWidths.map {
        OverlayChannelPill.chipWidth(nameWidth: $0, form: form, scale: scale)
      }, spacing: spacing)
  }

  /// The chips' row: their measured widths and the gaps between them.
  static func rowWidth(chipWidths: [CGFloat], spacing: CGFloat) -> CGFloat {
    guard !chipWidths.isEmpty else { return 0 }
    return chipWidths.reduce(0, +) + spacing * CGFloat(chipWidths.count - 1)
  }
}
