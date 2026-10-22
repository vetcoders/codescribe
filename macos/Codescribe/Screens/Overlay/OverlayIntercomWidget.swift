import AppKit
import SwiftUI

/// AppKit owns placement only. The header and each pill observe the existing
/// OverlayState directly; no roster reader, recorder or delivery owner lives here.
@MainActor
final class OverlayIntercomWidgetController: NSObject, NSWindowDelegate {
  private static let positionKey = "codescribe.overlay.intercom.vertical.origin"
  private let state: OverlayState
  private let textScale: TextScaleController
  private let revealConversation: () -> Void
  private var header: OverlayIntercomPanel?
  private var rows: [String: OverlayIntercomPanel] = [:]
  private var digits: [String] = []
  private var preview: OverlayIntercomPanel?
  private var previewChoice = OverlayIntercomPreviewChoice.hidden
  private var previewDigit: String? { previewChoice.digit }
  private var previewDismissal: Task<Void, Never>?
  private var visible = false
  private var positioning = false
  private var invalidated = false

  init(
    state: OverlayState, textScale: TextScaleController,
    revealConversation: @escaping () -> Void = {}
  ) {
    self.state = state
    self.textScale = textScale
    self.revealConversation = revealConversation
    super.init()
  }

  func setVisible(_ visible: Bool, relativeTo anchor: NSWindow?) {
    guard !invalidated else { return }
    self.visible = visible
    guard visible else {
      dismissPreview()
      for row in rows.values {
        row.contentView = nil
        row.close()
      }
      rows.removeAll()
      header?.orderOut(nil)
      return
    }
    if header == nil {
      let panel = OverlayIntercomPanel()
      panel.delegate = self
      panel.isMovableByWindowBackground = true
      panel.contentView = NSHostingView(
        rootView: TextScaleRoot(controller: textScale) {
          OverlayIntercomHeader(state: state, controller: self)
        })
      let fallback = anchor?.frame ?? NSScreen.main?.visibleFrame ?? .zero
      let stored = UserDefaults.standard.array(forKey: Self.positionKey) as? [Double]
      let origin: CGPoint
      if let stored, stored.count == 2, stored.allSatisfy(\.isFinite) {
        origin = CGPoint(x: stored[0], y: stored[1])
      } else {
        origin = CGPoint(x: fallback.maxX + 16, y: fallback.maxY - 30)
      }
      panel.setFrame(CGRect(origin: origin, size: CGSize(width: 280, height: 30)), display: false)
      header = panel
    }
    // The SwiftUI representable also repeats this when the observed roster or
    // text scale changes; showing never waits for another history poll.
    reconcile(chips: state.channelMapLayoutModel.agents, scale: textScale.scale)
    header?.orderFrontRegardless()
  }

  func invalidate() {
    guard !invalidated else { return }
    setVisible(false, relativeTo: nil)
    invalidated = true
    header?.delegate = nil
    header?.contentView = nil
    for row in rows.values {
      row.contentView = nil
      row.close()
    }
    rows.removeAll()
    header?.close()
    header = nil
  }

  fileprivate func reconcile(chips: [OverlayChannelMapChip], scale: CGFloat) {
    guard !invalidated, visible, let header else { return }
    let nextDigits = chips.map(\.digit)
    for digit in Set(rows.keys).subtracting(nextDigits) {
      rows.removeValue(forKey: digit)?.close()
      if previewDigit == digit { dismissPreview() }
    }
    digits = nextDigits
    let font = NSFont.systemFont(ofSize: 13 * min(scale, 1.3), weight: .semibold)
    let longest =
      chips.map { ($0.name as NSString).size(withAttributes: [.font: font]).width }.max() ?? 120
    let width = min(420, max(250, longest + 140))
    var frame = header.frame
    frame.size.width = width
    positioning = true
    let fitted = OverlayIntercomWidgetLayout.clampedHeader(
      frame, count: digits.count, visibleFrame: screenFrame)
    if header.frame != fitted { header.setFrame(fitted, display: true) }
    positioning = false
    for (index, digit) in digits.enumerated() where rows[digit] == nil {
      let panel = OverlayIntercomPanel()
      panel.contentView = NSHostingView(
        rootView: TextScaleRoot(controller: textScale) {
          OverlayIntercomRow(
            state: state, digit: digit, index: index,
            onPreview: { [weak self] in self?.togglePreview(digit: digit) },
            onHover: { [weak self] inside in self?.hover(digit: digit, inside: inside) })
        })
      rows[digit] = panel
    }
    positionRows()
    if visible {
      for row in rows.values { row.orderFrontRegardless() }
    }
  }

  private var screenFrame: CGRect {
    let frame = header?.frame ?? .zero
    return NSScreen.screens.max(by: {
      $0.visibleFrame.intersection(frame).width * $0.visibleFrame.intersection(frame).height
        < $1.visibleFrame.intersection(frame).width * $1.visibleFrame.intersection(frame).height
    })?.visibleFrame ?? NSScreen.main?.visibleFrame ?? CGRect(x: 0, y: 0, width: 1440, height: 900)
  }

  private func positionRows() {
    guard let header else { return }
    let screen = screenFrame
    for (index, digit) in digits.enumerated() {
      let frame = OverlayIntercomWidgetLayout.rowFrame(
        index: index, header: header.frame, count: digits.count, visibleFrame: screen)
      if let row = rows[digit], row.frame != frame { row.setFrame(frame, display: true) }
    }
    if let previewDigit, let row = rows[previewDigit] {
      let frame = OverlayIntercomWidgetLayout.previewFrame(beside: row.frame, visibleFrame: screen)
      if let preview, preview.frame != frame { preview.setFrame(frame, display: true) }
    }
  }

  func windowDidMove(_ notification: Notification) {
    guard !positioning, let header else { return }
    positioning = true
    header.setFrame(
      OverlayIntercomWidgetLayout.clampedHeader(
        header.frame, count: digits.count, visibleFrame: screenFrame), display: true)
    positioning = false
    positionRows()
    UserDefaults.standard.set(
      [Double(header.frame.minX), Double(header.frame.minY)], forKey: Self.positionKey)
  }

  private func openConversation(_ id: String) {
    state.chooseConversation(id)
    revealConversation()
  }

  private func togglePreview(digit: String) {
    presentPreview(previewChoice.clicking(digit))
  }

  fileprivate func hover(digit: String, inside: Bool) {
    if !inside, previewDigit != digit { return }
    previewDismissal?.cancel()
    previewDismissal = nil
    if inside {
      presentPreview(previewChoice.entering(digit))
    } else if !previewChoice.isPinned {
      // A short grace permits crossing the real desktop gap into the preview.
      previewDismissal = Task { [weak self] in
        do { try await Task.sleep(for: .milliseconds(180)) } catch { return }
        guard let self else { return }
        self.presentPreview(self.previewChoice.leaving(digit))
      }
    }
  }

  private func presentPreview(_ choice: OverlayIntercomPreviewChoice) {
    previewDismissal?.cancel()
    previewDismissal = nil
    guard let digit = choice.digit else {
      dismissPreview()
      return
    }
    guard visible, let row = rows[digit] else { return }
    if previewDigit == digit, preview != nil {
      previewChoice = choice
      return
    }
    dismissPreview()
    let panel = OverlayIntercomPanel()
    panel.contentView = NSHostingView(
      rootView: TextScaleRoot(controller: textScale) {
        OverlayIntercomPreview(
          state: state, digit: digit,
          onOpenConversation: { [weak self] id in self?.openConversation(id) },
          onClose: { [weak self] in self?.dismissPreview() },
          onHover: { [weak self] inside in self?.hover(digit: digit, inside: inside) })
      })
    panel.setFrame(
      OverlayIntercomWidgetLayout.previewFrame(beside: row.frame, visibleFrame: screenFrame),
      display: true)
    preview = panel
    previewChoice = choice
    panel.orderFrontRegardless()
  }

  private func dismissPreview() {
    previewDismissal?.cancel()
    previewDismissal = nil
    preview?.orderOut(nil)
    preview?.contentView = nil
    preview?.close()
    preview = nil
    previewChoice = .hidden
  }
}

/// A window only as large as its visible capsule. Empty spaces between the
/// windows are genuinely desktop space, including when another app is active.
@MainActor
private final class OverlayIntercomPanel: NSPanel {
  init() {
    super.init(
      contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
      backing: .buffered, defer: false)
    isReleasedWhenClosed = false
    isOpaque = false
    backgroundColor = .clear
    hasShadow = true
    level = .floating
    collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
    hidesOnDeactivate = false
    becomesKeyOnlyIfNeeded = true
    animationBehavior = .none
  }

  override var canBecomeKey: Bool { false }
  override var canBecomeMain: Bool { false }
}

private struct OverlayIntercomHeader: View {
  let state: OverlayState
  weak var controller: OverlayIntercomWidgetController?

  var body: some View {
    let chips = state.channelMapLayoutModel.agents
    HStack(spacing: 8) {
      Image(systemName: "circle.grid.2x2.fill").font(.system(size: 10))
      Text("Intercom").csFont(11, .semibold)
      Spacer(minLength: 4)
      Text(chips.count.formatted()).csMono(10)
      Image(systemName: "hand.draw").font(.system(size: 10))
    }
    .foregroundStyle(.secondary)
    .padding(.horizontal, 12)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background(.regularMaterial, in: Capsule())
    .background(OverlayIntercomWindowProjection(chips: chips, controller: controller))
    .overlay(OverlayIntercomDragHandle())
    .help(String(localized: "Drag to move"))
    .accessibilityIdentifier("overlay-intercom-drag-handle")
  }
}

/// SwiftUI observation drives native placement. This is not an imperative
/// observation loop and never installs a second state callback on OverlayState.
private struct OverlayIntercomWindowProjection: NSViewRepresentable {
  @Environment(\.csTextScale) private var scale
  let chips: [OverlayChannelMapChip]
  weak var controller: OverlayIntercomWidgetController?

  func makeNSView(context: Context) -> NSView { NSView() }
  func updateNSView(_ nsView: NSView, context: Context) {
    controller?.reconcile(chips: chips, scale: scale)
  }
}

/// No controls live in the header, so its complete visible capsule is the
/// explicit drag handle. Dragging does not acquire keyboard focus.
private struct OverlayIntercomDragHandle: NSViewRepresentable {
  func makeNSView(context: Context) -> OverlayIntercomDragView { OverlayIntercomDragView() }
  func updateNSView(_ nsView: OverlayIntercomDragView, context: Context) {}
}

private final class OverlayIntercomDragView: NSView {
  override var isOpaque: Bool { false }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  override func mouseDown(with event: NSEvent) { window?.performDrag(with: event) }
  override func resetCursorRects() { addCursorRect(bounds, cursor: .openHand) }
}

private struct OverlayIntercomRow: View {
  @Environment(\.colorScheme) private var colorScheme
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @State private var entered = false
  let state: OverlayState
  let digit: String
  let index: Int
  let onPreview: () -> Void
  let onHover: (Bool) -> Void

  private var chip: OverlayChannelMapChip? {
    state.channelMapLayoutModel.agents.first { $0.digit == digit }
  }

  private var conversation: OverlayConversation? {
    intercomConversation(state: state, digit: digit)
  }

  var body: some View {
    let palette = OverlayAppearancePalette.resolve(colorScheme)
    let open = state.channelHudStates[digit]?.open == true
    let disconnected = state.channelHudStates[digit]?.followerAlive == false
    let reachable =
      !state.channelStatusUnavailable && state.channelHudStates[digit]?.followerAlive == true
    HStack(spacing: 3) {
      Button(action: onPreview) {
        HStack(spacing: 7) {
          Circle()
            .fill(reachable ? palette.successStatus.color : palette.mutedText.color)
            .frame(width: 7, height: 7)
            .accessibilityLabel(
              reachable
                ? String(localized: "Ready")
                : (disconnected
                  ? String(localized: "Disconnected") : String(localized: "Status unavailable")))
          Text(digit).csMono(10).foregroundStyle(palette.mutedText.color)
          Text(chip?.name ?? digit).csFont(13, .semibold).lineLimit(1)
            .minimumScaleFactor(0.8)
          Spacer(minLength: 3)
          if let conversation, state.unreadReplies(in: conversation) > 0 {
            Circle().fill(palette.agentAccent.color).frame(width: 6, height: 6)
          }
        }
        .padding(.leading, 13)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .contentShape(Capsule())
      }
      .buttonStyle(.plain)
      .accessibilityLabel(String(localized: "Preview message") + ": " + (chip?.name ?? digit))
      .accessibilityIdentifier("overlay-intercom-name-\(digit)")

      Button {
        guard let number = UInt8(digit) else { return }
        Task { await state.toggleAgentChannel(number, keepsAgentList: true) }
      } label: {
        Image(systemName: open ? "mic.fill" : "mic")
          .font(.system(size: 13, weight: .semibold))
          .symbolEffect(.pulse, options: .repeating, isActive: open && !reduceMotion)
          .frame(width: 32, height: 32)
          .background(open ? palette.agentAccent.color.opacity(0.18) : Color.clear, in: Circle())
          .contentShape(Circle())
      }
      .buttonStyle(.plain)
      .conversationActionHover(
        restingColor: palette.agentAccent.color.opacity(0.8), hoverColor: palette.agentAccent.color
      )
      .disabled(
        state.pendingChannelToggles.contains(digit) || state.channelStatusUnavailable
          || (!open && disconnected)
      )
      .accessibilityLabel(
        (open ? String(localized: "Stop recording") : String(localized: "Start recording"))
          + ": " + (chip?.name ?? digit)
      )
      .accessibilityValue(
        open ? String(localized: "Microphone open") : String(localized: "Microphone closed")
      )
      .accessibilityIdentifier("overlay-intercom-microphone-\(digit)")
      .padding(.trailing, 5)
    }
    .foregroundStyle(palette.primaryText.color)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background {
      OverlayCanvasBackdrop(palette: palette, reduceTransparency: reduceTransparency)
        .clipShape(Capsule())
    }
    .overlay {
      Capsule().strokeBorder(
        open ? palette.agentAccent.color.opacity(0.7) : palette.border.color, lineWidth: 1
      )
      .allowsHitTesting(false)
    }
    .opacity(entered ? 1 : 0)
    .offset(y: entered || reduceMotion ? 0 : -6)
    .scaleEffect(entered || reduceMotion ? 1 : 0.98)
    .onAppear {
      withAnimation(
        reduceMotion
          ? nil : .spring(response: 0.28, dampingFraction: 0.86).delay(Double(index) * 0.025)
      ) {
        entered = true
      }
    }
    .onHover(perform: onHover)
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-intercom-row-\(digit)")
  }
}

private struct OverlayIntercomPreview: View {
  @Environment(\.colorScheme) private var colorScheme
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  let state: OverlayState
  let digit: String
  let onOpenConversation: (String) -> Void
  let onClose: () -> Void
  let onHover: (Bool) -> Void

  var body: some View {
    let palette = OverlayAppearancePalette.resolve(colorScheme)
    let conversation = intercomConversation(state: state, digit: digit)
    let message = conversation?.messages.last
    VStack(alignment: .leading, spacing: 10) {
      HStack {
        Text(conversation?.name ?? digit).csFont(12, .semibold)
        Spacer(minLength: 8)
        Button("Close", systemImage: "xmark", action: onClose)
          .labelStyle(.iconOnly)
          .buttonStyle(.plain)
          .conversationActionHover(
            restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
          )
          .accessibilityIdentifier("overlay-intercom-preview-close")
      }
      if let message {
        Text(OverlayChannelStatusView.messageRoute(of: message).label).csFont(11, .semibold)
          .foregroundStyle(palette.agentAccent.color).lineLimit(2)
        Text(String(message.bodyText.prefix(640)))
          .csFont(12).foregroundStyle(palette.bodyText.color)
          .lineLimit(4).frame(maxWidth: .infinity, alignment: .leading)
      } else {
        Text("No messages yet").csFont(12).foregroundStyle(palette.mutedText.color)
      }
      Spacer(minLength: 0)
      HStack(spacing: 12) {
        if let conversation {
          Button("Open conversation") { onOpenConversation(conversation.id) }
            .buttonStyle(.plain).csFont(11, .semibold)
        }
        Spacer(minLength: 0)
        if let muted = state.channelPlaybackMuted[digit] {
          Button {
            Task { await state.toggleChannelPlayback(digit) }
          } label: {
            Image(systemName: muted ? "speaker.slash" : "speaker.wave.2")
              .frame(width: 26, height: 26)
          }
          .buttonStyle(.plain)
          .disabled(state.pendingPlaybackChannels.contains(digit))
          .help(muted ? String(localized: "Unmute") : String(localized: "Mute"))
          .accessibilityLabel(muted ? String(localized: "Unmute") : String(localized: "Mute"))
          .accessibilityIdentifier("overlay-intercom-playback-\(digit)")
        }
        if let owner = state.archiveCandidate(for: digit), state.canFocusAgentPanel(owner) {
          Button {
            state.focusAgentPanel(owner)
          } label: {
            Image(systemName: "arrow.up.forward.app").frame(width: 26, height: 26)
          }
          .buttonStyle(.plain)
          .disabled(state.agentPanelFocusStates[digit]?.isRequesting == true)
          .help(state.agentPanelFocusHelp(owner) ?? String(localized: "Open agent"))
          .accessibilityLabel(state.agentPanelFocusHelp(owner) ?? String(localized: "Open agent"))
          .accessibilityIdentifier("overlay-intercom-focus-\(digit)")
        }
      }
    }
    .padding(14)
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    .background(OverlayCanvasBackdrop(palette: palette, reduceTransparency: reduceTransparency))
    .clipShape(RoundedRectangle(cornerRadius: 16))
    .overlay {
      RoundedRectangle(cornerRadius: 16).strokeBorder(palette.border.color, lineWidth: 1)
        .allowsHitTesting(false)
    }
    .onHover(perform: onHover)
    .accessibilityIdentifier("overlay-intercom-preview-\(digit)")
  }

}

@MainActor
private func intercomConversation(state: OverlayState, digit: String) -> OverlayConversation? {
  OverlayChannelStatusView.currentConversations(
    conversations: state.conversations, archivedOwners: state.archivedAgentOwners,
    hudStates: state.channelHudStates
  ).first { $0.channel == digit }
}
