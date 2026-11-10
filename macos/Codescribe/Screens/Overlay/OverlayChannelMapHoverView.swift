import AppKit
import SwiftUI

/// One row of the list under ✦, as plain values: an agent, or the one
/// quick-access entry above them, your inbox (Founder, 2026-11-04; channel 0
/// left the list on 2026-11-07, it lives on the bar). Every field comes from a projection `OverlayChannelStatusView`
/// already owns; the list computes no status of its own. The reception axes
/// are read live from `OverlayBroadcastPolicyLedger` when the row draws.
struct OverlayAgentsHoverRow: Equatable, Identifiable {
  /// What the row stands for; only an agent carries presence and reception.
  enum Kind: Equatable {
    case agent
    /// Your inbox (10): the envelope and its name, as the sidebar's card.
    case inbox
  }

  var kind: Kind = .agent
  /// The channel's digit; the inbox's is "10" and it draws an envelope.
  let digit: String
  let name: String
  /// The channel's current conversation; nil leaves the row unopenable.
  let conversationID: String?
  /// `OverlayChannelStatusView.presenceDotKind(for:)`: presence alone, not
  /// the stage of the last utterance (Founder, 2026-11-04).
  let dot: OverlayAgentDotKind
  /// The presence facts of the chip's hover card (`spokenStatus`), read by
  /// VoiceOver beside the dot; the delivery stage stays with the message.
  let status: String
  let microphoneOpen: Bool
  let disconnected: Bool
  let identity: AgentPlaybackIdentity?
  /// The roster's policy; the row draws it as the ledger paints it, the
  /// same two states as the roster's marks and the context menu's checkmark.
  let policy: AgentBroadcastPolicy
  let unread: Int

  var id: String { digit }

  /// A mailbox row: no presence dot, no antenna or moon, no microphone.
  var mailbox: Bool { kind != .agent }
}

extension OverlayChannelStatusView {
  /// The list under ✦: your inbox first, then the agents. Channel 0 has no
  /// row in any mode: it lives on the bar (Founder, 2026-11-07). An inbox
  /// without a current conversation has no row, as it has no rail pill.
  var hoverListRows: [OverlayAgentsHoverRow] {
    var rows: [OverlayAgentsHoverRow] = []
    if let inbox = currentConversations.first(where: \.isHumanInbox) {
      rows.append(
        OverlayAgentsHoverRow(
          kind: .inbox, digit: "10", name: inbox.displayTitle,
          conversationID: inbox.id, dot: .idle, status: "", microphoneOpen: false,
          disconnected: false, identity: nil, policy: .on,
          unread: unreadCounts[inbox.id] ?? 0))
    }
    return rows + agentsHoverRows
  }

  /// Agents 1–9 in ascending channel order; channel 0 has no agent row.
  var agentsHoverRows: [OverlayAgentsHoverRow] {
    channels
      .compactMap { channel -> (Int, OverlayChannelDelivery)? in
        guard let digit = Int(channel.channel), (1...9).contains(digit) else { return nil }
        return (digit, channel)
      }
      .sorted { $0.0 < $1.0 }
      .map { _, channel in
        let conversation = conversation(for: channel)
        return OverlayAgentsHoverRow(
          digit: channel.channel, name: channel.agent, conversationID: conversation?.id,
          dot: presenceDotKind(for: channel),
          status: OverlayChannelChipHoverCard.spokenStatus(
            channelChipStatus(for: channel, current: currentConversations)
          ).joined(separator: ". "),
          microphoneOpen: isOpen(channel), disconnected: hasDeadFollower(channel),
          identity: channel.broadcastIdentity, policy: channel.broadcastPolicy,
          unread: conversation.flatMap { unreadCounts[$0.id] } ?? 0)
      }
  }
}

/// Live source of the open list. The hint panel keeps the content it was
/// shown with, so the content reads this reference instead of captured rows:
/// a roster or policy refresh repaints the rows without re-showing the panel,
/// which would raise it again and restart or cancel its grace delay.
@MainActor
@Observable
final class OverlayAgentsHoverListModel {
  var rows: [OverlayAgentsHoverRow] = []
  /// Esc withdrew the list; the next entry onto ✦ clears it.
  var dismissed = false
  @ObservationIgnored var onOpen: ((String) -> Void)?
  @ObservationIgnored var onHover: ((Bool) -> Void)?
  @ObservationIgnored fileprivate var escapeMonitor: Any?
  @ObservationIgnored fileprivate var reportedHover = false
}

/// The ✦ button's hover: the agents list on the shared interactive hint
/// panel (`OverlayHeaderControlChrome`, as the roster's message preview).
/// With no row to show (no mailbox conversation and no agent), or while
/// agent capture is open and a click stops it, the button keeps its one-line
/// hint instead.
struct OverlayAgentsHoverListChrome: ViewModifier {
  /// The pause before the list opens, so passing over ✦ opens nothing.
  static let openDelay: TimeInterval = 0.3

  let title: String
  let palette: OverlayAppearancePalette
  let selected: Bool
  let rows: [OverlayAgentsHoverRow]
  let onOpen: (String) -> Void
  let onHover: ((Bool) -> Void)?
  @State private var model = OverlayAgentsHoverListModel()

  func body(content: Content) -> some View {
    let lists = !rows.isEmpty
    let list = lists ? AnyView(OverlayAgentsHoverList(model: model, palette: palette)) : nil
    content
      .modifier(
        OverlayHeaderControlChrome(
          title: title, accent: .agents, palette: palette, selected: selected,
          showsHint: !(lists && model.dismissed),
          hintContent: list,
          hintSize: lists ? OverlayAgentsHoverList.size(rows: rows) : nil,
          hintDelay: lists ? Self.openDelay : 0,
          hintInteractive: lists,
          onHoverChanged: { inside in
            if inside, model.dismissed { model.dismissed = false }
          })
      )
      .onAppear { sync() }
      .onChange(of: rows) { _, _ in sync() }
  }

  private func sync() {
    if model.rows != rows { model.rows = rows }
    model.onOpen = onOpen
    model.onHover = onHover
  }
}

/// The list under ✦: one quick-access row (your inbox) above one
/// low row per agent, no header or cards. Columns: numbered pill with the
/// name (opens the conversation), status dot, antenna (channel 0), moon (do
/// not disturb), the unread dot in the text tone. A mailbox row has the pill
/// and the unread dot only. The antenna and the moon are sibling buttons,
/// never nested in the pill, and switch reception through
/// `OverlayBroadcastPolicyLedger` without opening or closing anything.
/// No hint opens from inside the list: the shared hint panel is the list.
struct OverlayAgentsHoverList: View {
  let model: OverlayAgentsHoverListModel
  let palette: OverlayAppearancePalette

  static let rowHeight: CGFloat = OverlayChannelPill.headerHeight
  static let rowSpacing: CGFloat = 2
  static let horizontalPadding: CGFloat = 8
  static let verticalPadding: CGFloat = 4
  static let columnGap: CGFloat = 6
  static let markSize: CGFloat = 18
  static let markGap: CGFloat = 2
  static let unreadDiameter: CGFloat = 6
  static let minimumWidth: CGFloat = 140

  /// Sized to its rows: nine agents fit without a scroll view. The width is
  /// the widest row's own need: an agent row is its chip, the antenna and
  /// moon and the unread dot; the inbox row is its chip and the unread dot
  /// only, so a long inbox name no longer pays for marks it does not draw.
  @MainActor
  static func size(rows: [OverlayAgentsHoverRow]) -> CGSize {
    let content = rows.map(rowWidth).max() ?? 0
    let width = 2 * horizontalPadding + content
    let count = CGFloat(rows.count)
    return CGSize(
      width: max(minimumWidth, ceil(width)),
      height: 2 * verticalPadding + count * rowHeight + max(0, count - 1) * rowSpacing)
  }

  /// One row without the list's padding. Full names: the list is sized to
  /// its content, never truncated.
  @MainActor
  static func rowWidth(_ row: OverlayAgentsHoverRow) -> CGFloat {
    let chip = OverlayChannelPill.chipWidth(
      nameWidth: OverlayChannelPill.nameWidth(row.name, scale: 1), form: .chip, scale: 1,
      presenceDot: !row.mailbox)
    guard !row.mailbox else { return chip + columnGap + unreadDiameter }
    return chip + 3 * columnGap + 2 * markSize + markGap + unreadDiameter
  }

  var body: some View {
    VStack(alignment: .leading, spacing: Self.rowSpacing) {
      ForEach(model.rows) { row in
        OverlayAgentsHoverListRow(
          row: row, palette: palette,
          onOpen: { id in model.onOpen?(id) })
      }
    }
    .padding(.horizontal, Self.horizontalPadding)
    .padding(.vertical, Self.verticalPadding)
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    .background(palette.desktopBackground.color.opacity(0.9))
    .environment(\.colorScheme, palette.appearance == .dark ? .dark : .light)
    .background { OverlayMenuCursorRegion { reportHover($0) } }
    .onAppear { installEscape() }
    .onDisappear {
      removeEscape()
      reportHover(false)
    }
    .accessibilityElement(children: .contain)
    .accessibilityLabel(Text("Agents"))
    .accessibilityIdentifier("overlay-agents-hover-list")
  }

  private func reportHover(_ inside: Bool) {
    guard model.reportedHover != inside else { return }
    model.reportedHover = inside
    model.onHover?(inside)
  }

  /// The panel never becomes key, so Esc reaches the app's key window. While
  /// the list is on screen a local monitor takes Esc to close it.
  private func installEscape() {
    guard model.escapeMonitor == nil else { return }
    let model = model
    model.escapeMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
      let isEscape = event.keyCode == 53
      // Local key monitors deliver on the main thread.
      let consumed = MainActor.assumeIsolated { () -> Bool in
        guard isEscape, !model.dismissed else { return false }
        model.dismissed = true
        return true
      }
      return consumed ? nil : event
    }
  }

  private func removeEscape() {
    if let monitor = model.escapeMonitor { NSEvent.removeMonitor(monitor) }
    model.escapeMonitor = nil
  }
}

private struct OverlayAgentsHoverListRow: View {
  let row: OverlayAgentsHoverRow
  let palette: OverlayAppearancePalette
  let onOpen: (String) -> Void

  private var ledger: OverlayBroadcastPolicyLedger { .shared }

  private var policy: AgentBroadcastPolicy {
    row.identity.map { ledger.policy(for: $0, polled: row.policy) } ?? row.policy
  }

  private var pending: Bool { ledger.isPending(row.identity) }

  var body: some View {
    HStack(spacing: OverlayAgentsHoverList.columnGap) {
      pill
      if row.mailbox {
        // No reception, so no marks and no reserved width: the pill takes the
        // row and the unread dot stands at the right edge, in the agents'
        // unread column (`rowWidth`).
        unreadDot
      } else {
        Spacer(minLength: 0)
        // Two states each, as the roster's marks and the context menu's
        // checkmark (Founder, 2026-11-04): lit or not. No third, greyed
        // state for a policy the helper has not receipted; a receipt only
        // confirms this app's own click.
        HStack(spacing: OverlayAgentsHoverList.markGap) {
          mark(
            symbol: policy.statusSymbol, lit: policy.member,
            litColor: OverlayReceptionMarks.antennaInk(palette).color,
            label: policy.statusHelp, identifier: "overlay-agents-hover-antenna-\(row.digit)"
          ) { $0.member.toggle() }
          // The moon speaks through its shape, as on the roster's marks.
          mark(
            symbol: policy.quietSymbol, lit: policy.moon, litColor: palette.primaryText.color,
            unlitOpacity: 0.45, label: policy.quietHelp,
            identifier: "overlay-agents-hover-moon-\(row.digit)"
          ) { $0.moon.toggle() }
        }
        unreadDot
      }
    }
    .frame(height: OverlayAgentsHoverList.rowHeight)
    .accessibilityElement(children: .contain)
    .accessibilityLabel(Text(verbatim: accessibilitySummary))
    .accessibilityIdentifier("overlay-agents-hover-row-\(row.digit)")
  }

  private var pill: some View {
    // The presence dot sits between the digit and the name, as on the Bus
    // window's roster rows (Founder, 2026-11-04): `nr · dot · name`. The
    // inbox row has no presence: the envelope with its name, as the
    // sidebar's card. It fills its row, so its unread dot reaches the right
    // edge one gap after the pill.
    OverlayChannelPill(
      form: .chip,
      display: OverlayChannelPill.Display(
        figure: row.kind == .inbox ? "" : row.digit,
        symbol: row.kind == .inbox ? "envelope" : nil, name: row.name,
        dot: row.mailbox ? nil : row.dot, disconnected: row.disconnected,
        microphoneOpen: row.microphoneOpen, enabled: row.conversationID != nil),
      hint: "", accessibilityText: String(localized: "Open conversation"),
      identifier: "overlay-agents-hover-open-\(row.digit)", palette: palette,
      textScale: 1, showsHint: false,
      action: { if let id = row.conversationID { onOpen(id) } }
    )
    .frame(maxWidth: row.mailbox ? .infinity : nil, alignment: .leading)
  }

  /// New replies in the text tone: amber means warning, violet your voice.
  private var unreadDot: some View {
    Circle()
      .fill(palette.primaryText.color)
      .frame(
        width: OverlayAgentsHoverList.unreadDiameter,
        height: OverlayAgentsHoverList.unreadDiameter
      )
      .opacity(row.unread > 0 ? 1 : 0)
      .accessibilityHidden(true)
  }

  /// One sentence with every state the dots and marks show; a mailbox row
  /// names itself and its unread count only.
  private var accessibilitySummary: String {
    var parts = [row.digit, row.name]
    if !row.mailbox {
      parts.append(row.status)
      parts.append(
        policy.member
          ? String(localized: "in channel 0", comment: "Agents list row: member of channel 0")
          : String(localized: "off channel 0", comment: "Agents list row: not on channel 0"))
      if policy.moon { parts.append(String(localized: "Do not disturb")) }
    }
    if row.unread > 0 { parts.append(String(localized: "\(row.unread) unread replies")) }
    return parts.joined(separator: " · ")
  }


  /// Held while any surface's change for this agent is in flight; the next
  /// paint comes from the read-back receipt, not a local flag.
  private func mark(
    symbol: String, lit: Bool, litColor: Color, unlitOpacity: Double = 1,
    label: String, identifier: String,
    edit: @escaping (inout AgentBroadcastPolicy) -> Void
  ) -> some View {
    Button {
      apply(edit)
    } label: {
      Image(systemName: symbol)
        .font(.system(size: 10))
        .frame(width: OverlayAgentsHoverList.markSize, height: OverlayAgentsHoverList.markSize)
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .conversationActionHover(
      restingColor: lit ? litColor : palette.mutedText.color.opacity(unlitOpacity),
      hoverColor: palette.primaryText.color
    )
    .disabled(pending || row.identity == nil)
    .accessibilityLabel(Text(verbatim: label))
    .accessibilityIdentifier(identifier)
  }

  private func apply(_ edit: @escaping (inout AgentBroadcastPolicy) -> Void) {
    guard let identity = row.identity, !pending else { return }
    let polled = policy
    Task { @MainActor in
      // A refused command leaves the mark as the receipt says; the pill menu
      // is the surface that names the refusal.
      try? await OverlayBroadcastPolicyLedger.shared.change(identity, polled: polled, edit)
    }
  }
}
