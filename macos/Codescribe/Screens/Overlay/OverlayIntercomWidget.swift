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
  /// One group-resize gesture scales from one baseline, identified by the
  /// resize view's per-mouse-down gesture id (a bare frame cannot tell two
  /// gestures apart when the second starts from the same place and size).
  @ObservationIgnored private var groupResizeGesture: (gesture: UUID, baseScale: Double)?
  /// A card's resize that converted it into an instance hands the rest of
  /// that same gesture to the instance.
  @ObservationIgnored private var resizeHandoff: (gesture: UUID, id: UUID)?
  fileprivate private(set) var rowOffset = 0
  fileprivate private(set) var availableHeight: CGFloat = 900
  fileprivate private(set) var presentation = OverlayIntercomPresentation()
  fileprivate private(set) var focusRevisions: [OverlayIntercomCardIdentity: UInt64] = [:]
  /// The durable desktop: detached fiszki and the group's proportional scale
  /// (Maciej, 2026-10-22). Tracked, so cards re-render on zoom and restore.
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

  /// Group geometry multiplies the text scale by the persisted group zoom.
  /// Detached instances keep their own geometry and the plain text scale.
  fileprivate var groupScale: CGFloat { CGFloat(desktop.groupScale) }
  private var effectiveScale: CGFloat { textScale.scale * groupScale }

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
          OverlayIntercomGroupZoom(controller: self) {
            OverlayIntercomHeader(state: state, controller: self)
          }
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
          OverlayIntercomGroupZoom(controller: self) {
            OverlayIntercomCard(
              state: state, identity: identity, index: index, controller: self,
              onRevealConversation: revealConversation)
          }
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
      scale: effectiveScale)
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
    let unit = max(1, textScale.scale)
    let nameFont = NSFont.systemFont(ofSize: 11 * textScale.scale, weight: .semibold)
    let nameWidth = (instance.name as NSString).size(withAttributes: [.font: nameFont]).width
    let height = OverlayIntercomWidgetLayout.controlHeight(scale: textScale.scale)
    // Grip 24 + dots ~24 + audio slot 30 + the X's fixed slot 26 + padding.
    let width = min(stored.width, nameWidth + (24 + 30 + 26) * unit + 36)
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
        visibleFrame: screenFrame, scale: effectiveScale)
      let frame: CGRect
      if expanded {
        frame = OverlayIntercomWidgetLayout.conversationFrame(
          below: row,
          size: sizes[identity]
            ?? OverlayIntercomWidgetLayout.conversationSize(scale: effectiveScale),
          visibleFrame: screenFrame, scale: effectiveScale)
      } else if presentation.hovered == identity {
        frame = OverlayIntercomWidgetLayout.previewFrame(
          beside: row, visibleFrame: screenFrame, scale: effectiveScale)
      } else {
        // A grouped fiszka inherits the header's size — one row, the column's
        // width. Only a detached instance owns its geometry (Maciej, 2026-10-23).
        frame = row
      }
      if panel.frame != frame {
        // Hover transitions land instantly: an animated setFrame moves the
        // window under a resting cursor, whose tracking churn fires fresh
        // enter/leave pairs and the stack "dances" once per crossing
        // (Maciej, 2026-10-23). Only the explicit expand animates.
        panel.setFrame(
          frame, display: true,
          animate: expanded && panel.isVisible
            && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
      }
      panel.setPresented(true, animated: !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
    }
    // Geometry and hover leave the order of presented windows alone. Only an
    // explicit expand below (or a native click/focus) raises a conversation.
  }

  fileprivate func hover(_ identity: OverlayIntercomCardIdentity, inside: Bool) {
    guard visible, !presentation.folded else { return }
    let before = presentation.hovered
    if inside { presentation.enter(identity) } else { presentation.leave(identity) }
    // Repeated enter/leave events for the same state move no windows: every
    // positionCards here is a geometry transition, and echoed events must
    // not replay one (the "dance", Maciej, 2026-10-23).
    guard presentation.hovered != before else { return }
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
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
  }

  fileprivate func toggleGroup() {
    presentation.folded.toggle()
    presentation.hovered = nil
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
  }

  /// Detaching makes the fiszka durable (Maciej, 2026-10-22). A closed saved
  /// instance of this exact conversation reopens instead of a duplicate; a
  /// second detach while one is already open deliberately duplicates.
  fileprivate func detach(_ identity: OverlayIntercomCardIdentity) {
    guard presentation.expanded.contains(identity) else { return }
    detachIntoInstance(identity)
    presentation.folded = true
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
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
    let fitted = OverlayIntercomWidgetLayout.resizedFrame(
      frame, delta: delta, edge: edge,
      minimum: OverlayIntercomWidgetLayout.minimumConversationSize(scale: textScale.scale),
      visibleFrame: visibleFrame(for: panel))
    if let id = detachIntoInstance(identity, frame: fitted) {
      resizeHandoff = (gesture: gesture, id: id)
    }
    presentation.folded = true
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
  }

  /// The group resizes like one window: the drag ratio becomes the shared
  /// proportional scale of every attached fiszka, persisted with the desktop.
  /// One gesture keeps one scale baseline — the resize view replays absolute
  /// deltas against its mouse-down frame (also on mouse-up), so compounding
  /// the current scale per event would run away (Olga, D1 on 181d27afb).
  fileprivate func resizeGroup(
    from frame: CGRect, delta: CGSize, edge: OverlayIntercomWidgetLayout.ResizeEdge, gesture: UUID
  ) {
    guard let header else { return }
    if groupResizeGesture?.gesture != gesture {
      groupResizeGesture = (gesture: gesture, baseScale: desktop.groupScale)
    }
    let base = groupResizeGesture?.baseScale ?? desktop.groupScale
    let fitted = OverlayIntercomWidgetLayout.resizedFrame(
      frame, delta: delta, edge: edge,
      minimum: CGSize(width: 80, height: OverlayIntercomWidgetLayout.headerHeight),
      visibleFrame: visibleFrame(for: header))
    let scaled = OverlayIntercomDesktopState.groupScale(
      current: base, currentWidth: frame.width, requestedWidth: fitted.width)
    guard scaled != desktop.groupScale else { return }
    desktop.groupScale = scaled
    saveDesktop()
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
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
      OverlayIntercomAudioSlot(
        state: state, conversation: broadcast, name: "All", digit: "0", palette: palette,
        speaking: broadcast?.replySpeaking == true,
        open: broadcast.map { state.conversationMicrophoneOpen($0) } ?? false)
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
    .overlay {
      // The group's left edge resizes every attached fiszka proportionally.
      // The header's corners keep their buttons; pills carry the corners.
      OverlayIntercomResizeHandles(corners: false) { frame, delta, edge, gesture in
        controller.resizeGroup(from: frame, delta: delta, edge: edge, gesture: gesture)
      }
    }
    .contextMenu {
      // The explicit road back to a saved closed fiszka, independent of the
      // roster. Reopening is a window operation: no microphone, no send.
      let closed = controller.closedInstances
      if closed.isEmpty {
        Text("No saved fiszki")
      } else {
        ForEach(closed) { instance in
          Button("\(instance.digit) · \(instance.name)") {
            controller.reopenInstance(instance.id)
          }
        }
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
private struct OverlayIntercomDragHandle: NSViewRepresentable {
  var onDrag: () -> Void = {}
  func makeNSView(context: Context) -> OverlayIntercomDragView { OverlayIntercomDragView() }
  func updateNSView(_ nsView: OverlayIntercomDragView, context: Context) { nsView.onDrag = onDrag }
}

private final class OverlayIntercomDragView: NSView {
  var onDrag: () -> Void = {}
  override var isOpaque: Bool { false }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  // A real drag moves the window (and detaches, where the grip detaches).
  // A click that never moves is not a detach and not a move. The window
  // reference is taken BEFORE onDrag: a detach swaps the panel's content
  // view, which removes this view from the window and nils self.window
  // (Astra, D2 trace on 05c013749).
  override func mouseDragged(with event: NSEvent) {
    let window = window
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
        OverlayIntercomResizeHandles { frame, delta, edge, gesture in
          controller.resize(identity, from: frame, delta: delta, edge: edge, gesture: gesture)
        }
      } else {
        // The group resizes like one window from a pill's left edge and
        // bottom corners: every attached fiszka scales proportionally. The
        // bottom-right zone steps left of the audio slot (Astra, V2).
        OverlayIntercomResizeHandles(
          trailingInset: OverlayIntercomWidgetLayout.controlHeight(scale: scale)
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
      .disabled(conversation == nil || (identity.digit != "0" && conversation?.owner == nil))
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
    OverlayIntercomAudioSlot(
      state: state, conversation: conversation, name: name, digit: identity.digit,
      palette: palette, speaking: speaking, open: open)
  }

  private func conversationBody(
    _ conversation: OverlayConversation, palette: OverlayAppearancePalette
  ) -> some View {
    OverlayIntercomConversationBody(
      state: state, conversation: conversation, palette: palette,
      focusRevision: controller.focusRevisions[identity] ?? 0)
  }
}

/// One audio slot: a microphone at rest, a speaker during a reply, driven by
/// the shared recording/playback commands. Group cards and durable instances
/// paint the same control.
private struct OverlayIntercomAudioSlot: View {
  @Environment(\.csTextScale) private var scale
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  let state: OverlayState
  let conversation: OverlayConversation?
  let name: String
  let digit: String
  let palette: OverlayAppearancePalette
  let speaking: Bool
  let open: Bool

  var body: some View {
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
    .disabled(audioDisabled)
    .accessibilityLabel(
      (speaking
        ? String(localized: "Mute agent replies")
        : open ? String(localized: "Stop recording") : String(localized: "Start recording"))
        + ": " + name
    )
    .accessibilityIdentifier("overlay-intercom-audio-\(digit)")
  }

  private var audioDisabled: Bool {
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
    ZStack(alignment: .bottomLeading) {
      HStack {
        OverlayIntercomResizeHandle(edge: .left, resize: resize).frame(width: 5)
        Spacer(minLength: 0)
      }
      if corners {
        HStack {
          OverlayIntercomResizeHandle(edge: .bottomLeft, resize: resize)
            .frame(width: 14, height: 14)
          Spacer(minLength: 0)
          OverlayIntercomResizeHandle(edge: .bottomRight, resize: resize)
            .frame(width: 14, height: 14)
            .padding(.trailing, trailingInset)
        }
        .frame(height: 14)
      }
    }
  }
}

/// Multiplies the surrounding text scale by the persisted group zoom. Group
/// windows live inside it; detached instances keep the plain text scale.
private struct OverlayIntercomGroupZoom<Content: View>: View {
  @Environment(\.csTextScale) private var scale
  let controller: OverlayIntercomWidgetController
  @ViewBuilder var content: Content

  var body: some View {
    content.environment(\.csTextScale, scale * controller.groupScale)
  }
}

/// A durable fiszka: its own window over the shared conversation state. Mini
/// rests with the name, activity and one audio slot; hover reveals restore
/// and close. X and minimize are window operations, never disconnect/archive.
private struct OverlayIntercomInstanceCard: View {
  @Environment(\.colorScheme) private var colorScheme
  @Environment(\.csTextScale) private var scale
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
    let reachable = current && state.channelHudStates[instance.digit]?.followerAlive == true
    let mini = instance.minimized
    return HStack(spacing: 3) {
      Image(systemName: "hand.draw").font(.system(size: 10 * scale))
        .frame(width: 24 * max(1, scale))
        .overlay(OverlayIntercomDragHandle())
        .accessibilityLabel("Move fiszka")
      Button {
        if mini {
          controller.setInstanceMinimized(instanceID, false)
        } else {
          controller.focusInstance(instanceID)
        }
      } label: {
        HStack(spacing: 5) {
          Text(verbatim: instance.name).csFont(11, .semibold).lineLimit(1)
          Circle().fill(reachable ? palette.successStatus.color : palette.mutedText.color)
            .frame(width: 5, height: 5)
          if let conversation, state.unreadReplies(in: conversation) > 0 {
            Circle().fill(palette.agentAccent.color).frame(width: 5, height: 5)
          }
          Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .conversationActionHover(
        restingColor: speaking ? palette.agentAccent.color : palette.primaryText.color,
        hoverColor: palette.agentAccent.color
      )
      .accessibilityLabel(
        (mini ? String(localized: "Restore fiszka") : String(localized: "Open conversation"))
          + ": " + instance.name)
      OverlayIntercomAudioSlot(
        state: state, conversation: conversation, name: instance.name, digit: instance.digit,
        palette: palette, speaking: speaking, open: open)
      // Mini rests with the name, activity and the one audio slot (Astra):
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
    .padding(.trailing, 4)
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
