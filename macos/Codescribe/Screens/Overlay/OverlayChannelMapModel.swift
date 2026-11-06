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

/// What the hover card of an agent's chip states (Founder, 2026-11-04): three
/// separate facts, never folded into one value, each read from a projection
/// the roster row already shows.
///
/// 1. Presence is the roster snapshot's `followerAlive`: `true` is online in
///    the current sample, `false` only "not active" (a capped discovery can
///    report false for a live agent), `nil` no data. A stall (alive, backlog
///    unread past the horizon) is its own flag. The delivery dot and the
///    channel 0 policy never stand in for presence.
/// 2. Reception is the channel 0 policy as the ledger paints it, two states
///    like the roster's antenna and the context menu's checkmark (Founder,
///    2026-11-04: no third, unconfirmed state). The moon is do-not-disturb.
///    The card names channel 0 by its display name (`channelZeroName`).
/// 3. Unread is the conversation's count of new replies.
///
/// `speaking` is the agent's reply playing aloud, from the same set of
/// conversations as the chip's own speaking dot (L2, 2026-11-06), so the
/// card's presence dot turns violet exactly when the chip's does.
struct OverlayChannelChipStatus: Equatable, Sendable {
  enum Presence: Equatable, Sendable {
    case online
    case inactive
    case unknown
  }

  enum Reception: Equatable, Sendable {
    case member
    case off
  }

  let presence: Presence
  /// Alive by heartbeat, yet its backlog unread past the stall horizon.
  let stalled: Bool
  let reception: Reception
  /// Do-not-disturb, as painted.
  let quiet: Bool
  let unread: Int
  /// The agent's provider (`codex`, `claude`) when the roster names one.
  let provider: String?
  /// The agent's reply plays aloud now.
  let speaking: Bool
  /// Channel 0's display name (`OverlaySharedChannelName.display`), which
  /// the reception line shows in the nominative, never declined.
  let channelZeroName: String

  init(
    presence: Presence, stalled: Bool = false, reception: Reception, quiet: Bool = false,
    unread: Int = 0, provider: String? = nil, speaking: Bool = false,
    channelZeroName: String = OverlaySharedChannelName.defaultName
  ) {
    self.presence = presence
    self.stalled = stalled
    self.reception = reception
    self.quiet = quiet
    self.unread = unread
    self.provider = provider
    self.speaking = speaking
    self.channelZeroName = channelZeroName
  }

  /// The card's facts from the roster's projections: the snapshot's
  /// liveness claim, its stall, the channel 0 policy as painted, and the
  /// unread count.
  init(
    followerAlive: Bool?, stalled: Bool, policy: AgentBroadcastPolicy, unread: Int,
    provider: String?, speaking: Bool = false,
    channelZeroName: String = OverlaySharedChannelName.defaultName
  ) {
    switch followerAlive {
    case true?: presence = .online
    case false?: presence = .inactive
    case nil: presence = .unknown
    }
    self.stalled = stalled
    reception = policy.member ? .member : .off
    quiet = policy.moon
    self.unread = max(0, unread)
    self.provider = provider.flatMap { $0.isEmpty ? nil : $0 }
    self.speaking = speaking
    self.channelZeroName = channelZeroName
  }
}

/// One channel of the overlay's map: a bound digit and its agent, or
/// channel 0, which leads the map whenever it is shown. Display values only;
/// the conversation and its exact owner are resolved by the roster when the
/// chip is clicked.
struct OverlayChannelMapChip: Equatable, Identifiable, Sendable {
  let digit: String
  /// The agent's name exactly as the roster supplies it. The Bus preserves
  /// its entered spelling; the map must not infer capitalisation.
  /// Channel 0 has none.
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
  /// The agent's state for its hover card; channel 0 has none (its hover is
  /// the channel's message carousel).
  var status: OverlayChannelChipStatus? = nil
  /// The agent's reply is playing aloud: the chip shows the blinking violet
  /// speaking dot (L1, 2026-11-06).
  var speaking = false
  /// Channel 0's short name for a tight bar (`OverlaySharedChannelName`).
  var shortName: String? = nil

  var id: String { digit }
  var isBroadcast: Bool { digit == "0" }

  /// The bar draws the name under or beside the digit: not for an agent
  /// whose follower is gone, which keeps its digit alone (L5, 2026-11-06).
  var captionShown: Bool { isBroadcast || !disconnected }

  /// The plain hint: `5 · Kuba · Ctrl+5`, or `0 · Ctrl+0`; without a chord
  /// `5 · Kuba` or `0`. It names the agent in full whatever the bar cuts its
  /// caption to. An agent's chip with a status shows its hover card instead
  /// (`OverlayChannelChipHoverCard`), whose last line is the chord.
  func hint(modifier: OverlayChannelChordModifier) -> String {
    var parts = name.isEmpty ? [digit] : [digit, name]
    if let shortcut = modifier.shortcut(digit: digit) { parts.append(shortcut) }
    return parts.joined(separator: " · ")
  }

  func accessibilityLabel(modifier: OverlayChannelChordModifier) -> String {
    guard let shortcut = modifier.spokenShortcut(digit: digit) else {
      return accessibilityLabelWithoutShortcut
    }
    if isBroadcast {
      return microphoneOpen
        ? String(
          localized: "Channel \(digit), shortcut \(shortcut), microphone open",
          comment:
            "Collapsed overlay channel 0 chip while broadcasting: digit, the channel chord (e.g. Ctrl 0)"
        )
        : String(
          localized: "Channel \(digit), shortcut \(shortcut)",
          comment: "Overlay channel 0 chip at rest: digit, the channel chord (e.g. Ctrl 0)")
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
      return microphoneOpen
        ? String(
          localized: "Channel \(digit), microphone open",
          comment:
            "Collapsed overlay channel 0 chip while broadcasting, when no key plus digit opens a channel"
        )
        : String(
          localized: "Channel \(digit)",
          comment: "Overlay channel 0 chip at rest, when no key plus digit opens a channel")
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

  /// A provider identifier for the status card (`codex` → `Codex`). Agent
  /// names do not use this formatting: their spelling belongs to the Bus.
  static func displayName(_ name: String) -> String {
    guard let first = name.first else { return name }
    return String(first).uppercased(with: .current) + String(name.dropFirst())
  }
}

/// The widths the expanded bar's map gives its chips' names (bus brief
/// 2026-11-04 §5, §11C). Every value is a string measured in the pill's own
/// font (`OverlayChannelPill.nameFont`); an agent with an empty name has
/// zero, so its pill is the circle alone. So has an agent whose follower is
/// gone (`disconnected`, L5 2026-11-06): the bar shows its digit alone and
/// gives the room back, while its hover card, the agents list under ✦ and
/// the Bus window still name it. Channel 0 carries its display name
/// by the same rules; with the default name a cut width is at most its short
/// form's (`OverlayChannelMapChip.shortName`), which the pill then shows.
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
    let names = chips.map { $0.captionShown ? $0.name : "" }
    full = names.map { $0.isEmpty ? 0 : OverlayChannelPill.nameWidth($0, scale: scale) }
    let preferredCut = names.map { name -> CGFloat in
      guard !name.isEmpty else { return 0 }
      return name.count <= OverlayChannelPill.preferredNameLength
        ? OverlayChannelPill.nameWidth(name, scale: scale)
        : OverlayChannelPill.truncatedNameWidth(
          name, prefix: OverlayChannelPill.preferredNamePrefix, scale: scale)
    }
    let shortestCut = names.map { name -> CGFloat in
      name.isEmpty
        ? 0
        : OverlayChannelPill.truncatedNameWidth(
          name, prefix: OverlayChannelPill.shortestNamePrefix, scale: scale)
    }
    // A short form narrower than a cut name stands in for the cut.
    let shortWidths = chips.map { chip in
      chip.shortName.map { OverlayChannelPill.nameWidth($0, scale: scale) }
    }
    preferred = zip(zip(preferredCut, full), shortWidths).map { pair, short in
      let (cut, whole) = pair
      guard let short, cut < whole else { return cut }
      return min(cut, short)
    }
    shortest = zip(shortestCut, shortWidths).map { cut, short in
      short.map { min(cut, $0) } ?? cut
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
  /// and short ones keep every letter. The cap is a whole point, rounded
  /// down from the bisection, so a name never ends on a fractional width;
  /// a smaller cap never widens the row, so the rounded row still fits.
  /// Below the shortest widths the row keeps them; the window's resize
  /// minimum never proposes less. A nil width is the layout's ideal-size
  /// probe, which the whole names answer.
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
    return capped(at: low.rounded(.down))
  }
}

/// The shared channel 0's display name (`SHARED_CHANNEL_NAME` in
/// settings.json). Only the name is personal: channel 0's digit, chord and
/// routing never depend on it. An empty stored name shows the interface's
/// default, "Shared" ("Wspólny" in Polish), whose short form ("Wsp.") a tight
/// bar may show instead of an ellipsis.
enum OverlaySharedChannelName {
  static var defaultName: String {
    String(
      localized: "overlay.sharedChannel.defaultName", defaultValue: "Shared",
      comment: "Default display name of the shared channel 0 on the Bus")
  }

  static var defaultShortName: String {
    String(
      localized: "overlay.sharedChannel.defaultShortName", defaultValue: "Shared",
      comment:
        "Short form of the shared channel's default name for a tight bar (Polish: Wsp.); the same word when it is already short"
    )
  }

  /// The name to show for a stored value.
  static func display(_ stored: String) -> String {
    let name = stored.trimmingCharacters(in: .whitespacesAndNewlines)
    return name.isEmpty ? defaultName : name
  }

  /// The short form a tight bar may show: the default name's only. A name
  /// of the user's own is cut with an ellipsis like an agent's.
  static func shortName(_ stored: String) -> String? {
    stored.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? defaultShortName : nil
  }
}

/// The overlay's channel map (bus intercom spec §5.1, §6, §13): a
/// projection of the same roster snapshot the Agents canvas reads. It holds
/// no state, polls nothing and never names a session; a click resolves the
/// conversation through the roster's existing selection path.
struct OverlayChannelMapModel: Equatable, Sendable {
  /// Channel 0, then the agents.
  let chips: [OverlayChannelMapChip]

  /// Channel 0's chip, then the bound digits 1–9 in order, the agents
  /// (Founder, 2026-11-04: `(0)` leads the row, under the codescribe dot, at
  /// rest as well as while the microphone broadcasts on it). A digit is
  /// bound when the roster snapshot carries it (`hudStates`); delivery
  /// history alone does not put a channel on the map. Channel 10 and other
  /// non-chord channels stay off it, since no chord reaches them. With no
  /// agent and no broadcast the map is empty: channel 0 alone is no map.
  /// `statuses` carries each agent's hover-card facts by digit.
  init(
    channels: [OverlayChannelDelivery],
    hudStates: [String: OverlayChannelHudProjection],
    selectedDigit: String? = nil,
    selectableDigits: Set<String>,
    unreadDigits: Set<String>,
    statuses: [String: OverlayChannelChipStatus] = [:],
    speakingDigits: Set<String> = [],
    sharedChannelName: String = ""
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
          digit: digit, name: channel.agent,
          disconnected: hud.followerAlive == false, microphoneOpen: hud.open,
          unread: unreadDigits.contains(digit), selectable: selectableDigits.contains(digit),
          selected: digit == selectedDigit, status: statuses[digit],
          speaking: speakingDigits.contains(digit)))
    }
    let broadcasting = hudStates["0"]?.open == true
    guard broadcasting || !agents.isEmpty else {
      chips = []
      return
    }
    // Channel 0 carries its name like an agent (number over the name in the
    // single row, beside it in the second); `sharedChannelName` is the
    // stored value, empty for the default.
    let zero = OverlayChannelMapChip(
      digit: "0", name: OverlaySharedChannelName.display(sharedChannelName),
      disconnected: false, microphoneOpen: broadcasting,
      unread: unreadDigits.contains("0"), selectable: selectableDigits.contains("0"),
      selected: selectedDigit == "0",
      shortName: OverlaySharedChannelName.shortName(sharedChannelName))
    chips = [zero] + agents
  }

  /// The bound agents, without channel 0's chip.
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
  /// 2026-11-04 §4). The limit counts agents; channel 0's chip leads either
  /// row and is not one. Measured width decides inside that preference: a
  /// single row the user narrowed below its shortest names takes two rows.
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
