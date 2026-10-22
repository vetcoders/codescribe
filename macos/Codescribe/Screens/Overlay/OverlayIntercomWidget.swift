import AppKit
import Observation
import SwiftUI

/// The existing Intercom window owner handles placement and presentation only.
/// Conversations, drafts, attachments, capture and commands remain in OverlayState.
@MainActor
@Observable
final class OverlayIntercomWidgetController: NSObject, NSWindowDelegate {
  private static let positionKey = "codescribe.overlay.intercom.vertical.origin"
  @ObservationIgnored private let state: OverlayState
  @ObservationIgnored private let textScale: TextScaleController
  @ObservationIgnored private let revealConversation: () -> Void
  @ObservationIgnored private var header: OverlayIntercomPanel?
  @ObservationIgnored private var cards: [OverlayIntercomCardIdentity: OverlayIntercomPanel] = [:]
  @ObservationIgnored private var identities: [OverlayIntercomCardIdentity] = []
  @ObservationIgnored private var sizes: [OverlayIntercomCardIdentity: CGSize] = [:]
  @ObservationIgnored private var visible = false
  @ObservationIgnored private var positioning = false
  @ObservationIgnored private var invalidated = false
  fileprivate private(set) var rowOffset = 0
  fileprivate private(set) var availableHeight: CGFloat = 900
  fileprivate private(set) var presentation = OverlayIntercomPresentation()
  fileprivate private(set) var focusRevisions: [OverlayIntercomCardIdentity: UInt64] = [:]

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
      // Reuse the same windows when the layout returns. Durable drafts never
      // belong to these views, and hiding never changes the capture route.
      presentation.hovered = nil
      header?.orderOut(nil)
      for panel in cards.values { panel.setPresented(false, animated: false) }
      return
    }
    if header == nil {
      let panel = OverlayIntercomPanel()
      panel.delegate = self
      panel.onScroll = { [weak self] delta in self?.scrollGroup(delta > 0 ? -1 : 1) }
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
      panel.setFrame(CGRect(origin: origin, size: CGSize(width: 160, height: 30)), display: false)
      header = panel
    }
    reconcile(chips: state.channelMapLayoutModel.agents, scale: textScale.scale)
    header?.orderFrontRegardless()
  }

  func invalidate() {
    guard !invalidated else { return }
    setVisible(false, relativeTo: nil)
    invalidated = true
    for panel in cards.values {
      panel.delegate = nil
      panel.contentView = nil
      panel.close()
    }
    cards.removeAll()
    header?.delegate = nil
    header?.contentView = nil
    header?.close()
    header = nil
  }

  fileprivate func reconcile(chips: [OverlayChannelMapChip], scale: CGFloat) {
    guard !invalidated, visible, let header else { return }
    if availableHeight != screenFrame.height { availableHeight = screenFrame.height }
    identities = chips.map {
      OverlayIntercomCardIdentity(
        digit: $0.digit, conversation: intercomConversation(state: state, digit: $0.digit),
        name: $0.name)
    }
    // Channel reuse cannot replace the editor underneath a draft. The old
    // exact owner keeps its existing card, like an explicitly detached one.
    for identity in presentation.expanded where !identities.contains(identity) {
      presentation.detach(identity)
    }
    let retained = Set(identities).union(presentation.expanded)
    for identity in Set(cards.keys).subtracting(retained) {
      let panel = cards.removeValue(forKey: identity)
      panel?.delegate = nil
      panel?.contentView = nil
      panel?.close()
      sizes.removeValue(forKey: identity)
      focusRevisions.removeValue(forKey: identity)
      presentation.leave(identity)
    }
    let nameFont = NSFont.systemFont(ofSize: 11 * scale, weight: .semibold)
    let digitFont = CSFont.nsMono(10 * scale)
    let longest =
      chips.map {
        ($0.name as NSString).size(withAttributes: [.font: nameFont]).width
          + ($0.digit as NSString).size(withAttributes: [.font: digitFont]).width
      }.max() ?? 0
    let titleWidth =
      (String(localized: "Intercom") as NSString).size(withAttributes: [.font: nameFont]).width
      + (chips.count.formatted() as NSString).size(withAttributes: [.font: digitFont]).width
      + (hasOverflow && !presentation.folded ? 32 * max(1, scale) + 16 : 0)
    let width = OverlayIntercomWidgetLayout.restingWidth(
      headerText: titleWidth, agentText: longest, scale: scale)
    let frame = CGRect(
      x: header.frame.maxX - width, y: header.frame.minY,
      width: width, height: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
    positioning = true
    let fitted = OverlayIntercomWidgetLayout.clampedHeader(
      frame, count: presentation.folded ? 0 : visibleRows.count,
      visibleFrame: screenFrame, scale: scale)
    if header.frame != fitted { header.setFrame(fitted, display: true) }
    positioning = false
    for (index, identity) in identities.enumerated() where cards[identity] == nil {
      let panel = OverlayIntercomPanel()
      panel.delegate = self
      panel.onScroll = { [weak self] delta in self?.scrollGroup(delta > 0 ? -1 : 1) }
      panel.contentView = NSHostingView(
        rootView: TextScaleRoot(controller: textScale) {
          OverlayIntercomCard(
            state: state, identity: identity, index: index, controller: self,
            onRevealConversation: revealConversation)
        })
      cards[identity] = panel
    }
    positionCards()
  }

  private var screenFrame: CGRect { visibleFrame(for: header) }

  private var groupIdentities: [OverlayIntercomCardIdentity] {
    identities.filter { !presentation.detached.contains($0) }
  }

  fileprivate var visibleRows: Range<Int> {
    OverlayIntercomWidgetLayout.visibleRange(
      count: groupIdentities.count, offset: rowOffset, visibleHeight: availableHeight,
      scale: textScale.scale)
  }

  fileprivate var hasOverflow: Bool { visibleRows.count < groupIdentities.count }
  fileprivate var hasPrevious: Bool { visibleRows.lowerBound > 0 }
  fileprivate var hasNext: Bool { visibleRows.upperBound < groupIdentities.count }

  func presentsConversation(_ conversationID: String?) -> Bool {
    guard visible, let conversationID else { return false }
    return presentation.expanded.contains { identity in
      identity.conversationID == conversationID && cards[identity]?.isPresented == true
        && state.conversations.contains(where: { identity.matches($0) })
    }
  }

  fileprivate func scrollGroup(_ steps: Int) {
    guard visible, !presentation.folded else { return }
    rowOffset = min(
      max(0, visibleRows.lowerBound + steps), max(0, groupIdentities.count - visibleRows.count))
    presentation.hovered = nil
    positionCards()
  }

  private func visibleFrame(for panel: NSWindow?) -> CGRect {
    let frame = panel?.frame ?? .zero
    return NSScreen.screens.max(by: {
      $0.visibleFrame.intersection(frame).width * $0.visibleFrame.intersection(frame).height
        < $1.visibleFrame.intersection(frame).width * $1.visibleFrame.intersection(frame).height
    })?.visibleFrame ?? NSScreen.main?.visibleFrame ?? CGRect(x: 0, y: 0, width: 1440, height: 900)
  }

  private func positionCards() {
    guard visible, let header else { return }
    positioning = true
    defer { positioning = false }
    let range = visibleRows
    let group = groupIdentities
    for (identity, panel) in cards {
      let expanded = presentation.expanded.contains(identity)
      panel.acceptsConversationFocus = expanded
      if presentation.detached.contains(identity) {
        let fitted = OverlayIntercomWidgetLayout.conversationFrame(
          below: panel.frame, size: panel.frame.size, visibleFrame: visibleFrame(for: panel),
          scale: textScale.scale)
        if panel.frame != fitted { panel.setFrame(fitted, display: true) }
        panel.setPresented(
          true, animated: !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
        continue
      }
      guard !presentation.folded, let index = group.firstIndex(of: identity), range.contains(index)
      else {
        panel.setPresented(
          false, animated: !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
        continue
      }
      let row = OverlayIntercomWidgetLayout.rowFrame(
        index: index - range.lowerBound, header: header.frame, count: range.count,
        visibleFrame: screenFrame, scale: textScale.scale)
      let frame: CGRect
      if expanded {
        frame = OverlayIntercomWidgetLayout.conversationFrame(
          below: row,
          size: sizes[identity]
            ?? OverlayIntercomWidgetLayout.conversationSize(scale: textScale.scale),
          visibleFrame: screenFrame, scale: textScale.scale)
      } else if presentation.hovered == identity {
        frame = OverlayIntercomWidgetLayout.previewFrame(
          beside: row, visibleFrame: screenFrame, scale: textScale.scale)
      } else {
        frame = row
      }
      if panel.frame != frame {
        panel.setFrame(
          frame, display: true,
          animate: panel.isVisible && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
      }
      panel.setPresented(true, animated: !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
    }
    // Geometry and hover leave the order of presented windows alone. Only an
    // explicit expand below (or a native click/focus) raises a conversation.
  }

  fileprivate func hover(_ identity: OverlayIntercomCardIdentity, inside: Bool) {
    guard visible, !presentation.folded else { return }
    if inside { presentation.enter(identity) } else { presentation.leave(identity) }
    positionCards()
  }

  fileprivate func expand(_ identity: OverlayIntercomCardIdentity) {
    guard let conversation = state.conversations.first(where: { identity.matches($0) }),
      conversation.owner != nil
    else { return }
    presentation.expand(identity)
    focusRevisions[identity, default: 0] &+= 1
    positionCards()
    // Only this explicit click grants keyability. Hover never activates the app.
    cards[identity]?.makeKeyAndOrderFront(nil)
  }

  fileprivate func close(_ identity: OverlayIntercomCardIdentity) {
    cards[identity]?.resignKey()
    presentation.close(identity)
    reconcile(chips: state.channelMapLayoutModel.agents, scale: textScale.scale)
  }

  fileprivate func toggleGroup() {
    presentation.folded.toggle()
    presentation.hovered = nil
    reconcile(chips: state.channelMapLayoutModel.agents, scale: textScale.scale)
  }

  fileprivate func detach(_ identity: OverlayIntercomCardIdentity) {
    presentation.detach(identity)
    positionCards()
  }

  fileprivate func resize(
    _ identity: OverlayIntercomCardIdentity, from frame: CGRect, delta: CGSize,
    edge: OverlayIntercomWidgetLayout.ResizeEdge
  ) {
    guard let panel = cards[identity], presentation.expanded.contains(identity) else { return }
    // Resizing is an explicit independent placement, just like dragging the grip.
    presentation.detach(identity)
    let fitted = OverlayIntercomWidgetLayout.resizedFrame(
      frame, delta: delta, edge: edge,
      minimum: OverlayIntercomWidgetLayout.minimumConversationSize(scale: textScale.scale),
      visibleFrame: visibleFrame(for: panel))
    sizes[identity] = fitted.size
    panel.setFrame(fitted, display: true)
    positionCards()
  }

  func windowDidMove(_ notification: Notification) {
    guard !positioning, let panel = notification.object as? NSWindow else { return }
    if panel === header, let header {
      if availableHeight != screenFrame.height { availableHeight = screenFrame.height }
      positioning = true
      header.setFrame(
        OverlayIntercomWidgetLayout.clampedHeader(
          header.frame, count: presentation.folded ? 0 : visibleRows.count,
          visibleFrame: screenFrame, scale: textScale.scale), display: true)
      positioning = false
      positionCards()
      UserDefaults.standard.set(
        [Double(header.frame.minX), Double(header.frame.minY)], forKey: Self.positionKey)
    } else if let identity = cards.first(where: { $0.value === panel })?.key,
      presentation.detached.contains(identity)
    {
      let fitted = OverlayIntercomWidgetLayout.cardFrame(
        below: panel.frame, size: panel.frame.size, visibleFrame: visibleFrame(for: panel))
      if panel.frame != fitted {
        positioning = true
        panel.setFrame(fitted, display: true)
        positioning = false
      }
    }
  }
}

/// No window spans the invisible desktop gaps. Only explicitly opened
/// conversations may become key; nonactivating hover remains passive.
@MainActor
private final class OverlayIntercomPanel: NSPanel {
  var acceptsConversationFocus = false
  var onScroll: (CGFloat) -> Void = { _ in }
  private var presented = false
  private var visibilityRevision: UInt64 = 0

  var isPresented: Bool { presented && isVisible }

  func setPresented(_ value: Bool, animated: Bool) {
    guard presented != value else { return }
    presented = value
    visibilityRevision &+= 1
    let revision = visibilityRevision
    guard animated else {
      alphaValue = 1
      if value { orderFrontRegardless() } else { orderOut(nil) }
      return
    }
    if value {
      alphaValue = 0
      orderFrontRegardless()
    }
    NSAnimationContext.runAnimationGroup { context in
      context.duration = 0.15
      animator().alphaValue = value ? 1 : 0
    } completionHandler: { [weak self] in
      Task { @MainActor [weak self] in
        guard let self, self.visibilityRevision == revision, !self.presented else { return }
        self.orderOut(nil)
        self.alphaValue = 1
      }
    }
  }

  override func scrollWheel(with event: NSEvent) {
    if acceptsConversationFocus {
      super.scrollWheel(with: event)
    } else if abs(event.scrollingDeltaY) > 0 {
      onScroll(event.scrollingDeltaY)
    }
  }

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

  override var canBecomeKey: Bool { acceptsConversationFocus }
  override var canBecomeMain: Bool { false }
}

private struct OverlayIntercomHeader: View {
  @Environment(\.colorScheme) private var colorScheme
  @Environment(\.csTextScale) private var scale
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  let state: OverlayState
  let controller: OverlayIntercomWidgetController

  var body: some View {
    let chips = state.channelMapLayoutModel.agents
    let palette = OverlayAppearancePalette.resolve(colorScheme)
    HStack(spacing: 8) {
      Button {
        controller.toggleGroup()
      } label: {
        HStack(spacing: 8) {
          Image(systemName: "circle.grid.2x2.fill").font(.system(size: 10 * scale))
          Text("Intercom").csFont(11, .semibold)
          Spacer(minLength: 4)
          Text(chips.count.formatted()).csMono(10)
        }
        .contentShape(Capsule())
      }
      .buttonStyle(.plain)
      .conversationActionHover(
        restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
      )
      .accessibilityLabel(
        controller.presentation.folded
          ? String(localized: "Expand Intercom") : String(localized: "Collapse Intercom"))
      if controller.hasOverflow && !controller.presentation.folded {
        Button("Previous agents", systemImage: "chevron.up") { controller.scrollGroup(-1) }
          .labelStyle(.iconOnly).buttonStyle(.plain).disabled(!controller.hasPrevious)
          .frame(width: 16 * max(1, scale))
        Button("Next agents", systemImage: "chevron.down") { controller.scrollGroup(1) }
          .labelStyle(.iconOnly).buttonStyle(.plain).disabled(!controller.hasNext)
          .frame(width: 16 * max(1, scale))
      }
      Image(systemName: "hand.draw").font(.system(size: 10 * scale))
        .frame(width: 16 * max(1, scale))
        .overlay(OverlayIntercomDragHandle())
        .accessibilityLabel("Drag to move")
    }
    .foregroundStyle(palette.mutedText.color)
    .padding(.horizontal, 12)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background {
      OverlayCanvasBackdrop(palette: palette, reduceTransparency: reduceTransparency)
        .clipShape(Capsule())
    }
    .background(
      OverlayIntercomWindowProjection(
        chips: chips,
        identities: chips.map {
          OverlayIntercomCardIdentity(
            digit: $0.digit, conversation: intercomConversation(state: state, digit: $0.digit),
            name: $0.name)
        }, controller: controller
      )
    )
    .accessibilityIdentifier("overlay-intercom-drag-handle")
  }
}

/// Native placement observes the existing projection, including owner changes
/// behind an unchanged digit/name. No callback, reader or poller is installed.
private struct OverlayIntercomWindowProjection: NSViewRepresentable {
  @Environment(\.csTextScale) private var scale
  let chips: [OverlayChannelMapChip]
  let identities: [OverlayIntercomCardIdentity]
  weak var controller: OverlayIntercomWidgetController?

  func makeNSView(context: Context) -> NSView { NSView() }
  func updateNSView(_ nsView: NSView, context: Context) {
    let scale = scale
    Task { @MainActor [weak controller] in
      controller?.reconcile(chips: chips, scale: scale)
    }
  }
}

/// The header's existing flat grip is also the conversation's move handle.
private struct OverlayIntercomDragHandle: NSViewRepresentable {
  var onDrag: () -> Void = {}
  func makeNSView(context: Context) -> OverlayIntercomDragView { OverlayIntercomDragView() }
  func updateNSView(_ nsView: OverlayIntercomDragView, context: Context) { nsView.onDrag = onDrag }
}

private final class OverlayIntercomDragView: NSView {
  var onDrag: () -> Void = {}
  override var isOpaque: Bool { false }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  override func mouseDown(with event: NSEvent) {
    onDrag()
    window?.performDrag(with: event)
  }
  override func resetCursorRects() { addCursorRect(bounds, cursor: .openHand) }
}

private struct OverlayIntercomCard: View {
  @Environment(\.colorScheme) private var colorScheme
  @Environment(\.csTextScale) private var scale
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @State private var entered = false
  let state: OverlayState
  let identity: OverlayIntercomCardIdentity
  let index: Int
  let controller: OverlayIntercomWidgetController
  let onRevealConversation: () -> Void

  private var conversation: OverlayConversation? {
    state.conversations.first { identity.matches($0) }
  }

  private var name: String {
    conversation.map { OverlayChannelMapChip.displayName($0.name) } ?? identity.name
  }

  var body: some View {
    let palette = OverlayAppearancePalette.resolve(colorScheme)
    let expanded = controller.presentation.expanded.contains(identity)
    let hovered = controller.presentation.hovered == identity
    VStack(spacing: 0) {
      cardHeader(palette: palette, expanded: expanded, hovered: hovered)
        .frame(height: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
      if expanded {
        if let conversation {
          conversationBody(conversation, palette: palette)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
          Text("Conversation unavailable").csFont(12)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
      }
    }
    .foregroundStyle(palette.primaryText.color)
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    .background(OverlayCanvasBackdrop(palette: palette, reduceTransparency: reduceTransparency))
    .clipShape(RoundedRectangle(cornerRadius: expanded ? 16 : 30 * max(1, scale)))
    .overlay {
      RoundedRectangle(cornerRadius: expanded ? 16 : 30 * max(1, scale))
        .strokeBorder(palette.border.color, lineWidth: 1).allowsHitTesting(false)
    }
    .overlay {
      if expanded {
        OverlayIntercomResizeHandles { frame, delta, edge in
          controller.resize(identity, from: frame, delta: delta, edge: edge)
        }
      }
    }
    .opacity(entered ? 1 : 0)
    .offset(y: entered || reduceMotion ? 0 : -6)
    .onAppear {
      withAnimation(
        reduceMotion
          ? nil : .spring(response: 0.28, dampingFraction: 0.86).delay(Double(index) * 0.025)
      ) { entered = true }
    }
    .onHover { controller.hover(identity, inside: $0) }
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-intercom-row-\(identity.digit)")
  }

  private func cardHeader(
    palette: OverlayAppearancePalette, expanded: Bool, hovered: Bool
  ) -> some View {
    let conversation = conversation
    let speaking = conversation?.replySpeaking == true
    let open = conversation.map { state.conversationMicrophoneOpen($0) } ?? false
    let current =
      intercomConversation(state: state, digit: identity.digit).map { identity.matches($0) } == true
    let reachable = current && state.channelHudStates[identity.digit]?.followerAlive == true
    let info = current ? state.channelAgentDescriptors[identity.digit] : nil
    let roster = OverlayChannelStatusView.intercomRoster(
      state: state, palette: palette, animates: !reduceMotion,
      onRevealConversation: onRevealConversation,
      onOpenConversation: { id in
        if id == identity.conversationID { controller.expand(identity) }
      })
    // Reuse the menu execution, but Write stays in this card. Show in Agents
    // remains the explicit route to the full overlay.
    return HStack(spacing: 3) {
      if expanded {
        Image(systemName: "hand.draw").font(.system(size: 10 * scale))
          .frame(width: 24 * max(1, scale))
          .overlay(OverlayIntercomDragHandle { controller.detach(identity) })
          .accessibilityLabel("Detach conversation")
      }
      Button {
        controller.expand(identity)
      } label: {
        HStack(spacing: 6) {
          Text(identity.digit).csMono(10).foregroundStyle(palette.mutedText.color)
            .overlay(alignment: .topTrailing) {
              if open {
                Circle().fill(palette.agentAccent.color).frame(width: 4, height: 4)
                  .offset(x: 4, y: -1)
              }
            }
          VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 5) {
              Text(verbatim: name).csFont(11, .semibold).lineLimit(1)
              Circle().fill(reachable ? palette.successStatus.color : palette.mutedText.color)
                .frame(width: 5, height: 5)
              if let conversation, state.unreadReplies(in: conversation) > 0 {
                Circle().fill(palette.agentAccent.color).frame(width: 5, height: 5)
              }
            }
            if hovered && !expanded {
              Text(
                verbatim: conversation?.messages.last.map {
                  OverlayChannelStatusView.drawerPreview(message: $0)
                } ?? info ?? String(localized: "No messages yet")
              )
              .csFont(9).lineLimit(1).foregroundStyle(palette.mutedText.color)
            }
          }
          Spacer(minLength: 0)
        }
        .padding(.leading, expanded ? 0 : 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .conversationActionHover(
        restingColor: speaking ? palette.agentAccent.color : palette.primaryText.color,
        hoverColor: palette.agentAccent.color
      )
      .disabled(conversation?.owner == nil)
      .accessibilityLabel(String(localized: "Open conversation") + ": " + name)
      .accessibilityIdentifier("overlay-intercom-name-\(identity.digit)")
      audioSlot(conversation, palette: palette, speaking: speaking, open: open)
      if expanded {
        Button("Close", systemImage: "xmark") { controller.close(identity) }
          .labelStyle(.iconOnly).buttonStyle(.plain)
          .frame(width: 26 * max(1, scale), height: 30 * max(1, scale))
          .conversationActionHover(
            restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
          )
          .accessibilityIdentifier("overlay-intercom-close-\(identity.digit)")
      }
    }
    .modifier(
      current
        ? roster.agentPillMenu(for: identity.digit, showsInlineError: false)
        : OverlayBroadcastPolicyMenu(members: [], palette: palette, enabled: false))
  }

  private func audioSlot(
    _ conversation: OverlayConversation?, palette: OverlayAppearancePalette,
    speaking: Bool, open: Bool
  ) -> some View {
    Button {
      guard let conversation else { return }
      Task {
        if speaking {
          await state.toggleConversationPlayback(conversation)
        } else {
          await state.toggleConversationMicrophone(conversation, preservesPresentation: true)
        }
      }
    } label: {
      Image(systemName: speaking ? "speaker.wave.2.fill" : (open ? "mic.fill" : "mic"))
        .font(.system(size: 13 * scale, weight: .semibold))
        .symbolEffect(.pulse, options: .repeating, isActive: open && !speaking && !reduceMotion)
        .frame(
          width: OverlayIntercomWidgetLayout.controlHeight(scale: scale),
          height: OverlayIntercomWidgetLayout.controlHeight(scale: scale)
        )
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .conversationActionHover(
      restingColor: palette.agentAccent.color.opacity(speaking || open ? 1 : 0.8),
      hoverColor: palette.agentAccent.color
    )
    .disabled(audioDisabled(conversation, speaking: speaking))
    .accessibilityLabel(
      (speaking
        ? String(localized: "Mute agent replies")
        : open ? String(localized: "Stop recording") : String(localized: "Start recording"))
        + ": " + name
    )
    .accessibilityIdentifier("overlay-intercom-audio-\(identity.digit)")
  }

  private func audioDisabled(_ conversation: OverlayConversation?, speaking: Bool) -> Bool {
    guard let conversation else { return true }
    if speaking {
      return state.conversationPlaybackMuted(conversation) == nil
        || state.pendingPlaybackOwners.contains(conversation.owner?.id ?? "")
    }
    return !state.canToggleConversationMicrophone(conversation)
      || state.pendingChannelToggles.contains(conversation.channel)
      || (!state.conversationMicrophoneOpen(conversation)
        && state.channelHudStates[conversation.channel]?.followerAlive == false)
  }

  private func conversationBody(
    _ conversation: OverlayConversation, palette: OverlayAppearancePalette
  ) -> some View {
    OverlayConversationView(
      conversation: conversation, palette: palette, topInset: 0, bottomInset: 12,
      pendingControls: state.pendingReplyControls, controlErrors: state.replyControlErrors,
      onControl: { message, stop in Task { await state.controlReply(message, stop: stop) } },
      composerFocusRevision: controller.focusRevisions[identity] ?? 0,
      draft: Binding(
        get: { state.conversationDrafts[conversation.id] ?? "" },
        set: { state.conversationDrafts[conversation.id] = $0 }),
      sending: state.pendingTextMessages.contains(conversation.id),
      sendError: state.textMessageErrors[conversation.id]
        ?? (state.draftCapture.errorConversationID == conversation.id
          ? state.draftCapture.error : nil),
      onSend: { Task { await state.sendConversationText(conversation) } },
      hidesNavigation: true,
      onComposerEditorActive: { state.noteComposerEditorActive($0) },
      onComposerTypingActivity: { state.noteComposerTypingActivity() },
      replySelection: state.conversationReplies[conversation.id],
      onClearReply: { state.conversationReplies.removeValue(forKey: conversation.id) },
      draftMicrophoneRecording: state.draftCapture.conversationID == conversation.id,
      draftMicrophoneBusy: state.draftMicrophoneBusy(in: conversation),
      onToggleDraftMicrophone: { Task { await state.toggleDraftMicrophone(in: conversation) } },
      // Ordinary attachments remain in this exact draft for manual Send. The
      // existing capture owner alone may admit attachments to a live take.
      onStageCaptureAttachments: { state.stageConversationAttachments($0, in: conversation) },
      pendingCaptureAttachments: state.captureAttachmentDrafts[conversation.id]?.paths ?? [],
      onRemoveCaptureAttachment: { state.removeCaptureAttachment($0, in: conversation.id) },
      captureAttachmentError: state.captureAttachmentErrors[conversation.id],
      onRetryCaptureAttachments: {
        Task { await state.retryCaptureAttachments(in: conversation.id) }
      },
      onReply: {
        state.conversationReplies[conversation.id] = OverlayUserReplyReference(message: $0)
      }
    )
  }
}

private struct OverlayIntercomResizeHandles: View {
  let resize: (CGRect, CGSize, OverlayIntercomWidgetLayout.ResizeEdge) -> Void

  var body: some View {
    ZStack(alignment: .bottomLeading) {
      HStack {
        OverlayIntercomResizeHandle(edge: .left, resize: resize).frame(width: 5)
        Spacer(minLength: 0)
      }
      HStack {
        OverlayIntercomResizeHandle(edge: .bottomLeft, resize: resize).frame(width: 14, height: 14)
        Spacer(minLength: 0)
        OverlayIntercomResizeHandle(edge: .bottomRight, resize: resize).frame(width: 14, height: 14)
      }
      .frame(height: 14)
    }
  }
}

private struct OverlayIntercomResizeHandle: NSViewRepresentable {
  let edge: OverlayIntercomWidgetLayout.ResizeEdge
  let resize: (CGRect, CGSize, OverlayIntercomWidgetLayout.ResizeEdge) -> Void

  func makeNSView(context: Context) -> OverlayIntercomResizeView { OverlayIntercomResizeView() }
  func updateNSView(_ nsView: OverlayIntercomResizeView, context: Context) {
    nsView.edge = edge
    nsView.resize = resize
  }
}

private final class OverlayIntercomResizeView: NSView {
  var edge = OverlayIntercomWidgetLayout.ResizeEdge.left
  var resize: (CGRect, CGSize, OverlayIntercomWidgetLayout.ResizeEdge) -> Void = { _, _, _ in }
  private var origin: CGPoint?
  private var originalFrame: CGRect?

  override var isOpaque: Bool { false }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  override func mouseDown(with event: NSEvent) {
    origin = NSEvent.mouseLocation
    originalFrame = window?.frame
  }
  override func mouseDragged(with event: NSEvent) {
    guard let origin, let originalFrame else { return }
    let point = NSEvent.mouseLocation
    resize(originalFrame, CGSize(width: point.x - origin.x, height: point.y - origin.y), edge)
  }
  override func mouseUp(with event: NSEvent) {
    mouseDragged(with: event)
    origin = nil
    originalFrame = nil
  }
  override func resetCursorRects() {
    addCursorRect(bounds, cursor: edge == .left ? .resizeLeftRight : .crosshair)
  }
}

@MainActor
private func intercomConversation(state: OverlayState, digit: String) -> OverlayConversation? {
  OverlayChannelStatusView.currentConversations(
    conversations: state.conversations, archivedOwners: state.archivedAgentOwners,
    hudStates: state.channelHudStates
  ).first { $0.channel == digit }
}
