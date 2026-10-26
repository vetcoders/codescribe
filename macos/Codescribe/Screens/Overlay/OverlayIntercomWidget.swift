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
  @ObservationIgnored private let diagnosticLog: DiagnosticLog?
  @ObservationIgnored private let idleNow: () -> TimeInterval
  @ObservationIgnored private let idleSchedule: OverlayAutoCollapse.Scheduler
  @ObservationIgnored private var header: OverlayIntercomPanel?
  @ObservationIgnored private var appWindowYield: OverlayAppWindowYield?
  @ObservationIgnored private var cards: [OverlayIntercomCardIdentity: OverlayIntercomPanel] = [:]
  @ObservationIgnored private var instancePanels: [UUID: OverlayIntercomPanel] = [:]
  @ObservationIgnored private var identities: [OverlayIntercomCardIdentity] = []
  @ObservationIgnored private var sizes: [OverlayIntercomCardIdentity: CGSize] = [:]
  @ObservationIgnored private var visible = false
  @ObservationIgnored private var positioning = false
  @ObservationIgnored private var invalidated = false
  /// A reconciliation receipt, never an archive authority. Only newly observed
  /// canonical archives retire windows; explicitly reopened history stays open.
  @ObservationIgnored private var reconciledArchivedOwners: Set<OverlayConversationOwner> = []
  #if DEBUG
    @ObservationIgnored private(set) var reconciliationRequests = 0
  #endif
  /// Mouse deltas are absolute from mouse-down. Keep the whole group's frame
  /// as the baseline.
  @ObservationIgnored private var groupResizeGesture:
    (gesture: UUID, frame: CGRect, count: Int, index: Int?)?
  @ObservationIgnored private var resizingGroup = false
  /// A row drag's pointer and header origin at its start, in screen points.
  private struct GroupDrag {
    let pointer: CGPoint
    let origin: CGPoint
    let screen: CGRect
    let frames: [(panel: OverlayIntercomPanel, frame: CGRect)]
  }
  @ObservationIgnored private var groupDrag: GroupDrag?
  private enum ResizeTarget {
    case group(CGRect, count: Int, headerHeight: CGFloat)
    case instance(UUID, CGRect)
    case expanded(OverlayIntercomCardIdentity, CGRect)
  }
  @ObservationIgnored private var resizeTarget: ResizeTarget?
  @ObservationIgnored private var resizePreview: OverlayIntercomResizePreview?
  @ObservationIgnored private var reconcileAfterResize = false
  @ObservationIgnored private var groupedIdleClock: OverlayAutoCollapse?
  @ObservationIgnored private var menuTracking = 0
  @ObservationIgnored private var menuObservers: [NSObjectProtocol] = []
  /// Keep the native drag surface and geometry alive until the button is released.
  /// Only a newly detached row needs its conversation content mounted afterward.
  private struct InstanceDrag {
    let id: UUID
    let panel: OverlayIntercomPanel
    let ownerID: String
    let conversationID: String
    let digit: String
    let expandedSize: CGSize?

    init(instance: OverlayIntercomInstance, panel: OverlayIntercomPanel, expandedSize: CGSize?) {
      id = instance.id
      self.panel = panel
      ownerID = instance.ownerID
      conversationID = instance.conversationID
      digit = instance.digit
      self.expandedSize = expandedSize
    }

    func matches(_ instance: OverlayIntercomInstance) -> Bool {
      id == instance.id && ownerID == instance.ownerID
        && conversationID == instance.conversationID && digit == instance.digit
    }
  }
  @ObservationIgnored private var instanceDrag: InstanceDrag?
  /// The frame each group card was last sent toward. A window animation
  /// does not move the frame until its first tick, so a fast crossing that
  /// retargets within that tick sees `panel.frame` still at the old frame;
  /// a guard on the live frame then skips the retarget and the first
  /// animation wins — the stack of fiszki left wide after a quick sweep
  /// (Maciej's recording, 2026-10-23 09:05). Compare against the target.
  @ObservationIgnored private var cardTargets: [OverlayIntercomCardIdentity: CGRect] = [:]
  private(set) var rowOffset = 0
  fileprivate private(set) var availableHeight: CGFloat = 900
  private(set) var presentation = OverlayIntercomPresentation()
  fileprivate private(set) var focusRevisions: [OverlayIntercomCardIdentity: UInt64] = [:]
  /// The durable desktop: detached fiszki and the group's content geometry.
  private(set) var desktop: OverlayIntercomDesktopState
  fileprivate private(set) var instanceFocusRevisions: [UUID: UInt64] = [:]
  private struct DockedLayout {
    let desktop: OverlayIntercomDesktopState
    let presentation: OverlayIntercomPresentation
    let rowOffset: Int
    let sizes: [OverlayIntercomCardIdentity: CGSize]
  }
  private var dockedLayout: DockedLayout?

  var canRestoreDockedLayout: Bool { dockedLayout != nil }

  /// Read the existing owner windows without creating or presenting a panel.
  var interactionWindowCounts: (owned: Int, visible: Int, key: Int) {
    let panels = [header].compactMap { $0 } + Array(cards.values) + Array(instancePanels.values)
    return (
      panels.count, panels.filter(\.isVisible).count, panels.filter(\.isKeyWindow).count
    )
  }

  init(
    state: OverlayState, textScale: TextScaleController,
    revealConversation: @escaping () -> Void = {},
    desktopStore: OverlayIntercomDesktopStore = OverlayIntercomDesktopStore(),
    diagnosticLog: DiagnosticLog? = nil,
    idleNow: @escaping () -> TimeInterval = { Date.timeIntervalSinceReferenceDate },
    idleSchedule: @escaping OverlayAutoCollapse.Scheduler = OverlayAutoCollapse.liveScheduler
  ) {
    self.state = state
    self.textScale = textScale
    self.revealConversation = revealConversation
    self.desktopStore = desktopStore
    self.diagnosticLog = diagnosticLog
    self.idleNow = idleNow
    self.idleSchedule = idleSchedule
    desktop = desktopStore.load()
    super.init()
    for name in [NSMenu.didBeginTrackingNotification, NSMenu.didEndTrackingNotification] {
      let entering = name == NSMenu.didBeginTrackingNotification
      menuObservers.append(NotificationCenter.default.addObserver(
        forName: name, object: nil, queue: .main
      ) { [weak self] _ in
        MainActor.assumeIsolated {
          guard let self else { return }
          self.menuTracking = max(0, self.menuTracking + (entering ? 1 : -1))
          if entering { self.groupedIdleClock?.suspend() } else { self.noteGroupedActivity() }
        }
      })
    }
  }

  #if DEBUG
    /// Exercise the same header without presenting any desktop windows.
    func offscreenHeader(width: CGFloat) -> some View {
      groupWidth = width
      return OverlayIntercomHeader(state: state, controller: self)
    }

    /// Renders the production card with explicit geometry, without creating or
    /// presenting its desktop windows. Capture and draft state remain shared.
    func offscreenCard(
      _ identity: OverlayIntercomCardIdentity, width: CGFloat, height: CGFloat, scale: CGFloat,
      expanded: Bool = false
    ) -> some View {
      contentMetrics = IntercomContentMetrics(
        names: state.conversations.map(\.name), digits: state.conversations.map(\.channel),
        scale: scale)
      groupWidth = width
      groupCardHeight = height
      groupTier = contentMetrics.tier(width: width, cardHeight: height)
      if expanded { presentation.expand(identity) }
      return OverlayIntercomCard(
        state: state, identity: identity, index: 0, controller: self,
        onRevealConversation: {})
    }

    func offscreenHover(_ identity: OverlayIntercomCardIdentity, palette: OverlayAppearancePalette)
      -> some View
    {
      OverlayIntercomHoverPreview(state: state, identity: identity, palette: palette)
    }
  #endif

  private var effectiveScale: CGFloat { textScale.scale }
  fileprivate private(set) var contentMetrics = IntercomContentMetrics(
    names: [], digits: [], scale: 1)
  fileprivate private(set) var groupTier: IntercomContentTier = .actions
  fileprivate private(set) var groupCardHeight: CGFloat = 30

  fileprivate private(set) var groupWidth: CGFloat = 0
  fileprivate var headerMetrics: IntercomHeaderMetrics {
    IntercomHeaderMetrics(
      agentCount: state.channelMapLayoutModel.agents.count,
      scale: effectiveScale)
  }
  fileprivate var headerNeedsPaging: Bool {
    !presentation.folded && hasOverflow
  }
  /// Keep every compact command, including both global audio controls.
  var minimumHeaderWidth: CGFloat {
    headerMetrics.minimumWidth(paging: headerNeedsPaging)
  }
  fileprivate var rowControls: IntercomRowControls {
    contentMetrics.rowControls(width: groupWidth)
  }

  private func saveDesktop() {
    desktopStore.save(desktop)
  }

  private func beginInteraction(action: StaticString) -> DiagnosticLog.Interaction {
    diagnosticLog?.beginInteraction(action: action, surface: "intercom")
      ?? DiagnosticLog.beginInteraction(action: action, surface: "intercom")
  }

  func setVisible(_ visible: Bool, relativeTo anchor: NSWindow?) {
    guard !invalidated else { return }
    self.visible = visible
    guard visible else {
      finishInteractiveResize(cancelled: true)
      groupedIdleClock?.forget()
      // Reuse the same windows when the layout returns. Durable drafts never
      // belong to these views, and hiding never changes the capture route.
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
        }.csFocusPolicy())
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
      let observer = OverlayAppWindowYield(panel: panel, state: state, foldsOverlay: false) { [weak self] window in
        guard let self else { return }
        let panels =
          [self.header].compactMap { $0 }
          + Array(self.cards.values) + Array(self.instancePanels.values)
        for panel in panels { panel.yieldToAppWindow(window) }
      }
      appWindowYield = observer
      observer.start()
    }
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
    materializeInstances()
    header?.orderFrontRegardless()
  }

  func invalidate() {
    guard !invalidated else { return }
    setVisible(false, relativeTo: nil)
    appWindowYield?.invalidate()
    appWindowYield = nil
    for observer in menuObservers { NotificationCenter.default.removeObserver(observer) }
    menuObservers = []
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

  fileprivate func reconcile(
    chips: [OverlayChannelMapChip], scale: CGFloat, persistTier: Bool = true,
    materializeRetiredInstances: Bool = true
  ) {
    #if DEBUG
      reconciliationRequests += 1
    #endif
    guard !invalidated, visible, let header else { return }
    guard resizePreview == nil else {
      reconcileAfterResize = true
      return
    }
    if availableHeight != screenFrame.height { availableHeight = screenFrame.height }
    identities = intercomIdentities(state: state, chips: chips)
    let clearedHistoryOpening = desktop.clearHistoryOpenings(for: identities)
    // Channel reuse cannot replace the editor underneath a draft. The old
    // exact owner's open card survives as a durable detached instance.
    for identity in presentation.expanded where !identities.contains(identity) {
      detachIntoInstance(identity)
    }
    let newlyArchived = state.archivedAgentOwners.subtracting(reconciledArchivedOwners)
    reconciledArchivedOwners = state.archivedAgentOwners
    let retiredInstances = desktop.closeArchivedInstances(owners: newlyArchived)
    if retiredInstances || clearedHistoryOpening {
      saveDesktop()
    }
    if retiredInstances && materializeRetiredInstances {
      materializeInstances()
    }
    // Each conversation has one visible placement: in the group or detached.
    let retained = Set(groupIdentities)
    for identity in Set(cards.keys).subtracting(retained) {
      let panel = cards.removeValue(forKey: identity)
      panel?.delegate = nil
      panel?.contentView = nil
      panel?.close()
      sizes.removeValue(forKey: identity)
      cardTargets.removeValue(forKey: identity)
      focusRevisions.removeValue(forKey: identity)
    }
    // Every agent's name sizes the group, standing in it or not, so detaching
    // a fiszka or handing it back never reflows the column.
    contentMetrics = IntercomContentMetrics(
      names: identities.map(\.name), digits: identities.map(\.displayDigit), scale: scale)
    // Establish the actual row budget before measuring optional pager slots.
    // A display too narrow for names retains the existing icon-row geometry.
    groupCardHeight =
      screenFrame.width < contentMetrics.minimumNameWidth
      ? contentMetrics.iconSide
      : contentMetrics.fittedCardHeight(
        CGFloat(desktop.groupRowHeight ?? Double(contentMetrics.iconSide)),
        count: groupIdentities.count, available: screenFrame.height)
    let headerMetrics = self.headerMetrics
    let defaultWidth = headerMetrics.width(
      showsCount: true, paging: headerNeedsPaging)
    let width = max(
      // If the screen itself is narrower, the native window clips the row;
      // no audio-hidden tier or claim that all commands fit is introduced.
      min(headerMetrics.minimumWidth(paging: headerNeedsPaging), screenFrame.width),
      contentMetrics.fittedWidth(
        CGFloat(desktop.groupWidth ?? Double(defaultWidth)), available: screenFrame.width))
    groupWidth = width
    groupTier = contentMetrics.tier(width: width, cardHeight: groupCardHeight)
    if desktop.groupTier != groupTier {
      desktop.groupTier = groupTier
      if persistTier { saveDesktop() }
    }
    let frame = CGRect(
      x: header.frame.maxX - width, y: header.frame.minY,
      width: width, height: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
    positioning = true
    let fitted = OverlayIntercomWidgetLayout.clampedHeader(
      frame, count: presentation.folded ? 0 : visibleRows.count,
      visibleFrame: screenFrame, scale: scale, cardHeight: groupCardHeight)
    if header.frame != fitted { header.setFrame(fitted, display: true) }
    positioning = false
    // Detached conversations are absent; the remaining rows use compact indices.
    for (index, identity) in groupIdentities.enumerated() where cards[identity] == nil {
      let panel = OverlayIntercomPanel()
      panel.delegate = self
      panel.onScroll = { [weak self] delta in self?.scrollGroup(delta > 0 ? -1 : 1) }
      panel.contentView = NSHostingView(
        rootView: TextScaleRoot(controller: textScale) {
          OverlayIntercomCard(
            state: state, identity: identity, index: index, controller: self,
            onRevealConversation: revealConversation)
        }.csFocusPolicy())
      cards[identity] = panel
    }
    positionCards()
  }

  private var screenFrame: CGRect { visibleFrame(for: header) }

  private var groupIdentities: [OverlayIntercomCardIdentity] { desktop.groupRows(of: identities) }

  fileprivate var visibleRows: Range<Int> {
    OverlayIntercomWidgetLayout.visibleRange(
      count: groupIdentities.count, offset: rowOffset, visibleHeight: availableHeight,
      scale: effectiveScale,
      cardHeight: groupCardHeight)
  }

  fileprivate var hasOverflow: Bool { visibleRows.count < groupIdentities.count }
  fileprivate var hasPrevious: Bool { visibleRows.lowerBound > 0 }
  fileprivate var hasNext: Bool { visibleRows.upperBound < groupIdentities.count }

  func presentsConversation(_ conversationID: String?) -> Bool {
    guard visible, let conversationID else { return false }
    if groupCardsShowComposer || !presentation.expanded.isEmpty,
      cards.contains(where: { identity, panel in
      identity.conversationID == conversationID && panel.isPresented
        && (groupCardsShowComposer || presentation.expanded.contains(identity))
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
    guard resizePreview == nil else {
      reconcileAfterResize = true
      return
    }
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
      // Replacing the view or resizing its panel before AppKit finishes a drag
      // can interrupt the native surface that started the gesture.
      if instanceDrag?.id == instance.id { continue }
      let panel: OverlayIntercomPanel
      if let existing = instancePanels[instance.id] {
        panel = existing
      } else {
        panel = OverlayIntercomPanel()
        panel.delegate = self
        installInstanceContent(on: panel, id: instance.id)
        instancePanels[instance.id] = panel
      }
      panel.acceptsConversationFocus = !instance.minimized
      panel.yieldToAppWindow(NSApp.isActive ? NSApp.keyWindow : nil)
      positioning = true
      let fitted = instanceFrame(instance, for: panel)
      if panel.frame != fitted { panel.setFrame(fitted, display: true) }
      positioning = false
      panel.setPresented(
        visible, animated: !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
    }
  }

  private func installInstanceContent(on panel: OverlayIntercomPanel, id: UUID) {
    panel.contentView = NSHostingView(
      rootView: TextScaleRoot(controller: textScale) {
        OverlayIntercomInstanceCard(
          state: state, instanceID: id, controller: self,
          onRevealConversation: revealConversation)
      }.csFocusPolicy())
  }

  func beginInstanceDrag(_ id: UUID) -> Bool {
    guard instanceDrag == nil, visible, let instance = instance(for: id), !instance.closed,
      let panel = instancePanels[id], panel.isPresented
    else { return false }
    instanceDrag = InstanceDrag(instance: instance, panel: panel, expandedSize: nil)
    return true
  }

  func finishInstanceDrag(at point: CGPoint? = nil) {
    guard let drag = instanceDrag else { return }
    instanceDrag = nil
    guard let panel = instancePanels[drag.id], panel === drag.panel,
      let index = desktop.instances.firstIndex(where: { drag.matches($0) })
    else { return }
    if drag.expandedSize == nil {
      var stored = desktop.instances[index].frame
      stored.origin.x = panel.frame.maxX - stored.width
      stored.origin.y = panel.frame.maxY - stored.height
      desktop.instances[index].frame = stored
    }
    if let expandedSize = drag.expandedSize {
      // The original hosting view stays in the panel throughout native tracking.
      // Mount the conversation only now, including for a concurrently retired
      // window, so a later explicit history reopen has the right content.
      installInstanceContent(on: panel, id: drag.id)
      if !desktop.instances[index].closed {
        desktop.instances[index].frame = CGRect(
          x: panel.frame.minX, y: panel.frame.maxY - expandedSize.height,
          width: expandedSize.width, height: expandedSize.height)
      }
    }
    guard visible, !invalidated, !desktop.instances[index].closed else {
      saveDesktop()
      materializeInstances()
      return
    }
    let instance = desktop.instances[index]
    // Geometry is only a drop target. The canonical conversation projection
    // must still identify this exact current owner; saved history never docks
    // under a replacement agent who happens to use the same channel.
    let current = intercomConversation(state: state, digit: instance.digit)
      .map { instance.matches($0) } == true
    let point = point ?? NSEvent.mouseLocation
    // The header uses native ordering, not the cards' setPresented lifecycle.
    // It remains a drop target when the group is folded or its last row left.
    let targets = [header].compactMap { $0 }.filter(\.isVisible)
      + cards.values.filter(\.isPresented)
    if current && targets.contains(where: { $0.frame.contains(point) }) {
      closeInstance(drag.id)
    } else {
      materializeInstances()
      // Materialization contains the window inside its display. Persist the
      // geometry actually presented after that clamp, not the raw drag frame.
      // A mini still retains its full expanded frame underneath the pill.
      if !instance.minimized, instancePanels[drag.id] === panel,
        let settledIndex = desktop.instances.firstIndex(where: { drag.matches($0) })
      {
        desktop.instances[settledIndex].frame = panel.frame
      }
      saveDesktop()
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
      // Restore the full conversation inside its current display, even when
      // the saved frame came from a larger screen or an explicitly lowered drag.
      return OverlayIntercomWidgetLayout.cardFrame(
        below: stored, size: stored.size, visibleFrame: visibleFrame)
    }
    // Mini is a compact pill anchored at the stored window's top-right: its
    // width comes from its own content, never from the expanded conversation
    // (Astra, V3 — a 900 pt conversation must not leave a 900 pt mini). The
    // expanded frame survives untouched for restore.
    let height = OverlayIntercomWidgetLayout.controlHeight(scale: textScale.scale)
    let width = min(stored.width, minimizedWidth(name: instance.name, digit: instance.displayDigit))
    return OverlayIntercomWidgetLayout.cardFrame(
      below: CGRect(x: stored.maxX - width, y: stored.maxY, width: width, height: 0),
      size: CGSize(width: width, height: height), visibleFrame: visibleFrame)
  }

  /// One measure for a mini's width, from its own name and digit.
  private func minimizedWidth(name: String, digit: String) -> CGFloat {
    let nameFont = NSFont.systemFont(ofSize: 11 * textScale.scale, weight: .semibold)
    let nameWidth = (name as NSString).size(withAttributes: [.font: nameFont]).width
    let digitFont = CSFont.nsMono(10 * textScale.scale)
    let digitWidth = (digit as NSString).size(withAttributes: [.font: digitFont]).width
    return OverlayIntercomWidgetLayout.minimizedInstanceWidth(
      text: nameWidth + digitWidth, scale: textScale.scale)
  }

  func setInstanceMinimized(_ id: UUID, _ minimized: Bool) {
    guard visible, let index = desktop.instances.firstIndex(where: { $0.id == id }) else { return }
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
  /// The agent's row comes back into the group.
  func closeInstance(_ id: UUID) {
    let current = instance(for: id).map { instance in
      intercomConversation(state: state, digit: instance.digit).map { instance.matches($0) } == true
    } ?? false
    let interaction = beginInteraction(
      action: current ? "intercom.instance.dock" : "intercom.instance.closeHistory")
    guard let index = desktop.instances.firstIndex(where: { $0.id == id }) else {
      interaction.finish(.refused, effect: .noChange, cleanup: .notRequired, reason: .notFound)
      return
    }
    let wasClosed = desktop.instances[index].closed
    instancePanels[id]?.resignKey()
    desktop.instances[index].closed = true
    saveDesktop()
    materializeInstances()
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
    interaction.finish(
      .unverified, effect: wasClosed ? .noChange : .stateChanged, cleanup: .pending)
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
    resizeTarget = .instance(id, fitted)
    showResizePreview(frame: fitted, panels: [panel], group: false)
  }

  fileprivate func focusInstance(_ id: UUID) {
    guard visible, let panel = instancePanels[id], instance(for: id)?.minimized == false else {
      return
    }
    instanceFocusRevisions[id, default: 0] &+= 1
    panel.makeKeyAndOrderFront(nil)
  }

  /// A pill's click and a refused second detach both land here: the open
  /// fiszka comes forward, restored from mini when it was folded.
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

  func reopenInstance(_ id: UUID) {
    let interaction = beginInteraction(action: "intercom.instance.reopenSaved")
    guard let index = desktop.instances.firstIndex(where: { $0.id == id }) else {
      interaction.finish(.refused, effect: .noChange, cleanup: .notRequired, reason: .notFound)
      return
    }
    let wasClosed = desktop.instances[index].closed
    desktop.instances[index].recordHistoryOpening(archivedOwners: state.archivedAgentOwners)
    desktop.instances[index].closed = false
    saveDesktop()
    materializeInstances()
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
    interaction.finish(
      .unverified, effect: wasClosed ? .stateChanged : .noChange, cleanup: .pending)
  }

  fileprivate func scrollGroup(_ steps: Int) {
    guard visible, !presentation.folded else { return }
    rowOffset = min(
      max(0, visibleRows.lowerBound + steps), max(0, groupIdentities.count - visibleRows.count))
    positionCards()
  }

  private func visibleFrame(for panel: NSWindow?) -> CGRect {
    visibleFrame(around: panel?.frame ?? .zero)
  }

  private func visibleFrame(around frame: CGRect) -> CGRect {
    // Elect the screen under the grip. A tall body on a vertically offset
    // display must not pull its header back onto the body's display.
    let height = min(frame.height, OverlayIntercomWidgetLayout.controlHeight(scale: effectiveScale))
    let grip = CGRect(x: frame.minX, y: frame.maxY - height, width: frame.width, height: height)
    let screens = NSScreen.screens.map(\.visibleFrame)
    let intersecting = screens.filter { $0.intersects(grip) }
    if let visible = intersecting.max(by: {
      $0.intersection(grip).width * $0.intersection(grip).height
        < $1.intersection(grip).width * $1.intersection(grip).height
    }) {
      return visible
    }
    // A removed monitor or a pointer beyond an edge has no intersection.
    // Recover onto the nearest display, including negative/offset origins.
    let point = CGPoint(x: grip.midX, y: grip.midY)
    return screens.min(by: {
      hypot(
        max($0.minX - point.x, 0, point.x - $0.maxX),
        max($0.minY - point.y, 0, point.y - $0.maxY))
        < hypot(
          max($1.minX - point.x, 0, point.x - $1.maxX),
          max($1.minY - point.y, 0, point.y - $1.maxY))
    }) ?? NSScreen.main?.visibleFrame ?? CGRect(x: 0, y: 0, width: 1440, height: 900)
  }

  private func positionCards() {
    guard visible, let header else { return }
    positioning = true
    defer { positioning = false }
    let range = visibleRows
    let group = groupIdentities
    let inlineComposer = groupCardsShowComposer
    var siblingAppeared = false
    for (identity, panel) in cards {
      let expanded = presentation.expanded.contains(identity)
      panel.acceptsConversationFocus = expanded || inlineComposer
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
      } else {
        // A grouped fiszka inherits the header's size — one row, the column's
        // width, hovered or not. Only a detached instance owns its geometry
        // (Maciej, 2026-10-23).
        frame = row
      }
      if cardTargets[identity] != frame {
        cardTargets[identity] = frame
        // Slides must CONVERGE: a new target retargets the running animation
        // instead of queueing another, and the duration is fixed instead of
        // distance-scaled. setFrame(animate:) did neither, so a fast change
        // left the stack dancing long after it settled (Maciej, 2026-10-23).
        if panel.isVisible && !resizingGroup && panel.frame.size == frame.size
          && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        {
          NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.1
            context.timingFunction = CAMediaTimingFunction(name: .easeOut)
            panel.animator().setFrame(frame, display: true)
          }
        } else {
          panel.setFrame(frame, display: true)
        }
      }
      let wasPresented = panel.isPresented
      panel.setPresented(true, animated: !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
      if !wasPresented && !expanded { siblingAppeared = true }
    }
    // A newly shown sibling must not cover the expanded editor. Geometry,
    // hover and ordinary content refreshes do not reorder existing windows.
    if siblingAppeared { raisePresentedGroup() }
  }

  fileprivate func expand(_ identity: OverlayIntercomCardIdentity) {
    guard visible, identity.canExpand else { return }
    if let open = desktop.openInstance(for: identity) {
      showInstance(open.id)
      return
    }
    guard let conversation = state.conversations.first(where: { identity.matches($0) }) else {
      return
    }
    if conversation.owner == nil {
      openMailbox(conversation.id)
      return
    }
    presentation.expand(identity)
    focusRevisions[identity, default: 0] &+= 1
    positionCards()
    // Only this explicit click grants keyability. Hover never activates the app.
    cards[identity]?.makeKeyAndOrderFront(nil)
    noteGroupedActivity()
  }

  private var groupCardsShowComposer: Bool {
    contentMetrics.compactPreviewLayout(width: groupWidth, cardHeight: groupCardHeight)?
      .showsComposer == true
  }

  /// Reuse the widget's chosen idle interval and clock implementation. Only
  /// click-open group cards owe a return; detached instances never enter it.
  fileprivate func noteGroupedActivity() {
    guard !presentation.expanded.isEmpty, visible else {
      groupedIdleClock?.forget()
      return
    }
    if groupedIdleClock == nil {
      let clock = OverlayAutoCollapse(
        now: idleNow, schedule: idleSchedule)
      clock.onDeadline = { [weak self] in self?.groupedDeadlineReached() }
      groupedIdleClock = clock
    }
    guard let clock = groupedIdleClock else { return }
    clock.baseIdleSeconds = TimeInterval(state.autoFoldSeconds)
    clock.noteExpansion(from: .midi)
    // The wake rechecks capture and modal holds. A capture already active at
    // expansion must not leave the return permanently suspended after it ends.
    clock.restartDeadline(after: TimeInterval(state.autoFoldSeconds))
  }

  private var groupedInteractionHeld: Bool {
    resizePreview != nil || groupDrag != nil || instanceDrag != nil
      || state.recording || state.warmingUp || state.transcribing || state.channelAudioCaptureActive
      || !state.pendingTextMessages.isEmpty || menuTracking > 0 || NSApp.modalWindow != nil
      || NSEvent.pressedMouseButtons != 0
  }

  private func groupedDeadlineReached() {
    guard visible, !presentation.expanded.isEmpty else {
      groupedIdleClock?.forget()
      return
    }
    if groupedInteractionHeld {
      groupedIdleClock?.restartDeadline(after: TimeInterval(state.autoFoldSeconds))
      return
    }
    for identity in presentation.expanded { cards[identity]?.resignKey() }
    presentation.expanded.removeAll()
    groupedIdleClock?.forget()
    positionCards()
  }

  /// Mailbox buttons select the exact ownerless conversation and present its
  /// fiszka when visible, without changing capture or message delivery.
  func openMailbox(_ conversationID: String) {
    guard let conversation = state.conversations.first(where: { $0.id == conversationID }),
      conversation.owner == nil,
      OverlayIntercomCardIdentity(digit: conversation.channel, conversation: conversation).canExpand
    else { return }
    let identity = OverlayIntercomCardIdentity(
      digit: conversation.channel, conversation: conversation)
    state.chooseConversation(conversation.id)
    if let open = desktop.openInstance(for: identity) {
      showInstance(open.id)
      return
    }
    if let saved = desktop.instances.first(where: {
      $0.closed && $0.conversationID == identity.conversationID
        && $0.ownerID == (identity.ownerID ?? "")
    }) {
      reopenInstance(saved.id)
      showInstance(saved.id)
      return
    }
    guard let id = detachIntoInstance(identity) else { return }
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
    showInstance(id)
  }

  fileprivate func close(_ identity: OverlayIntercomCardIdentity) {
    cards[identity]?.resignKey()
    presentation.close(identity)
    if presentation.expanded.isEmpty { groupedIdleClock?.forget() }
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
  }

  func toggleGroup() {
    let action: StaticString = presentation.folded ? "intercom.group.unfold" : "intercom.group.fold"
    let interaction = beginInteraction(action: action)
    presentation.folded.toggle()
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
    interaction.finish(.unverified, effect: .stateChanged, cleanup: .pending)
  }

  /// Docking and restoring are explicit menu actions, never an ambiguous toggle.
  /// Closed and minimized windows keep their records; every current roster row
  /// becomes visible in the group, including rows hidden by group folding.
  func dockAll() {
    let interaction = beginInteraction(action: "intercom.desktop.dock")
    guard !invalidated else {
      interaction.finish(.refused, effect: .noChange, cleanup: .notRequired, reason: .lifetimeEnded)
      return
    }
    if dockedLayout == nil {
      dockedLayout = DockedLayout(
        desktop: desktop, presentation: presentation, rowOffset: rowOffset, sizes: sizes)
    }
    hideAll(foldGroup: false)
    interaction.finish(.unverified, effect: .stateChanged, cleanup: .pending)
  }

  func restoreDockedLayout() {
    let interaction = beginInteraction(action: "intercom.desktop.restore")
    guard !invalidated, let previous = dockedLayout else {
      interaction.finish(.refused, effect: .noChange, cleanup: .notRequired, reason: .notFound)
      return
    }
    dockedLayout = nil
    let latestInstances = desktop.instances
    let previousIDs = Set(previous.desktop.instances.map(\.id))
    let newInstances = desktop.instances.filter { !previousIDs.contains($0.id) }
    desktop = previous.desktop
    // Geometry snapshots must not discard a later exact-owner history-open
    // receipt, including when reopen reused an existing instance UUID.
    for index in desktop.instances.indices {
      let restored = desktop.instances[index]
      if let latest = latestInstances.first(where: {
        $0.id == restored.id && $0.ownerID == restored.ownerID
          && $0.conversationID == restored.conversationID && $0.digit == restored.digit
      }) {
        desktop.instances[index].historyOpening = latest.historyOpening
      }
    }
    for instance in newInstances { desktop.upsert(instance) }
    // A layout snapshot cannot undo a later canonical archive.
    desktop.closeArchivedInstances(owners: state.archivedAgentOwners)
    presentation = previous.presentation
    rowOffset = previous.rowOffset
    sizes = previous.sizes
    materializeInstances()
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale, persistTier: false)
    if desktop.closeArchivedInstances(owners: state.archivedAgentOwners) {
      materializeInstances()
    }
    saveDesktop()
    interaction.finish(.unverified, effect: .stateChanged, cleanup: .pending)
  }

  /// Dock every open fiszka and fold the group in one presentation transaction.
  /// Closed records retain their exact owners, geometry and saved-window road
  /// back. Drafts, sessions and recording remain owned by OverlayState.
  func hideAll(foldGroup: Bool = true) {
    guard !invalidated else { return }
    finishInteractiveResize(cancelled: true)
    groupedIdleClock?.forget()
    for index in desktop.instances.indices where !desktop.instances[index].closed {
      desktop.instances[index].closed = true
    }
    presentation.expanded.removeAll()
    presentation.folded = foldGroup
    rowOffset = 0
    groupResizeGesture = nil
    groupDrag = nil
    for panel in instancePanels.values { panel.resignKey() }
    for panel in cards.values {
      panel.resignKey()
      panel.setPresented(false, animated: false)
    }
    materializeInstances()
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale, persistTier: false)
    saveDesktop()
  }

  // MARK: Middle-click chooser host

  /// The chooser's rows: the header's channel 0 first, as the agents list
  /// leads with 0 · All, then every agent in roster order. An agent standing
  /// in its own window is still an agent (Maciej, 2026-10-23 seal: "pomija —
  /// to bardzo źle"): the wheel reaches its window too.
  func chooserTargets() -> [OverlayConversation] {
    let agents = identities.compactMap { identity in
      state.conversations.first { identity.matches($0) }
    }
    return (intercomConversation(state: state, digit: "0").map { [$0] } ?? []) + agents
  }

  /// The middle click summons the group: unfolded, in front, and with no
  /// conversation unrolled over the rows the wheel walks.
  func presentChooser() {
    guard visible else { return }
    for identity in presentation.expanded { close(identity) }
    revealGroup()
  }

  /// A left click on the chooser's focus opens that row's conversation here.
  /// Channel 0 uses the same existing conversation surface as its header button.
  func openChosenConversation(_ conversationID: String) {
    guard visible else { return }
    revealGroup()
    if let identity = identities.first(where: { $0.conversationID == conversationID }) {
      expand(identity)
    } else {
      openMailbox(conversationID)
    }
  }

  /// The focused row stays on screen: the group pages to it, and an agent in
  /// its own window has that window brought above the others, never made key.
  fileprivate func revealChooserFocus(_ conversationID: String?) {
    guard visible else { return }
    if let conversationID,
      let open = desktop.instances.first(where: {
        !$0.closed && $0.conversationID == conversationID
      })
    {
      instancePanels[open.id]?.orderFront(nil)
      return
    }
    let group = groupIdentities
    guard visible, !presentation.folded, let conversationID,
      let index = group.firstIndex(where: { $0.conversationID == conversationID })
    else { return }
    let range = visibleRows
    guard !range.contains(index) else { return }
    let start = index < range.lowerBound ? index : index - range.count + 1
    rowOffset = min(max(0, start), max(0, group.count - range.count))
    positionCards()
  }

  /// The main overlay's mini counter in the vertical layout: the group
  /// unfolds and stands in front, its detached windows still above it.
  func revealGroup() {
    guard visible else { return }
    if presentation.folded { toggleGroup() }
    raisePresentedGroup()
  }

  private func raisePresentedGroup() {
    for identity in presentation.frontOrder(groupIdentities) {
      if let panel = cards[identity], panel.isPresented { panel.orderFrontRegardless() }
    }
    for instance in desktop.instances where !instance.closed {
      if let panel = instancePanels[instance.id], panel.isPresented { panel.orderFrontRegardless() }
    }
    header?.orderFrontRegardless()
  }

  /// Detaching makes the fiszka durable (Maciej, 2026-10-22). One agent keeps
  /// one open window (Maciej, 2026-10-23): while it stands somewhere, the
  /// gesture shows it instead of minting a copy. Its grouped row returns only
  /// when the detached window is docked or closed.
  /// Returns whether this card's panel became that window, so the grip knows
  /// whether there is anything to drag.
  @discardableResult
  func detach(_ identity: OverlayIntercomCardIdentity) -> Bool {
    guard identity.canExpand else {
      let interaction = beginInteraction(action: "intercom.instance.detach")
      interaction.finish(.refused, effect: .noChange, cleanup: .notRequired, reason: .ownerMismatch)
      return false
    }
    if let open = desktop.openInstance(for: identity) {
      let interaction = beginInteraction(action: "intercom.instance.revealExisting")
      close(identity)
      showInstance(open.id)
      // An attempted reveal is not a verified raise, mini restore or focus.
      interaction.finish(.unverified, effect: .unverified, cleanup: .unverified)
      return false
    }
    let interaction = beginInteraction(action: "intercom.instance.detach")
    var frame = cards[identity]?.frame
    if !presentation.expanded.contains(identity), let current = frame {
      let size = OverlayIntercomWidgetLayout.conversationSize(scale: effectiveScale)
      // The compact grip keeps its screen position as the conversation opens
      // downward from this card's top-left, in the same native panel.
      frame = CGRect(
        x: current.minX, y: current.maxY - size.height,
        width: size.width, height: size.height)
    }
    let detached = detachIntoInstance(identity, frame: frame) != nil
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
    interaction.finish(
      detached ? .unverified : .refused, effect: detached ? .stateChanged : .noChange,
      cleanup: detached ? .pending : .notRequired, reason: detached ? .none : .ownerMismatch)
    return detached
  }

  func detachForDrag(_ identity: OverlayIntercomCardIdentity) -> Bool {
    guard instanceDrag == nil, visible, identity.canExpand,
      desktop.openInstance(for: identity) == nil, let panel = cards[identity],
      intercomConversation(state: state, digit: identity.digit).map({ identity.matches($0) }) == true
    else { return false }
    let size = presentation.expanded.contains(identity)
      ? panel.frame.size : OverlayIntercomWidgetLayout.conversationSize(scale: effectiveScale)
    let frame = CGRect(
      x: panel.frame.minX, y: panel.frame.maxY - size.height,
      width: size.width, height: size.height)
    let interaction = beginInteraction(action: "intercom.instance.detach")
    let detached = detachIntoInstance(identity, frame: frame, preservingDragSurface: true) != nil
    // Remove the old row immediately and compact the remaining cards without
    // trailing the pointer with a window animation.
    resizingGroup = true
    reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
    resizingGroup = false
    interaction.finish(
      detached ? .unverified : .refused, effect: detached ? .stateChanged : .noChange,
      cleanup: detached ? .pending : .notRequired, reason: detached ? .none : .ownerMismatch)
    return detached
  }

  /// Dragging a collapsed row moves the whole group under the pointer, as
  /// the header does. Only the dedicated grip detaches a card; its name and
  /// background keep moving the group even while collapsed. Screen coordinates,
  /// because the row's own window moves with the group.
  fileprivate func moveGroup(ended: Bool = false) {
    guard let header else { return }
    if ended { finishGroupDrag(); return }
    if groupDrag == nil { _ = beginGroupDrag() }
    guard let drag = groupDrag else { return }
    let pointer = NSEvent.mouseLocation
    positioning = true
    header.setFrameOrigin(
      CGPoint(
        x: drag.origin.x + pointer.x - drag.pointer.x,
        y: drag.origin.y + pointer.y - drag.pointer.y))
    positioning = false
    translateGroupCards()
  }

  fileprivate func beginGroupDrag() -> Bool {
    guard let header, visible, resizePreview == nil else { return false }
    groupDrag = GroupDrag(
      pointer: NSEvent.mouseLocation, origin: header.frame.origin, screen: screenFrame,
      frames: cards.values.filter(\.isPresented).map { ($0, $0.frame) })
    groupedIdleClock?.suspend()
    return true
  }

  private func translateGroupCards() {
    guard let header, let drag = groupDrag else { return }
    let dx = header.frame.minX - drag.origin.x
    let dy = header.frame.minY - drag.origin.y
    positioning = true
    for entry in drag.frames where entry.panel.isPresented {
      let origin = CGPoint(x: entry.frame.minX + dx, y: entry.frame.minY + dy)
      entry.panel.setFrameOrigin(origin)
    }
    positioning = false
  }

  fileprivate func finishGroupDrag() {
    guard let header, let drag = groupDrag else { return }
    translateGroupCards()
    groupDrag = nil
    // A display crossing may change the available roster budget. Re-measure
    // once after release; an ordinary move never invalidates view content.
    if screenFrame != drag.screen {
      reconcile(chips: state.channelMapLayoutModel.agents, scale: effectiveScale)
    } else {
      for entry in drag.frames {
        if let identity = cards.first(where: { $0.value === entry.panel })?.key {
          cardTargets[identity] = entry.panel.frame
        }
      }
    }
    UserDefaults.standard.set(
      [Double(header.frame.minX), Double(header.frame.minY)], forKey: Self.positionKey)
    noteGroupedActivity()
  }

  @discardableResult
  private func detachIntoInstance(
    _ identity: OverlayIntercomCardIdentity, frame: CGRect? = nil,
    preservingDragSurface: Bool = false, settle: Bool = true
  ) -> UUID? {
    // Broadcast and the human inbox have stable conversations without an
    // agent owner; no other ownerless identity may detach.
    guard identity.canExpand else {
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
      let instance = OverlayIntercomInstance(
        identity: identity, frame: current, minimized: false)
      desktop.upsert(instance)
      id = instance.id
    }
    // A grip drag hands this exact window to the window server (performDrag),
    // so the same panel becomes the instance window and the drag continues
    // under the cursor instead of dying with a hidden group card.
    if let panel = cards.removeValue(forKey: identity) {
      panel.resignKey()
      if preservingDragSurface,
        let index = desktop.instances.firstIndex(where: { $0.id == id })
      {
        instanceDrag = InstanceDrag(
          instance: desktop.instances[index], panel: panel, expandedSize: current.size)
        desktop.instances[index].frame = panel.frame
      } else {
        installInstanceContent(on: panel, id: id)
      }
      sizes.removeValue(forKey: identity)
      cardTargets.removeValue(forKey: identity)
      focusRevisions.removeValue(forKey: identity)
      instancePanels[id] = panel
    }
    presentation.close(identity)
    if settle {
      saveDesktop()
      materializeInstances()
    }
    return id
  }

  /// Keep the gesture's native window intact. Its final placement is committed
  /// only on release; resizing never recreates an editor at pointer frequency.
  fileprivate func resize(
    _ identity: OverlayIntercomCardIdentity, from frame: CGRect, delta: CGSize,
    edge: OverlayIntercomWidgetLayout.ResizeEdge, gesture: UUID
  ) {
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
    resizeTarget = .expanded(identity, fitted)
    showResizePreview(frame: fitted, panels: [panel], group: false)
  }

  /// Width reveals content; height stretches the tiles and, from two rows,
  /// places controls below the name. Neither changes fonts. Every row's edges
  /// resize the group (Maciej, 2026-10-23: "na każdy rząd, w lewo, w prawo, w
  /// dół i w górę"); the edge a row is dragged by follows the pointer.
  fileprivate func resizeGroup(
    from frame: CGRect, delta: CGSize, edge: OverlayIntercomWidgetLayout.ResizeEdge,
    row: OverlayIntercomCardIdentity? = nil, gesture: UUID
  ) {
    guard let header else { return }
    if groupResizeGesture?.gesture != gesture {
      let initialCount = presentation.folded ? 0 : visibleRows.count
      let index = row.flatMap { groupIdentities.firstIndex(of: $0) }.map {
        $0 - visibleRows.lowerBound
      }
      let height =
        header.frame.height
        + CGFloat(initialCount) * (groupCardHeight + OverlayIntercomWidgetLayout.gap)
      groupResizeGesture = (
        gesture,
        CGRect(
          x: header.frame.minX, y: header.frame.maxY - height,
          width: header.frame.width, height: height), initialCount, index
      )
    }
    guard let drag = groupResizeGesture else { return }
    let count = drag.count
    let delta = CGSize(
      width: delta.width,
      height: delta.height
        * OverlayIntercomWidgetLayout.rowEdgeFactor(edge: edge, index: drag.index, count: count))
    let fitted = contentMetrics.resizedGroupFrame(
      drag.frame, delta: delta, edge: edge, count: count, visibleFrame: visibleFrame(for: header),
      minimumWidth: minimumHeaderWidth)
    resizeTarget = .group(fitted, count: count, headerHeight: header.frame.height)
    showResizePreview(
      frame: fitted, panels: [header] + groupIdentities[visibleRows].compactMap { cards[$0] },
      group: true)
  }

  private func showResizePreview(
    frame: CGRect, panels: @autoclosure () -> [OverlayIntercomPanel], group: Bool
  ) {
    if resizePreview == nil {
      groupedIdleClock?.suspend()
      resizePreview = OverlayIntercomResizePreview(
        panels: panels(), group: group,
        headerHeight: OverlayIntercomWidgetLayout.controlHeight(scale: effectiveScale),
        palette: OverlayAppearancePalette.resolve(
          NSApp.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? ColorScheme.dark : ColorScheme.light))
    }
    resizePreview?.update(frame: frame)
  }

  fileprivate func finishInteractiveResize(cancelled: Bool = false) {
    guard let preview = resizePreview else { return }
    let target = resizeTarget
    resizePreview = nil
    resizeTarget = nil
    groupResizeGesture = nil
    let refresh = reconcileAfterResize
    reconcileAfterResize = false
    var updateGroup = refresh
    var updateInstances = refresh
    var persist = false
    if !cancelled, visible, !invalidated, let target {
      switch target {
      case .group(let frame, let count, let headerHeight):
        desktop.groupWidth = Double(frame.width)
        if count > 0 {
          desktop.groupRowHeight = Double(
            (frame.height - headerHeight) / CGFloat(count) - OverlayIntercomWidgetLayout.gap)
        }
        if let header {
          positioning = true
          header.setFrame(
            CGRect(x: frame.minX, y: frame.maxY - headerHeight,
                   width: frame.width, height: headerHeight), display: true)
          positioning = false
          UserDefaults.standard.set(
            [Double(header.frame.minX), Double(header.frame.minY)], forKey: Self.positionKey)
        }
        updateGroup = true
        updateInstances = true
        persist = true
      case .instance(let id, let frame):
        if let index = desktop.instances.firstIndex(where: { $0.id == id && !$0.closed && !$0.minimized }) {
          desktop.instances[index].frame = frame
          updateInstances = true
          persist = true
        }
      case .expanded(let identity, let frame):
        if cards[identity] != nil, presentation.expanded.contains(identity) {
          _ = detachIntoInstance(identity, frame: frame, settle: false)
          updateGroup = true
          updateInstances = true
          persist = true
        }
      }
    }
    if visible, !invalidated {
      if updateGroup {
        reconcile(
          chips: state.channelMapLayoutModel.agents, scale: effectiveScale,
          persistTier: false, materializeRetiredInstances: false)
        updateInstances = true
      }
      if updateInstances { materializeInstances() }
      if persist { saveDesktop() }
    }
    preview.finish(animated: !cancelled && visible)
    noteGroupedActivity()
  }

  func windowDidMove(_ notification: Notification) {
    guard !positioning, let panel = notification.object as? NSWindow else { return }
    if panel === header, let header {
      if groupDrag != nil {
        translateGroupCards()
        return
      }
      positionCards()
      UserDefaults.standard.set(
        [Double(header.frame.minX), Double(header.frame.minY)], forKey: Self.positionKey)
    } else if let id = instancePanels.first(where: { $0.value === panel })?.key,
      let index = desktop.instances.firstIndex(where: { $0.id == id })
    {
      // Native Window Server movement must not rewrite an observable desktop
      // or persist JSON at pointer frequency. Finish commits its final frame.
      if instanceDrag?.id == id { return }
      // Window Server owns geometry during a native drag. Clamping happens
      // afterward, once the pointer has been released on its chosen display.
      let fitted = instanceDrag?.id == id ? panel.frame : OverlayIntercomWidgetLayout.movedFrame(
        panel.frame, visibleFrame: visibleFrame(for: panel),
        headerHeight: OverlayIntercomWidgetLayout.controlHeight(scale: effectiveScale))
      if panel.frame != fitted {
        positioning = true
        panel.setFrame(fitted, display: true)
        positioning = false
      }
      // A mini window moves the stored expanded frame by its top-right corner,
      // so restore opens where the person last left the fiszka.
      if instanceDrag?.id == id, instanceDrag?.expandedSize != nil {
        desktop.instances[index].frame = fitted
      } else {
        var stored = desktop.instances[index].frame
        stored.origin.x = fitted.maxX - stored.width
        stored.origin.y = fitted.maxY - stored.height
        desktop.instances[index].frame = stored
      }
      saveDesktop()
    }
  }
}

/// No window spans the invisible desktop gaps. Only explicitly opened
/// conversations may become key; nonactivating hover remains passive.
/// Internal so the text-size shortcuts can name the Intercom's windows.
@MainActor
final class OverlayIntercomPanel: NSPanel {
  /// Intercom cards stay above the ordinary main overlay at `.floating`;
  /// explicitly opened conversations stay above their compact chrome.
  /// Regular app windows retain the separate yield policy below.
  static let passiveLevel = NSWindow.Level(rawValue: NSWindow.Level.floating.rawValue + 1)
  static let detachedLevel = NSWindow.Level(rawValue: NSWindow.Level.floating.rawValue + 2)

  var acceptsConversationFocus = false {
    didSet {
      guard oldValue != acceptsConversationFocus else { return }
      yieldToAppWindow(yieldedWindow)
    }
  }
  var floatingLevel: NSWindow.Level {
    acceptsConversationFocus ? Self.detachedLevel : Self.passiveLevel
  }
  private weak var yieldedWindow: NSWindow?
  var onScroll: (CGFloat) -> Void = { _ in }
  private var presented = false
  private var visibilityRevision: UInt64 = 0

  var isPresented: Bool { presented && isVisible }

  /// Ordinary focus never lowers a pinned card under unrelated windows.
  /// A real application modal remains above the cards until it is dismissed.
  override func orderFrontRegardless() {
    let window = NSApp.modalWindow
    if let window {
      yieldToAppWindow(window)
      order(.below, relativeTo: window.windowNumber)
    } else {
      yieldToAppWindow(nil)
      super.orderFrontRegardless()
    }
  }

  func yieldToAppWindow(_ window: NSWindow?) {
    if let window, window === NSApp.modalWindow {
      yieldedWindow = window
      // Keep the real modal in charge; normal app windows do not own this level.
      let target = NSWindow.Level(
        rawValue: NSWindow.Level.normal.rawValue - (acceptsConversationFocus ? 0 : 1))
      if level != target { level = target }
      if isVisible && window.isVisible {
        order(.below, relativeTo: window.windowNumber)
      }
    } else {
      yieldedWindow = nil
      if level != floatingLevel { level = floatingLevel }
    }
  }

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

  // ⌘C on a selected bubble in an opened card copied nothing (Maciej,
  // 2026-10-25). The 19 X repair lived only on the overlay panel; this panel is
  // non-activating too, so the chord can reach it as a plain keyDown while
  // another app is active and never touches the Edit menu.
  override func performKeyEquivalent(with event: NSEvent) -> Bool {
    if performCopyChord(event) { return true }
    return super.performKeyEquivalent(with: event)
  }

  override func sendEvent(_ event: NSEvent) {
    if event.type == .keyDown, performCopyChord(event) { return }
    super.sendEvent(event)
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
    level = floatingLevel
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
    // Mailboxes open the existing conversation surface. Channel 0 also keeps
    // its existing microphone and reply controls in this header.
    let broadcast = intercomConversation(state: state, digit: "0")
    let inbox = state.conversations.first(where: \.isHumanInbox)
    let metrics = controller.headerMetrics
    let content = metrics.content(
      width: controller.groupWidth, paging: controller.headerNeedsPaging)
    HStack(spacing: IntercomHeaderMetrics.spacing) {
      Menu {
        // Frequent actions sit on the first menu. Saved conversations stays
        // one named submenu; it is a list, not a layout command.
        Button(
          controller.presentation.folded
            ? String(localized: "Expand Intercom") : String(localized: "Collapse Intercom")
        ) { controller.toggleGroup() }
        Button("Dock all agents", systemImage: "rectangle.stack") { controller.dockAll() }
          .accessibilityIdentifier("overlay-intercom-dock-all")
        if controller.canRestoreDockedLayout {
          Button("Restore window layout", systemImage: "arrow.uturn.backward") {
            controller.restoreDockedLayout()
          }
        }
        Divider()
        mailboxMenuItem(
          broadcast, title: String(localized: "Shared conversation"),
          symbol: "bubble.left.and.bubble.right", identifier: "overlay-intercom-all")
        mailboxMenuItem(
          inbox, title: String(localized: "Your Inbox"), symbol: "tray",
          identifier: "overlay-intercom-inbox")
        let closed = controller.closedInstances
        if !closed.isEmpty {
          Divider()
          Menu("Saved conversations") {
            ForEach(closed) { instance in
              Button {
                controller.reopenInstance(instance.id)
              } label: {
                Text(verbatim: instance.displayTitle)
              }
            }
          }
        }
      } label: {
        Image(systemName: "circle.grid.2x2.fill")
          .csFont(10, .semibold)
          .frame(width: metrics.groupWidth, height: metrics.controlHeight)
          .contentShape(Rectangle())
          .overlay(alignment: .topTrailing) {
            if [inbox, broadcast].compactMap({ $0 }).contains(where: {
              state.unreadReplies(in: $0) > 0
            }) {
              Circle().fill(palette.primaryText.color).frame(width: 4, height: 4)
                .accessibilityHidden(true)
            }
          }
      }
      .menuStyle(.borderlessButton)
      .menuIndicator(.hidden)
      .fixedSize()
      .conversationActionHover(
        restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color)
      .accessibilityLabel("Intercom menu")
      .accessibilityIdentifier("overlay-intercom-menu")
      .help("Intercom menu")
      if content.showsCount {
        Spacer(minLength: IntercomHeaderMetrics.spacerMinimum)
        Text(verbatim: chips.count.formatted()).csMono(10)
          .frame(width: metrics.countWidth).allowsHitTesting(false)
      }
      if controller.headerNeedsPaging {
        Button("Previous agents", systemImage: "chevron.up") { controller.scrollGroup(-1) }
          .labelStyle(.iconOnly).buttonStyle(.plain).disabled(!controller.hasPrevious)
          .frame(width: metrics.pagingControlWidth)
        Button("Next agents", systemImage: "chevron.down") { controller.scrollGroup(1) }
          .labelStyle(.iconOnly).buttonStyle(.plain).disabled(!controller.hasNext)
          .frame(width: metrics.pagingControlWidth)
      }
      OverlayIntercomAudioControls(
        state: state, conversation: broadcast, digit: "0", palette: palette,
        open: broadcast.map { state.conversationMicrophoneOpen($0) } ?? false,
        speaking: broadcast?.replySpeaking == true
      )
      .frame(width: metrics.audioWidth)
    }
    .foregroundStyle(palette.mutedText.color)
    .padding(.horizontal, content.horizontalInset)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    // Any free spot on the bar moves the group — not a grip that showed a
    // finger drawing (Maciej, 2026-10-23). Buttons above it keep their
    // clicks; the count and the gaps fall through to the move surface.
    // A click that never becomes a drag toggles the existing group fold.
    // Instance headers and detach grips do not.
    .background(
      OverlayIntercomDragHandle(
        onClick: { controller.toggleGroup() },
        onDrag: { controller.beginGroupDrag() },
        onDragEnded: { controller.finishGroupDrag() }, cursorHint: false))
    .background {
      OverlayCanvasBackdrop(
        palette: palette, reduceTransparency: reduceTransparency,
        cornerRadius: OverlayIntercomWidgetLayout.rowCornerRadius(scale: scale)
      )
      .clipShape(
        RoundedRectangle(
          cornerRadius: OverlayIntercomWidgetLayout.rowCornerRadius(scale: scale),
          style: .continuous))
    }
    // The chooser's channel 0 is this bar: its ring lands here.
    .modifier(
      OverlayGestureFocusRing(
        focused: broadcast != nil && state.agentGestureFocusID == broadcast?.id,
        shape: RoundedRectangle(
          cornerRadius: OverlayIntercomWidgetLayout.rowCornerRadius(scale: scale),
          style: .continuous),
        palette: palette, targetID: broadcast?.id,
        onHover: { state.hoverAgentGestureTarget($0) })
    )
    .background(
      OverlayIntercomWindowProjection(
        chips: chips,
        identities: intercomIdentities(state: state, chips: chips),
        archivedOwners: state.archivedAgentOwners, controller: controller
      )
    )
    // The header is always on screen while the group is: it pages the stack
    // to the row the wheel focused.
    .onChange(of: state.agentGestureFocusID) { _, focus in
      controller.revealChooserFocus(focus)
    }
    .accessibilityIdentifier("overlay-intercom-drag-handle")
    .accessibilityAction(
      named: Text(
        controller.presentation.folded
          ? String(localized: "Expand Intercom") : String(localized: "Collapse Intercom"))
    ) { controller.toggleGroup() }
  }

  private func mailboxMenuItem(
    _ conversation: OverlayConversation?, title: String, symbol: String, identifier: String
  ) -> some View {
    let unread = conversation.map { state.unreadReplies(in: $0) } ?? 0
    let selected = conversation != nil && state.selectedConversationID == conversation?.id
    return Button {
      if let conversation { controller.openMailbox(conversation.id) }
    } label: {
      Label(unread > 0 ? title + " (" + unread.formatted() + ")" : title, systemImage: symbol)
    }
    .disabled(conversation == nil)
    .accessibilityLabel(Text(verbatim: title))
    .accessibilityValue(Text("\(unread) unread replies"))
    .accessibilityAddTraits(selected ? .isSelected : [])
    .accessibilityHint(Text("View conversation without changing the microphone"))
    .accessibilityIdentifier(identifier)
  }
}

/// Native placement observes the existing projection, including owner changes
/// behind an unchanged digit/name. No callback, reader or poller is installed.
struct OverlayIntercomWindowProjection: NSViewRepresentable {
  @Environment(\.csTextScale) private var scale
  let chips: [OverlayChannelMapChip]
  let identities: [OverlayIntercomCardIdentity]
  var archivedOwners: Set<OverlayConversationOwner> = []
  weak var controller: OverlayIntercomWidgetController?

  func makeCoordinator() -> Coordinator { Coordinator() }

  func makeNSView(context: Context) -> NSView { NSView() }

  func updateNSView(_ nsView: NSView, context: Context) {
    context.coordinator.submit(
      chips: chips, identities: identities, scale: scale, controller: controller,
      archivedOwners: archivedOwners)
  }

  static func dismantleNSView(_ nsView: NSView, coordinator: Coordinator) {
    coordinator.cancel()
  }

  @MainActor
  final class Coordinator {
    private weak var controller: OverlayIntercomWidgetController?
    private var chips: [OverlayChannelMapChip]?
    private var identities: [OverlayIntercomCardIdentity]?
    private var scale: CGFloat?
    private var visibleFrames: [CGRect]?
    private var archivedOwners: Set<OverlayConversationOwner>?
    private var pendingTask: Task<Void, Never>?
    private var generation: UInt64 = 0

    func submit(
      chips: [OverlayChannelMapChip], identities: [OverlayIntercomCardIdentity], scale: CGFloat,
      controller: OverlayIntercomWidgetController?, visibleFrames: [CGRect]? = nil,
      archivedOwners: Set<OverlayConversationOwner> = []
    ) {
      guard let controller else {
        cancel()
        return
      }
      let visibleFrames = visibleFrames ?? NSScreen.screens.map(\.visibleFrame)

      // Card identity deliberately excludes its display name; placement also
      // depends on the label's measured width.
      let sameIdentities =
        self.identities?.elementsEqual(identities) {
          $0 == $1 && $0.name == $1.name
        } ?? false
      guard
        self.controller !== controller || self.chips != chips
          || !sameIdentities || self.scale != scale || self.visibleFrames != visibleFrames
          || self.archivedOwners != archivedOwners
      else { return }

      self.controller = controller
      self.chips = chips
      self.identities = identities
      self.scale = scale
      self.visibleFrames = visibleFrames
      self.archivedOwners = archivedOwners
      guard pendingTask == nil else { return }

      generation &+= 1
      let generation = generation
      pendingTask = Task { @MainActor [weak self] in
        // Let same-turn representable updates replace these inputs before the
        // existing controller path performs its geometry reconciliation.
        await Task.yield()
        guard let self, !Task.isCancelled, self.generation == generation else { return }
        self.pendingTask = nil
        guard let controller = self.controller, let chips = self.chips,
          let scale = self.scale
        else { return }
        controller.reconcile(chips: chips, scale: scale)
      }
    }

    func cancel() {
      generation &+= 1
      pendingTask?.cancel()
      pendingTask = nil
      controller = nil
      chips = nil
      identities = nil
      scale = nil
      visibleFrames = nil
      archivedOwners = nil
    }
  }
}

/// The header's existing flat grip is also the conversation's move handle.
/// `onDrag` answers whether the window under the grip is the one to move: a
/// refused detach leaves the group card in the stack, and the stack is not
/// dragged.
private struct OverlayIntercomDragHandle: NSViewRepresentable {
  /// A click that never crosses the drag threshold. Only the group's top bar
  /// sets this, so a detached window's header and a detach grip stay still.
  var onClick: () -> Void = {}
  var onDrag: () -> Bool = { true }
  var onDragEnded: () -> Void = {}
  /// A visible grip shows the open hand; a whole-header surface under the
  /// buttons keeps their arrow.
  var cursorHint = true
  var minimumDragDistance: CGFloat = 4
  var onPull: (CGSize) -> Void = { _ in }
  func makeNSView(context: Context) -> OverlayIntercomDragView { OverlayIntercomDragView() }
  func updateNSView(_ nsView: OverlayIntercomDragView, context: Context) {
    nsView.onClick = onClick
    nsView.onDrag = onDrag
    nsView.onDragEnded = onDragEnded
    nsView.cursorHint = cursorHint
    nsView.minimumDragDistance = minimumDragDistance
    nsView.onPull = onPull
  }
}

final class OverlayIntercomDragView: NSView {
  var onClick: () -> Void = {}
  var onDrag: () -> Bool = { true }
  var onDragEnded: () -> Void = {}
  var cursorHint = true
  var minimumDragDistance: CGFloat = 4
  var onPull: (CGSize) -> Void = { _ in }
  private var moving = false
  private var dragged = false
  private var mouseDownEvent: NSEvent?
  private var dragCompletion: (id: UUID, finish: () -> Void)?
  override var isOpaque: Bool { false }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  override func mouseDown(with event: NSEvent) {
    if let completion = dragCompletion { finishDrag(completion.id) }
    moving = false
    dragged = false
    mouseDownEvent = event
    onPull(.zero)
    super.mouseDown(with: event)
  }
  // A real drag moves the window (and detaches, where the grip detaches).
  // A click that never moves is not a detach and not a move. The window
  // reference is taken before onDrag. One performDrag per gesture hands the
  // original mouse-down event to Window Server, as required by AppKit. That
  // call returns immediately and AppKit may consume mouse-up, so release is
  // also observed through pressedMouseButtons for this gesture only.
  override func mouseDragged(with event: NSEvent) {
    guard !moving, let mouseDownEvent, let window else { return }
    let dragStart = mouseDownEvent.locationInWindow
    let delta = CGSize(
      width: event.locationInWindow.x - dragStart.x,
      height: event.locationInWindow.y - dragStart.y)
    onPull(delta)
    // A detach grip has a deliberate latch; ordinary window/header moves
    // retain their small threshold. No window or owner changes before the threshold.
    guard hypot(delta.width, delta.height) >= minimumDragDistance else { return }
    moving = true
    onPull(.zero)
    guard onDrag() else { return }
    dragged = true
    let id = UUID()
    dragCompletion = (id, onDragEnded)
    window.performDrag(with: mouseDownEvent)
    // Retain this drag surface until release even if the window is hidden or
    // retired in the meantime. The controller revalidates its canonical state.
    Task { @MainActor [self] in
      while dragCompletion?.id == id && NSEvent.pressedMouseButtons & 1 != 0 {
        try? await Task.sleep(for: .milliseconds(16))
      }
      finishDrag(id)
    }
  }
  override func mouseUp(with event: NSEvent) {
    let clicked = !dragged
    if let completion = dragCompletion { finishDrag(completion.id) }
    mouseDownEvent = nil
    onPull(.zero)
    // A drag already called performDrag. Only a click that stayed under the
    // threshold reaches onClick, and only the top bar uses that to fold.
    if clicked { onClick() }
    super.mouseUp(with: event)
  }
  private func finishDrag(_ id: UUID) {
    guard let completion = dragCompletion, completion.id == id else { return }
    dragCompletion = nil
    moving = false
    mouseDownEvent = nil
    onPull(.zero)
    completion.finish()
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
  @State private var hovering = false
  @State private var detachPull: CGSize = .zero
  let state: OverlayState
  let identity: OverlayIntercomCardIdentity
  let index: Int
  let controller: OverlayIntercomWidgetController
  let onRevealConversation: () -> Void

  private var conversation: OverlayConversation? {
    state.conversations.first { identity.matches($0) }
  }

  private var canOpenConversation: Bool {
    let current =
      intercomConversation(state: state, digit: identity.digit).map { identity.matches($0) } == true
    guard current, let conversation else { return false }
    return identity.digit == "0" || identity.isHumanInbox || conversation.owner != nil
  }

  /// Dragging a collapsed row moves the group; a click still unrolls it.
  private var moveGesture: some Gesture {
    DragGesture(minimumDistance: 4)
      .onChanged { _ in controller.moveGroup() }
      .onEnded { _ in controller.moveGroup(ended: true) }
  }

  private var name: String {
    conversation.map { OverlayChannelMapChip.displayName($0.name) } ?? identity.name
  }

  var body: some View {
    let palette = OverlayAppearancePalette.resolve(colorScheme)
    let expanded = controller.presentation.expanded.contains(identity)
    let content = controller.contentMetrics.cardContent(
      width: controller.groupWidth, cardHeight: controller.groupCardHeight)
    let preview = controller.contentMetrics.compactPreviewLayout(
      width: controller.groupWidth, cardHeight: controller.groupCardHeight)
    let corner =
      expanded
      ? OverlayIntercomWidgetLayout.conversationCornerRadius
      : OverlayIntercomWidgetLayout.rowCornerRadius(scale: scale)
    VStack(spacing: 0) {
      cardHeader(palette: palette, expanded: expanded)
        .frame(height: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
      if expanded || content != .header, let conversation {
        // One physical metadata line uses the card width, not the narrow name
        // button between status marks and audio controls. Its contrast does
        // not inherit a disconnected or detached row's dimming.
        OverlayIntercomConversationContext(
          descriptor: state.conversationAgentDescriptor(conversation)
            ?? conversation.owner?.provider,
          palette: palette
        )
        .frame(height: OverlayIntercomWidgetLayout.contextStripHeight(scale: scale), alignment: .top)
      }
      if expanded {
        if let conversation {
          conversationBody(conversation, palette: palette)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
          Text("Conversation unavailable").csFont(12)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
      } else if let preview, preview.showsComposer, let conversation {
        OverlayIntercomConversationBody(
          state: state, conversation: conversation, palette: palette, focusRevision: 0,
          presentation: .intercom, showsComposer: true, messageWindowLimit: 8,
          composerEditorHeightLimit: preview.composerEditorHeightLimit,
          onActivity: { controller.noteGroupedActivity() },
          onEditingChanged: { _ in controller.noteGroupedActivity() }
        )
        .frame(height: preview.height)
      } else {
        Group {
          if let preview, let conversation {
            OverlayIntercomMessagePreview(conversation: conversation, palette: palette)
              .frame(height: preview.height)
          } else {
            Color.clear
              .contentShape(Rectangle())
              .onTapGesture {
                if canOpenConversation { controller.expand(identity) }
              }
          }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .contentShape(Rectangle())
        .contextMenu {
          if let message = conversation?.messages.last {
            CopyMessageButton(text: message.text, palette: palette)
          }
          Button("Open conversation") { controller.expand(identity) }
            .disabled(!canOpenConversation)
        }
        .accessibilityLabel(String(localized: "Open conversation") + ": " + name)
        .accessibilityAction(named: Text("Open conversation")) {
          if canOpenConversation { controller.expand(identity) }
        }
        .accessibilityIdentifier("overlay-intercom-preview-open-\(identity.digit)")
      }
    }
    .foregroundStyle(palette.primaryText.color)
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    // Header-only icons retain their compact row; text uses its measured space.
    .frame(
      height: !expanded && controller.groupTier == .icon && content == .header
        ? OverlayIntercomWidgetLayout.controlHeight(scale: scale) : nil
    )
    .background(
      OverlayCanvasBackdrop(
        palette: palette, reduceTransparency: reduceTransparency, cornerRadius: corner)
    )
    .clipShape(RoundedRectangle(cornerRadius: corner, style: .continuous))
    .overlay {
      RoundedRectangle(cornerRadius: corner, style: .continuous)
        .strokeBorder(palette.border.color, lineWidth: 1).allowsHitTesting(false)
    }
    // The middle-click chooser's focus, as on the agents list's rows; the
    // pointer entering a row moves it there.
    .modifier(
      OverlayGestureFocusRing(
        focused: identity.conversationID != nil
          && state.agentGestureFocusID == identity.conversationID,
        shape: RoundedRectangle(cornerRadius: corner, style: .continuous), palette: palette,
        targetID: identity.conversationID,
        onHover: { state.hoverAgentGestureTarget($0) })
    )
    .overlay {
      if expanded {
        OverlayIntercomResizeHandles(
          cornerRadius: corner, topRight: false, revealed: hovering,
          onEnded: { controller.finishInteractiveResize() }
        ) { frame, delta, edge, gesture in
          controller.resize(identity, from: frame, delta: delta, edge: edge, gesture: gesture)
        }
      } else {
        // Detach owns the top-left corner. Only the other three visible grips
        // resize; the remaining perimeter does not intercept content controls.
        OverlayIntercomResizeHandles(
          cornerRadius: corner, revealed: hovering,
          onEnded: { controller.finishInteractiveResize() }
        ) { frame, delta, edge, gesture in
          controller.resizeGroup(
            from: frame, delta: delta, edge: edge, row: identity, gesture: gesture)
        }
      }
    }
    .opacity(entered ? 1 : 0)
    .onHover {
      hovering = $0
      if $0 { controller.noteGroupedActivity() }
    }
    .onChange(of: state.autoFoldSeconds) { _, _ in controller.noteGroupedActivity() }
    .offset(y: entered || reduceMotion ? 0 : -6)
    .onAppear {
      withAnimation(
        reduceMotion
          ? nil : .spring(response: 0.28, dampingFraction: 0.86).delay(Double(index) * 0.025)
      ) { entered = true }
    }
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-intercom-row-\(identity.digit)")
    .frame(maxHeight: .infinity, alignment: .top)
  }

  @ViewBuilder
  private func cardHeader(palette: OverlayAppearancePalette, expanded: Bool) -> some View {
    let conversation = conversation
    let current =
      intercomConversation(state: state, digit: identity.digit).map { identity.matches($0) } == true
    let speaking = current && conversation?.replySpeaking == true
    let open = current && (conversation.map { state.conversationMicrophoneOpen($0) } ?? false)
    let roster = OverlayChannelStatusView.intercomRoster(
      state: state, palette: palette, animates: !reduceMotion,
      onRevealConversation: onRevealConversation,
      onOpenConversation: { id in
        if id == identity.conversationID { controller.expand(identity) }
      })
    let channelRow = current ? intercomChannelRow(state: state, digit: identity.digit) : nil
    let inactive = !current || (channelRow.map { roster.isInactive($0) } ?? false)
    let tier = controller.groupTier
    let row = expanded ? IntercomRowControls.all : controller.rowControls
    let hasInlinePreview =
      controller.contentMetrics.cardContent(
        width: controller.groupWidth, cardHeight: controller.groupCardHeight) == .preview
    let hasMessage = conversation?.messages.isEmpty == false
    let nameHint = OverlayHeaderControlChrome(
      title: identity.displayDigit + " · " + name, accent: nil, palette: palette,
      hintLeading: true, showsHint: !expanded && controller.groupCardHeight <=
        OverlayIntercomWidgetLayout.controlHeight(scale: scale) && !hasInlinePreview,
      hintContent: hasMessage
        ? AnyView(
          OverlayIntercomHoverPreview(state: state, identity: identity, palette: palette)
            .environment(\.csTextScale, scale)
            .environment(\.colorScheme, colorScheme)) : nil,
      hintSize: hasMessage ? CGSize(width: 360 * scale, height: 240 * scale) : nil)
    // The grip stays separate from the open button at every content tier.
    // Its full-height native hit area never overlaps audio or the agent name.
    let detached = controller.desktop.openInstance(for: identity) != nil
    let detachGrip = OverlayIntercomDetachGrip(palette: palette, highlighted: hovering)
      .offset(x: detachPull.width, y: detachPull.height)
      .frame(
        width: 24 * max(1, scale),
        height: OverlayIntercomWidgetLayout.controlHeight(scale: scale)
      )
      .overlay(
        OverlayIntercomDragHandle(
          onDrag: {
            let detached = controller.detachForDrag(identity)
            if detached {
              NSHapticFeedbackManager.defaultPerformer.perform(.alignment, performanceTime: .now)
            }
            return detached
          },
          onDragEnded: { controller.finishInstanceDrag() },
          minimumDragDistance: 24,
          onPull: { delta in
            if delta == .zero {
              withAnimation(reduceMotion ? nil : .snappy(duration: 0.18)) { detachPull = .zero }
            } else {
              // The grip stretches slightly against its latch, then snaps free.
              detachPull = CGSize(
                width: min(6, max(-6, delta.width * 0.22)),
                height: min(6, max(-6, -delta.height * 0.22)))
            }
          }))
      .overlayFastHint(
        detached ? String(localized: "Open conversation") : String(localized: "Drag to detach"),
        palette: palette)
      .accessibilityElement(children: .ignore)
      .accessibilityLabel("Detach conversation")
      .accessibilityAddTraits(.isButton)
      .accessibilityAction { _ = controller.detach(identity) }
      .accessibilityIdentifier("overlay-intercom-detach-\(identity.digit)")
      .allowsHitTesting(identity.canExpand)
      .accessibilityHidden(!identity.canExpand)
    // The icon square names its agent in the hint panel, beside the column;
    // the card itself never grows under the pointer.
    if tier == .icon && !expanded {
      HStack(spacing: 3) {
        Button {
          controller.expand(identity)
        } label: {
          HStack(spacing: 4) {
            if identity.isHumanInbox {
              Image(systemName: "tray").csFont(11)
            } else {
              Text(verbatim: identity.displayDigit).csMono(10)
                .foregroundStyle(detached ? palette.mutedText.color : palette.primaryText.color)
              OverlayIntercomStatusMark(
                roster: roster, channel: channelRow,
                kind: channelRow.map { roster.statusDotKind(for: $0) } ?? .idle,
                palette: palette, hints: false)
            }
          }
          .frame(maxWidth: .infinity, maxHeight: .infinity)
          .contentShape(Rectangle())
          .overlay(alignment: .bottomTrailing) {
            // The icon tier has no grip. The badge sits inside the square
            // so a detached row does not grow the group.
            if detached {
              Image(systemName: "macwindow")
                .font(.system(size: 7 * scale, weight: .semibold))
                .foregroundStyle(palette.mutedText.color)
                .accessibilityHidden(true)
            }
          }
          .overlay(alignment: .topTrailing) {
            if current, let conversation, state.hasSynchronizedDraft(conversation.id) {
              OverlayIntercomSyncIndicator(
                state: state, conversationID: conversation.id, palette: palette, compact: true)
                .padding(2)
            }
          }
        }
        .buttonStyle(.plain)
        .opacity(detached ? 0.72 : 1)
        .modifier(nameHint)
        .simultaneousGesture(moveGesture)
        .modifier(OverlayDisconnectedRowDim(disconnected: inactive))
        .modifier(OverlayDetachedRosterValue(detached: detached))
        .accessibilityLabel(
          Text(verbatim: identity.isHumanInbox ? name : identity.digit + " · " + name)
        )
        .accessibilityActions {
          if identity.canExpand {
            Button("Detach conversation") { controller.detach(identity) }
          }
        }
        .accessibilityIdentifier("overlay-intercom-name-\(identity.digit)")
      }
    } else {
      // Reuse the menu execution, but Write stays in this card.
      HStack(spacing: 3) {
        detachGrip
        HStack(spacing: 6) {
          if !identity.isHumanInbox {
            Text(verbatim: identity.displayDigit).csMono(10)
              .foregroundStyle(palette.mutedText.color)
              .accessibilityIdentifier("overlay-intercom-digit-\(identity.digit)")
          }
          if let channelRow, row.marks {
            OverlayReceptionMarks(channel: channelRow, palette: palette, textScale: scale)
          }
          if identity.isHumanInbox {
            Image(systemName: "tray").csFont(11)
          } else {
            OverlayIntercomStatusMark(
              roster: roster, channel: channelRow,
              kind: channelRow.map { roster.statusDotKind(for: $0) } ?? .idle,
              palette: palette)
          }
          Button {
            controller.expand(identity)
          } label: {
            Text(verbatim: name).csFont(11, .semibold).lineLimit(1)
              .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
              .contentShape(Rectangle())
          }
          .buttonStyle(.plain)
          // Names rest lightly smoked; the agent talking brightens instead of
          // turning violet (Maciej, 2026-10-23: "można by się tego fioletu
          // pozbyć … na rzecz rozjaśnienia").
          .conversationActionHover(
            restingColor: inactive || detached
              ? palette.mutedText.color
              : speaking ? palette.primaryText.color : palette.bodyText.color,
            hoverColor: inactive ? palette.mutedText.color : palette.primaryText.color
          )
          .disabled(!canOpenConversation)
          .modifier(OverlayDetachedRosterValue(detached: detached))
          .accessibilityLabel(String(localized: "Open conversation") + ": " + name)
          .accessibilityActions {
            if identity.canExpand {
              Button("Detach conversation") { controller.detach(identity) }
            }
          }
          .accessibilityIdentifier("overlay-intercom-name-\(identity.digit)")
          .modifier(nameHint)
        }
        .opacity(detached ? 0.72 : 1)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .contentShape(Rectangle())
        .simultaneousGesture(moveGesture, including: expanded ? .subviews : .all)
        if current, let conversation, state.hasSynchronizedDraft(conversation.id) {
          OverlayIntercomSyncIndicator(state: state, conversationID: conversation.id, palette: palette)
        }
        // Focus precedes the shared audio pair: live input on the left,
        // reply playback on the right. Narrow rows retain one shared slot.
        Group {
          if row.slots >= 3 {
            OverlayIntercomFocusButton(
              roster: roster, channel: channelRow,
              palette: palette, diameter: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
          }
          if row.slots >= 1 {
            audioSlot(
              current ? conversation : nil, palette: palette, speaking: speaking, open: open,
              shared: row.slots == 1)
          }
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
      .modifier(OverlayDisconnectedRowDim(disconnected: inactive))
      .modifier(
        current && !identity.isHumanInbox
          ? roster.agentPillMenu(for: identity.digit, showsInlineError: false)
          : OverlayBroadcastPolicyMenu(members: [], palette: palette, enabled: false))
    }
  }

  private func audioSlot(
    _ conversation: OverlayConversation?, palette: OverlayAppearancePalette,
    speaking: Bool, open: Bool, shared: Bool = false
  ) -> some View {
    OverlayIntercomAudioControls(
      state: state, conversation: conversation, digit: identity.digit, palette: palette,
      open: open, speaking: speaking, shared: shared)
  }

  private func conversationBody(
    _ conversation: OverlayConversation, palette: OverlayAppearancePalette
  ) -> some View {
    OverlayIntercomConversationBody(
      state: state, conversation: conversation, palette: palette,
      focusRevision: controller.focusRevisions[identity] ?? 0,
      presentation: .intercom,
      onActivity: { controller.noteGroupedActivity() },
      onEditingChanged: { _ in controller.noteGroupedActivity() })
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
  var shared = false

  var body: some View {
    if conversation?.isHumanInbox != true {
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
        diameter: OverlayIntercomWidgetLayout.controlHeight(scale: scale),
        shared: shared
      )
      .fixedSize()
      .accessibilityIdentifier("overlay-intercom-audio-\(digit)")
    }
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
/// The row's state mark: the activity dot while someone reads the channel,
/// an explicit glyph once nobody does — the follower gone, or alive with a
/// backlog nobody read past the stall horizon (Maciej, 2026-10-23: inactive
/// agents "powinny mieć jawne disconnected"). The glyphs are the roster's
/// own, so the overlay list and the Intercom say one thing.
private struct OverlayIntercomStatusMark: View {
  @Environment(\.csTextScale) private var textScale
  let roster: OverlayChannelStatusView
  let channel: OverlayChannelDelivery?
  let kind: OverlayAgentDotKind
  let palette: OverlayAppearancePalette
  var hints = true

  var body: some View {
    if let channel, roster.isInactive(channel) {
      let glyph = Image(systemName: roster.statusSymbol(for: channel))
        .font(.system(size: 9 * textScale, weight: .semibold))
        .foregroundStyle(palette.mutedText.color)
        .frame(width: 12 * textScale, height: 10 * textScale)
        .accessibilityLabel(Text(verbatim: roster.detail(for: channel)))
        .accessibilityIdentifier("overlay-intercom-inactive-\(channel.channel)")
      if hints {
        glyph.overlayFastHint(roster.detail(for: channel), palette: palette)
      } else {
        glyph
      }
    } else {
      OverlayAgentStatusDot(kind: kind, palette: palette)
    }
  }
}

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
            .modifier(
              OverlayGlyphInk(
                ink: focusState?.isSucceeded == true
                  ? AnyShapeStyle(palette.successStatus.color)
                  : focusState?.isFailed == true
                    ? AnyShapeStyle(palette.errorStatus.color) : nil))
          }
        }
        .frame(width: diameter, height: diameter)
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      // Smoked at rest like the wordmark and the audio pair, brighter under
      // the pointer (Maciej, 2026-10-23: "nie tylko w pierwszym").
      .conversationActionHover(
        restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
      )
      .disabled(!action.enabled)
      .overlayFastHint(action.title, palette: palette)
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
  var presentation: OverlayConversationPresentation = .standard
  var showsComposer = true
  var messageWindowLimit: Int? = nil
  var composerEditorHeightLimit: CGFloat? = nil
  var onActivity: () -> Void = {}
  var onEditingChanged: (Bool) -> Void = { _ in }

  private var current: Bool {
    conversation.isHumanInbox
      || intercomConversation(state: state, digit: conversation.channel).map {
        $0.id == conversation.id && $0.owner == conversation.owner
      } == true
  }

  var body: some View {
    OverlayConversationView(
      conversation: conversation, palette: palette, topInset: 0,
      bottomInset: presentation == .intercom ? 8 : 12,
      pendingControls: state.pendingReplyControls, controlErrors: state.replyControlErrors,
      onControl: { message, stop in
        guard current else { return }
        Task { await state.controlReply(message, stop: stop) }
      },
      composerFocusRevision: focusRevision,
      draft: Binding(
        get: { state.conversationDraft(for: conversation.id) },
        set: { state.setConversationDraft($0, for: conversation.id) }),
      sending: state.pendingTextMessages.contains(conversation.id),
      sendError: state.draftSyncSendError(for: conversation.id)
        ?? (state.draftCapture.errorConversationID == conversation.id
          ? state.draftCapture.error : nil),
      onSend: {
        guard current else { return }
        Task { await state.sendConversationText(conversation) }
      },
      hidesNavigation: true,
      presentation: presentation,
      showsComposer: showsComposer && current,
      allowsLiveActions: current,
      messageWindowLimit: messageWindowLimit,
      composerEditorHeightLimit: composerEditorHeightLimit,
      onComposerEditorActive: {
        state.noteComposerEditorActive($0)
        onEditingChanged($0)
      },
      onComposerTypingActivity: {
        state.noteComposerTypingActivity()
        onActivity()
      },
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
      },
      draftSyncChoices: current ? state.draftSyncChoices(for: conversation) : [],
      draftSyncNames: state.draftSyncNames(for: conversation.id),
      draftSyncArmed: state.armedDraftSyncConversationID == conversation.id,
      draftSyncEnabled: current && state.canActivateDraftSync(for: conversation.id),
      onActivateDraftSync: {
        guard current else { return }
        state.activateDraftSync(for: conversation.id)
      },
      onProseEdit: { edit in
        guard current else { return }
        state.applyConversationProseEdit(edit, for: conversation.id)
      },
      onToggleDraftSync: {
        guard current else { return }
        state.toggleDraftSync(from: conversation.id, with: $0)
      },
      onStopDraftSync: {
        guard current else { return }
        state.stopDraftSync(for: conversation.id)
      }
    )
  }
}

/// Only metadata already projected for this exact conversation is shown.
/// The canonical descriptor supplies provider, model and declared repository
/// on one line. Execution coordinates remain outside the card's metadata row.
private struct OverlayIntercomConversationContext: View {
  @Environment(\.csTextScale) private var scale
  let descriptor: String?
  let palette: OverlayAppearancePalette
  var leadingInset: CGFloat = 12
  var trailingInset: CGFloat = 12
  var bottomInset: CGFloat = 6

  private var contextLine: String? {
    guard let descriptor else { return nil }
    let trimmed = descriptor.trimmingCharacters(in: .whitespacesAndNewlines)
    return trimmed.isEmpty ? nil : trimmed
  }

  var body: some View {
    if let contextLine {
      Text(verbatim: contextLine)
        .lineLimit(1)
        // Keep the provider at the start and a declared repository at the end.
        .truncationMode(.middle)
        .multilineTextAlignment(.leading)
        .frame(maxWidth: .infinity, alignment: .leading)
        .help(contextLine)
        .font(.system(size: 10 * scale))
        .foregroundStyle(palette.mutedText.color)
        .padding(.leading, leadingInset)
        .padding(.trailing, trailingInset)
        .padding(.bottom, bottomInset)
        .fixedSize(horizontal: false, vertical: true)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("overlay-intercom-context")
    }
  }
}

/// A tonal surface relief. Its small light/shadow pairs suggest grip without
/// borrowing the high-contrast four-dot icon of the Intercom's main menu.
private struct OverlayIntercomDetachGrip: View {
  @Environment(\.colorScheme) private var colorScheme
  @Environment(\.csTextScale) private var scale
  let palette: OverlayAppearancePalette
  let highlighted: Bool

  var body: some View {
    Canvas { context, _ in
      let strength = highlighted ? 1.0 : 0.65
      for row in 0..<3 {
        for column in 0..<2 {
          let rect = CGRect(
            x: (0.7 + CGFloat(column) * 3) * scale,
            y: (0.7 + CGFloat(row) * 3) * scale,
            width: 1.6 * scale, height: 1.6 * scale)
          context.fill(
            Path(ellipseIn: rect.offsetBy(dx: 0, dy: 0.5 * scale)),
            with: .color(.black.opacity((colorScheme == .dark ? 0.35 : 0.2) * strength)))
          context.fill(
            Path(ellipseIn: rect.offsetBy(dx: 0, dy: -0.4 * scale)),
            with: .color(.white.opacity((colorScheme == .dark ? 0.22 : 0.7) * strength)))
          context.fill(
            Path(ellipseIn: rect), with: .color(palette.desktopBackground.color.opacity(0.65)))
        }
      }
    }
    .frame(width: 7 * scale, height: 10 * scale)
    .accessibilityHidden(true)
  }
}

/// Membership is projected from the existing draft owner even when there is
/// no room for an editor. It never opens a capture channel or links drafts.
private struct OverlayIntercomSyncIndicator: View {
  @Environment(\.csTextScale) private var scale
  let state: OverlayState
  let conversationID: String
  let palette: OverlayAppearancePalette
  var compact = false

  var body: some View {
    let names = state.draftSyncNames(for: conversationID).joined(separator: " · ")
    Image(systemName: "link")
      .font(.system(size: (compact ? 7 : 10) * scale, weight: .semibold))
      .foregroundStyle(palette.agentAccent.color)
      .frame(width: (compact ? 10 : 18) * max(1, scale))
      .overlayFastHint(String(localized: "Sync draft") + ": " + names, palette: palette)
      .accessibilityLabel(Text("Sync draft"))
      .accessibilityValue(Text(verbatim: names))
      .accessibilityIdentifier("overlay-intercom-sync-\(conversationID)")
  }
}

/// A detached agent stays in the roster. The value names that state without
/// borrowing the disconnected-row treatment, which means a different thing.
private struct OverlayDetachedRosterValue: ViewModifier {
  let detached: Bool

  func body(content: Content) -> some View {
    if detached {
      content.accessibilityValue(Text("Detached"))
    } else {
      content
    }
  }
}

/// Cards and passive hovers preview the latest committed message through the
/// shared rich-body renderer without mounting an editable conversation.
private struct OverlayIntercomMessagePreview: View {
  @Environment(\.csTextScale) private var textScale
  let conversation: OverlayConversation
  let palette: OverlayAppearancePalette

  var body: some View {
    ScrollView {
      if let message = conversation.messages.last {
        VStack(alignment: .leading, spacing: OverlayIntercomWidgetLayout.compactPreviewSpacing) {
          let route = OverlayChannelStatusView.messageRoute(of: message)
          if !route.label.isEmpty {
            Text(verbatim: route.label)
              .font(.system(size: 10 * textScale, weight: .bold))
              .foregroundStyle(palette.mutedText.color)
              .accessibilityIdentifier("overlay-intercom-message-route")
          }
          OverlayConversationMessageContent(message: message, palette: palette)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 12)
        .padding(.vertical, OverlayIntercomWidgetLayout.compactPreviewVerticalPadding)
      }
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    .accessibilityIdentifier("overlay-intercom-message-preview")
  }
}

private struct OverlayIntercomHoverPreview: View {
  let state: OverlayState
  let identity: OverlayIntercomCardIdentity
  let palette: OverlayAppearancePalette

  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      Text(verbatim: identity.displayDigit + " · " + identity.name).csFont(11, .semibold)
        .padding(.horizontal, 12).padding(.top, 10)
      if let conversation = state.conversations.first(where: { identity.matches($0) }) {
        OverlayIntercomMessagePreview(conversation: conversation, palette: palette)
        .allowsHitTesting(false)
      }
    }
    .foregroundStyle(palette.primaryText.color)
    .background(palette.desktopBackground.color.opacity(0.9))
  }
}

/// One resize event: the gesture's mouse-down frame, the absolute delta, the
/// dragged edge, and the gesture's identity (one mouse-down, one UUID).
private typealias OverlayIntercomResizeAction = (
  CGRect, CGSize, OverlayIntercomWidgetLayout.ResizeEdge, UUID
) -> Void

private struct OverlayIntercomResizeHandles: View {
  @Environment(\.colorScheme) private var colorScheme
  let cornerRadius: CGFloat
  var topRight = true
  var revealed = false
  var onEnded: () -> Void = {}
  let resize: OverlayIntercomResizeAction

  var body: some View {
    VStack(spacing: 0) {
      HStack(spacing: 0) {
        Spacer(minLength: 0)
        if topRight { handle(.topRight) }
      }
      Spacer(minLength: 0)
      HStack(spacing: 0) {
        handle(.bottomLeft)
        Spacer(minLength: 0)
        handle(.bottomRight)
      }
    }
    .padding(4)
  }

  private func handle(_ edge: OverlayIntercomWidgetLayout.ResizeEdge) -> some View {
    OverlayIntercomResizeHandle(
      edge: edge, accent: OverlayAppearancePalette.resolve(colorScheme).agentAccent.nsColor,
      cornerRadius: cornerRadius, revealed: revealed, onEnded: onEnded, resize: resize
    )
    .frame(width: Self.cornerBand, height: Self.cornerBand)
  }

  /// Visible corner targets only. Top-left belongs to detach; a reserved top-right
  /// arrow omits its target too. No side bands cover buttons, selection or scrolling.
  static let cornerBand: CGFloat = 12
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
    let corner =
      mini
      ? OverlayIntercomWidgetLayout.rowCornerRadius(scale: scale)
      : OverlayIntercomWidgetLayout.conversationCornerRadius
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
    .background(
      OverlayCanvasBackdrop(
        palette: palette, reduceTransparency: reduceTransparency, cornerRadius: corner)
    )
    .clipShape(RoundedRectangle(cornerRadius: corner, style: .continuous))
    .overlay {
      RoundedRectangle(cornerRadius: corner, style: .continuous)
        .strokeBorder(palette.border.color, lineWidth: 1).allowsHitTesting(false)
    }
    .overlay {
      if !mini {
        OverlayIntercomResizeHandles(
          cornerRadius: corner, topRight: false, revealed: hovering,
          onEnded: { controller.finishInteractiveResize() }
        ) { frame, delta, edge, _ in
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
    let current =
      intercomConversation(state: state, digit: instance.digit).map { instance.matches($0) }
      == true
    let speaking = current && conversation?.replySpeaking == true
    let open = current && (conversation.map { state.conversationMicrophoneOpen($0) } ?? false)
    let mini = instance.minimized
    let layout = instanceLayout(instance)
    let roster = OverlayChannelStatusView.intercomRoster(
      state: state, palette: palette, animates: !reduceMotion,
      onRevealConversation: onRevealConversation,
      onOpenConversation: { id in
        if id == instance.conversationID { controller.focusInstance(instanceID) }
      })
    // A digit whose owner changed belongs to the replacement agent: its marks
    // and focus would act on that agent under this fiszka's name (Copilot on
    // #183). Only the exact owner's row is this window's to control.
    let channelRow = current ? intercomChannelRow(state: state, digit: instance.digit) : nil
    let inactive = !current || (channelRow.map { roster.isInactive($0) } ?? false)
    let nameRow = Text(verbatim: instance.name).csFont(11, .semibold).lineLimit(1)
      .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
      .contentShape(Rectangle())
    return HStack(spacing: 3) {
      if !mini && !instance.isHumanInbox {
        Text(verbatim: instance.displayDigit).csMono(10).foregroundStyle(palette.mutedText.color)
      }
      if layout.tier != .icon, layout.controls.marks, let channelRow {
        OverlayReceptionMarks(channel: channelRow, palette: palette, textScale: scale)
      }
      if instance.isHumanInbox {
        Image(systemName: "tray").csFont(11)
      } else if !current {
        Image(systemName: "clock.arrow.circlepath").csFont(11)
          .overlayFastHint(String(localized: "Saved conversation"), palette: palette)
          .accessibilityLabel("Saved conversation")
          .accessibilityIdentifier("overlay-intercom-history-\(instance.digit)")
      } else if !mini {
        OverlayIntercomStatusMark(
          roster: roster, channel: channelRow,
          kind: channelRow.map { roster.statusDotKind(for: $0) } ?? .idle, palette: palette)
      } else if layout.tier == .icon {
        Image(systemName: "bubble.left").csFont(11)
          .overlayFastHint(instance.name, palette: palette)
      }
      // Mini keeps its restore button for keyboard and accessibility. Its
      // pointer events reach the existing header drag surface, which restores
      // on a click and moves on a drag. Expanded names are part of that surface too.
      if layout.tier != .icon {
        if mini {
          Button {
            controller.setInstanceMinimized(instanceID, false)
          } label: {
            nameRow
          }
          .buttonStyle(.plain)
          .conversationActionHover(
            restingColor: speaking ? palette.primaryText.color : palette.bodyText.color,
            hoverColor: palette.primaryText.color
          )
          .accessibilityLabel(String(localized: "Restore fiszka") + ": " + instance.name)
          .allowsHitTesting(false)
        } else {
          // Glyphs would otherwise take the hit from the drag surface below:
          // the hosting view hands only text and buttons to SwiftUI.
          nameRow
            .foregroundStyle(speaking ? palette.primaryText.color : palette.bodyText.color)
            .allowsHitTesting(false)
            .accessibilityLabel(Text(verbatim: instance.name))
        }
      }
      if current, let conversation, state.hasSynchronizedDraft(conversation.id) {
        OverlayIntercomSyncIndicator(state: state, conversationID: conversation.id, palette: palette)
      }
      if layout.tier != .icon {
        if layout.controls.slots >= 3 {
          OverlayIntercomFocusButton(
            roster: roster, channel: channelRow, palette: palette,
            diameter: OverlayIntercomWidgetLayout.controlHeight(scale: scale))
        }
        if layout.controls.slots >= 1 {
          OverlayIntercomAudioControls(
            state: state, conversation: current ? conversation : nil, digit: instance.digit,
            palette: palette,
            open: open, speaking: speaking, shared: layout.controls.slots == 1)
        }
      }
      // Mini rests with the name, activity and the audio pair (Astra):
      // clicking the fiszka itself restores it, so no extra Restore button.
      if !mini {
        Button {
          controller.setInstanceMinimized(instanceID, true)
        } label: {
          Label("Minimize", systemImage: "minus")
            .labelStyle(.iconOnly)
            .frame(width: 22 * max(1, scale), height: 30 * max(1, scale))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .conversationActionHover(
          restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
        )
        .accessibilityIdentifier("overlay-intercom-instance-mini-\(instance.digit)")
      }
      // The X keeps a fixed place so the audio slot never moves: a hover
      // only changes its visibility, never the row's geometry (Astra, V1 —
      // a reflow let a click aimed at the remembered microphone close the
      // fiszka). Closing hands the conversation back to its group row, so it
      // wears the overlay's dock glyph, not an X (Maciej, 2026-10-23: "dokładnie
      // tym samym piktogramem Dock").
      Button {
        controller.closeInstance(instanceID)
      } label: {
        Label(
          current ? String(localized: "Collapse") : String(localized: "Close conversation"),
          systemImage: current ? OverlayControlSymbols.returnToMini : "xmark")
          .labelStyle(.iconOnly)
          .frame(width: 26 * max(1, scale), height: 30 * max(1, scale))
          .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .conversationActionHover(
        restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color
      )
      .overlayFastHint(
        current ? String(localized: "Collapse into the group")
          : String(localized: "Close conversation"), palette: palette)
      .opacity(!mini || hovering ? 1 : 0)
      .allowsHitTesting(!mini || hovering)
      .accessibilityIdentifier("overlay-intercom-instance-close-\(instance.digit)")
    }
    .modifier(OverlayDisconnectedRowDim(disconnected: inactive))
    .padding(.leading, 12)
    .padding(.trailing, 4)
    // Any free spot on the header moves the fiszka, not one grip on one side
    // (Maciej, 2026-10-23). `isMovableByWindowBackground` is inert under this
    // hosting view — it reports mouseDownCanMoveWindow everywhere, so AppKit
    // never decides — and `WindowDragGesture` does not move a non-key
    // nonactivating panel (Klaudiusz-div0's HID probe, macOS 27;
    // OverlayWindowDragRegion). The grip's own performDrag view, under the
    // whole header, is the one road that already moves these panels.
    .background(
      OverlayIntercomDragHandle(
        onClick: {
          if mini { controller.setInstanceMinimized(instanceID, false) }
        },
        onDrag: { controller.beginInstanceDrag(instanceID) },
        onDragEnded: { controller.finishInstanceDrag() }, cursorHint: false))
    // The title bar carries the control glass of the composer and the name
    // pill, not a glass of its own (Maciej, 2026-10-23). The drag surface
    // sits inside the glass: outside it, the glass takes the hit and no
    // drag reaches the window (Klaudiusz-div0's HID probe).
    .modifier(
      OverlayControlGlass(
        cornerRadius: mini
          ? OverlayIntercomWidgetLayout.rowCornerRadius(scale: scale)
          : OverlayIntercomWidgetLayout.conversationCornerRadius)
    )
    // The wheel reaches an agent in its own window: the same ring as its row.
    .modifier(
      OverlayGestureFocusRing(
        focused: state.agentGestureFocusID == instance.conversationID,
        shape: RoundedRectangle(
          cornerRadius: mini
            ? OverlayIntercomWidgetLayout.rowCornerRadius(scale: scale)
            : OverlayIntercomWidgetLayout.conversationCornerRadius,
          style: .continuous),
        palette: palette, targetID: instance.conversationID,
        onHover: { state.hoverAgentGestureTarget($0) })
    )
    // The same right-click menu as the row in the group: Show in Agents,
    // microphone, playback, archive. A detached fiszka had none.
    .modifier(
      current && !instance.isHumanInbox
        ? roster.agentPillMenu(for: instance.digit, showsInlineError: false)
        : OverlayBroadcastPolicyMenu(members: [], palette: palette, enabled: false))
  }

  /// Detached windows derive their own content from their own saved frame.
  /// No group tier or group dimensions participate in this decision.
  private func instanceLayout(_ instance: OverlayIntercomInstance)
    -> (tier: IntercomContentTier, controls: IntercomRowControls)
  {
    let metrics = IntercomContentMetrics(
      names: [instance.name], digits: [instance.displayDigit], scale: scale)
    let windowSlots = (instance.minimized ? 26 : 48) * max(1, scale)
    let width = CGFloat(instance.width) - windowSlots
    let height = instance.minimized ? metrics.iconSide : CGFloat(instance.height)
    return (metrics.tier(width: width, cardHeight: height), metrics.rowControls(width: width))
  }

}

private struct OverlayIntercomResizeHandle: NSViewRepresentable {
  let edge: OverlayIntercomWidgetLayout.ResizeEdge
  let accent: NSColor
  let cornerRadius: CGFloat
  let revealed: Bool
  let onEnded: () -> Void
  let resize: OverlayIntercomResizeAction

  func makeNSView(context: Context) -> OverlayIntercomResizeView { OverlayIntercomResizeView() }
  func updateNSView(_ nsView: OverlayIntercomResizeView, context: Context) {
    nsView.edge = edge
    nsView.accent = accent
    nsView.cornerRadius = cornerRadius
    nsView.revealed = revealed
    nsView.resize = resize
    nsView.onEnded = onEnded
    nsView.needsDisplay = true
  }
}

private final class OverlayIntercomResizeView: NSView {
  var edge = OverlayIntercomWidgetLayout.ResizeEdge.bottomRight
  var accent = OverlayAppearancePalette.dark.agentAccent.nsColor
  var cornerRadius: CGFloat = OverlayIntercomWidgetLayout.conversationCornerRadius
  var revealed = false
  var resize: OverlayIntercomResizeAction = { _, _, _, _ in }
  var onEnded: () -> Void = {}
  private var origin: CGPoint?
  private var originalFrame: CGRect?
  private var gesture = UUID()
  private var moved = false
  private var hovering = false
  private var hoverArea: NSTrackingArea?

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
    if moved {
      mouseDragged(with: event)
      onEnded()
    }
    origin = nil
    originalFrame = nil
    moved = false
  }
  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    guard hoverArea == nil else { return }
    let area = NSTrackingArea(
      rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
      owner: self, userInfo: nil)
    hoverArea = area
    addTrackingArea(area)
  }
  override func mouseEntered(with event: NSEvent) { hovering = true; needsDisplay = true }
  override func mouseExited(with event: NSEvent) { hovering = false; needsDisplay = true }
  override func draw(_ dirtyRect: NSRect) {
    guard revealed || hovering || moved else { return }
    accent.withAlphaComponent(hovering || moved ? 0.6 : 0.32).setStroke()
    let path = NSBezierPath()
    let left = edge == .bottomLeft
    let top = edge == .topRight
    // The target sits 4pt inside the card. A further 1pt stroke inset follows
    // its rounded contour instead of drawing two straight teeth. Clip the arc
    // to the existing target even when text scale increases the card radius.
    let x = left ? bounds.minX + cornerRadius - 4 : bounds.maxX - cornerRadius + 4
    let y = top ? bounds.maxY - cornerRadius + 4 : bounds.minY + cornerRadius - 4
    NSGraphicsContext.saveGraphicsState()
    NSBezierPath(rect: bounds).addClip()
    path.appendArc(
      withCenter: NSPoint(x: x, y: y), radius: max(1, cornerRadius - 5),
      startAngle: top ? 0 : left ? 180 : 270,
      endAngle: top ? 90 : left ? 270 : 360)
    path.lineWidth = 1.25
    path.lineCapStyle = .round
    path.stroke()
    NSGraphicsContext.restoreGraphicsState()
  }
  override func resetCursorRects() {
    discardCursorRects()
    addCursorRect(bounds, cursor: .crosshair)
  }
}

/// Transient native feedback owned by the existing window controller. It has
/// no document, draft or delivery state and performs no SwiftUI layout work.
@MainActor
private final class OverlayIntercomResizePreview {
  private let panel: NSPanel
  private let drawing: OverlayIntercomResizePreviewView
  private let sources: [(panel: OverlayIntercomPanel, alpha: CGFloat)]

  init(panels: [OverlayIntercomPanel], group: Bool, headerHeight: CGFloat,
       palette: OverlayAppearancePalette) {
    sources = panels.map { ($0, $0.alphaValue) }
    let headers: [NSImage?] = panels.map { panel in
      guard let view = panel.contentView else { return nil }
      let height = min(headerHeight, view.bounds.height)
      let rect = NSRect(
        x: view.bounds.minX,
        y: view.isFlipped ? view.bounds.minY : view.bounds.maxY - height,
        width: view.bounds.width, height: height)
      guard !rect.isEmpty,
        let bitmap = view.bitmapImageRepForCachingDisplay(in: rect)
      else { return nil }
      view.cacheDisplay(in: rect, to: bitmap)
      guard let image = bitmap.cgImage else { return nil }
      return NSImage(cgImage: image, size: rect.size)
    }
    drawing = OverlayIntercomResizePreviewView(
      headers: headers, group: group, headerHeight: headerHeight, palette: palette)
    panel = NSPanel(
      contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = false
    panel.ignoresMouseEvents = true
    panel.hidesOnDeactivate = false
    panel.level = panels.map(\.level).max(by: { $0.rawValue < $1.rawValue })
      ?? OverlayIntercomPanel.passiveLevel
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
    panel.contentView = drawing
    for source in sources { source.panel.alphaValue = 0.12 }
  }

  func update(frame: CGRect) {
    panel.setFrame(frame, display: true)
    drawing.needsDisplay = true
    if !panel.isVisible { panel.orderFrontRegardless() }
  }

  func finish(animated: Bool) {
    let animate = animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
    for source in sources { source.panel.contentView?.layoutSubtreeIfNeeded() }
    guard animate else {
      for source in sources { source.panel.alphaValue = source.alpha }
      panel.close()
      return
    }
    NSAnimationContext.runAnimationGroup { context in
      context.duration = 0.12
      for source in sources { source.panel.animator().alphaValue = source.alpha }
      panel.animator().alphaValue = 0
    } completionHandler: { [panel] in
      Task { @MainActor in panel.close() }
    }
  }
}

private final class OverlayIntercomResizePreviewView: NSView {
  let headers: [NSImage?]
  let group: Bool
  let headerHeight: CGFloat
  let palette: OverlayAppearancePalette

  init(headers: [NSImage?], group: Bool, headerHeight: CGFloat,
       palette: OverlayAppearancePalette) {
    self.headers = headers
    self.group = group
    self.headerHeight = headerHeight
    self.palette = palette
    super.init(frame: .zero)
  }
  required init?(coder: NSCoder) { return nil }
  override var isOpaque: Bool { false }
  override func draw(_ dirtyRect: NSRect) {
    let count = max(0, headers.count - 1)
    let rowHeight = group && count > 0
      ? (bounds.height - headerHeight) / CGFloat(count) - OverlayIntercomWidgetLayout.gap
      : bounds.height
    var top = bounds.maxY
    for (index, image) in headers.enumerated() {
      let height = group && index == 0 ? headerHeight : rowHeight
      let rect = NSRect(x: bounds.minX, y: top - height, width: bounds.width, height: height)
      guard height > 0 else { continue }
      let path = NSBezierPath(roundedRect: rect.insetBy(dx: 1, dy: 1), xRadius: 10, yRadius: 10)
      palette.desktopBackground.nsColor.withAlphaComponent(0.92).setFill()
      path.fill()
      palette.agentAccent.nsColor.withAlphaComponent(0.45).setStroke()
      path.lineWidth = 1
      path.stroke()
      if let image {
        NSGraphicsContext.saveGraphicsState()
        path.addClip()
        image.draw(in: NSRect(x: rect.minX, y: rect.maxY - min(headerHeight, height),
                             width: image.size.width, height: min(headerHeight, height)))
        NSGraphicsContext.restoreGraphicsState()
      }
      top = rect.minY - OverlayIntercomWidgetLayout.gap
    }
  }
}

@MainActor
private func intercomConversation(state: OverlayState, digit: String) -> OverlayConversation? {
  OverlayChannelStatusView.currentConversations(
    conversations: state.conversations, archivedOwners: state.archivedAgentOwners,
    hudStates: state.channelHudStates
  ).first { $0.channel == digit }
}

/// Only agents occupy group rows. The two mailboxes remain header controls.
@MainActor
func intercomIdentities(state: OverlayState, chips: [OverlayChannelMapChip])
  -> [OverlayIntercomCardIdentity]
{
  let current = OverlayChannelStatusView.currentConversations(
    conversations: state.conversations, archivedOwners: state.archivedAgentOwners,
    hudStates: state.channelHudStates)
  var byChannel: [String: OverlayConversation] = [:]
  for conversation in current where byChannel[conversation.channel] == nil {
    byChannel[conversation.channel] = conversation
  }
  return chips.map {
    OverlayIntercomCardIdentity(
      digit: $0.digit, conversation: byChannel[$0.digit],
      name: $0.name)
  }
}
