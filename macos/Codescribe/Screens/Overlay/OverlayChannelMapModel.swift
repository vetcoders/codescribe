import CoreGraphics
import Foundation

/// The Agent channel chord's modifier as Settings › Shortcuts › Extra
/// shortcuts stores it (`AGENT_CHANNEL_MODIFIER`). The channel map names it in
/// every shortcut hint, so a hint never claims Ctrl while the chord is Fn.
enum OverlayChannelChordModifier: Equatable, Sendable {
  case control
  case function

  /// The Settings normalization: `"fn"` is Fn, every other token, Command
  /// included, is Ctrl (`HotkeysEngine.normalizeChannelModifier`).
  init(setting: String) {
    self = setting.lowercased() == "fn" ? .function : .control
  }

  /// The key's name as the Settings picker prints it, verbatim in every
  /// language there as well.
  var keyName: String {
    switch self {
    case .control: "Ctrl"
    case .function: "Fn"
    }
  }

  /// The written chord in a hint: `Ctrl+5`.
  func shortcut(digit: String) -> String { keyName + "+" + digit }

  /// The chord as VoiceOver reads it: `Ctrl 5`.
  func spokenShortcut(digit: String) -> String { keyName + " " + digit }
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
  /// The selected conversation is on this channel: when the strip drops the
  /// names, this chip keeps its own.
  let selected: Bool

  var id: String { digit }
  var isBroadcast: Bool { digit == "0" }

  /// Whether the chip shows its name at a presentation: every agent chip
  /// with names, only the selected one with digits, channel 0 never.
  func showsName(in presentation: OverlayChannelMapPresentation) -> Bool {
    guard !isBroadcast else { return false }
    switch presentation {
    case .chips: return true
    case .digits: return selected
    case .counter, .hidden: return false
    }
  }

  /// The floating hint: `5 · Kuba · Ctrl+5`, or `0 · Ctrl+0`.
  func hint(modifier: OverlayChannelChordModifier) -> String {
    let shortcut = modifier.shortcut(digit: digit)
    return isBroadcast ? digit + " · " + shortcut : digit + " · " + name + " · " + shortcut
  }

  func accessibilityLabel(modifier: OverlayChannelChordModifier) -> String {
    let shortcut = modifier.spokenShortcut(digit: digit)
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

  /// A lease name as the map prints it: the first grapheme upper-cased for
  /// the current locale (`igor` → `Igor`). Leases, bindings and identifiers
  /// keep the name as the bus has it.
  static func displayName(_ name: String) -> String {
    guard let first = name.first else { return name }
    return String(first).uppercased(with: .current) + String(name.dropFirst())
  }
}

/// What the compact strip shows of the map at a given width (spec §13.1).
enum OverlayChannelMapPresentation: Equatable, Sendable {
  /// Every chip with its agent's name, in digit order.
  case chips
  /// Digit-only chips; the selected conversation's chip keeps its name.
  case digits
  /// One counter pill: `4 ▾`, or the open channel's digit with the badge.
  case counter
  /// Not even the counter fits; the map yields so the actions and the fold
  /// button keep their place.
  case hidden
}

/// Display values for the horizontal intercom's existing agent commands.
/// The header resolves availability through the roster; the map owns no
/// capture, playback preference or panel destination.
struct OverlayChannelMapControls: Equatable, Sendable {
  var microphoneEnabled = false
  var muted: Bool? = nil
  var playbackEnabled = false
  var speaking = false
  var focusSymbol: String? = nil
  var focusLabel = ""
  var focusEnabled = false
  var focusPending = false
  var focusSucceeded = false
  var focusFailed = false
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

  /// The chips' measured widths at a presentation.
  @MainActor
  func chipWidths(for presentation: OverlayChannelMapPresentation, scale: CGFloat) -> [CGFloat] {
    chips.map {
      OverlayChannelPill.chipWidth(
        name: $0.showsName(in: presentation) ? $0.name : nil, scale: scale)
    }
  }

  @MainActor
  func rowWidth(
    for presentation: OverlayChannelMapPresentation, scale: CGFloat, spacing: CGFloat
  ) -> CGFloat {
    Self.rowWidth(chipWidths: chipWidths(for: presentation, scale: scale), spacing: spacing)
  }

  /// The chips' row: their measured widths and the gaps between them.
  static func rowWidth(chipWidths: [CGFloat], spacing: CGFloat) -> CGFloat {
    guard !chipWidths.isEmpty else { return 0 }
    return chipWidths.reduce(0, +) + spacing * CGFloat(chipWidths.count - 1)
  }

  /// Named chips when their row fits, else digit-only chips, else the
  /// counter, else nothing. Half a point absorbs the layout's fractional
  /// rounding. A nil width is the layout's ideal-size probe, which the named
  /// row answers.
  static func presentation(
    availableWidth: CGFloat?, namedWidth: CGFloat, digitsWidth: CGFloat, counterWidth: CGFloat
  ) -> OverlayChannelMapPresentation {
    guard namedWidth > 0 else { return .hidden }
    guard let availableWidth, availableWidth.isFinite else { return .chips }
    if availableWidth + 0.5 >= namedWidth { return .chips }
    if availableWidth + 0.5 >= digitsWidth { return .digits }
    if availableWidth + 0.5 >= counterWidth { return .counter }
    return .hidden
  }

  /// The width the map claims for a proposal: the width of what it shows.
  /// Reporting the shown row's or the counter's width, not the proposal,
  /// hands the rest back to the waveform.
  static func occupiedWidth(
    availableWidth: CGFloat?, namedWidth: CGFloat, digitsWidth: CGFloat, counterWidth: CGFloat,
    preferredWidths: [CGFloat] = []
  ) -> CGFloat {
    // Rich named rows give up their extra controls before any name goes away.
    // Every candidate is measured from the same button frames the map draws.
    if namedWidth > 0,
      let preferred = preferredWidths.first(where: { width in
        let fits = availableWidth.map { !$0.isFinite || $0 + 0.5 >= width }
        return width > 0 && (fits ?? true)
      })
    {
      return preferred
    }
    return switch presentation(
      availableWidth: availableWidth, namedWidth: namedWidth, digitsWidth: digitsWidth,
      counterWidth: counterWidth)
    {
    case .chips: namedWidth
    case .digits: digitsWidth
    case .counter: counterWidth
    case .hidden: 0
    }
  }
}
