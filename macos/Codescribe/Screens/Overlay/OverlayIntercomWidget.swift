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
  @ObservationIgnored private let desktopStore: OverlayIntercomDesktopStore
  @ObservationIgnored private var header: OverlayIntercomPanel?
  @ObservationIgnored private var cards: [OverlayIntercomCardIdentity: OverlayIntercomPanel] = [:]
  @ObservationIgnored private var instancePanels: [UUID: OverlayIntercomPanel] = [:]
  @ObservationIgnored private var identities: [OverlayIntercomCardIdentity] = []
  @ObservationIgnored private var sizes: [OverlayIntercomCardIdentity: CGSize] = [:]
  @ObservationIgnored private var visible = false
  @ObservationIgnored private var positioning = false
  @ObservationIgnored private var invalidated = false
  /// Mouse deltas are absolute from mouse-down, including when the gesture
  /// starts on a hovered card. Keep the whole group's frame as the baseline.
  @ObservationIgnored private var groupResizeGesture: (gesture: UUID, frame: CGRect)?
  @ObservationIgnored private var resizingGroup = false
  /// A card's resize that converted it into an instance hands the rest of
  /// that same gesture to the instance.
  @ObservationIgnored private var resizeHandoff: (gesture: UUID, id: UUID)?
  /// The pending hover-leave grace period; a fresh enter cancels it.
  @ObservationIgnored private var hoverLeaveTask: Task<Void, Never>?
  /// The frame each group card was last sent toward. A window animation
  /// does not move the frame until its first tick, so a fast crossing that
  /// retargets within that tick sees `panel.frame` still at the old frame;
  /// a guard on the live frame then skips the retarget and the first
  /// animation wins — the stack of fiszki left wide after a quick sweep
  /// (Maciej's recording, 2026-10-23 09:05). Compare against the target.
  @ObservationIgnored private var cardTargets: [OverlayIntercomCardIdentity: CGRect] = [:]
  fileprivate private(set) var rowOffset = 0
  fileprivate private(set) var availableHeight: CGFloat = 900
  fileprivate private(set) var presentation = OverlayIntercomPresentation()
  fileprivate private(set) var focusRevisions: [OverlayIntercomCardIdentity: UInt64] = [:]
  /// The durable desktop: detached fiszki and the group's content geometry.
  fileprivate private(set) var desktop: OverlayIntercomDesktopState
  fileprivate private(set) var instanceFocusRevisions: [UUID: UInt64] = [:]

  init(
    state: OverlayState, textScale: TextScaleController,
    revealConversation: @escaping () -> Void = {},
    desktopStore: OverlayIntercomDesktopStore = OverlayIntercomDesktopStore()
  ) {
    self.state = state
    self.textScale = textScale
    self.revealConversation = revealConversation
    self.desktopStore = desktopStore
    desktop = desktopStore.load()
    super.init()
  }

  private var effectiveScale: CGFloat { textScale.scale }
  fileprivate private(set) var contentMetrics = IntercomContentMetrics(
    names: [], digits: [], scale: 1)
  fileprivate private(set) var groupTier: IntercomContentTier = .actions
  fileprivate private(set) var groupCardHeight: CGFloat = 30

  fileprivate private(set) var headerControlsVisible = true
  fileprivate private(set) var groupWidth: CGFloat = 0
  fileprivate var stacksGroupControls: Bool {
    groupTier == .actions && groupWidth < contentMetrics.actionsWidth
  }

  private func saveDesktop() {
    desktopStore.save(desktop)
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
      for panel in instancePanels.values { panel.setPresented(false, animated: false) }
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
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
    materializeInstances()
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
    for panel in instancePanels.values {
      panel.delegate = nil
      panel.contentView = nil
      panel.close()
    }
    instancePanels.removeAll()
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
    // exact owner's open card survives as a durable detached instance.
    for identity in presentation.expanded where !identities.contains(identity) {
      detachIntoInstance(identity)
    }
    let retained = Set(identities)
    for identity in Set(cards.keys).subtracting(retained) {
      let panel = cards.removeValue(forKey: identity)
      panel?.delegate = nil
      panel?.contentView = nil
      panel?.close()
      sizes.removeValue(forKey: identity)
      cardTargets.removeValue(forKey: identity)
      focusRevisions.removeValue(forKey: identity)
      presentation.leave(identity)
    }
    let nameFont = NSFont.systemFont(ofSize: 11 * scale, weight: .semibold)
    let digitFont = CSFont.nsMono(10 * scale)
    contentMetrics = IntercomContentMetrics(
      names: chips.map(\.name), digits: chips.map(\.digit), scale: scale,
      hasDetached: identities.contains { detachedInstance(for: $0) != nil })
    let titleWidth =
      (String(localized: "Intercom") as NSString).size(withAttributes: [.font: nameFont]).width
      + (chips.count.formatted() as NSString).size(withAttributes: [.font: digitFont]).width
      + 12 * scale + 8
      + (hasOverflow && !presentation.folded ? 32 * max(1, scale) + 16 : 0)
    let defaultWidth = max(
      contentMetrics.actionsWidth,
      OverlayIntercomWidgetLayout.restingWidth(headerText: titleWidth, agentText: 0, scale: scale))
    let width = contentMetrics.fittedWidth(
      CGFloat(desktop.groupWidth ?? Double(defaultWidth)), available: screenFrame.width)
    groupCardHeight = contentMetrics.fittedCardHeight(
      CGFloat(desktop.groupRowHeight ?? Double(contentMetrics.iconSide)), count: visibleRows.count,
      available: screenFrame.height)
    headerControlsVisible =
      width
      >= OverlayIntercomWidgetLayout.restingWidth(
        headerText: titleWidth, agentText: 0, scale: scale)
    if width < contentMetrics.nameWidth { groupCardHeight = contentMetrics.iconSide }
    groupWidth = width
    groupTier = contentMetrics.tier(width: width, cardHeight: groupCardHeight)
    if desktop.groupTier != groupTier {
      desktop.groupTier = groupTier
      saveDesktop()
    }
    if !groupTier.hoverPreviewEnabled { presentation.hovered = nil }
    let frame = CGRect(
      x: header.frame.maxX - width, y: header.frame.minY,
      width: width, height: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
    positioning = true
    let fitted = OverlayIntercomWidgetLayout.clampedHeader(
      frame, count: presentation.folded ? 0 : visibleRows.count,
      visibleFrame: screenFrame, scale: scale, cardHeight: groupCardHeight)
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

  private var groupIdentities: [OverlayIntercomCardIdentity] { identities }

  fileprivate var visibleRows: Range<Int> {
    OverlayIntercomWidgetLayout.visibleRange(
      count: groupIdentities.count, offset: rowOffset, visibleHeight: availableHeight,
      scale: contentMetrics.iconSide / OverlayIntercomWidgetLayout.rowHeight)
  }

  fileprivate var hasOverflow: Bool { visibleRows.count < groupIdentities.count }
  fileprivate var hasPrevious: Bool { visibleRows.lowerBound > 0 }
  fileprivate var hasNext: Bool { visibleRows.upperBound < groupIdentities.count }

  func presentsConversation(_ conversationID: String?) -> Bool {
    guard visible, let conversationID else { return false }
    if presentation.expanded.contains(where: { identity in
      identity.conversationID == conversationID && cards[identity]?.isPresented == true
        && state.conversations.contains(where: { identity.matches($0) })
    }) {
      return true
    }
    return desktop.instances.contains { instance in
      instance.conversationID == conversationID && !instance.closed && !instance.minimized
        && instancePanels[instance.id]?.isPresented == true
        && state.conversations.contains(where: { instance.matches($0) })
    }
  }

  // MARK: - Durable instances

  fileprivate func instance(for id: UUID) -> OverlayIntercomInstance? {
    desktop.instances.first { $0.id == id }
  }

  /// Window creation and frame projection for the saved desktop. Restore is a
  /// UI operation: it never opens a microphone or re-sends anything, and a
  /// closed instance stays closed until the fiszka is detached again.
  private func materializeInstances() {
    guard !invalidated else { return }
    for (id, panel) in instancePanels
    where !desktop.instances.contains(where: { $0.id == id }) {
      panel.delegate = nil
      panel.contentView = nil
      panel.close()
      instancePanels.removeValue(forKey: id)
      instanceFocusRevisions.removeValue(forKey: id)
    }
    for instance in desktop.instances {
      guard !instance.closed else {
        instancePanels[instance.id]?.setPresented(false, animated: false)
        continue
      }
      let panel: OverlayIntercomPanel
      if let existing = instancePanels[instance.id] {
        panel = existing
      } else {
        panel = OverlayIntercomPanel()
        panel.delegate = self
        panel.contentView = NSHostingView(
          rootView: TextScaleRoot(controller: textScale) {
            OverlayIntercomInstanceCard(
              state: state, instanceID: instance.id, controller: self,
              onRevealConversation: revealConversation)
          })
        instancePanels[instance.id] = panel
      }
      panel.acceptsConversationFocus = !instance.minimized
      positioning = true
      let fitted = instanceFrame(instance, for: panel)
      if panel.frame != fitted { panel.setFrame(fitted, display: true) }
      positioning = false
      panel.setPresented(
        visible, animated: !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
    }
  }

  /// Mini keeps the stored window's top-right corner and shrinks to one row;
  /// the expanded frame survives underneath for restore. The monitor comes
  /// from the SAVED geometry: a freshly created panel still has a zero frame,
  /// which would elect one arbitrary screen for every restored fiszka
  /// (Olga, D3 on 181d27afb).
  private func instanceFrame(_ instance: OverlayIntercomInstance, for panel: NSWindow)
    -> CGRect
  {
    let stored = instance.frame
    let visibleFrame = visibleFrame(around: panel.frame == .zero ? stored : panel.frame)
    guard instance.minimized else {
      return OverlayIntercomWidgetLayout.cardFrame(
        below: CGRect(x: stored.minX, y: stored.maxY, width: stored.width, height: 0),
        size: stored.size, visibleFrame: visibleFrame)
    }
    // Mini is a compact pill anchored at the stored window's top-right: its
    // width comes from its own content, never from the expanded conversation
    // (Astra, V3 — a 900 pt conversation must not leave a 900 pt mini). The
    // expanded frame survives untouched for restore.
    let nameFont = NSFont.systemFont(ofSize: 11 * textScale.scale, weight: .semibold)
    let nameWidth = (instance.name as NSString).size(withAttributes: [.font: nameFont]).width
    let digitFont = CSFont.nsMono(10 * textScale.scale)
    let digitWidth = (instance.digit as NSString).size(withAttributes: [.font: digitFont]).width
    let height = OverlayIntercomWidgetLayout.controlHeight(scale: textScale.scale)
    let width = min(
      stored.width,
      OverlayIntercomWidgetLayout.minimizedInstanceWidth(
        text: nameWidth + digitWidth, scale: textScale.scale))
    return OverlayIntercomWidgetLayout.cardFrame(
      below: CGRect(x: stored.maxX - width, y: stored.maxY, width: width, height: 0),
      size: CGSize(width: width, height: height), visibleFrame: visibleFrame)
  }

  fileprivate func setInstanceMinimized(_ id: UUID, _ minimized: Bool) {
    guard let index = desktop.instances.firstIndex(where: { $0.id == id }) else { return }
    desktop.instances[index].minimized = minimized
    if minimized { instancePanels[id]?.resignKey() }
    saveDesktop()
    materializeInstances()
    if !minimized, let panel = instancePanels[id] {
      instanceFocusRevisions[id, default: 0] &+= 1
      panel.makeKeyAndOrderFront(nil)
    }
  }

  /// X is a window operation: the saved fiszka stays recorded as closed and
  /// detaching the same conversation reopens it. No disconnect, no archive.
  fileprivate func closeInstance(_ id: UUID) {
    guard let index = desktop.instances.firstIndex(where: { $0.id == id }) else { return }
    instancePanels[id]?.resignKey()
    desktop.instances[index].closed = true
    saveDesktop()
    materializeInstances()
  }

  fileprivate func resizeInstance(
    _ id: UUID, from frame: CGRect, delta: CGSize, edge: OverlayIntercomWidgetLayout.ResizeEdge
  ) {
    guard let index = desktop.instances.firstIndex(where: { $0.id == id }),
      let panel = instancePanels[id], !desktop.instances[index].minimized
    else { return }
    let fitted = OverlayIntercomWidgetLayout.resizedFrame(
      frame, delta: delta, edge: edge,
      minimum: OverlayIntercomWidgetLayout.minimumConversationSize(scale: textScale.scale),
      visibleFrame: visibleFrame(for: panel))
    desktop.instances[index].frame = fitted
    saveDesktop()
    positioning = true
    panel.setFrame(fitted, display: true)
    positioning = false
  }

  fileprivate func focusInstance(_ id: UUID) {
    guard let panel = instancePanels[id], instance(for: id)?.minimized == false else { return }
    instanceFocusRevisions[id, default: 0] &+= 1
    panel.makeKeyAndOrderFront(nil)
  }

  /// The open fiszka of a group row's conversation, if one stands on the
  /// desktop. The row then greys out and becomes the road to that window.
  fileprivate func detachedInstance(for identity: OverlayIntercomCardIdentity)
    -> OverlayIntercomInstance?
  {
    desktop.openInstance(for: identity)
  }

  /// A greyed row's click and a refused second detach both land here: the
  /// open fiszka comes forward, restored from mini when it was folded.
  fileprivate func showInstance(_ id: UUID) {
    if instance(for: id)?.minimized == true {
      setInstanceMinimized(id, false)
    } else {
      focusInstance(id)
    }
  }

  /// The explicit road back to a saved closed fiszka, independent of the
  /// roster (Olga, D4): a replaced channel owner removes the group card, and
  /// without this the saved record had no UI that could show it again.
  fileprivate var closedInstances: [OverlayIntercomInstance] {
    desktop.instances.filter(\.closed)
  }

  fileprivate func reopenInstance(_ id: UUID) {
    guard let index = desktop.instances.firstIndex(where: { $0.id == id }) else { return }
    desktop.instances[index].closed = false
    saveDesktop()
    materializeInstances()
  }

  fileprivate func scrollGroup(_ steps: Int) {
    guard visible, !presentation.folded else { return }
    rowOffset = min(
      max(0, visibleRows.lowerBound + steps), max(0, groupIdentities.count - visibleRows.count))
    presentation.hovered = nil
    positionCards()
  }

  private func visibleFrame(for panel: NSWindow?) -> CGRect {
    visibleFrame(around: panel?.frame ?? .zero)
  }

  private func visibleFrame(around frame: CGRect) -> CGRect {
    NSScreen.screens.max(by: {
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
      guard !presentation.folded, let index = group.firstIndex(of: identity), range.contains(index)
      else {
        panel.setPresented(
          false, animated: !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
        continue
      }
      let row = OverlayIntercomWidgetLayout.rowFrame(
        index: index - range.lowerBound, header: header.frame, count: range.count,
        visibleFrame: screenFrame, scale: effectiveScale, cardHeight: groupCardHeight)
      let frame: CGRect
      if expanded {
        frame = OverlayIntercomWidgetLayout.conversationFrame(
          below: row,
          size: sizes[identity]
            ?? OverlayIntercomWidgetLayout.conversationSize(scale: effectiveScale),
          visibleFrame: screenFrame, scale: effectiveScale)
      } else if presentation.hovered == identity && groupTier.hoverPreviewEnabled {
        if groupTier == .icon {
          frame = OverlayIntercomWidgetLayout.cardFrame(
            below: row, size: CGSize(width: contentMetrics.nameWidth, height: row.height),
            visibleFrame: screenFrame)
        } else {
          frame = OverlayIntercomWidgetLayout.previewFrame(
            beside: row, visibleFrame: screenFrame, scale: effectiveScale)
        }
      } else {
        // A grouped fiszka inherits the header's size — one row, the column's
        // width. Only a detached instance owns its geometry (Maciej, 2026-10-23).
        frame = row
      }
      if cardTargets[identity] != frame {
        cardTargets[identity] = frame
        // Hover slides stay smooth, but they must CONVERGE: a new target
        // retargets the running animation instead of queueing another, and
        // the duration is fixed instead of distance-scaled. setFrame(animate:)
        // did neither, so a fast crossing left the stack dancing long after
        // the mouse stopped (Maciej, 2026-10-23).
        if panel.isVisible && !resizingGroup
          && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        {
          NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.18
            context.timingFunction = CAMediaTimingFunction(name: .easeOut)
            panel.animator().setFrame(frame, display: true)
          }
        } else {
          panel.setFrame(frame, display: true)
        }
      }
      panel.setPresented(true, animated: !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
    }
    // Geometry and hover leave the order of presented windows alone. Only an
    // explicit expand below (or a native click/focus) raises a conversation.
  }

  fileprivate func hover(_ identity: OverlayIntercomCardIdentity, inside: Bool) {
    // A row whose conversation stands in its own window has nothing to slide
    // out: the words are in that window.
    guard visible, !presentation.folded, groupTier.hoverPreviewEnabled,
      desktop.openInstance(for: identity) == nil
    else { return }
    // Enter lands immediately; leave waits a grace period. A cursor resting
    // on a card's edge flips the hit test on every animated frame change and
    // the card oscillates several times a second (Maciej's recording,
    // 2026-10-23 08:50). Edge jitter shorter than the grace changes nothing,
    // and a crossing leaves a soft tail of cards easing back.
    if inside {
      hoverLeaveTask?.cancel()
      hoverLeaveTask = nil
      let before = presentation.hovered
      presentation.enter(identity)
      guard presentation.hovered != before else { return }
      positionCards()
    } else {
      guard presentation.hovered == identity else { return }
      hoverLeaveTask?.cancel()
      hoverLeaveTask = Task { @MainActor [weak self] in
        try? await Task.sleep(nanoseconds: 220_000_000)
        guard let self, !Task.isCancelled, self.presentation.hovered == identity else { return }
        self.presentation.leave(identity)
        self.positionCards()
      }
    }
  }

  fileprivate func expand(_ identity: OverlayIntercomCardIdentity) {
    if let open = desktop.openInstance(for: identity) {
      showInstance(open.id)
      return
    }
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
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
  }

  fileprivate func toggleGroup() {
    presentation.folded.toggle()
    presentation.hovered = nil
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
  }

  /// Detaching makes the fiszka durable (Maciej, 2026-10-22). One agent keeps
  /// one open window (Maciej, 2026-10-23): while it stands somewhere, the
  /// gesture shows it instead of minting a copy, and the group row stays in
  /// the stack, greyed, as the road back to it — the group no longer folds.
  /// Returns whether this card's panel became that window, so the grip knows
  /// whether there is anything to drag.
  @discardableResult
  fileprivate func detach(_ identity: OverlayIntercomCardIdentity) -> Bool {
    guard presentation.expanded.contains(identity) else { return false }
    if let open = desktop.openInstance(for: identity) {
      close(identity)
      showInstance(open.id)
      return false
    }
    let detached = detachIntoInstance(identity) != nil
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
    return detached
  }

  @discardableResult
  private func detachIntoInstance(
    _ identity: OverlayIntercomCardIdentity, frame: CGRect? = nil
  ) -> UUID? {
    // The broadcast channel "0" has a conversation but no owner; it is the
    // one ownerless identity a fiszka may carry (Maciej, 2026-10-23).
    guard identity.conversationID != nil, identity.ownerID != nil || identity.digit == "0" else {
      presentation.close(identity)
      return nil
    }
    if let open = desktop.openInstance(for: identity) {
      presentation.close(identity)
      return open.id
    }
    let current =
      frame ?? cards[identity]?.frame
      ?? CGRect(
        origin: header?.frame.origin ?? .zero,
        size: OverlayIntercomWidgetLayout.conversationSize(scale: textScale.scale))
    let id: UUID
    if let index = desktop.instances.firstIndex(where: {
      $0.conversationID == identity.conversationID && $0.ownerID == (identity.ownerID ?? "")
        && $0.closed
    }) {
      // The reopened fiszka lands under the gesture that summoned it; its
      // saved placement is superseded by where the person is holding it now.
      desktop.instances[index].closed = false
      desktop.instances[index].minimized = false
      desktop.instances[index].frame = current
      id = desktop.instances[index].id
    } else {
      let instance = OverlayIntercomInstance(identity: identity, frame: current)
      desktop.upsert(instance)
      id = instance.id
    }
    // A grip drag hands this exact window to the window server (performDrag),
    // so the same panel becomes the instance window and the drag continues
    // under the cursor instead of dying with a hidden group card.
    if let panel = cards.removeValue(forKey: identity) {
      panel.resignKey()
      panel.contentView = NSHostingView(
        rootView: TextScaleRoot(controller: textScale) {
          OverlayIntercomInstanceCard(
            state: state, instanceID: id, controller: self,
            onRevealConversation: revealConversation)
        })
      sizes.removeValue(forKey: identity)
      cardTargets.removeValue(forKey: identity)
      focusRevisions.removeValue(forKey: identity)
      instancePanels[id] = panel
    }
    presentation.close(identity)
    saveDesktop()
    materializeInstances()
    return id
  }

  /// Resizing an expanded group card is an explicit independent placement:
  /// it becomes a durable instance with its own geometry (brief: a single
  /// conversation's resize never replaces the proportional group resize).
  /// The gesture then continues on the new instance until mouse-up — the
  /// resize view replays events against its mouse-down frame, which hands
  /// the rest of the drag over (Astra on 05c013749).
  fileprivate func resize(
    _ identity: OverlayIntercomCardIdentity, from frame: CGRect, delta: CGSize,
    edge: OverlayIntercomWidgetLayout.ResizeEdge, gesture: UUID
  ) {
    if resizeHandoff?.gesture != gesture { resizeHandoff = nil }
    if let handoff = resizeHandoff {
      resizeInstance(handoff.id, from: frame, delta: delta, edge: edge)
      return
    }
    guard let panel = cards[identity], presentation.expanded.contains(identity) else { return }
    if let open = desktop.openInstance(for: identity) {
      close(identity)
      showInstance(open.id)
      return
    }
    let fitted = OverlayIntercomWidgetLayout.resizedFrame(
      frame, delta: delta, edge: edge,
      minimum: OverlayIntercomWidgetLayout.minimumConversationSize(scale: textScale.scale),
      visibleFrame: visibleFrame(for: panel))
    if let id = detachIntoInstance(identity, frame: fitted) {
      resizeHandoff = (gesture: gesture, id: id)
    }
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
  }

  /// Width reveals content; height can place controls below the name. Neither changes fonts.
  fileprivate func resizeGroup(
    from frame: CGRect, delta: CGSize, edge: OverlayIntercomWidgetLayout.ResizeEdge, gesture: UUID
  ) {
    guard let header else { return }
    resizingGroup = true
    defer { resizingGroup = false }
    let count = presentation.folded ? 0 : visibleRows.count
    if groupResizeGesture?.gesture != gesture {
      let height =
        header.frame.height + CGFloat(count) * (groupCardHeight + OverlayIntercomWidgetLayout.gap)
      groupResizeGesture = (
        gesture,
        CGRect(
          x: header.frame.minX, y: header.frame.maxY - height,
          width: header.frame.width, height: height)
      )
    }
    guard let base = groupResizeGesture?.frame else { return }
    let fitted = contentMetrics.resizedGroupFrame(
      base, delta: delta, edge: edge, count: count, visibleFrame: visibleFrame(for: header))
    desktop.groupWidth = Double(fitted.width)
    if count > 0 && edge != .left {
      desktop.groupRowHeight = Double(
        (fitted.height - header.frame.height) / CGFloat(count)
          - OverlayIntercomWidgetLayout.gap)
    }
    positioning = true
    header.setFrame(
      CGRect(
        x: fitted.minX, y: fitted.maxY - header.frame.height,
        width: fitted.width, height: header.frame.height), display: true)
    positioning = false
    presentation.hovered = nil
    saveDesktop()
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
    UserDefaults.standard.set(
      [Double(header.frame.minX), Double(header.frame.minY)], forKey: Self.positionKey)
  }

  func windowDidMove(_ notification: Notification) {
    guard !positioning, let panel = notification.object as? NSWindow else { return }
    if panel === header, let header {
      // A destination display may fit fewer rows or only one control row.
      // Re-measure before placing, rather than clamping an oversized stack.
      reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
      UserDefaults.standard.set(
        [Double(header.frame.minX), Double(header.frame.minY)], forKey: Self.positionKey)
    } else if let id = instancePanels.first(where: { $0.value === panel })?.key,
      let index = desktop.instances.firstIndex(where: { $0.id == id })
    {
      let fitted = OverlayIntercomWidgetLayout.cardFrame(
        below: panel.frame, size: panel.frame.size, visibleFrame: visibleFrame(for: panel))
      if panel.frame != fitted {
        positioning = true
        panel.setFrame(fitted, display: true)
        positioning = false
      }
      // A mini window moves the stored expanded frame by its top-right corner,
      // so restore opens where the person last left the fiszka.
      var stored = desktop.instances[index].frame
      stored.origin.x = fitted.maxX - stored.width
      stored.origin.y = fitted.maxY - stored.height
      desktop.instances[index].frame = stored
      saveDesktop()
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
    // The header's first row IS channel 0 (Maciej, 2026-10-23 05:18Z): the
    // broadcast keeps its microphone and reply control here, nothing more —
    // no pill of its own and no detach.
    let broadcast = intercomConversation(state: state, digit: "0")
    HStack(spacing: 8) {
      Button {
        controller.toggleGroup()
      } label: {
        HStack(spacing: 8) {
          // The shared channel's mark is the antenna the agents list wears on
          // its All row — a state icon, not a status verb (Maciej,
          // 2026-10-23: "Intercom też nie potrzebuje verba stanu"). It takes
          // the place of the grid glyph so the wordmark keeps one line.
          OverlayAgentStatusDot(kind: .broadcast, palette: palette)
          if controller.headerControlsVisible {
            Text("Intercom").csFont(11, .semibold).lineLimit(1).fixedSize()
          }
        }
        .contentShape(Capsule())
      }
      .buttonStyle(.plain)
      .conversationActionHover(
        restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
      )
      .accessibilityLabel(
        controller.presentation.folded
          ? String(localized: "Expand Intercom") : String(localized: "Collapse Intercom")
      )
      .contextMenu {
        // The explicit road back to a saved closed fiszka, independent of
        // the roster. Reopening is a window operation: no microphone, no
        // send. It lives on the wordmark: the rest of the bar is the move
        // surface, which SwiftUI's menu never reaches.
        let closed = controller.closedInstances
        if closed.isEmpty {
          Text("No saved fiszki")
        } else {
          ForEach(closed) { instance in
            // A digit and a name are not a phrase to translate: verbatim keeps
            // `%@ · %@` out of the catalog (the l10n gate refused it).
            Button {
              controller.reopenInstance(instance.id)
            } label: {
              Text(verbatim: "\(instance.digit) · \(instance.name)")
            }
          }
        }
      }
      if controller.groupTier != .icon
        && (!controller.hasOverflow || controller.headerControlsVisible)
      {
        Spacer(minLength: 4)
        Text(verbatim: chips.count.formatted()).csMono(10).allowsHitTesting(false)
      }
      if controller.hasOverflow && !controller.presentation.folded && controller.groupTier != .icon
      {
        Button("Previous agents", systemImage: "chevron.up") { controller.scrollGroup(-1) }
          .labelStyle(.iconOnly).buttonStyle(.plain).disabled(!controller.hasPrevious)
          .frame(width: 16 * max(1, scale))
        Button("Next agents", systemImage: "chevron.down") { controller.scrollGroup(1) }
          .labelStyle(.iconOnly).buttonStyle(.plain).disabled(!controller.hasNext)
          .frame(width: 16 * max(1, scale))
      }
      if controller.headerControlsVisible {
        OverlayIntercomAudioControls(
          state: state, conversation: broadcast, digit: "0", palette: palette,
          open: broadcast.map { state.conversationMicrophoneOpen($0) } ?? false,
          speaking: broadcast?.replySpeaking == true)
      }
    }
    .foregroundStyle(palette.mutedText.color)
    .padding(.horizontal, controller.groupTier == .icon ? 0 : 12)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    // Any free spot on the bar moves the group — not a grip that showed a
    // finger drawing (Maciej, 2026-10-23). Buttons above it keep their
    // clicks; the count and the gaps fall through to the move surface.
    .background(OverlayIntercomDragHandle(cursorHint: false))
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
    .overlay {
      // The group's left edge selects content for every attached fiszka.
      // The header's corners keep their buttons; pills carry the corners.
      OverlayIntercomResizeHandles(corners: false) { frame, delta, edge, gesture in
        controller.resizeGroup(from: frame, delta: delta, edge: edge, gesture: gesture)
      }
    }
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
/// `onDrag` answers whether the window under the grip is the one to move: a
/// refused detach leaves the group card in the stack, and the stack is not
/// dragged.
private struct OverlayIntercomDragHandle: NSViewRepresentable {
  var onDrag: () -> Bool = { true }
  /// A visible grip shows the open hand; a whole-header surface under the
  /// buttons keeps their arrow.
  var cursorHint = true
  func makeNSView(context: Context) -> OverlayIntercomDragView { OverlayIntercomDragView() }
  func updateNSView(_ nsView: OverlayIntercomDragView, context: Context) {
    nsView.onDrag = onDrag
    nsView.cursorHint = cursorHint
  }
}

private final class OverlayIntercomDragView: NSView {
  var onDrag: () -> Bool = { true }
  var cursorHint = true
  private var moving = false
  override var isOpaque: Bool { false }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  override func mouseDown(with event: NSEvent) {
    moving = false
    super.mouseDown(with: event)
  }
  // A real drag moves the window (and detaches, where the grip detaches).
  // A click that never moves is not a detach and not a move. The window
  // reference is taken BEFORE onDrag: a detach swaps the panel's content
  // view, which removes this view from the window and nils self.window
  // (Astra, D2 trace on 05c013749). One performDrag per gesture: the window
  // server owns the move from there, and every further mouseDragged would
  // log "Window move completed without beginning" (Klaudiusz-div0, HID
  // probe on macOS 27).
  override func mouseDragged(with event: NSEvent) {
    guard !moving else { return }
    let window = window
    guard onDrag() else { return }
    moving = true
    window?.performDrag(with: event)
  }
  override func mouseUp(with event: NSEvent) {
    moving = false
    super.mouseUp(with: event)
  }
  override func resetCursorRects() {
    if cursorHint { addCursorRect(bounds, cursor: .openHand) }
  }
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
      } else if controller.stacksGroupControls {
        stackedControls(palette: palette)
          .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
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
        OverlayIntercomResizeHandles { frame, delta, edge, gesture in
          controller.resize(identity, from: frame, delta: delta, edge: edge, gesture: gesture)
        }
      } else {
        // Existing edges and corners resize the group. Audio keeps its hit
        // target when present; compact cards have no trailing audio slot.
        OverlayIntercomResizeHandles(
          trailingInset: controller.groupTier == .actions
            ? OverlayIntercomWidgetLayout.controlHeight(scale: scale) : 0
        ) { frame, delta, edge, gesture in
          controller.resizeGroup(from: frame, delta: delta, edge: edge, gesture: gesture)
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

  @ViewBuilder
  private func cardHeader(
    palette: OverlayAppearancePalette, expanded: Bool, hovered: Bool
  ) -> some View {
    let conversation = conversation
    let speaking = conversation?.replySpeaking == true
    let open = conversation.map { state.conversationMicrophoneOpen($0) } ?? false
    let current =
      intercomConversation(state: state, digit: identity.digit).map { identity.matches($0) } == true
    let info = current ? state.channelAgentDescriptors[identity.digit] : nil
    // The conversation stands in its own window: the row stays in the group,
    // greyed, and its click shows that window (Maciej, 2026-10-23).
    let detached = controller.detachedInstance(for: identity)
    let roster = OverlayChannelStatusView.intercomRoster(
      state: state, palette: palette, animates: !reduceMotion,
      onRevealConversation: onRevealConversation,
      onOpenConversation: { id in
        if id == identity.conversationID { controller.expand(identity) }
      })
    let channelRow = intercomChannelRow(state: state, digit: identity.digit)
    let dead = channelRow.map { roster.hasDeadFollower($0) } ?? false
    let tier = controller.groupTier
    // Icon hover adds only the name to the left of the anchored square.
    if tier == .icon && !expanded {
      Button {
        if let detached {
          controller.showInstance(detached.id)
        } else {
          controller.expand(identity)
        }
      } label: {
        HStack(spacing: 0) {
          if hovered {
            Text(verbatim: name).csFont(11, .semibold).lineLimit(1)
              .frame(maxWidth: .infinity, alignment: .leading).padding(.leading, 12)
          }
          HStack(spacing: 4) {
            OverlayAgentStatusDot(
              kind: intercomStatusDot(roster, state: state, digit: identity.digit), palette: palette
            )
            Text(verbatim: identity.digit).csMono(10).foregroundStyle(palette.mutedText.color)
          }
          .frame(
            width: controller.contentMetrics.iconSide, height: controller.contentMetrics.iconSide)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .modifier(OverlayDisconnectedRowDim(disconnected: dead || detached != nil))
      .accessibilityLabel(Text(verbatim: identity.digit + " · " + name))
      .accessibilityIdentifier("overlay-intercom-name-\(identity.digit)")
    } else {
      // Reuse the menu execution, but Write stays in this card.
      HStack(spacing: 3) {
        if expanded {
          // A window-with-plus, not the finger that read as "draw" (Maciej,
          // 2026-10-23): dragging it lifts the card into its own window.
          Image(systemName: "macwindow.badge.plus").font(.system(size: 10 * scale))
            .frame(width: 24 * max(1, scale))
            .overlay(OverlayIntercomDragHandle { controller.detach(identity) })
            .overlayFastHint(String(localized: "Drag to detach"), palette: palette)
            .accessibilityLabel("Detach conversation")
        }
        HStack(spacing: 6) {
          Text(verbatim: identity.digit).csMono(10).foregroundStyle(palette.mutedText.color)
          if let channelRow {
            OverlayReceptionMarks(channel: channelRow, palette: palette, textScale: scale)
          }
          OverlayAgentStatusDot(
            kind: intercomStatusDot(roster, state: state, digit: identity.digit),
            palette: palette)
          Button {
            if let detached {
              controller.showInstance(detached.id)
            } else {
              controller.expand(identity)
            }
          } label: {
            HStack(spacing: 5) {
              VStack(alignment: .leading, spacing: 0) {
                Text(verbatim: name).csFont(11, .semibold).lineLimit(1)
                if hovered && tier == .name && !expanded && detached == nil {
                  Text(
                    verbatim: conversation?.messages.last.map {
                      OverlayChannelStatusView.drawerPreview(message: $0)
                    } ?? info ?? String(localized: "No messages yet")
                  )
                  .csFont(9).lineLimit(1).foregroundStyle(palette.mutedText.color)
                }
              }
              if detached != nil {
                Image(systemName: "macwindow").font(.system(size: 9 * scale))
                  .frame(width: 9 * scale)
              }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
            .contentShape(Rectangle())
          }
          .buttonStyle(.plain)
          .conversationActionHover(
            restingColor: dead || detached != nil
              ? palette.mutedText.color
              : speaking ? palette.agentAccent.color : palette.primaryText.color,
            hoverColor: dead || detached != nil
              ? palette.mutedText.color : palette.agentAccent.color
          )
          .disabled(conversation == nil || (identity.digit != "0" && conversation?.owner == nil))
          .overlayFastHint(
            String(localized: "Detached — click to show its window"), palette: palette,
            enabled: detached != nil
          )
          .accessibilityLabel(
            (detached != nil
              ? String(localized: "Show detached fiszka") : String(localized: "Open conversation"))
              + ": " + name
          )
          .accessibilityIdentifier("overlay-intercom-name-\(identity.digit)")
        }
        .padding(.leading, expanded ? 0 : 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        // The road to the pane, then what I hear, then what I say.
        if expanded || (tier == .actions && !controller.stacksGroupControls) {
          OverlayIntercomFocusButton(
            roster: roster, channel: channelRow,
            palette: palette, diameter: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
          audioSlot(conversation, palette: palette, speaking: speaking, open: open)
        }
        if expanded {
          // This folds the conversation back into its one-row fiszka. An X
          // here read as the agent pill's X, which archives the agent
          // (Maciej, 2026-10-23); the ↗ is the overlay's return-to-mini glyph.
          Button("Collapse", systemImage: OverlayControlSymbols.returnToMini) {
            controller.close(identity)
          }
          .labelStyle(.iconOnly).buttonStyle(.plain)
          .frame(width: 26 * max(1, scale), height: 30 * max(1, scale))
          .conversationActionHover(
            restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
          )
          .overlayFastHint(String(localized: "Collapse into the group"), palette: palette)
          .accessibilityIdentifier("overlay-intercom-collapse-\(identity.digit)")
        }
      }
      .modifier(OverlayDisconnectedRowDim(disconnected: dead || detached != nil))
      .modifier(
        current
          ? roster.agentPillMenu(for: identity.digit, showsInlineError: false)
          : OverlayBroadcastPolicyMenu(members: [], palette: palette, enabled: false))
    }
  }

  private func stackedControls(palette: OverlayAppearancePalette) -> some View {
    let roster = OverlayChannelStatusView.intercomRoster(
      state: state, palette: palette, animates: !reduceMotion,
      onRevealConversation: onRevealConversation,
      onOpenConversation: { id in
        if id == identity.conversationID { controller.expand(identity) }
      })
    let channel = intercomChannelRow(state: state, digit: identity.digit)
    let dead = channel.map { roster.hasDeadFollower($0) } ?? false
    return HStack(spacing: 3) {
      OverlayIntercomFocusButton(
        roster: roster, channel: channel, palette: palette,
        diameter: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
      audioSlot(
        conversation, palette: palette, speaking: conversation?.replySpeaking == true,
        open: conversation.map { state.conversationMicrophoneOpen($0) } ?? false)
    }
    .modifier(
      OverlayDisconnectedRowDim(
        disconnected: dead || controller.detachedInstance(for: identity) != nil))
  }

  private func audioSlot(
    _ conversation: OverlayConversation?, palette: OverlayAppearancePalette,
    speaking: Bool, open: Bool
  ) -> some View {
    OverlayIntercomAudioControls(
      state: state, conversation: conversation, digit: identity.digit, palette: palette,
      open: open, speaking: speaking)
  }

  private func conversationBody(
    _ conversation: OverlayConversation, palette: OverlayAppearancePalette
  ) -> some View {
    OverlayIntercomConversationBody(
      state: state, conversation: conversation, palette: palette,
      focusRevision: controller.focusRevisions[identity] ?? 0)
  }
}

/// The microphone slot, driven by the shared recording commands. Channel 0
/// speaks through the left-facing megaphone of the main overlay's All row
/// (Maciej, 2026-10-23: the Intercom is where one talks to channel zero).
/// The header, group cards and durable instances paint the same control.
/// The fiszka's audio pair is the roster's pair: the same glyphs, monochrome
/// at rest and violet with a pulse when lit, the same hints — painted flat on
/// the fiszka, which is the surface, not in glass circles of their own
/// (Maciej, 2026-10-23: "ta sama kolejność to nie te same okrągłe pile").
/// Channel 0 keeps the roster's megaphone but not its larger tarcza: the bar
/// has one control height.
private struct OverlayIntercomAudioControls: View {
  @Environment(\.csTextScale) private var scale
  let state: OverlayState
  let conversation: OverlayConversation?
  let digit: String
  let palette: OverlayAppearancePalette
  let open: Bool
  let speaking: Bool

  var body: some View {
    let muted = conversation.flatMap { state.conversationPlaybackMuted($0) }
    OverlayAgentAudioControls(
      open: open, muted: muted,
      microphoneEnabled: microphoneEnabled,
      playbackEnabled: muted != nil && playbackEnabled,
      palette: palette,
      onMicrophone: {
        guard let conversation else { return }
        Task {
          await state.toggleConversationMicrophone(conversation, preservesPresentation: true)
        }
      },
      onPlayback: {
        guard let conversation else { return }
        Task { await state.toggleConversationPlayback(conversation) }
      },
      broadcast: digit == "0", speaking: speaking, broadcastEmphasis: false,
      chrome: false,
      diameter: OverlayIntercomWidgetLayout.controlHeight(scale: scale)
    )
    .fixedSize()
    .accessibilityIdentifier("overlay-intercom-audio-\(digit)")
  }

  private var microphoneEnabled: Bool {
    guard let conversation else { return false }
    return state.canToggleConversationMicrophone(conversation)
      && !state.pendingChannelToggles.contains(conversation.channel)
      && state.channelHudStates[conversation.channel]?.followerAlive != false
  }

  private var playbackEnabled: Bool {
    guard let conversation else { return false }
    return !state.pendingPlaybackOwners.contains(conversation.owner?.id ?? "")
      && state.channelHudStates[conversation.channel]?.followerAlive != false
  }
}

/// The roster's own status dot for one channel, so a fiszka and the agents
/// list never disagree (Maciej, 2026-10-23: "kropka pojedyncza, niosąca ten
/// sam status co na liście w pełnym widoku").
@MainActor
private func intercomStatusDot(
  _ roster: OverlayChannelStatusView, state: OverlayState, digit: String
) -> OverlayAgentDotKind {
  guard let channel = intercomChannelRow(state: state, digit: digit) else { return .idle }
  return roster.statusDotKind(for: channel)
}

/// The roster row behind a fiszka: the bus evidence the list reads.
@MainActor
private func intercomChannelRow(state: OverlayState, digit: String) -> OverlayChannelDelivery? {
  state.visibleChannelRows.first { $0.channel == digit }
}

/// The list's focus button, drawn flat on the fiszka from the pill menu's
/// own `focus` action: the same destination, availability and callback. The
/// slot is reserved even without a button so the audio pair never moves.
private struct OverlayIntercomFocusButton: View {
  let roster: OverlayChannelStatusView
  let channel: OverlayChannelDelivery?
  let palette: OverlayAppearancePalette
  let diameter: CGFloat

  var body: some View {
    if let channel, let action = roster.agentPillMenuActions(for: channel.channel)?.focus {
      let focusState = roster.panelFocusStates[channel.channel]
      Button(action: action.perform) {
        Group {
          if focusState?.isRequesting == true {
            ProgressView().controlSize(.mini)
          } else {
            Image(
              systemName: roster.panelFocusSymbol(
                focusState,
                target: roster.panelFocusTarget(for: channel)
                  ?? (roster.hasCodexCloudThread(channel) ? .codexThread : nil))
            )
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(
              focusState?.isSucceeded == true
                ? palette.successStatus.color
                : focusState?.isFailed == true
                  ? palette.errorStatus.color
                  : action.enabled ? palette.primaryText.color : palette.mutedText.color)
          }
        }
        .frame(width: diameter, height: diameter)
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .csFocusOutline()
      .disabled(!action.enabled)
      .overlayFastHint(roster.panelFocusHelp(for: channel, state: focusState), palette: palette)
      .accessibilityLabel(Text(verbatim: action.title))
      .accessibilityIdentifier("overlay-intercom-focus-\(channel.channel)")
    } else {
      Color.clear.frame(width: diameter, height: diameter)
    }
  }
}

/// The one shared conversation surface a card or an instance unrolls: the
/// existing messages, composer, attachments and manual Send. A view instance
/// is a new presentation of this shared state, never a second document.
private struct OverlayIntercomConversationBody: View {
  let state: OverlayState
  let conversation: OverlayConversation
  let palette: OverlayAppearancePalette
  let focusRevision: UInt64

  var body: some View {
    OverlayConversationView(
      conversation: conversation, palette: palette, topInset: 0, bottomInset: 12,
      pendingControls: state.pendingReplyControls, controlErrors: state.replyControlErrors,
      onControl: { message, stop in Task { await state.controlReply(message, stop: stop) } },
      composerFocusRevision: focusRevision,
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

/// One resize event: the gesture's mouse-down frame, the absolute delta, the
/// dragged edge, and the gesture's identity (one mouse-down, one UUID).
private typealias OverlayIntercomResizeAction = (
  CGRect, CGSize, OverlayIntercomWidgetLayout.ResizeEdge, UUID
) -> Void

private struct OverlayIntercomResizeHandles: View {
  /// The header keeps its corners for buttons; its left edge still resizes
  /// the group. Pills and cards expose the left edge and both bottom corners.
  var corners = true
  /// A pill's audio slot owns the true bottom-right corner, so its resize
  /// zone steps left by this much instead of covering the microphone.
  var trailingInset: CGFloat = 0
  let resize: OverlayIntercomResizeAction

  var body: some View {
    // Wider bands than the 5/14 pt they started with, and each one lights
    // its edge under the pointer: a resize that works but shows nothing
    // reads as impossible (Maciej, 2026-10-23).
    ZStack(alignment: .bottomLeading) {
      HStack {
        OverlayIntercomResizeHandle(edge: .left, resize: resize).frame(width: Self.edgeBand)
        Spacer(minLength: 0)
      }
      if corners {
        HStack {
          OverlayIntercomResizeHandle(edge: .bottomLeft, resize: resize)
            .frame(width: Self.cornerBand, height: Self.cornerBand)
          Spacer(minLength: 0)
          OverlayIntercomResizeHandle(edge: .bottomRight, resize: resize)
            .frame(width: Self.cornerBand, height: Self.cornerBand)
            .padding(.trailing, trailingInset)
        }
        .frame(height: Self.cornerBand)
      }
    }
  }

  static let edgeBand: CGFloat = 8
  static let cornerBand: CGFloat = 18
}

/// A durable fiszka: its own window over the shared conversation state. Mini
/// rests with the name, activity and one audio slot; hover reveals restore
/// and close. X and minimize are window operations, never disconnect/archive.
private struct OverlayIntercomInstanceCard: View {
  @Environment(\.colorScheme) private var colorScheme
  @Environment(\.csTextScale) private var scale
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @State private var hovering = false
  let state: OverlayState
  let instanceID: UUID
  let controller: OverlayIntercomWidgetController
  let onRevealConversation: () -> Void

  var body: some View {
    if let instance = controller.instance(for: instanceID) {
      content(instance)
    } else {
      Color.clear
    }
  }

  private func content(_ instance: OverlayIntercomInstance) -> some View {
    let palette = OverlayAppearancePalette.resolve(colorScheme)
    let conversation = state.conversations.first { instance.matches($0) }
    let mini = instance.minimized
    return VStack(spacing: 0) {
      headerRow(instance, conversation: conversation, palette: palette)
        .frame(height: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
      if instanceLayout(instance).stacked {
        instanceControls(instance, conversation: conversation, palette: palette)
          .frame(maxWidth: .infinity, alignment: .trailing)
          .frame(height: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
          .padding(.trailing, 4)
      }
      if !mini {
        if let conversation {
          OverlayIntercomConversationBody(
            state: state, conversation: conversation, palette: palette,
            focusRevision: controller.instanceFocusRevisions[instanceID] ?? 0
          )
          .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
          // The exact owner is gone from the roster. The saved words stay
          // readable; live actions withdraw instead of guessing a successor.
          Text("Conversation unavailable").csFont(12)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
      }
    }
    .foregroundStyle(palette.primaryText.color)
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    .background(OverlayCanvasBackdrop(palette: palette, reduceTransparency: reduceTransparency))
    .clipShape(RoundedRectangle(cornerRadius: mini ? 30 * max(1, scale) : 16))
    .overlay {
      RoundedRectangle(cornerRadius: mini ? 30 * max(1, scale) : 16)
        .strokeBorder(palette.border.color, lineWidth: 1).allowsHitTesting(false)
    }
    .overlay {
      if !mini {
        OverlayIntercomResizeHandles { frame, delta, edge, _ in
          controller.resizeInstance(instanceID, from: frame, delta: delta, edge: edge)
        }
      }
    }
    .onHover { hovering = $0 }
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-intercom-instance-\(instance.digit)")
  }

  private func headerRow(
    _ instance: OverlayIntercomInstance, conversation: OverlayConversation?,
    palette: OverlayAppearancePalette
  ) -> some View {
    let speaking = conversation?.replySpeaking == true
    let open = conversation.map { state.conversationMicrophoneOpen($0) } ?? false
    let current =
      intercomConversation(state: state, digit: instance.digit).map { instance.matches($0) }
      == true
    let mini = instance.minimized
    let layout = instanceLayout(instance)
    let roster = OverlayChannelStatusView.intercomRoster(
      state: state, palette: palette, animates: !reduceMotion,
      onRevealConversation: onRevealConversation,
      onOpenConversation: { id in
        if id == instance.conversationID { controller.focusInstance(instanceID) }
      })
    let channelRow = intercomChannelRow(state: state, digit: instance.digit)
    let dead = channelRow.map { roster.hasDeadFollower($0) } ?? false
    let nameRow = Text(verbatim: instance.name).csFont(11, .semibold).lineLimit(1)
      .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
      .contentShape(Rectangle())
    return HStack(spacing: 3) {
      Text(verbatim: instance.digit).csMono(10).foregroundStyle(palette.mutedText.color)
      if layout.tier != .icon, let channelRow {
        OverlayReceptionMarks(channel: channelRow, palette: palette, textScale: scale)
      }
      OverlayAgentStatusDot(
        kind: intercomStatusDot(roster, state: state, digit: instance.digit), palette: palette)
      // Mini restores on its name; an open window is already key under the
      // click, so its name is plain text and part of the drag surface.
      if layout.tier != .icon {
        if mini {
          Button {
            controller.setInstanceMinimized(instanceID, false)
          } label: {
            nameRow
          }
          .buttonStyle(.plain)
          .conversationActionHover(
            restingColor: speaking ? palette.agentAccent.color : palette.primaryText.color,
            hoverColor: palette.agentAccent.color
          )
          .accessibilityLabel(String(localized: "Restore fiszka") + ": " + instance.name)
        } else {
          // Glyphs would otherwise take the hit from the drag surface below:
          // the hosting view hands only text and buttons to SwiftUI.
          nameRow
            .foregroundStyle(speaking ? palette.agentAccent.color : palette.primaryText.color)
            .allowsHitTesting(false)
            .accessibilityLabel(Text(verbatim: instance.name))
        }
      }
      if layout.tier == .actions && !layout.stacked {
        OverlayIntercomFocusButton(
          roster: roster, channel: channelRow, palette: palette,
          diameter: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
        OverlayIntercomAudioControls(
          state: state, conversation: conversation, digit: instance.digit, palette: palette,
          open: open, speaking: speaking)
      }
      // Mini rests with the name, activity and the audio pair (Astra):
      // clicking the fiszka itself restores it, so no extra Restore button.
      if !mini {
        Button("Minimize", systemImage: "minus") {
          controller.setInstanceMinimized(instanceID, true)
        }
        .labelStyle(.iconOnly).buttonStyle(.plain)
        .frame(width: 22 * max(1, scale), height: 30 * max(1, scale))
        .conversationActionHover(
          restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
        )
        .accessibilityIdentifier("overlay-intercom-instance-mini-\(instance.digit)")
      }
      // The X keeps a fixed place so the audio slot never moves: a hover
      // only changes its visibility, never the row's geometry (Astra, V1 —
      // a reflow let a click aimed at the remembered microphone close the
      // fiszka).
      Button("Close", systemImage: "xmark") { controller.closeInstance(instanceID) }
        .labelStyle(.iconOnly).buttonStyle(.plain)
        .frame(width: 26 * max(1, scale), height: 30 * max(1, scale))
        .conversationActionHover(
          restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
        )
        .opacity(!mini || hovering ? 1 : 0)
        .allowsHitTesting(!mini || hovering)
        .accessibilityIdentifier("overlay-intercom-instance-close-\(instance.digit)")
    }
    .modifier(OverlayDisconnectedRowDim(disconnected: dead))
    .padding(.leading, 12)
    .padding(.trailing, 4)
    // Any free spot on the header moves the fiszka, not one grip on one side
    // (Maciej, 2026-10-23). `isMovableByWindowBackground` is inert under this
    // hosting view — it reports mouseDownCanMoveWindow everywhere, so AppKit
    // never decides — and `WindowDragGesture` does not move a non-key
    // nonactivating panel (Klaudiusz-div0's HID probe, macOS 27;
    // OverlayWindowDragRegion). The grip's own performDrag view, under the
    // whole header, is the one road that already moves these panels.
    .background(OverlayIntercomDragHandle(cursorHint: false))
    // The title bar carries the control glass of the composer and the name
    // pill, not a glass of its own (Maciej, 2026-10-23). The drag surface
    // sits inside the glass: outside it, the glass takes the hit and no
    // drag reaches the window (Klaudiusz-div0's HID probe).
    .modifier(OverlayControlGlass(cornerRadius: mini ? 30 * max(1, scale) : 16))
    // The same right-click menu as the row in the group: Show in Agents,
    // microphone, playback, archive. A detached fiszka had none.
    .modifier(
      current
        ? roster.agentPillMenu(for: instance.digit, showsInlineError: false)
        : OverlayBroadcastPolicyMenu(members: [], palette: palette, enabled: false))
  }

  /// Detached windows derive their own content from their own saved frame.
  /// No group tier or group dimensions participate in this decision.
  private func instanceLayout(_ instance: OverlayIntercomInstance)
    -> (tier: IntercomContentTier, stacked: Bool)
  {
    let metrics = IntercomContentMetrics(
      names: [instance.name], digits: [instance.digit], scale: scale)
    let windowSlots = (instance.minimized ? 26 : 48) * max(1, scale)
    let width = CGFloat(instance.width) - windowSlots
    let height = instance.minimized ? metrics.iconSide : CGFloat(instance.height)
    let tier = metrics.tier(width: width, cardHeight: height)
    return (tier, tier == .actions && width < metrics.actionsWidth)
  }

  private func instanceControls(
    _ instance: OverlayIntercomInstance,
    conversation: OverlayConversation?, palette: OverlayAppearancePalette
  ) -> some View {
    let roster = OverlayChannelStatusView.intercomRoster(
      state: state, palette: palette, animates: !reduceMotion,
      onRevealConversation: onRevealConversation,
      onOpenConversation: { id in
        if id == instance.conversationID { controller.focusInstance(instanceID) }
      })
    let channel = intercomChannelRow(state: state, digit: instance.digit)
    return HStack(spacing: 3) {
      OverlayIntercomFocusButton(
        roster: roster, channel: channel, palette: palette,
        diameter: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
      OverlayIntercomAudioControls(
        state: state, conversation: conversation, digit: instance.digit,
        palette: palette, open: conversation.map { state.conversationMicrophoneOpen($0) } ?? false,
        speaking: conversation?.replySpeaking == true)
    }
    .modifier(
      OverlayDisconnectedRowDim(disconnected: channel.map { roster.hasDeadFollower($0) } ?? false))
  }

}

private struct OverlayIntercomResizeHandle: NSViewRepresentable {
  let edge: OverlayIntercomWidgetLayout.ResizeEdge
  let resize: OverlayIntercomResizeAction

  func makeNSView(context: Context) -> OverlayIntercomResizeView { OverlayIntercomResizeView() }
  func updateNSView(_ nsView: OverlayIntercomResizeView, context: Context) {
    nsView.edge = edge
    nsView.resize = resize
  }
}

private final class OverlayIntercomResizeView: NSView {
  var edge = OverlayIntercomWidgetLayout.ResizeEdge.left
  var resize: OverlayIntercomResizeAction = { _, _, _, _ in }
  private var origin: CGPoint?
  private var originalFrame: CGRect?
  /// One mouse-down is one gesture. A bare frame cannot identify it: the
  /// next gesture can start from the same place and size (Astra on
  /// fca13c1c9), and only this identity keys the controller's handoffs.
  private var gesture = UUID()
  private var moved = false

  /// The band paints nothing: the cursor is the whole affordance. Maciej,
  /// 2026-10-23: the accent line this band used to light along the edge was
  /// "coś, o co nikt nie prosił" — the panel's frame already shows the size.
  override var isOpaque: Bool { false }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  override func mouseDown(with event: NSEvent) {
    origin = NSEvent.mouseLocation
    originalFrame = window?.frame
    gesture = UUID()
    moved = false
  }
  override func mouseDragged(with event: NSEvent) {
    guard let origin, let originalFrame else { return }
    moved = true
    let point = NSEvent.mouseLocation
    resize(
      originalFrame, CGSize(width: point.x - origin.x, height: point.y - origin.y), edge, gesture)
  }
  override func mouseUp(with event: NSEvent) {
    // A motionless click on a corner is not a resize: replaying it with a
    // zero delta would convert the card into an instance for nothing.
    if moved { mouseDragged(with: event) }
    origin = nil
    originalFrame = nil
    moved = false
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
