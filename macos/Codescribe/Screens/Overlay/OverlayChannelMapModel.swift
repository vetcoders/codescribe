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

/// One channel of the compact overlay's map: a bound digit and its agent, or
/// channel 0. Display values only; the conversation and its exact owner are
/// resolved by the roster when the chip is clicked.
struct OverlayChannelMapChip: Equatable, Identifiable, Sendable {
  let digit: String
  let name: String
  /// The follower behind the channel is gone: the chip dims like the
  /// roster's Disconnected row. The digit stays, because the chord still
  /// reaches the channel.
  let disconnected: Bool
  /// The microphone feeds this channel now: the chip reads `MIC → 5 Kuba`.
  let microphoneOpen: Bool
  let unread: Bool
  /// A current conversation exists for the digit, so a click can select it.
  let selectable: Bool

  var id: String { digit }
  var isBroadcast: Bool { digit == "0" }

  /// The floating hint: `5 · Kuba · Ctrl+5`.
  func hint(modifier: OverlayChannelChordModifier) -> String {
    digit + " · " + name + " · " + modifier.shortcut(digit: digit)
  }

  func accessibilityLabel(modifier: OverlayChannelChordModifier) -> String {
    let shortcut = modifier.spokenShortcut(digit: digit)
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
}

/// What the compact strip shows of the map at a given width.
enum OverlayChannelMapPresentation: Equatable, Sendable {
  /// Every chip, in digit order.
  case chips
  /// One counter pill: `4 ▾`, or `MIC → 5` while a channel records.
  case counter
  /// Not even the counter fits; the map yields so the actions and the fold
  /// button keep their place.
  case hidden
}

/// The compact overlay's channel map (bus intercom spec §5.1, §5.4, §6): a
/// projection of the same roster snapshot the Agents canvas reads. It holds
/// no state, polls nothing and never names a session; a click resolves the
/// conversation through the roster's existing selection path.
struct OverlayChannelMapModel: Equatable, Sendable {
  let chips: [OverlayChannelMapChip]

  /// Channel 0 first, then the bound digits 1–9 in order. A digit is bound
  /// when the roster snapshot carries it (`hudStates`); delivery history alone
  /// does not put a channel on the map. Channel 10 and other non-chord
  /// channels stay off it, since no chord reaches them.
  init(
    channels: [OverlayChannelDelivery],
    hudStates: [String: OverlayChannelHudProjection],
    broadcastName: String,
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
          digit: digit, name: channel.agent, disconnected: hud.followerAlive == false,
          microphoneOpen: hud.open, unread: unreadDigits.contains(digit),
          selectable: selectableDigits.contains(digit)))
    }
    guard hudStates["0"] != nil || !agents.isEmpty else {
      chips = []
      return
    }
    let broadcast = OverlayChannelMapChip(
      digit: "0", name: broadcastName, disconnected: false,
      microphoneOpen: hudStates["0"]?.open == true, unread: unreadDigits.contains("0"),
      selectable: selectableDigits.contains("0"))
    chips = [broadcast] + agents
  }

  var count: Int { chips.count }

  /// Digits whose microphone is open, in map order.
  var openDigits: [String] { chips.filter(\.microphoneOpen).map(\.digit) }

  var microphoneOpen: Bool { !openDigits.isEmpty }

  var anyUnread: Bool { chips.contains(where: \.unread) }

  /// The counter's figure: the number of mapped channels, channel 0
  /// included, or the recording digits while a channel records.
  var counterFigure: String {
    microphoneOpen ? openDigits.joined(separator: ", ") : String(count)
  }

  /// The whole map in one line, for the counter's hint:
  /// `0 All · 2 Miksa · 3 Igor · 5 Kuba`.
  var counterHint: String {
    chips.map { $0.digit + " " + $0.name }.joined(separator: " · ")
  }

  var counterAccessibilityValue: String {
    guard microphoneOpen else { return counterHint }
    let open = String(
      localized: "Microphone open on channel \(counterFigure)",
      comment: "Compact overlay channel map counter: the channel digits the microphone feeds")
    return open + ". " + counterHint
  }

  /// The chips' row: their measured widths and the gaps between them.
  static func rowWidth(chipWidths: [CGFloat], spacing: CGFloat) -> CGFloat {
    guard !chipWidths.isEmpty else { return 0 }
    return chipWidths.reduce(0, +) + spacing * CGFloat(chipWidths.count - 1)
  }

  /// Chips when the whole row fits, else the counter, else nothing. Half a
  /// point absorbs the layout's fractional rounding. A nil width is the
  /// layout's ideal-size probe, which the full row answers.
  static func presentation(
    availableWidth: CGFloat?, rowWidth: CGFloat, counterWidth: CGFloat
  ) -> OverlayChannelMapPresentation {
    guard rowWidth > 0 else { return .hidden }
    guard let availableWidth, availableWidth.isFinite else { return .chips }
    if availableWidth + 0.5 >= rowWidth { return .chips }
    if availableWidth + 0.5 >= counterWidth { return .counter }
    return .hidden
  }

  static func presentation(
    availableWidth: CGFloat?, chipWidths: [CGFloat], spacing: CGFloat, counterWidth: CGFloat
  ) -> OverlayChannelMapPresentation {
    presentation(
      availableWidth: availableWidth, rowWidth: rowWidth(chipWidths: chipWidths, spacing: spacing),
      counterWidth: counterWidth)
  }

  /// The width the map claims for a proposal: the width of what it shows.
  /// Reporting the counter's width, not the proposal, hands the rest back to
  /// the waveform.
  static func occupiedWidth(
    availableWidth: CGFloat?, rowWidth: CGFloat, counterWidth: CGFloat
  ) -> CGFloat {
    switch presentation(
      availableWidth: availableWidth, rowWidth: rowWidth, counterWidth: counterWidth)
    {
    case .chips: rowWidth
    case .counter: counterWidth
    case .hidden: 0
    }
  }
}
