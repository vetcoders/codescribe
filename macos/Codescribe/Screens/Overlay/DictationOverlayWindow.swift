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
  var onPresentationClick: (() -> Void)?
  var onUserResize: (() -> Void)?
  var onUserResizeEnded: (() -> Void)?
  var onResizePresentationSelected: ((OverlayPresentationMode) -> Void)?
  var onFrameTransitionCompleted: (() -> Void)?
  /// Mounted controls and the drawer share this panel's cursor authority.
  /// Weak membership prevents a removed or hidden SwiftUI candidate owning it.
  let menuCursorRegions = NSHashTable<NSView>.weakObjects()
  /// Selectable SwiftUI text that opens the keyboard gate without owning clicks.
  let selectableTextRegions = NSHashTable<NSView>.weakObjects()
  /// Grow-only expanded height for a candidate width. Nil keeps the remembered
  /// or default expanded size: manual sizing, an open editor, or a dirty draft.
  /// The panel remains the only writer of the transition frame.
  var qualifiedExpandedHeight: ((CGFloat) -> CGFloat?)?
  fileprivate var presence: OverlayPresence?
  fileprivate var appWindowYield: OverlayAppWindowYield?
  private var dragStart: (mouse: NSPoint, frame: NSRect, miniFrame: NSRect?)?
  private var dragMoved = false
  private var resizeStart: (mouse: NSPoint, frame: NSRect, edge: OverlayResizeHit.Edge)?
  private var requestedResizeSize: NSSize?
  private var pendingResizePoint: NSPoint?
  private var resizePaintTask: Task<Void, Never>?
  private var pendingResizePresentation: (mode: OverlayPresentationMode, animated: Bool)?
  private var expandedSize: NSSize?
  private var miniFrame: NSRect?
  fileprivate var presentationMode: OverlayPresentationMode = .expanded
  /// Duration of the morph most recently started; the SwiftUI cross-fade
  /// reads it so content and frame land together.
  private(set) var lastMorphDuration: TimeInterval = DictationOverlayWindow.presentationDuration
  private var transitionTop: CGFloat?
  private var transitionRight: CGFloat?
  private var frameTransitionTarget: NSRect?
  private var frameTransitionGeneration: UInt64 = 0
  private(set) var isFrameTransitioning = false
  var isUserResizing: Bool { resizeStart != nil }
  var isUserDragging: Bool { dragStart != nil }
  var sizeForPersistence: NSSize { expandedSize ?? frame.size }
  /// MIDI borrows width around the parked mini; its temporary left edge is
  /// never the saved position. A real drag moves that mini by the same edges.
  var originForPersistence: NSPoint {
    let positionedFrame = frameTransitionTarget ?? frame
    guard presentationMode == .midi else { return positionedFrame.origin }
    if let miniFrame {
      guard let dragStart, let parked = dragStart.miniFrame else { return miniFrame.origin }
      return NSPoint(
        x: parked.minX + positionedFrame.maxX - dragStart.frame.maxX,
        y: parked.minY + positionedFrame.maxY - dragStart.frame.maxY)
    }
    return NSPoint(
      x: positionedFrame.maxX - DictationOverlayWindow.collapsedSize.width,
      y: positionedFrame.maxY - DictationOverlayWindow.collapsedSize.height)
  }

  /// Grow leftward so the microphone and fold controls keep their screen position.
  /// Display containment wins when the complete strip cannot fit to the left.
  func setPresentationMode(
    _ mode: OverlayPresentationMode, animated: Bool = false, settleResize: Bool = false
  ) {
    if isUserResizing {
      pendingResizePresentation = (mode, animated)
      OverlayPresentationTrace.shared.record(
        .frameTransitionDeferredByResize, from: presentationMode, mode: mode, animated: animated,
        frame: frame)
      return
    }
    guard mode != presentationMode || settleResize else { return }
    let previous = presentationMode
    if previous == .mini && !settleResize { miniFrame = frameTransitionTarget ?? frame }
    presentationMode = mode
    let wasApplyingFrame = OverlayController.isApplyingFrame
    OverlayController.isApplyingFrame = true
    defer { OverlayController.isApplyingFrame = wasApplyingFrame }
    let top = isFrameTransitioning ? transitionTop ?? frame.maxY : frame.maxY
    let right =
      if previous == .midi, let miniFrame, !settleResize {
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
      size = expandedPresentationSize()
    }
    // Intermediate frames may be smaller than the expanded window's floor.
    minSize = DictationOverlayWindow.collapsedSize
    contentMinSize = minSize
    styleMask.remove(.resizable)
    let visible = screen?.visibleFrame ?? NSScreen.main?.visibleFrame
    let proposed = NSRect(
      x: right - size.width, y: top - size.height, width: size.width, height: size.height)
    // One rect is the animation target. A screen clamp that narrows the width
    // is remeasured here, still before the animator, so completion does not
    // write a second endpoint.
    var restored = DictationOverlayWindow.visibleExpansionFrame(proposed, in: visible)
    if mode == .expanded, abs(restored.width - size.width) > 0.5,
      let qualified = qualifiedExpandedHeight?(restored.width)
    {
      let height = max(restored.height, qualified)
      let regrown = NSRect(
        x: restored.maxX - restored.width, y: restored.maxY - height, width: restored.width,
        height: height)
      restored = DictationOverlayWindow.visibleExpansionFrame(regrown, in: visible)
    }
    frameTransitionGeneration &+= 1
    let generation = frameTransitionGeneration
    frameTransitionTarget = restored
    isFrameTransitioning = animated
    OverlayPresentationTrace.shared.record(
      .frameTransitionBegan, from: previous, mode: mode, animated: animated, frame: frame,
      target: restored, generation: generation, note: settleResize ? "settle_resize" : nil)
    lastMorphDuration =
      animated ? DictationOverlayWindow.presentationDuration(from: frame, to: restored) : 0
    NSAnimationContext.runAnimationGroup { context in
      context.duration = lastMorphDuration
      context.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
      animator().setFrame(restored, display: true)
    } completionHandler: { [weak self] in
      Task { @MainActor in
        self?.completeFrameTransition(generation: generation)
      }
    }
    if !animated { completeFrameTransition(generation: generation) }
  }

  /// Remembered or default expanded size, raised to the qualified content
  /// height when the controller still owns automatic sizing.
  private func expandedPresentationSize() -> NSSize {
    let expanded = expandedSize ?? DictationOverlayWindow.defaultSize
    let restingHeight = max(expanded.height, DictationOverlayWindow.minSize.height)
    let qualified = qualifiedExpandedHeight?(expanded.width) ?? restingHeight
    return NSSize(width: expanded.width, height: max(restingHeight, qualified))
  }

  private func completeFrameTransition(generation: UInt64) {
    guard generation == frameTransitionGeneration, let target = frameTransitionTarget else {
      return
    }
    OverlayPresentationTrace.shared.record(
      .frameTransitionCompleted, mode: presentationMode, frame: frame, target: target,
      generation: generation, note: frame == target ? nil : "frame≠target")
    isFrameTransitioning = false
    frameTransitionTarget = nil
    transitionTop = nil
    transitionRight = nil
    // Mini is a drag surface; MIDI exposes only horizontal resize handles.
    minSize = DictationOverlayWindow.minimumSize(for: presentationMode)
    contentMinSize = minSize
    if presentationMode != .mini { styleMask.insert(.resizable) }
    if let contentView {
      invalidateCursorRects(for: contentView)
      (contentView as? OverlayContentContainer)?.refreshGrabPad()
      OverlayPresentationTrace.shared.record(.cursorRectsInvalidated, mode: presentationMode)
    }
    if presentationMode == .expanded {
      expandedSize = nil
      miniFrame = nil
    }
    onFrameTransitionCompleted?()
  }

  func settleFrameTransition() {
    guard let target = frameTransitionTarget else { return }
    OverlayPresentationTrace.shared.record(
      .frameTransitionSettled, mode: presentationMode, frame: frame, target: target,
      generation: frameTransitionGeneration)
    NSAnimationContext.runAnimationGroup { context in
      context.duration = 0
      animator().setFrame(target, display: true)
    }
    completeFrameTransition(generation: frameTransitionGeneration)
  }

  func resetPresentationPosition() { miniFrame = nil }

  func startPresence() {
    presence?.start()
  }

  func refreshPresence() {
    presence?.refresh()
  }

  func invalidatePresence() {
    cancelUserResize()
    presence?.invalidate()
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
    if isKeyWindow, event.type == .keyDown,
      event.modifierFlags.intersection([.command, .control, .option, .shift]) == .command,
      event.charactersIgnoringModifiers?.lowercased() == "c",
      let responder = firstResponder, responder.responds(to: #selector(NSText.copy(_:)))
    {
      // The focused responder owns its copy: the transcript canvas routes
      // through the tagged clipboard bridge in its own `copy(_:)`, and any
      // other text view keeps the behaviour it overrides.
      if let textView = responder as? NSTextView {
        textView.copy(nil)
        return true
      }
      return responder.tryToPerform(#selector(NSText.copy(_:)), with: nil)
    }
    // This panel is non-activating, so the app's Edit menu does not receive
    // key equivalents while another app is active; the action has to be sent
    // explicitly down the responder chain.
    if isKeyWindow, event.type == .keyDown,
      event.modifierFlags.intersection([.command, .control, .option, .shift]) == .command,
      event.charactersIgnoringModifiers?.lowercased() == "c",
      firstResponder?.responds(to: #selector(NSText.copy(_:))) != true,
      NSApp.sendAction(#selector(NSText.copy(_:)), to: nil, from: self)
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
    // An unpinned panel sits among ordinary windows and never activates, so
    // nothing else would lift it back out from under the window covering it.
    if event.type == .leftMouseDown, level == .normal {
      orderFrontRegardless()
    }
    if event.type == .mouseMoved {
      for case let region as OverlayMenuCursorRegionView in menuCursorRegions.allObjects {
        region.updateHover(at: event.locationInWindow)
      }
    }
    // The drawer owns cursor motion before overlapping transcript tracking
    // can set an I-beam. Clicks still follow the native controls below.
    if event.type == .mouseMoved || event.type == .cursorUpdate,
      containsMenuCursorPoint(event.locationInWindow)
    {
      refreshCursor(at: event.locationInWindow)
      return
    }
    // Text tracking areas can overlap the resize strip even when hitTest
    // returns the container. Dispatching these motions to AppKit first sets
    // an iBeam, then the panel sets a resize cursor on every event.
    // The strip owns motion as well as dragging; interior tracking stays native.
    if event.type == .mouseMoved || event.type == .cursorUpdate,
      styleMask.contains(.resizable), let contentView,
      OverlayResizeHit.edge(
        at: event.locationInWindow, in: contentView.bounds, mode: presentationMode) != nil
    {
      refreshCursor(at: event.locationInWindow)
      return
    }
    if event.type == .mouseMoved || event.type == .cursorUpdate,
      isWindowDragHit(at: event.locationInWindow)
    {
      refreshCursor(at: event.locationInWindow)
      return
    }
    if event.type == .leftMouseDown {
      if isUserResizing { endUserResize() }
      dragStart = nil
      dragMoved = false
      if styleMask.contains(.resizable), let contentView,
        contentView.bounds.contains(event.locationInWindow),
        let edge = OverlayResizeHit.edge(
          at: event.locationInWindow, in: contentView.bounds, mode: presentationMode)
      {
        beginUserResize(edge: edge, at: screenPoint(for: event))
        return
      }
    }
    switch event.type {
    case .leftMouseDragged where isUserResizing:
      scheduleUserResize(to: screenPoint(for: event))
    case .leftMouseUp where isUserResizing:
      pendingResizePoint = screenPoint(for: event)
      endUserResize()
    case .leftMouseDown where isWindowDragHit(at: event.locationInWindow):
      settleFrameTransition()
      dragStart = (screenPoint(for: event), frame, miniFrame)
    case .leftMouseDragged where dragStart != nil:
      guard let dragStart else { return }
      let current = screenPoint(for: event)
      // Pointer jitter belongs to a click. Once the gesture crosses the
      // threshold it remains a drag, even if it returns to its origin.
      guard dragMoved || hypot(current.x - dragStart.mouse.x, current.y - dragStart.mouse.y) >= 3
      else { return }
      dragMoved = true
      let proposed = NSPoint(
        x: dragStart.frame.minX + current.x - dragStart.mouse.x,
        y: dragStart.frame.minY + current.y - dragStart.mouse.y
      )
      // Follow the destination display while dragging; persist only a visible
      // origin, including when that display is left of the primary screen.
      let visible =
        NSScreen.screens.first { $0.frame.contains(current) }?.visibleFrame
        ?? screen?.visibleFrame
      setFrameOrigin(
        visible.map { OverlayPlacement.clampOrigin(proposed, size: frame.size, in: $0) }
          ?? proposed)
    case .leftMouseUp where dragStart != nil:
      let current = screenPoint(for: event)
      let clickedPresentation =
        !dragMoved
        && dragStart.map { hypot(current.x - $0.mouse.x, current.y - $0.mouse.y) < 3 } == true
        && isPresentationClickHit(at: event.locationInWindow)
      let origin = originForPersistence
      let moved = dragMoved
      if moved, presentationMode == .midi {
        miniFrame = NSRect(origin: origin, size: DictationOverlayWindow.collapsedSize)
      }
      dragStart = nil
      dragMoved = false
      if moved { onUserDragEnded?(origin) }
      if clickedPresentation { onPresentationClick?() }
    default:
      if event.type == .leftMouseDown {
        for case let region as OverlaySelectableTextRegionView in selectableTextRegions.allObjects
        where region.contains(windowPoint: event.locationInWindow) {
          takeKeyForTranscript()
          break
        }
      }
      if event.type == .leftMouseDown, let contentView {
        var hit = contentView.hitTest(contentView.convert(event.locationInWindow, from: nil))
        while let view = hit {
          // SwiftUI uses different native text classes across macOS versions.
          // Follow public responder capabilities, not a private class name.
          if view.acceptsFirstResponder && view.responds(to: #selector(NSText.copy(_:))) {
            takeKeyForTranscript()
            break
          }
          hit = view.superview
        }
      }
      super.sendEvent(event)
      if event.type == .mouseMoved { refreshCursor(at: event.locationInWindow) }
    }
  }

  /// Keep only the latest pointer within a display beat. Reflowing SwiftUI
  /// synchronously for every queued drag event leaves the pointer behind.
  private func scheduleUserResize(to point: NSPoint) {
    pendingResizePoint = point
    guard resizePaintTask == nil else { return }
    let framesPerSecond = max(30, min(screen?.maximumFramesPerSecond ?? 60, 120))
    let delay = Duration.nanoseconds(1_000_000_000 / framesPerSecond)
    resizePaintTask = Task { @MainActor [weak self] in
      try? await Task.sleep(for: delay)
      guard !Task.isCancelled, let self else { return }
      self.flushPendingResize()
    }
  }

  private func flushPendingResize() {
    resizePaintTask?.cancel()
    resizePaintTask = nil
    guard let point = pendingResizePoint else { return }
    pendingResizePoint = nil
    updateUserResize(to: point)
  }

  @discardableResult
  func beginUserResize(edge: OverlayResizeHit.Edge, at point: NSPoint) -> Bool {
    guard styleMask.contains(.resizable), !isFrameTransitioning, !isUserResizing,
      presentationMode != .mini,
      presentationMode != .midi || edge == .left || edge == .right
    else {
      return false
    }
    resizeStart = (point, frame, edge)
    requestedResizeSize = frame.size
    OverlayPresentationTrace.shared.record(
      .userResizeBegan, mode: presentationMode, frame: frame, note: "edge=\(edge)")
    onUserResize?()
    return true
  }

  @discardableResult
  func updateUserResize(to point: NSPoint) -> Bool {
    guard let resizeStart else { return false }
    let requested = OverlayResizeHit.apply(
      edge: resizeStart.edge, start: resizeStart.frame,
      dx: point.x - resizeStart.mouse.x, dy: point.y - resizeStart.mouse.y,
      minSize: DictationOverlayWindow.collapsedSize)
    // Track the fold intent independently of the painted frame. Expanded
    // content cannot fit inside a strip while the gesture is still active.
    requestedResizeSize = requested.size
    // The grabbed edge follows the pointer; its opposite edge stays fixed.
    // Upper-right anchoring belongs to presentation-mode morphs, not gestures.
    let target = OverlayResizeHit.apply(
      edge: resizeStart.edge, start: resizeStart.frame,
      dx: point.x - resizeStart.mouse.x, dy: point.y - resizeStart.mouse.y,
      minSize: DictationOverlayWindow.minimumSize(for: presentationMode))
    guard target != frame else { return false }
    // Do not force an intermediate paint while hosting geometry is invalidated.
    setFrame(target, display: false)
    return true
  }

  func endUserResize() {
    flushPendingResize()
    guard let resizeStart else { return }
    let requested = requestedResizeSize ?? frame.size
    let changed = frame.size != resizeStart.frame.size || requested != resizeStart.frame.size
    if changed, pendingResizePresentation == nil, presentationMode == .expanded {
      let mode = DictationOverlayWindow.presentationMode(forResizedSize: requested)
      // OverlayState remains the presentation authority. Its ordinary
      // callback queues the selected mode until this gesture ends.
      onResizePresentationSelected?(mode)
      if pendingResizePresentation == nil { pendingResizePresentation = (mode, false) }
    }
    if changed {
      if pendingResizePresentation?.mode == .expanded {
        expandedSize = DictationOverlayWindow.clamp(frame.size)
      } else if presentationMode == .expanded {
        expandedSize = resizeStart.frame.size
      }
      miniFrame = nil
    }
    self.resizeStart = nil
    requestedResizeSize = nil
    let pending = pendingResizePresentation
    pendingResizePresentation = nil
    OverlayPresentationTrace.shared.record(
      .userResizeEnded, mode: presentationMode, frame: frame,
      note: pending.map { "pending=\($0.mode)" } ?? (changed ? "changed" : "unchanged"))
    if let pending {
      setPresentationMode(pending.mode, animated: pending.animated, settleResize: changed)
    }
    onUserResizeEnded?()
  }

  /// Closing a panel must not flush a deferred show or revive its geometry.
  private func cancelUserResize() {
    resizePaintTask?.cancel()
    resizePaintTask = nil
    pendingResizePoint = nil
    resizeStart = nil
    requestedResizeSize = nil
    pendingResizePresentation = nil
  }

  /// Native tracking owns the interior, including SwiftUI text and links
  /// whose hit surface is not an NSTextView. Only resize edges and the
  /// explicitly inert window-drag chrome override it.
  @discardableResult
  func refreshCursor(at point: NSPoint) -> Bool {
    guard let contentView else { return false }
    if let desired = menuCursor(at: point) {
      guard NSCursor.current != desired else { return false }
      desired.set()
      return true
    }
    let resizeEdge =
      styleMask.contains(.resizable)
      && OverlayResizeHit.edge(at: point, in: contentView.bounds, mode: presentationMode) != nil
    guard resizeEdge || isWindowDragHit(at: point) else { return false }
    let desired = resizeEdge ? cursor(at: point) : NSCursor.arrow
    guard desired != .iBeam, NSCursor.current != desired else { return false }
    desired.set()
    return true
  }

  /// Resolve from this panel's hit surface, never the inactive app below it.
  func cursor(at point: NSPoint) -> NSCursor {
    guard let contentView else { return .arrow }
    if let desired = menuCursor(at: point) { return desired }
    if styleMask.contains(.resizable),
      let edge = OverlayResizeHit.edge(at: point, in: contentView.bounds, mode: presentationMode)
    {
      return OverlayResizeHit.cursor(for: edge)
    }
    var hit = contentView.hitTest(point)
    while let view = hit {
      if view is NSTextView { return .iBeam }
      if let field = view as? NSTextField, field.isEditable { return .iBeam }
      hit = view.superview
    }
    return .arrow
  }

  private func containsMenuCursorPoint(_ point: NSPoint) -> Bool {
    menuCursor(at: point) != nil
  }

  private func menuCursor(at point: NSPoint) -> NSCursor? {
    if styleMask.contains(.resizable), let contentView,
      OverlayResizeHit.edge(at: point, in: contentView.bounds, mode: presentationMode) != nil
    {
      return nil
    }
    // The drawer can contain native text entry (live search). Its field
    // editor keeps native tracking, unlike the transcript behind the drawer.
    var hit = contentView?.hitTest(point)
    while let view = hit {
      if let text = view as? NSTextView, text.isEditable,
        !(text is LiveTranscriptNativeTextView)
      {
        return nil
      }
      if let field = view as? NSTextField, field.isEditable { return nil }
      hit = view.superview
    }
    // Events use window coordinates. Bound the override by the mounted
    // drawer's frame as well as its clipped visible rect, in that same space.
    // A native text hit outside this frame must never inherit menu ownership.
    // A small divider region wins over the enclosing drawer. The panel stays
    // the single cursor owner; nested SwiftUI hover handlers cannot fight it.
    return menuCursorRegions.allObjects.compactMap { $0 as? OverlayMenuCursorRegionView }.filter {
      region in
      guard region.window === self, !region.isHiddenOrHasHiddenAncestor,
        let parent = region.superview
      else { return false }
      let frameInWindow = parent.convert(region.frame, to: nil)
      let visibleInWindow = region.convert(region.visibleRect, to: nil)
      return frameInWindow.contains(point) && visibleInWindow.contains(point)
    }.min { lhs, rhs in
      lhs.bounds.width * lhs.bounds.height < rhs.bounds.width * rhs.bounds.height
    }?.cursor
  }

  func isWindowDragHit(at point: NSPoint) -> Bool {
    guard let contentView, let hit = contentView.hitTest(point) else { return false }
    // The container claims the resize band, which sendEvent routes through
    // the panel's edge gesture; a container hit is never a drag handle.
    if hit === contentView { return false }
    return hit is OverlayWindowDragRegionView
  }

  /// A still click on draggable chrome advances the presentation. In mini the
  /// whole widget is that chrome; in midi and expanded only the header band
  /// is, so a body drag that never moved stays an ordinary click.
  func isPresentationClickHit(at point: NSPoint) -> Bool {
    guard let contentView, let hit = contentView.hitTest(point), hit !== contentView,
      let region = hit as? OverlayWindowDragRegionView
    else { return false }
    if presentationMode == .mini { return true }
    return region.accessibilityIdentifier().hasPrefix("overlay-header")
  }

  private func screenPoint(for event: NSEvent) -> NSPoint {
    if let point = event.cgEvent?.location {
      // Quartz is top-left/y-down; AppKit window origins are bottom-left/y-up.
      return NSPoint(x: point.x, y: -point.y)
    }
    return convertPoint(toScreen: event.locationInWindow)
  }

  func windowDidMove(_ notification: Notification) {
    OverlayPresentationTrace.shared.record(
      .windowMoved, mode: presentationMode, frame: frame,
      note: isFrameTransitioning
        ? "transitioning" : OverlayController.isApplyingFrame ? "programmatic" : "user")
    guard !isFrameTransitioning else { return }
    if !OverlayController.isApplyingFrame, presentationMode != .midi {
      resetPresentationPosition()
    }
    onUserMove?()
  }

  func windowDidResize(_ notification: Notification) {
    // Animator steps land here too: the ledger holds the painted trajectory.
    OverlayPresentationTrace.shared.record(
      .windowResized, mode: presentationMode, frame: frame,
      generation: isFrameTransitioning ? frameTransitionGeneration : nil,
      note: isFrameTransitioning
        ? "transitioning"
        : isUserResizing ? "user" : OverlayController.isApplyingFrame ? "programmatic" : "idle")
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
  let grabPad = OverlayResizeGrabPad()

  init(hosting: NSView) {
    self.hosting = hosting
    super.init(frame: .zero)
    addSubview(grabPad)
    addSubview(hosting)
    hosting.frame = bounds
  }

  @available(*, unavailable)
  required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

  override func setFrameSize(_ newSize: NSSize) {
    let previousBounds = bounds
    super.setFrameSize(newSize)
    if hosting.frame != bounds { hosting.frame = bounds }
    if previousBounds != bounds {
      window?.invalidateCursorRects(for: self)
      refreshGrabPad()
    }
  }

  override func layout() {
    super.layout()
    if hosting.frame != bounds { hosting.frame = bounds }
    refreshGrabPad()
  }

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    refreshGrabPad()
  }

  /// The pad follows the frame and the mode; a mini panel has no resize zone.
  func refreshGrabPad() {
    let resizable = window?.styleMask.contains(.resizable) == true
    let mode = (window as? FloatingOverlayPanel)?.presentationMode ?? .expanded
    grabPad.update(bounds: bounds, mode: resizable ? mode : .mini)
  }

  /// AppKit's borderless resize strip is ~1–2 px. Claim the 16 pt band first,
  /// including the bottom capsule and its margin, so SwiftUI cannot steal it.
  override func hitTest(_ point: NSPoint) -> NSView? {
    if window?.styleMask.contains(.resizable) == true,
      OverlayResizeHit.edge(
        at: point, in: bounds,
        mode: (window as? FloatingOverlayPanel)?.presentationMode ?? .expanded) != nil
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
    // No whole-window arrow rectangle: it overlaps every native text/link
    // rectangle and makes AppKit alternate cursor owners on entry. Interior
    // views own their cursors; this container owns only the resize band.
    guard window?.styleMask.contains(.resizable) == true else { return }
    for (rect, cursor) in OverlayResizeHit.cursorRects(
      in: bounds, mode: (window as? FloatingOverlayPanel)?.presentationMode ?? .expanded)
    {
      addCursorRect(rect, cursor: cursor)
    }
  }

  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

/// The window server passes a mouse event through every fully transparent
/// pixel of a non-opaque window, and the rounded glass leaves the corners of
/// the resize zone transparent: a corner grab reached the app beneath before
/// the panel saw it. A fill of 2/255 keeps those pixels inside the panel's
/// event shape; it is invisible on white and on black. It paints only
/// `OverlayResizeHit.grabPadRects`, all of them resize zones.
final class OverlayResizeGrabPad: NSView {
  static let fillAlpha: CGFloat = 2 / 255
  private let shape = CAShapeLayer()
  private(set) var rects: [NSRect] = []

  init() {
    super.init(frame: .zero)
    wantsLayer = true
    shape.fillColor = NSColor(white: 0.5, alpha: Self.fillAlpha).cgColor
    shape.actions = ["path": NSNull(), "bounds": NSNull(), "position": NSNull()]
    layer?.addSublayer(shape)
  }

  @available(*, unavailable)
  required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

  override func hitTest(_ point: NSPoint) -> NSView? { nil }

  func update(bounds container: NSRect, mode: OverlayPresentationMode) {
    if frame != container { frame = container }
    let rects = OverlayResizeHit.grabPadRects(in: container, mode: mode)
    guard rects != self.rects || shape.frame != bounds else { return }
    self.rects = rects
    shape.frame = bounds
    let path = CGMutablePath()
    for rect in rects { path.addRect(rect) }
    shape.path = path
  }
}

enum DictationOverlayWindow {
  /// Founder, 2026-10-15: the collapse animations are too fast; a big
  /// full-screen overlay folded in the same time as MIDI to mini. The morph
  /// keeps one speed instead of one clock: the edge that travels farthest
  /// sets the duration, and a short step keeps the former 0.28 s.
  static let presentationSpeed: CGFloat = 1000
  static let presentationDuration: TimeInterval = 0.28

  static func presentationDuration(from start: NSRect, to end: NSRect) -> TimeInterval {
    let travel = max(
      abs(end.minX - start.minX), abs(end.maxX - start.maxX),
      abs(end.minY - start.minY), abs(end.maxY - start.maxY))
    return max(presentationDuration, TimeInterval(travel / presentationSpeed))
  }
  static let collapsedHeight: CGFloat = 46
  static let collapsedSize = NSSize(width: 200, height: collapsedHeight)
  static let midiSize = NSSize(width: 410, height: collapsedHeight)

  static func minimumSize(for mode: OverlayPresentationMode) -> NSSize {
    switch mode {
    case .mini: collapsedSize
    case .midi: midiSize
    case .expanded: minSize
    }
  }

  static func presentationMode(forResizedSize size: NSSize) -> OverlayPresentationMode {
    if size.height >= (collapsedHeight + minSize.height) / 2 { return .expanded }
    return size.width < (collapsedSize.width + midiSize.width) / 2 ? .mini : .midi
  }

  /// Shared geometry seam: a low-dragged/bottom-anchored bar must not unfold
  /// below the display. Keep its top unchanged whenever the full frame fits.
  static func visibleExpansionFrame(_ proposed: NSRect, in visible: NSRect?) -> NSRect {
    guard let visible else { return proposed }
    let size = NSSize(
      width: min(proposed.width, visible.width), height: min(proposed.height, visible.height))
    return NSRect(
      origin: OverlayPlacement.clampOrigin(proposed.origin, size: size, in: visible), size: size)
  }
  /// Readability floor for settled regular content and programmatic content
  /// sizing via `clamp(_:to:)`. A live edge gesture records fold intent below
  /// this floor, but paints the current form at its floor until release.
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
    state.onPresentationModeChanged = { [weak panel, weak state] mode in
      guard let panel else { return }
      panel.setPresentationMode(
        mode,
        animated: panel.isVisible && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
      state?.presentationMorphDuration = panel.lastMorphDuration
    }
    panel.onResizePresentationSelected = { [weak state] mode in
      state?.setPresentationMode(mode)
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
    panel.onPresentationClick = { [weak state] in
      state?.advancePresentationFromHeader()
    }
    panel.onUserResize = { [weak state] in
      guard !OverlayController.isApplyingFrame else { return }
      state?.userResizedOverlay()
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

    // Maciej (2026-10-17): Pin means on top no matter what; unpinned stays
    // visible but slips under whatever window is clicked.
    let presence = OverlayPresence(panel: panel) { [weak state] in
      state?.keepVisibleBetweenTakes ?? true
    }
    panel.presence = presence
    state.onPinChanged = { [weak panel] in panel?.refreshPresence() }
    panel.startPresence()

    let appWindowYield = OverlayAppWindowYield(panel: panel, state: state)
    panel.appWindowYield = appWindowYield
    appWindowYield.start()

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

/// Rest / unpinned / yield / capture. A pinned panel rests above ordinary
/// windows; an unpinned one lives among them. Screenshot chords rise above the
/// forest so PrintScreen actually sees the panel; system alerts push it back down.
enum OverlayPresencePolicy: Equatable {
  case rest
  case unpinned
  case yield
  case capture

  var windowLevel: NSWindow.Level {
    switch self {
    case .rest: .floating
    case .unpinned, .yield: .normal
    case .capture: .statusBar
    }
  }

  static let yieldBundleIds: Set<String> = [
    "com.apple.SecurityAgent",
    "com.apple.UserNotificationCenter",
    "com.apple.CoreServicesUIAgent",
    "com.apple.loginwindow",
  ]

  static func resolve(
    screenshotChord: Bool, shouldYield: Bool, pinned: Bool = true
  ) -> OverlayPresencePolicy {
    if screenshotChord { return .capture }
    if shouldYield { return .yield }
    return pinned ? .rest : .unpinned
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
  private let pinned: @MainActor () -> Bool
  private var localMonitor: Any?
  private var globalMonitor: Any?
  private var workspaceObserver: NSObjectProtocol?
  private var captureUntil: Date?
  private var captureTimer: Timer?

  init(panel: NSPanel, pinned: @escaping @MainActor () -> Bool = { true }) {
    self.panel = panel
    self.pinned = pinned
  }

  /// Re-resolve the level now, e.g. after the pin was toggled.
  func refresh() {
    apply()
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
      ),
      pinned: pinned()
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
  /// The bottom corners are whole squares, 30 pt a side (Maciej, 2026-10-14:
  /// a 16 pt corner under a 22 pt glass radius left a sliver to grab). The top
  /// corners keep the band: the header's close dot sits 16 pt from the left.
  static let bottomCorner: CGFloat = 30
  /// The top edge is thinner than the sides. Founder, 2026-10-15 (build 1050):
  /// the pointer turned into a resize cursor over Pin. The header's controls
  /// start 10 pt below the top edge (its vertical padding), so a 16 pt band
  /// took their upper third, cursor and click alike. The band ends above them.
  static let topBand: CGFloat = 8

  enum Edge: Sendable, Hashable {
    case left, right, top, bottom
    case topLeft, topRight, bottomLeft, bottomRight
  }

  static func edge(
    at point: NSPoint, in bounds: NSRect, mode: OverlayPresentationMode = .expanded
  ) -> Edge? {
    guard mode != .mini else { return nil }
    guard bounds.width > band * 2, bounds.height > band * 2 else { return nil }
    let left = point.x <= bounds.minX + band
    let right = point.x >= bounds.maxX - band
    if mode == .midi { return left ? .left : right ? .right : nil }
    let corner = bottomCornerSide(in: bounds)
    if point.y <= bounds.minY + corner {
      if point.x <= bounds.minX + corner { return .bottomLeft }
      if point.x >= bounds.maxX - corner { return .bottomRight }
    }
    // The bottom corner squares above already own every point that is both
    // within the band of a side and of the bottom edge.
    let bottom = point.y <= bounds.minY + band
    let top = point.y >= bounds.maxY - topBand
    switch (left, right, bottom, top) {
    case (true, false, false, true): return .topLeft
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
  static func cursorRects(
    in bounds: NSRect, mode: OverlayPresentationMode = .expanded
  ) -> [(NSRect, NSCursor)] {
    if mode == .mini { return [] }
    if mode == .midi {
      return [
        (
          NSRect(x: bounds.minX, y: bounds.minY, width: band, height: bounds.height),
          cursor(for: .left)
        ),
        (
          NSRect(x: bounds.maxX - band, y: bounds.minY, width: band, height: bounds.height),
          cursor(for: .right)
        ),
      ]
    }
    let b = band
    let t = topBand
    let c = bottomCornerSide(in: bounds)
    let w = bounds.width
    let h = bounds.height
    return [
      (NSRect(x: 0, y: c, width: b, height: max(0, h - c - t)), cursor(for: .left)),
      (NSRect(x: w - b, y: c, width: b, height: max(0, h - c - t)), cursor(for: .right)),
      (NSRect(x: b, y: h - t, width: max(0, w - 2 * b), height: t), cursor(for: .top)),
      (NSRect(x: c, y: 0, width: max(0, w - 2 * c), height: b), cursor(for: .bottom)),
      (NSRect(x: 0, y: h - t, width: b, height: t), cursor(for: .topLeft)),
      (NSRect(x: w - b, y: h - t, width: b, height: t), cursor(for: .topRight)),
      (NSRect(x: 0, y: 0, width: c, height: c), cursor(for: .bottomLeft)),
      (NSRect(x: w - c, y: 0, width: c, height: c), cursor(for: .bottomRight)),
    ]
  }

  /// The bottom corner square, never larger than half the panel.
  static func bottomCornerSide(in bounds: NSRect) -> CGFloat {
    max(band, min(bottomCorner, bounds.width / 2, bounds.height / 2))
  }

  /// Where the panel must own pointer events although the glass leaves its
  /// pixels transparent: the rounded corners. Every rect lies inside a resize
  /// zone, so the pad never takes a click that the content receives today.
  static func grabPadRects(in bounds: NSRect, mode: OverlayPresentationMode) -> [NSRect] {
    guard mode != .mini, bounds.width > band * 2, bounds.height > band * 2 else { return [] }
    let b = band
    let w = bounds.width
    let h = bounds.height
    if mode == .midi {
      return [
        NSRect(x: 0, y: 0, width: b, height: h),
        NSRect(x: w - b, y: 0, width: b, height: h),
      ]
    }
    let t = topBand
    let c = bottomCornerSide(in: bounds)
    // Along each top edge the band reaches past the glass curve (~22 pt).
    let reach = min(c, w / 2, h / 2)
    return [
      NSRect(x: 0, y: 0, width: c, height: c),
      NSRect(x: w - c, y: 0, width: c, height: c),
      NSRect(x: 0, y: h - reach, width: b, height: reach),
      NSRect(x: 0, y: h - t, width: reach, height: t),
      NSRect(x: w - b, y: h - reach, width: b, height: reach),
      NSRect(x: w - reach, y: h - t, width: reach, height: t),
    ]
  }

  @MainActor
  static func cursor(for edge: Edge) -> NSCursor {
    resizeCursors[edge] ?? .arrow
  }

  // frameResize creates cursor objects. Reuse one object per edge so repeated
  // mouseMoved/cursorUpdate beats do not regenerate the same cursor image.
  @MainActor
  private static let resizeCursors: [Edge: NSCursor] = Dictionary(
    uniqueKeysWithValues: [
      Edge.left, .right, .top, .bottom, .topLeft, .topRight, .bottomLeft, .bottomRight,
    ]
    .map { ($0, makeCursor(for: $0)) })

  @MainActor
  private static func makeCursor(for edge: Edge) -> NSCursor {
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
