import AppKit
import QuartzCore
import SwiftUI

// Borderless floating window host for the dictation overlay.
//
// This is a FACTORY ONLY. Summon/dismiss wiring (hotkey, placement, focus handoff,
// activation policy) belongs to the orchestrator in App.swift — this file just
// builds a correctly-configured panel whose content is `DictationOverlayView`,
// with a clear background so the appearance-aware material inside the SwiftUI
// sheet blurs whatever is underneath.

/// Borderless, non-activating panel. Explicit transcript interaction takes
/// keyboard focus for native selection/copy or editing. Showing the overlay
/// and clicking its chrome do not open this gate.
final class FloatingOverlayPanel: NSPanel, NSWindowDelegate {
  var onUserMove: (() -> Void)?
  var onUserDragEnded: ((NSPoint) -> Void)?
  var onUserResize: (() -> Void)?
  var onUserResizeEnded: (() -> Void)?
  var onFrameTransitionCompleted: (() -> Void)?
  var onWidgetInteractionChanged: ((OverlayWidgetInteraction, Bool) -> Void)?
  fileprivate var presence: OverlayPresence?
  private var dragStart: (mouse: NSPoint, frame: NSRect)?
  private var dragMoved = false
  private var resizeStart: (mouse: NSPoint, frame: NSRect, edge: OverlayResizeHit.Edge)?
  private var pendingResizePresentation: (mode: OverlayPresentationMode, animated: Bool)?
  private var expandedSize: NSSize?
  private var miniFrame: NSRect?
  private var presentationMode: OverlayPresentationMode = .expanded
  private var transitionTop: CGFloat?
  private var transitionRight: CGFloat?
  private var frameTransitionTarget: NSRect?
  private var frameTransitionGeneration: UInt64 = 0
  private var menuObservers: [NSObjectProtocol] = []
  private var trackingMenus: Set<ObjectIdentifier> = []
  private(set) var isFrameTransitioning = false
  var isUserResizing: Bool { resizeStart != nil }
  var sizeForPersistence: NSSize { expandedSize ?? frame.size }

  /// Grow leftward so the microphone and fold controls keep their screen position.
  /// Display containment wins when the complete strip cannot fit to the left.
  func setPresentationMode(_ mode: OverlayPresentationMode, animated: Bool = false) {
    if isUserResizing {
      pendingResizePresentation = (mode, animated)
      return
    }
    guard mode != presentationMode else { return }
    let previous = presentationMode
    if previous == .mini { miniFrame = frameTransitionTarget ?? frame }
    presentationMode = mode
    let wasApplyingFrame = OverlayController.isApplyingFrame
    OverlayController.isApplyingFrame = true
    defer { OverlayController.isApplyingFrame = wasApplyingFrame }
    let top = isFrameTransitioning ? transitionTop ?? frame.maxY : frame.maxY
    let right =
      if previous == .midi, let miniFrame {
        miniFrame.maxX
      } else {
        isFrameTransitioning ? transitionRight ?? frame.maxX : frame.maxX
      }
    transitionTop = top
    transitionRight = right
    let size: NSSize
    if mode != .expanded {
      // A hidden editor must not keep accepting the Founder's keystrokes.
      makeFirstResponder(nil)
      releaseKeyAfterTranscript()
      if previous == .expanded && expandedSize == nil { expandedSize = frame.size }
      size = mode == .mini ? DictationOverlayWindow.collapsedSize : DictationOverlayWindow.midiSize
    } else {
      let expanded = expandedSize ?? DictationOverlayWindow.defaultSize
      size = NSSize(
        width: expanded.width,
        height: max(expanded.height, DictationOverlayWindow.minSize.height))
    }
    // Intermediate frames may be smaller than the expanded window's floor.
    minSize = DictationOverlayWindow.collapsedSize
    contentMinSize = minSize
    styleMask.remove(.resizable)
    let proposed = NSRect(
      x: right - size.width, y: top - size.height, width: size.width, height: size.height)
    let restored = DictationOverlayWindow.visibleExpansionFrame(
      proposed, in: screen?.visibleFrame ?? NSScreen.main?.visibleFrame)
    frameTransitionGeneration &+= 1
    let generation = frameTransitionGeneration
    frameTransitionTarget = restored
    isFrameTransitioning = animated
    NSAnimationContext.runAnimationGroup { context in
      context.duration = animated ? DictationOverlayWindow.presentationDuration : 0
      context.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
      animator().setFrame(restored, display: true)
    } completionHandler: { [weak self] in
      Task { @MainActor in
        self?.completeFrameTransition(generation: generation)
      }
    }
    if !animated { completeFrameTransition(generation: generation) }
  }

  private func completeFrameTransition(generation: UInt64) {
    guard generation == frameTransitionGeneration, frameTransitionTarget != nil else { return }
    isFrameTransitioning = false
    frameTransitionTarget = nil
    transitionTop = nil
    transitionRight = nil
    minSize = presentationMode == .expanded ? DictationOverlayWindow.minSize : frame.size
    contentMinSize = minSize
    if presentationMode == .expanded {
      styleMask.insert(.resizable)
      expandedSize = nil
      miniFrame = nil
    }
    onFrameTransitionCompleted?()
  }

  func settleFrameTransition() {
    guard let target = frameTransitionTarget else { return }
    NSAnimationContext.runAnimationGroup { context in
      context.duration = 0
      animator().setFrame(target, display: true)
    }
    completeFrameTransition(generation: frameTransitionGeneration)
  }

  func resetPresentationPosition() { miniFrame = nil }

  func startPresence() {
    presence?.start()
    guard menuObservers.isEmpty else { return }
    for (name, tracking) in [
      (NSMenu.didBeginTrackingNotification, true), (NSMenu.didEndTrackingNotification, false),
    ] {
      menuObservers.append(
        NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) {
          [weak self] notification in
          guard let menu = notification.object as? NSMenu else { return }
          let identity = ObjectIdentifier(menu)
          MainActor.assumeIsolated {
            guard let self, !self.menuObservers.isEmpty else { return }
            if tracking {
              guard self.isVisible else { return }
              self.trackingMenus.insert(identity)
            } else {
              self.trackingMenus.remove(identity)
            }
            self.onWidgetInteractionChanged?(.menu, !self.trackingMenus.isEmpty)
          }
        })
    }
  }

  func invalidatePresence() {
    cancelUserResize()
    presence?.invalidate()
    menuObservers.forEach(NotificationCenter.default.removeObserver)
    menuObservers.removeAll()
    trackingMenus.removeAll()
    onWidgetInteractionChanged?(.menu, false)
    onWidgetInteractionChanged?(.dragging, false)
  }

  override var canBecomeKey: Bool { allowsKeyForTranscript }
  override var canBecomeMain: Bool { false }
  /// Only explicit transcript interaction opens the keyboard gate.
  private(set) var allowsKeyForTranscript = false

  /// The transcript canvas needs keyboard input for selection or editing.
  func takeKeyForTranscript() {
    allowsKeyForTranscript = true
    if !isKeyWindow { makeKey() }
  }

  /// The canvas resigned. Drop key status so keystrokes return to the app the
  /// user was dictating into.
  func releaseKeyAfterTranscript() {
    guard allowsKeyForTranscript else { return }
    allowsKeyForTranscript = false
    if isKeyWindow { resignKey() }
  }

  /// Key left from outside: resign the canvas. An active edit uses its
  /// existing focus-exit commit; a selection has no revision side effects.
  func windowDidResignKey(_ notification: Notification) {
    guard allowsKeyForTranscript else { return }
    allowsKeyForTranscript = false
    makeFirstResponder(nil)
  }

  override func performKeyEquivalent(with event: NSEvent) -> Bool {
    if isKeyWindow, let transcript = firstResponder as? LiveTranscriptNativeTextView,
      transcript.performSelectedCopy(with: event)
    {
      return true
    }
    return super.performKeyEquivalent(with: event)
  }

  /// A non-activating panel does not turn SwiftUI background hits into window
  /// motion, so intercept only explicit AppKit drag regions.
  /// Native transcript and SwiftUI controls keep ordinary AppKit dispatch.
  /// The existing resize band is tracked here without a nested event loop.
  override func sendEvent(_ event: NSEvent) {
    // Text tracking areas can overlap the resize strip even when hitTest
    // returns the container. Dispatching these motions to AppKit first sets
    // an iBeam, then the panel sets a resize cursor on every event.
    // The strip owns motion as well as dragging; interior tracking stays native.
    if event.type == .mouseMoved, styleMask.contains(.resizable), let contentView,
      OverlayResizeHit.edge(at: event.locationInWindow, in: contentView.bounds) != nil
    {
      refreshCursor(at: event.locationInWindow)
      return
    }
    if event.type == .leftMouseDown {
      if isUserResizing { endUserResize() }
      if dragStart != nil { onWidgetInteractionChanged?(.dragging, false) }
      dragStart = nil
      dragMoved = false
      if styleMask.contains(.resizable), let contentView,
        contentView.bounds.contains(event.locationInWindow),
        let edge = OverlayResizeHit.edge(at: event.locationInWindow, in: contentView.bounds)
      {
        beginUserResize(edge: edge, at: screenPoint(for: event))
        return
      }
    }
    switch event.type {
    case .leftMouseDragged where isUserResizing:
      updateUserResize(to: screenPoint(for: event))
    case .leftMouseUp where isUserResizing:
      endUserResize()
    case .leftMouseDown where isWindowDragHit(at: event.locationInWindow):
      settleFrameTransition()
      dragStart = (screenPoint(for: event), frame)
      onWidgetInteractionChanged?(.dragging, true)
    case .leftMouseDragged where dragStart != nil:
      guard let dragStart else { return }
      let current = screenPoint(for: event)
      let previousOrigin = frame.origin
      setFrameOrigin(
        NSPoint(
          x: dragStart.frame.minX + current.x - dragStart.mouse.x,
          y: dragStart.frame.minY + current.y - dragStart.mouse.y
        )
      )
      dragMoved = dragMoved || frame.origin != previousOrigin
    case .leftMouseUp where dragStart != nil:
      dragStart = nil
      let moved = dragMoved
      dragMoved = false
      onWidgetInteractionChanged?(.dragging, false)
      if moved { onUserDragEnded?(frame.origin) }
    default:
      super.sendEvent(event)
      if event.type == .mouseMoved { refreshCursor(at: event.locationInWindow) }
    }
  }

  /// Track the edge gesture through the ordinary event loop, so SwiftUI can
  /// commit each layout before AppKit presents the next window frame.
  @discardableResult
  func beginUserResize(edge: OverlayResizeHit.Edge, at point: NSPoint) -> Bool {
    guard styleMask.contains(.resizable), !isFrameTransitioning, !isUserResizing else {
      return false
    }
    resizeStart = (point, frame, edge)
    onWidgetInteractionChanged?(.dragging, true)
    onUserResize?()
    return true
  }

  @discardableResult
  func updateUserResize(to point: NSPoint) -> Bool {
    guard let resizeStart else { return false }
    let target = OverlayResizeHit.apply(
      edge: resizeStart.edge, start: resizeStart.frame,
      dx: point.x - resizeStart.mouse.x, dy: point.y - resizeStart.mouse.y,
      minSize: minSize)
    guard target != frame else { return false }
    // Do not force an intermediate paint while hosting geometry is invalidated.
    setFrame(target, display: false)
    return true
  }

  func endUserResize() {
    guard isUserResizing else { return }
    resizeStart = nil
    onWidgetInteractionChanged?(.dragging, false)
    let pending = pendingResizePresentation
    pendingResizePresentation = nil
    if let pending { setPresentationMode(pending.mode, animated: pending.animated) }
    onUserResizeEnded?()
  }

  /// Closing a panel must not flush a deferred show or revive its geometry.
  private func cancelUserResize() {
    resizeStart = nil
    pendingResizePresentation = nil
    onWidgetInteractionChanged?(.dragging, false)
  }

  /// Native text tracking has already selected its cursor in super.sendEvent.
  /// Keep link/selection cursors intact; the panel owns chrome and resize edges.
  @discardableResult
  func refreshCursor(at point: NSPoint) -> Bool {
    let desired = cursor(at: point)
    guard desired != .iBeam, NSCursor.current != desired else { return false }
    desired.set()
    return true
  }

  /// Resolve from this panel's hit surface, never the inactive app below it.
  func cursor(at point: NSPoint) -> NSCursor {
    guard let contentView else { return .arrow }
    if styleMask.contains(.resizable),
      let edge = OverlayResizeHit.edge(at: point, in: contentView.bounds)
    {
      return OverlayResizeHit.cursor(for: edge)
    }
    var hit = contentView.hitTest(point)
    while let view = hit {
      if view is NSTextView { return .iBeam }
      hit = view.superview
    }
    return .arrow
  }

  func isWindowDragHit(at point: NSPoint) -> Bool {
    guard let contentView, let hit = contentView.hitTest(point) else { return false }
    // The container claims the resize band, which sendEvent routes through
    // the panel's edge gesture; a container hit is never a drag handle.
    if hit === contentView { return false }
    return hit is OverlayWindowDragRegionView
  }

  private func screenPoint(for event: NSEvent) -> NSPoint {
    if let point = event.cgEvent?.location {
      // Quartz is top-left/y-down; AppKit window origins are bottom-left/y-up.
      return NSPoint(x: point.x, y: -point.y)
    }
    return convertPoint(toScreen: event.locationInWindow)
  }

  func windowDidMove(_ notification: Notification) {
    guard !isFrameTransitioning else { return }
    if !OverlayController.isApplyingFrame { resetPresentationPosition() }
    onUserMove?()
  }

  func windowDidResize(_ notification: Notification) {
    guard !isFrameTransitioning else { return }
    onUserResize?()
  }
}

/// Content container for the overlay panel. Its job is to keep the SwiftUI
/// hosting view's frame identical to its own bounds on every resize — including each
/// step of a live edge-drag — via an ABSOLUTE frame sync rather than an autoresizing
/// mask. The mask resizes by DELTAS measured from the hosting view's initial frame;
/// on a borderless resizable panel those deltas drift the hosting view off the
/// window's content bounds after an edge-drag, so content spilled past the window
/// edge (clipped action row, left-anchored pill/waveform) and — because the SwiftUI
/// rounded glass background was then painted beyond the window rectangle — the
/// visible corners squared off. Re-asserting `hosting.frame = bounds` per resize step
/// keeps the glass panel covering the window 1:1 at any size. Exports no layout
/// constraints, so the content↔window sizing feedback loop that once hung the app
/// stays structurally dead.
final class OverlayContentContainer: NSView {
  private let hosting: NSView

  init(hosting: NSView) {
    self.hosting = hosting
    super.init(frame: .zero)
    addSubview(hosting)
    hosting.frame = bounds
  }

  @available(*, unavailable)
  required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

  override func setFrameSize(_ newSize: NSSize) {
    super.setFrameSize(newSize)
    if hosting.frame != bounds { hosting.frame = bounds }
    window?.invalidateCursorRects(for: self)
  }

  override func layout() {
    super.layout()
    if hosting.frame != bounds { hosting.frame = bounds }
  }

  /// AppKit's borderless resize strip is ~1–2 px. Claim the 16 pt band first,
  /// including the bottom capsule and its margin, so SwiftUI cannot steal it.
  override func hitTest(_ point: NSPoint) -> NSView? {
    if window?.styleMask.contains(.resizable) == true,
      OverlayResizeHit.edge(at: point, in: bounds) != nil
    {
      return self
    }
    return super.hitTest(point)
  }

  /// The resize band owns pointer drags, not scrolling. Its hit lands on this
  /// container, so deliver the unchanged wheel event to the nearest visible
  /// existing scroll view, including when the pointer is over a corner.
  override func scrollWheel(with event: NSEvent) {
    let point = convert(event.locationInWindow, from: nil)
    var target: NSScrollView?
    var distance = CGFloat.infinity
    func visit(_ view: NSView) {
      guard !view.isHidden, !view.visibleRect.isEmpty else { return }
      for child in view.subviews { visit(child) }
      guard let scroll = view as? NSScrollView else { return }
      let rect = convert(scroll.visibleRect, from: scroll)
      let dx = max(rect.minX - point.x, 0, point.x - rect.maxX)
      let dy = max(rect.minY - point.y, 0, point.y - rect.maxY)
      let candidateDistance = dx * dx + dy * dy
      if candidateDistance < distance {
        target = scroll
        distance = candidateDistance
      }
    }
    visit(hosting)
    if let target {
      target.scrollWheel(with: event)
    } else {
      super.scrollWheel(with: event)
    }
  }

  override func resetCursorRects() {
    discardCursorRects()
    // A non-activating glass panel still owns the cursor above its chrome.
    // Descendant NSTextView cursor rects retain native selection/editing cursors.
    addCursorRect(bounds, cursor: .arrow)
    guard window?.styleMask.contains(.resizable) == true else { return }
    for (rect, cursor) in OverlayResizeHit.cursorRects(in: bounds) {
      addCursorRect(rect, cursor: cursor)
    }
  }

  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

enum DictationOverlayWindow {
  static let presentationDuration: TimeInterval = 0.28
  static let collapsedHeight: CGFloat = 46
  static let collapsedSize = NSSize(width: 200, height: collapsedHeight)
  static let midiSize = NSSize(width: 410, height: collapsedHeight)

  /// Shared geometry seam: a low-dragged/bottom-anchored bar must not unfold
  /// below the display. Keep its top unchanged whenever the full frame fits.
  static func visibleExpansionFrame(_ proposed: NSRect, in visible: NSRect?) -> NSRect {
    guard let visible else { return proposed }
    let size = NSSize(
      width: min(proposed.width, visible.width), height: min(proposed.height, visible.height))
    return NSRect(
      origin: OverlayPlacement.clampOrigin(proposed.origin, size: size, in: visible), size: size)
  }
  /// Hard floor for the panel's content size. Enforced for user edge-drag
  /// (`minSize`/`contentMinSize`) AND for every programmatic `setFrame` via
  /// `clamp(_:to:)` (AppKit does not apply `minSize` to programmatic frames).
  /// Slim chrome cut: modeMeta + bottom action row removed; waveform moved into
  /// the primary bar. Height 300 → 260 keeps `bodyMinHeight` (~3 transcript
  /// lines) without the old action-layer mass. Width floor (320) is unchanged.
  static let minSize = NSSize(width: 320, height: 260)
  /// First-launch content size (no persisted value yet). LANDSCAPE rectangle —
  /// Founder spec: the resting state is a horizontal bar (waveform + a few
  /// transcript lines), never a portrait column. Resizing persists, so users
  /// who prefer a tall panel drag it once and keep it.
  static let defaultSize = NSSize(width: 470, height: 280)
  /// Bumped v5 → v6: slim evidence chrome lowers the resting landscape height.
  private static let sizeDefaultsKey = "DictationOverlayPanel.contentSize.v6"

  /// Build the floating overlay panel around an injected `OverlayState`.
  /// The state's `engine`, `onClose`, and `onSendToAgent` are wired by the
  /// orchestrator before the panel is shown.
  @MainActor
  static func make(state: OverlayState, textScale: TextScaleController) -> NSPanel {
    // Wrap in TextScaleRoot so ⌘+/-/0 on this panel scale the overlay text
    // (transcript + status) via `\.csTextScale`, independently of the chat.
    let root = TextScaleRoot(controller: textScale) { DictationOverlayView(state: state) }
    let hosting = NSHostingView(rootView: root)
    // CRITICAL: the WINDOW owns its size; the SwiftUI content only fills whatever
    // frame the window has. An NSHostingView otherwise installs Auto Layout
    // min/max/intrinsic constraints derived from its (flexible, constantly
    // animating) fitting size and pushes them onto the window every display
    // cycle. On a `.resizable` panel that closed a content↔window feedback loop:
    // the window resized to the fitting size → the flexible content re-fit to the
    // new frame → a different fitting size → … The two chased each other,
    // oscillating between two sizes and grinding the main thread in
    // `updateConstraintsIfNeeded → NSHostingView.updateConstraints` until the app
    // hung. Empty `sizingOptions` removes those constraints entirely; the panel is
    // sized only by us (`setFrame`) and by the user's edge-drag. Setting the
    // hosting VIEW (not just an NSHostingController) is what actually stops the
    // constraint export.
    hosting.sizingOptions = []
    // Fill by an ABSOLUTE frame sync (OverlayContentContainer), not an
    // autoresizing mask. AppKit's spring mask resizes by deltas from the view's
    // initial frame; on a borderless resizable panel those deltas drift the
    // hosting view off the window's content bounds after an edge-drag, clipping
    // content at the edges and squaring off the rounded glass corners. Frame-based
    // layout (no exported constraints) keeps the sizing feedback loop dead while
    // the container re-pins the hosting frame to its bounds on every resize step.
    hosting.translatesAutoresizingMaskIntoConstraints = true
    hosting.autoresizingMask = []

    let panel = FloatingOverlayPanel(
      contentRect: NSRect(origin: .zero, size: restoredContentSize()),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered,
      defer: false
    )
    panel.delegate = panel
    state.onPresentationModeChanged = { [weak panel] mode in
      guard let panel else { return }
      panel.setPresentationMode(
        mode,
        animated: panel.isVisible && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
    }
    state.onAgentSidebarPresented = { [weak panel] in
      panel?.makeFirstResponder(nil)
      panel?.releaseKeyAfterTranscript()
    }
    panel.onUserMove = { [weak state] in
      guard !OverlayController.isApplyingFrame else { return }
      state?.userDraggedOverlay()
    }
    panel.onUserDragEnded = { [weak state] origin in
      state?.recordUserDrag(at: origin)
    }
    panel.onUserResize = { [weak state] in
      guard !OverlayController.isApplyingFrame else { return }
      state?.userResizedOverlay()
    }
    panel.onWidgetInteractionChanged = { [weak state] interaction, held in
      state?.setWidgetInteraction(interaction, held: held)
    }
    panel.contentView = OverlayContentContainer(hosting: hosting)

    // User-resizable: borderless windows still honour edge-drag resize when
    // `.resizable` is set. Floor keeps the glass chrome + action row readable.
    panel.minSize = minSize
    panel.contentMinSize = minSize
    // Size is persisted manually (see `persist`/`restoredContentSize`), NOT via
    // `setFrameAutosaveName`: autosave on a borderless resizable panel wrote back
    // the runaway sizes produced by the old feedback loop and restored a stale,
    // oversized frame on relaunch (ghost-outline / clipped-content states). The
    // orchestrator re-centres the origin on every show() and clamps the restored
    // size to the current screen.

    // Transparent chrome so the SwiftUI glass material is the only surface.
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = false  // The SwiftUI sheet paints its own adaptive shadow.

    // Float above normal windows, ride along every Space, never take app focus.
    // sharingType stays readable so PrintScreen can see the panel; presence
    // raises to statusBar for the capture chord and yields to system alerts.
    panel.level = OverlayPresencePolicy.rest.windowLevel
    panel.sharingType = .readOnly
    // AppKit hides transient panels during Mission Control and restores them
    // afterwards without ending the take or rebuilding its content.
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
    panel.isFloatingPanel = true
    panel.becomesKeyOnlyIfNeeded = true
    panel.hidesOnDeactivate = false
    panel.acceptsMouseMovedEvents = true
    // One explicit AppKit path owns dragging on every supported OS version.
    panel.isMovableByWindowBackground = false

    panel.titleVisibility = .hidden
    panel.titlebarAppearsTransparent = true
    panel.standardWindowButton(.closeButton)?.isHidden = true
    panel.standardWindowButton(.miniaturizeButton)?.isHidden = true
    panel.standardWindowButton(.zoomButton)?.isHidden = true

    let presence = OverlayPresence(panel: panel)
    panel.presence = presence
    panel.startPresence()

    panel.setPresentationMode(state.presentationMode)

    // Size is window-owned (user-resizable) — do NOT resize to fittingSize each frame.
    return panel
  }

  /// Clamp a content size to the hard floor and to the screen's visible frame, so a
  /// programmatic `setFrame` (which AppKit does NOT clamp to `minSize`) or a stale
  /// persisted size can never render smaller than the layout minimum or larger than
  /// the current display.
  static func clamp(_ size: NSSize, to screen: NSScreen? = NSScreen.main) -> NSSize {
    var width = max(size.width, minSize.width)
    var height = max(size.height, minSize.height)
    if let visible = screen?.visibleFrame {
      width = min(width, visible.width)
      height = min(height, visible.height)
    }
    return NSSize(width: width, height: height)
  }

  /// Restore the user's last content size (clamped), or the default on first launch.
  static func restoredContentSize(
    for screen: NSScreen? = NSScreen.main,
    defaults: UserDefaults = .standard
  ) -> NSSize {
    let width = defaults.double(forKey: sizeDefaultsKey + ".w")
    let height = defaults.double(forKey: sizeDefaultsKey + ".h")
    let raw = (width > 0 && height > 0) ? NSSize(width: width, height: height) : defaultSize
    return clamp(raw, to: screen)
  }

  /// Persist the current content size so it survives relaunch. Called on hide().
  static func persist(size: NSSize, defaults: UserDefaults = .standard) {
    defaults.set(Double(size.width), forKey: sizeDefaultsKey + ".w")
    defaults.set(Double(size.height), forKey: sizeDefaultsKey + ".h")
  }
}

/// Rest / yield / capture. Screenshot chords rise above the forest so PrintScreen
/// actually sees the panel; system alerts push it back down.
enum OverlayPresencePolicy: Equatable {
  case rest
  case yield
  case capture

  var windowLevel: NSWindow.Level {
    switch self {
    case .rest: .floating
    case .yield: .normal
    case .capture: .statusBar
    }
  }

  static let yieldBundleIds: Set<String> = [
    "com.apple.SecurityAgent",
    "com.apple.UserNotificationCenter",
    "com.apple.CoreServicesUIAgent",
    "com.apple.loginwindow",
  ]

  static func resolve(screenshotChord: Bool, shouldYield: Bool) -> OverlayPresencePolicy {
    if screenshotChord { return .capture }
    if shouldYield { return .yield }
    return .rest
  }

  static func isScreenshotChord(_ event: NSEvent) -> Bool {
    let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
    guard flags.contains([.command, .shift]), !flags.contains(.option) else { return false }
    return event.keyCode == 20 || event.keyCode == 21 || event.keyCode == 23
  }

  static func shouldYield(frontmostBundleId: String?, modalWindowPresent: Bool) -> Bool {
    if modalWindowPresent { return true }
    guard let frontmostBundleId else { return false }
    return yieldBundleIds.contains(frontmostBundleId)
  }
}

/// Keeps the overlay in the forest until a screenshot needs it on top, or an
/// alert needs it out of the way. Never takes the insertion point.
@MainActor
final class OverlayPresence {
  private weak var panel: NSPanel?
  private var localMonitor: Any?
  private var globalMonitor: Any?
  private var workspaceObserver: NSObjectProtocol?
  private var captureUntil: Date?
  private var captureTimer: Timer?

  init(panel: NSPanel) {
    self.panel = panel
  }

  func start() {
    guard localMonitor == nil, globalMonitor == nil, workspaceObserver == nil else { return }
    localMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
      self?.noteScreenshotIfNeeded(event)
      return event
    }
    globalMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
      self?.noteScreenshotIfNeeded(event)
    }
    workspaceObserver = NSWorkspace.shared.notificationCenter.addObserver(
      forName: NSWorkspace.didActivateApplicationNotification,
      object: nil,
      queue: .main
    ) { [weak self] _ in
      Task { @MainActor in self?.apply() }
    }
    apply()
  }

  func invalidate() {
    if let localMonitor { NSEvent.removeMonitor(localMonitor) }
    if let globalMonitor { NSEvent.removeMonitor(globalMonitor) }
    if let workspaceObserver {
      NSWorkspace.shared.notificationCenter.removeObserver(workspaceObserver)
    }
    localMonitor = nil
    globalMonitor = nil
    workspaceObserver = nil
    captureTimer?.invalidate()
    captureTimer = nil
    captureUntil = nil
  }

  private func noteScreenshotIfNeeded(_ event: NSEvent) {
    guard OverlayPresencePolicy.isScreenshotChord(event) else { return }
    captureUntil = Date().addingTimeInterval(4)
    apply()
    captureTimer?.invalidate()
    captureTimer = Timer.scheduledTimer(withTimeInterval: 4.1, repeats: false) { [weak self] _ in
      Task { @MainActor in self?.apply() }
    }
  }

  private func apply() {
    guard let panel else { return }
    let capturing = captureUntil.map { $0 > Date() } ?? false
    let front = NSWorkspace.shared.frontmostApplication?.bundleIdentifier
    let policy = OverlayPresencePolicy.resolve(
      screenshotChord: capturing,
      shouldYield: OverlayPresencePolicy.shouldYield(
        frontmostBundleId: front,
        modalWindowPresent: NSApp.modalWindow != nil
      )
    )
    if panel.level != policy.windowLevel {
      panel.level = policy.windowLevel
    }
    if policy == .capture {
      panel.orderFrontRegardless()
    }
  }
}

/// Geometry for a fat resize band on a borderless panel. AppKit's own strip
/// is one or two pixels; this is a forgiving visible-surface target (macOS 15+).
enum OverlayResizeHit: Sendable {
  static let band: CGFloat = 16
  static let scrollbarInset: CGFloat = 15

  enum Edge: Sendable, Equatable {
    case left, right, top, bottom
    case topLeft, topRight, bottomLeft, bottomRight
  }

  static func edge(at point: NSPoint, in bounds: NSRect, band: CGFloat = band) -> Edge? {
    guard bounds.width > band * 2, bounds.height > band * 2 else { return nil }
    let left = point.x <= bounds.minX + band
    let right = point.x >= bounds.maxX - band
    let bottom = point.y <= bounds.minY + band
    let top = point.y >= bounds.maxY - band
    switch (left, right, bottom, top) {
    case (true, false, true, false): return .bottomLeft
    case (true, false, false, true): return .topLeft
    case (false, true, true, false): return .bottomRight
    case (false, true, false, true): return .topRight
    case (true, false, false, false): return .left
    case (false, true, false, false): return .right
    case (false, false, true, false): return .bottom
    case (false, false, false, true): return .top
    default: return nil
    }
  }

  static func apply(
    edge: Edge,
    start: NSRect,
    dx: CGFloat,
    dy: CGFloat,
    minSize: NSSize
  ) -> NSRect {
    var frame = start
    switch edge {
    case .right, .topRight, .bottomRight:
      frame.size.width = max(minSize.width, start.width + dx)
    case .left, .topLeft, .bottomLeft:
      let width = max(minSize.width, start.width - dx)
      frame.origin.x = start.maxX - width
      frame.size.width = width
    case .top, .bottom:
      break
    }
    switch edge {
    case .top, .topLeft, .topRight:
      frame.size.height = max(minSize.height, start.height + dy)
    case .bottom, .bottomLeft, .bottomRight:
      let height = max(minSize.height, start.height - dy)
      frame.origin.y = start.maxY - height
      frame.size.height = height
    case .left, .right:
      break
    }
    return frame
  }

  @MainActor
  static func cursorRects(in bounds: NSRect, band: CGFloat = band) -> [(NSRect, NSCursor)] {
    let b = band
    let w = bounds.width
    let h = bounds.height
    return [
      (NSRect(x: 0, y: b, width: b, height: max(0, h - 2 * b)), cursor(for: .left)),
      (NSRect(x: w - b, y: b, width: b, height: max(0, h - 2 * b)), cursor(for: .right)),
      (NSRect(x: b, y: h - b, width: max(0, w - 2 * b), height: b), cursor(for: .top)),
      (NSRect(x: b, y: 0, width: max(0, w - 2 * b), height: b), cursor(for: .bottom)),
      (NSRect(x: 0, y: h - b, width: b, height: b), cursor(for: .topLeft)),
      (NSRect(x: w - b, y: h - b, width: b, height: b), cursor(for: .topRight)),
      (NSRect(x: 0, y: 0, width: b, height: b), cursor(for: .bottomLeft)),
      (NSRect(x: w - b, y: 0, width: b, height: b), cursor(for: .bottomRight)),
    ]
  }

  @MainActor
  static func cursor(for edge: Edge) -> NSCursor {
    if #available(macOS 15.0, *) {
      let position: NSCursor.FrameResizePosition
      switch edge {
      case .left: position = .left
      case .right: position = .right
      case .top: position = .top
      case .bottom: position = .bottom
      case .topLeft:
        position = .topLeading(relativeTo: NSApp.userInterfaceLayoutDirection)
      case .topRight:
        position = .topTrailing(relativeTo: NSApp.userInterfaceLayoutDirection)
      case .bottomLeft:
        position = .bottomLeading(relativeTo: NSApp.userInterfaceLayoutDirection)
      case .bottomRight:
        position = .bottomTrailing(relativeTo: NSApp.userInterfaceLayoutDirection)
      }
      return NSCursor.frameResize(position: position, directions: [.inward, .outward])
    }
    switch edge {
    case .left, .right: return .resizeLeftRight
    case .top, .bottom: return .resizeUpDown
    default: return .crosshair
    }
  }
}
