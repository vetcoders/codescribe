import AppKit
import SwiftUI

/// One agent of the list under ✦, as plain values. Every field comes from a
/// projection `OverlayChannelStatusView` already owns; the list computes no
/// status of its own. The reception axes are read live from
/// `OverlayBroadcastPolicyLedger` when the row draws.
struct OverlayAgentsHoverRow: Equatable, Identifiable {
  let digit: String
  let name: String
  /// The channel's current conversation; nil leaves the row unopenable.
  let conversationID: String?
  /// `OverlayChannelStatusView.statusDotKind(for:)`.
  let dot: OverlayAgentDotKind
  /// `OverlayChannelStatusView.shortStatus(for:)`, read by VoiceOver.
  let status: String
  let microphoneOpen: Bool
  let disconnected: Bool
  let identity: AgentPlaybackIdentity?
  /// The roster's (fail-open) policy; `policyKnown` says whether a receipt
  /// stands behind it.
  let policy: AgentBroadcastPolicy
  let unread: Int

  var id: String { digit }
}

extension OverlayChannelStatusView {
  /// Agents 1–9 in ascending channel order; channel 0 has no row.
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
          dot: statusDotKind(for: channel), status: shortStatus(for: channel),
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
/// With no agents, or while agent capture is open and a click stops it, the
/// button keeps its one-line hint instead.
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

/// The agents under ✦: one low row per agent, no header or cards. Columns:
/// numbered pill with the name (opens the conversation), status dot, antenna
/// (channel 0), moon (do not disturb), amber unread dot. The antenna and the
/// moon are sibling buttons, never nested in the pill, and switch reception
/// through `OverlayBroadcastPolicyLedger` without opening or closing anything.
/// No hint opens from inside the list: the shared hint panel is the list.
struct OverlayAgentsHoverList: View {
  let model: OverlayAgentsHoverListModel
  let palette: OverlayAppearancePalette

  static let rowHeight: CGFloat = OverlayChannelPill.headerHeight
  static let rowSpacing: CGFloat = 2
  static let horizontalPadding: CGFloat = 8
  static let verticalPadding: CGFloat = 4
  static let columnGap: CGFloat = 6
  static let dotColumn: CGFloat = 10
  static let markSize: CGFloat = 18
  static let markGap: CGFloat = 2
  static let unreadDiameter: CGFloat = 6
  static let minimumWidth: CGFloat = 140

  /// Sized to its rows: nine agents fit without a scroll view.
  @MainActor
  static func size(rows: [OverlayAgentsHoverRow]) -> CGSize {
    // Full names: the list is sized to its content, never truncated.
    let chip =
      rows.map {
        OverlayChannelPill.chipWidth(
          nameWidth: OverlayChannelPill.nameWidth($0.name, scale: 1), form: .chip, scale: 1)
      }.max() ?? 0
    let width =
      2 * horizontalPadding + chip + 4 * columnGap + dotColumn + 2 * markSize + markGap
      + unreadDiameter
    let count = CGFloat(rows.count)
    return CGSize(
      width: max(minimumWidth, ceil(width)),
      height: 2 * verticalPadding + count * rowHeight + max(0, count - 1) * rowSpacing)
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

  private var known: Bool { ledger.policyKnown(row.identity) }
  private var pending: Bool { ledger.isPending(row.identity) }

  var body: some View {
    HStack(spacing: OverlayAgentsHoverList.columnGap) {
      OverlayChannelPill(
        form: .chip,
        display: OverlayChannelPill.Display(
          figure: row.digit, name: row.name, disconnected: row.disconnected,
          microphoneOpen: row.microphoneOpen, enabled: row.conversationID != nil),
        hint: "", accessibilityText: String(localized: "Open conversation"),
        identifier: "overlay-agents-hover-open-\(row.digit)", palette: palette,
        textScale: 1, showsHint: false,
        action: { if let id = row.conversationID { onOpen(id) } })
      Spacer(minLength: 0)
      OverlayAgentStatusDot(kind: row.dot, palette: palette)
        .frame(width: OverlayAgentsHoverList.dotColumn)
        .accessibilityHidden(true)
      HStack(spacing: OverlayAgentsHoverList.markGap) {
        if known {
          mark(
            symbol: policy.statusSymbol, lit: policy.member, litColor: palette.agentAccent.color,
            label: policy.statusHelp, identifier: "overlay-agents-hover-antenna-\(row.digit)"
          ) { $0.member.toggle() }
          // The moon speaks through its shape, as on the roster's marks.
          mark(
            symbol: policy.quietSymbol, lit: policy.moon, litColor: palette.primaryText.color,
            unlitOpacity: 0.45, label: policy.quietHelp,
            identifier: "overlay-agents-hover-moon-\(row.digit)"
          ) { $0.moon.toggle() }
        } else {
          unknownMark(
            symbol: "antenna.radiowaves.left.and.right",
            identifier: "overlay-agents-hover-antenna-\(row.digit)")
          unknownMark(symbol: "moon", identifier: "overlay-agents-hover-moon-\(row.digit)")
        }
      }
      Circle()
        .fill(palette.processingStatus.color)
        .frame(
          width: OverlayAgentsHoverList.unreadDiameter,
          height: OverlayAgentsHoverList.unreadDiameter
        )
        .opacity(row.unread > 0 ? 1 : 0)
        .accessibilityHidden(true)
    }
    .frame(height: OverlayAgentsHoverList.rowHeight)
    .accessibilityElement(children: .contain)
    .accessibilityLabel(Text(verbatim: accessibilitySummary))
    .accessibilityIdentifier("overlay-agents-hover-row-\(row.digit)")
  }

  /// One sentence with every state the dots and marks show.
  private var accessibilitySummary: String {
    var parts = [row.digit, row.name, row.status]
    if known {
      parts.append(
        policy.member
          ? String(localized: "in channel 0", comment: "Agents list row: member of channel 0")
          : String(localized: "off channel 0", comment: "Agents list row: not on channel 0"))
      if policy.moon { parts.append(String(localized: "Do not disturb")) }
    } else {
      parts.append(Self.unknownText)
    }
    if row.unread > 0 { parts.append(String(localized: "\(row.unread) unread replies")) }
    return parts.joined(separator: " · ")
  }

  static var unknownText: String {
    String(
      localized: "Reception policy unknown",
      comment: "Agents list: no reception receipt has been read for this agent yet")
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
      hoverColor: palette.primaryText.color)
    .disabled(pending || row.identity == nil)
    .accessibilityLabel(Text(verbatim: label))
    .accessibilityIdentifier(identifier)
  }

  /// No receipt: a grey outline that switches nothing.
  private func unknownMark(symbol: String, identifier: String) -> some View {
    Image(systemName: symbol)
      .font(.system(size: 10))
      .foregroundStyle(palette.mutedText.color.opacity(0.45))
      .frame(width: OverlayAgentsHoverList.markSize, height: OverlayAgentsHoverList.markSize)
      .help(Text(verbatim: Self.unknownText))
      .accessibilityLabel(Text(verbatim: Self.unknownText))
      .accessibilityIdentifier(identifier)
  }

  private func apply(_ edit: @escaping (inout AgentBroadcastPolicy) -> Void) {
    guard let identity = row.identity, known, !pending else { return }
    let polled = policy
    Task { @MainActor in
      // A refused command leaves the mark as the receipt says; the pill menu
      // is the surface that names the refusal.
      try? await OverlayBroadcastPolicyLedger.shared.change(identity, polled: polled, edit)
    }
  }
}
