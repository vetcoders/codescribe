import AppKit
import QuartzCore
import SwiftUI

/// The two canvases share a panel and fold control, but remember their own size.
enum OverlayContentSurface: String, CaseIterable, Hashable, Sendable {
  case transcription
  case agents
}

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
  private var expandedSizes: [OverlayContentSurface: NSSize] = [:]
  fileprivate(set) var contentSurface: OverlayContentSurface = .transcription
  var contentSizeDefaults: UserDefaults = .standard
  private var expandedSize: NSSize? {
    get { expandedSizes[contentSurface] }
    set { expandedSizes[contentSurface] = newValue }
  }
  private var miniFrame: NSRect?
  /// An agent records: mini hugs its three controls instead of the resting strip.
  private(set) var miniHugsAgentControls = false
  var miniSize: NSSize {
    miniHugsAgentControls
      ? DictationOverlayWindow.agentMiniSize : DictationOverlayWindow.collapsedSize
  }
  /// Compact's natural width: the 410 pt floor plus the channel map
  /// (`DictationOverlayWindow.midiNaturalWidth`). The controller reports it
  /// whenever the map's channels change.
  private(set) var midiNaturalWidth = DictationOverlayWindow.midiSize.width
  /// The width the user gave the compact strip with its edge in this session,
  /// and whether that was narrower than the natural width at the time. A
  /// narrowed strip keeps the user's width: roster changes no longer resize it
  /// and the map drops the chips' names, or collapses to its counter, whose
  /// click gives the natural width back (`restoreMidiNaturalWidth`). A strip
  /// the user widened grows when the map outgrows it and never shrinks below
  /// the user's width.
  private var midiUserWidth: CGFloat?
  private(set) var midiShrunkByUser = false
  /// The width compact settles to: the user's after a manual shrink, else the
  /// natural width, raised to a width the user chose.
  var midiPresentationWidth: CGFloat {
    if midiShrunkByUser, let midiUserWidth { return midiUserWidth }
    return max(midiNaturalWidth, midiUserWidth ?? 0)
  }
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
  var sizeForPersistence: NSSize {
    expandedSize
      ?? (presentationMode == .expanded
        ? frame.size
        : DictationOverlayWindow.restoredContentSize(
          surface: contentSurface, defaults: contentSizeDefaults))
  }

  func persistContentSize() {
    DictationOverlayWindow.persist(
      size: sizeForPersistence, surface: contentSurface, defaults: contentSizeDefaults)
  }

  /// Busy gestures and morphs finish first. The controller's existing completion
  /// callbacks retry the latest projected surface, without a second pending owner.
  @discardableResult
  func setContentSurface(_ surface: OverlayContentSurface, animated: Bool = false) -> Bool {
    guard surface != contentSurface, !isUserResizing, !isFrameTransitioning else { return false }
    expandedSize = sizeForPersistence
    persistContentSize()
    contentSurface = surface
    if presentationMode == .expanded {
      if expandedSize == nil {
        expandedSize = DictationOverlayWindow.restoredContentSize(
          surface: surface, defaults: contentSizeDefaults)
      }
      setPresentationMode(.expanded, animated: animated, settleResize: true)
    }
    return true
  }
  /// MIDI borrows width around the parked mini; its temporary left edge is
  /// never the saved position. A real drag moves that mini by the same edges.
  var originForPersistence: NSPoint {
    let positionedFrame = frameTransitionTarget ?? frame
    guard presentationMode == .midi else {
      // A hugging mini is the resting strip anchored at the same trailing edge.
      guard presentationMode == .mini, miniHugsAgentControls else { return positionedFrame.origin }
      return NSPoint(
        x: positionedFrame.maxX - DictationOverlayWindow.collapsedSize.width,
        y: positionedFrame.minY)
    }
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
      size =
        mode == .mini
        ? miniSize
        : NSSize(width: midiPresentationWidth, height: DictationOverlayWindow.collapsedHeight)
    } else {
      size = expandedPresentationSize()
    }
    // Intermediate frames may be smaller than the expanded window's floor.
    minSize = miniSize
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
    // A morph holds one layout from its first step to its landing. An unroll
    // holds the landing layout and the growing window reveals it; a fold
    // holds the departing layout and the shrinking window covers it, so a
    // step only moves the hosting view instead of laying the panel out at
    // every intermediate size. A fold that catches an unroll keeps the size
    // the unroll already holds: the layout on screen never changes size
    // mid-flight, whichever way the morph is travelling.
    let container = contentView as? OverlayContentContainer
    let unrolls = animated && mode == .expanded && previous != .expanded
    let folds =
      animated && !unrolls && mode != .expanded
      && (restored.width < frame.width - 0.5 || restored.height < frame.height - 0.5)
    if unrolls {
      container?.pinnedContentSize = restored.size
    } else if folds {
      container?.pinnedContentSize = container?.pinnedContentSize ?? frame.size
    } else {
      container?.pinnedContentSize = nil
    }
    OverlayPresentationTrace.shared.record(
      .frameTransitionBegan, from: previous, mode: mode, animated: animated, frame: frame,
      target: restored, generation: generation,
      note: settleResize ? "settle_resize" : unrolls ? "unroll" : folds ? "fold" : nil)
    lastMorphDuration =
      animated ? DictationOverlayWindow.presentationDuration(from: frame, to: restored) : 0
    guard unrolls || folds else {
      animateFrame(to: restored, generation: generation)
      if !animated { completeFrameTransition(generation: generation) }
      return
    }
    // The held layout is built before the animator's clock starts: a layout
    // inside the first step would cost that step its frames. One turn later the
    // controller has also published the morph duration the SwiftUI fade reads.
    Task { @MainActor [weak self] in
      guard let self, generation == frameTransitionGeneration, isFrameTransitioning else {
        return
      }
      let wasApplyingFrame = OverlayController.isApplyingFrame
      OverlayController.isApplyingFrame = true
      defer { OverlayController.isApplyingFrame = wasApplyingFrame }
      contentView?.layoutSubtreeIfNeeded()
      animateFrame(to: restored, generation: generation)
    }
  }

  private func animateFrame(to target: NSRect, generation: UInt64) {
    NSAnimationContext.runAnimationGroup { context in
      context.duration = lastMorphDuration
      context.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
      animator().setFrame(target, display: true)
    } completionHandler: { [weak self] in
      Task { @MainActor in
        self?.completeFrameTransition(generation: generation)
      }
    }
  }

  /// Remembered or default expanded size, raised to the qualified content
  /// height when the controller still owns automatic sizing.
  private func expandedPresentationSize() -> NSSize {
    let expanded =
      expandedSize
      ?? DictationOverlayWindow.restoredContentSize(
        surface: contentSurface, defaults: contentSizeDefaults)
    // Both axes meet the floor: a size remembered before the floor was raised
    // would otherwise unfold narrower than the header's ideal width.
    let restingWidth = max(expanded.width, DictationOverlayWindow.minSize.width)
    let restingHeight = max(expanded.height, DictationOverlayWindow.minSize.height)
    let qualified = qualifiedExpandedHeight?(restingWidth) ?? restingHeight
    return NSSize(width: restingWidth, height: max(restingHeight, qualified))
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
    // The window has landed on the target frame. Releasing the held size in
    // this same pass lays the settled layout out once, at the size the window
    // already has, before the next paint: no second layout follows it and no
    // element that was visible mid-morph changes its screen position.
    (contentView as? OverlayContentContainer)?.pinnedContentSize = nil
    // Mini is a drag surface; MIDI exposes only horizontal resize handles.
    minSize =
      presentationMode == .mini
      ? miniSize : DictationOverlayWindow.minimumSize(for: presentationMode)
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
    settleMiniWidth()
    settleMidiWidth()
    onFrameTransitionCompleted?()
  }

  /// The map's channels changed. In compact the strip follows at once, with
  /// its upper-right corner, and so its fold button, where they are; display
  /// containment still applies. A morph, drag or resize in flight settles
  /// first, and its completion lands here again.
  func setMidiNaturalWidth(_ width: CGFloat) {
    let natural = max(width, DictationOverlayWindow.midiSize.width)
    guard abs(natural - midiNaturalWidth) > 0.5 else { return }
    midiNaturalWidth = natural
    settleMidiWidth()
  }

  /// The channel counter's click in compact (spec §13.4): the width the user
  /// gave the strip is forgotten, so it settles to its natural width at its
  /// upper-right pin and the chips return. A morph, drag or resize in flight
  /// settles first, and its completion lands on the natural width.
  func restoreMidiNaturalWidth() {
    midiUserWidth = nil
    midiShrunkByUser = false
    settleMidiWidth()
  }

  /// The end of a manual resize in compact: the user's width is kept for the
  /// session, and a width below the natural one stops the roster resizing it.
  private func noteUserMidiWidth(_ width: CGFloat) {
    midiUserWidth = width
    midiShrunkByUser = width < midiNaturalWidth - 0.5
  }

  private func settleMidiWidth() {
    guard presentationMode == .midi, !isFrameTransitioning, !isUserDragging, !isUserResizing
    else { return }
    let width = midiPresentationWidth
    guard abs(frame.width - width) > 0.5 else { return }
    let wasApplyingFrame = OverlayController.isApplyingFrame
    OverlayController.isApplyingFrame = true
    defer { OverlayController.isApplyingFrame = wasApplyingFrame }
    let visible = screen?.visibleFrame ?? NSScreen.main?.visibleFrame
    // One step, no morph, as mini's width settles: the strip's content is
    // already laid out for the new map and the trailing controls stay put.
    let target = DictationOverlayWindow.visibleExpansionFrame(
      NSRect(x: frame.maxX - width, y: frame.minY, width: width, height: frame.height),
      in: visible)
    setFrame(target, display: true)
  }

  /// Maciej, 2026-10-19 09:36: the gap mini kept beside the short "cs" was
  /// "simply brzydka"; the brand should come up to the controls. While an
  /// agent records, mini narrows to its three controls; the trailing edge
  /// stays, so the microphones keep their screen position. A morph in
  /// flight lands on the size it started with; its completion settles here.
  func setMiniHugsAgentControls(_ hugs: Bool) {
    guard hugs != miniHugsAgentControls else { return }
    miniHugsAgentControls = hugs
    settleMiniWidth()
  }

  private func settleMiniWidth() {
    guard presentationMode == .mini, !isFrameTransitioning, !isUserDragging, !isUserResizing,
      abs(frame.width - miniSize.width) > 0.5
    else { return }
    let wasApplyingFrame = OverlayController.isApplyingFrame
    OverlayController.isApplyingFrame = true
    defer { OverlayController.isApplyingFrame = wasApplyingFrame }
    minSize = miniSize
    contentMinSize = minSize
    // One step, no morph: the header swaps the word and the agent's control
    // in the same pass, so an animated width would clip the trailing
    // controls while the strip widens.
    setFrame(
      NSRect(
        x: frame.maxX - miniSize.width, y: frame.minY, width: miniSize.width,
        height: frame.height),
      display: true)
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

  /// True while the screenshot and app-activation monitors are installed.
  var isObservingPresence: Bool { presence?.isObserving ?? false }

  override var canBecomeKey: Bool { allowsKeyForTranscript }
  override var canBecomeMain: Bool { false }
  /// Only explicit transcript interaction opens the keyboard gate.
  private(set) var allowsKeyForTranscript = false
  /// Set while a click that opened the gate is dispatched. That click hands
  /// first responder to text inside this panel, so the transcript canvas
  /// resigning on the way out must not give the keyboard back.
  private var dispatchingGateClick = false

  /// The transcript canvas needs keyboard input for selection or editing.
  func takeKeyForTranscript() {
    allowsKeyForTranscript = true
    if !isKeyWindow { makeKey() }
  }

  /// The canvas resigned. Drop key status so keystrokes return to the app the
  /// user was dictating into.
  func releaseKeyAfterTranscript() {
    guard allowsKeyForTranscript, !dispatchingGateClick else { return }
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

  /// ⌘C for whatever holds the focus in this panel.
  func performCopyChord(_ event: NSEvent) -> Bool {
    guard isKeyWindow, event.type == .keyDown,
      event.modifierFlags.intersection([.command, .control, .option, .shift]) == .command,
      event.charactersIgnoringModifiers?.lowercased() == "c"
    else { return false }
    if let responder = firstResponder, responder.responds(to: #selector(NSText.copy(_:))) {
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
    return NSApp.sendAction(#selector(NSText.copy(_:)), to: nil, from: self)
  }

  override func performKeyEquivalent(with event: NSEvent) -> Bool {
    if performCopyChord(event) { return true }
    return super.performKeyEquivalent(with: event)
  }

  /// A non-activating panel does not turn SwiftUI background hits into window
  /// motion, so intercept only explicit AppKit drag regions.
  /// Native transcript and SwiftUI controls keep ordinary AppKit dispatch.
  /// The existing resize band is tracked here without a nested event loop.
  override func sendEvent(_ event: NSEvent) {
    // ⌘C on a selected bubble did nothing in the installed app, while the key
    // equivalent path passed in a test host that is the active app (Maciej,
    // 2026-10-19). With another app active the chord can arrive as a plain
    // keyDown for the first responder; the transcript canvas catches it in its
    // own keyDown, and the panel now does the same for every responder.
    if event.type == .keyDown, performCopyChord(event) { return }
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
      settleMiniWidth()
      settleMidiWidth()
      if moved { onUserDragEnded?(origin) }
      if clickedPresentation { onPresentationClick?() }
    default:
      var openedKeyboardGate = false
      if event.type == .leftMouseDown {
        for case let region as OverlaySelectableTextRegionView in selectableTextRegions.allObjects
        where region.contains(windowPoint: event.locationInWindow) {
          takeKeyForTranscript()
          openedKeyboardGate = true
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
            openedKeyboardGate = true
            break
          }
          hit = view.superview
        }
      }
      // The transcript canvas is first responder until something takes it, and
      // its resign closed the gate in the middle of this click: the bubble text
      // was selected in a panel that was no longer key, so ⌘C went to the app
      // behind it (Maciej, 2026-10-19).
      dispatchingGateClick = openedKeyboardGate
      super.sendEvent(event)
      dispatchingGateClick = false
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
    if changed, pendingResizePresentation == nil, presentationMode == .midi {
      noteUserMidiWidth(frame.width)
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

  /// While the panel morphs, the content keeps one size, held at the window's
  /// top-right corner. An unroll holds the size it lands at and the growing
  /// window reveals it; a fold holds the size it departs from and the
  /// shrinking window covers it. Maciej, 2026-10-19: the unroll "robi z tego
  /// siekę", the parts "się rozpadają, przesuwają". His 13:40 unroll painted
  /// five frames in 1.9 s: every animator step laid the whole conversation out
  /// again at a new width. Laid out once, a step only moves this view. The
  /// rounded mask keeps the panel's corners on the window while it cuts
  /// through the held layout.
  var pinnedContentSize: NSSize? {
    didSet {
      guard pinnedContentSize != oldValue else { return }
      layer?.cornerCurve = .continuous
      layer?.cornerRadius = pinnedContentSize == nil ? 0 : CSRadius.window
      layer?.masksToBounds = pinnedContentSize != nil
      pinHosting()
    }
  }

  /// The hosting view's frame: the bounds, or the held size during a morph.
  var contentFrame: NSRect { hosting.frame }

  init(hosting: NSView) {
    self.hosting = hosting
    super.init(frame: .zero)
    wantsLayer = true
    addSubview(grabPad)
    addSubview(hosting)
    hosting.frame = bounds
  }

  @available(*, unavailable)
  required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

  override func setFrameSize(_ newSize: NSSize) {
    let previousBounds = bounds
    super.setFrameSize(newSize)
    pinHosting()
    if previousBounds != bounds {
      window?.invalidateCursorRects(for: self)
      refreshGrabPad()
    }
  }

  override func layout() {
    super.layout()
    pinHosting()
    refreshGrabPad()
  }

  /// The bounds, or the held size anchored at the top-right corner (this view
  /// is not flipped), where the header's trailing controls sit in every form,
  /// so they keep their screen position through the whole morph.
  private func pinHosting() {
    let frame =
      pinnedContentSize.map {
        NSRect(
          x: bounds.maxX - $0.width, y: bounds.maxY - $0.height, width: $0.width,
          height: $0.height)
      } ?? bounds
    if hosting.frame != frame { hosting.frame = frame }
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
  ///
  /// Maciej, 2026-10-19: unrolling mini into the expanded view was far too slow
  /// ("ja chciałem takie wolne, żeby była jasność, ale nie tak wolne"); his
  /// 1149 pt unroll was declared at 1.15 s. The speed and the ceiling are
  /// Stasiu's choice, for his acceptance: 1800 pt/s, never past 0.6 s, so a
  /// big panel still takes visibly longer than a short step.
  static let presentationSpeed: CGFloat = 1800
  static let presentationDuration: TimeInterval = 0.28
  static let longestPresentationDuration: TimeInterval = 0.6

  static func presentationDuration(from start: NSRect, to end: NSRect) -> TimeInterval {
    let travel = max(
      abs(end.minX - start.minX), abs(end.maxX - start.maxX),
      abs(end.minY - start.minY), abs(end.maxY - start.maxY))
    return min(
      longestPresentationDuration,
      max(presentationDuration, TimeInterval(travel / presentationSpeed)))
  }
  static let collapsedHeight: CGFloat = 46
  static let collapsedSize = NSSize(width: 200, height: collapsedHeight)
  /// Mini while an agent records, hugging its controls: 32 outer padding
  /// (`16 - clipInset` + `clipInset` per side) + the 9 pt close dot + 6 + the
  /// 19 pt recording channel counter (`OverlayChannelPill.counterWidth` of one
  /// circled digit at scale 1, in the place the 15 pt "cs" had) + 2 × 6 row
  /// spacing + 4 for the collapsed waveform's minimum + 81 trailing (the 22 pt
  /// agent control, 8, and the 51 pt recording pair) = 163. A SwiftUI
  /// fitting-size probe measured the "cs" row at 159; the counter adds 4.
  static let agentMiniSize = NSSize(width: 164, height: collapsedHeight)
  /// Compact's floor, and its whole width while no channel is mapped.
  static let midiSize = NSSize(width: 410, height: collapsedHeight)
  /// The most compact grows to for its channel chips: two floors (bus
  /// intercom spec §13.1). Past it the chips drop their names.
  static let midiChannelMapBudget = 2 * midiSize.width

  /// Compact's natural width (spec §13.1): the floor, the map's leading gap,
  /// room for channel 0's digit chip, which leads the row while the
  /// microphone broadcasts, so the chord changes neither the width nor the
  /// tier, and the row of the first tier that fits the 820 pt budget: the
  /// named chips, else the digit-only chips with room for the longest name,
  /// which the selected conversation's chip keeps. The microphone badge adds
  /// no width. With no agent it is the floor; it never exceeds the budget or
  /// the screen's visible width, and never goes below the floor.
  @MainActor
  static func midiNaturalWidth(
    map: OverlayChannelMapModel, textScale: CGFloat, visibleWidth: CGFloat? = nil
  ) -> CGFloat {
    let agents = map.agents
    guard !agents.isEmpty else { return midiSize.width }
    let scale = OverlayChannelPill.headerScale(textScale)
    let spacing = OverlayChannelMapView.chipSpacing
    let digit = OverlayChannelPill.chipWidth(name: nil, scale: scale)
    let named = agents.map { OverlayChannelPill.chipWidth(name: $0.name, scale: scale) }
    let base = midiSize.width + OverlayChannelMapView.leadingGap + digit + spacing
    let namedWidth = ceil(
      base + OverlayChannelMapModel.rowWidth(chipWidths: named, spacing: spacing))
    let natural: CGFloat
    if namedWidth <= midiChannelMapBudget {
      natural = namedWidth
    } else {
      let digits = OverlayChannelMapModel.rowWidth(
        chipWidths: Array(repeating: digit, count: agents.count), spacing: spacing)
      let longestName = (named.max() ?? digit) - digit
      natural = min(ceil(base + digits + longestName), midiChannelMapBudget)
    }
    guard let visibleWidth else { return natural }
    return max(midiSize.width, min(natural, visibleWidth))
  }

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
  /// Height is the transcript-action column's floor, not a transcript-lines
  /// guess: the expanded form always shows the full fixed action column, so
  /// its minimum fits the header, the column's whole stack and its padding at
  /// once (`OverlayTranscriptActionsLayout.minimumExpandedHeight`). Mini and
  /// MIDI keep their own sizes and never see this height.
  ///
  /// Width 320 → 360 (Founder, 2026-10-18: at 320 the session counter broke
  /// into one glyph per line and the agent pills swallowed the messages). The
  /// floor is the header's own ideal width at text scale 1.0, expanded density:
  /// 32 outer padding (`16 - clipInset` + `clipInset` per side) + 3 × 10 row
  /// spacing + 89 leading (9 pt close dot + 5 + ~75 pt `codescribe` wordmark at
  /// `CSFont.ui(15, .bold)` with −0.3 tracking) + 12 waveform minimum
  /// (`OverlayHeaderDensity.waveformMinWidth`) + 47 session counter (`h:mm:ss`
  /// at `CSFont.mono(11)`, a 0.6 em advance) + 141 trailing (three 22 pt glyph
  /// buttons + the 51 pt recording pair + 3 × 8 spacing) = 351, rounded up to
  /// 360 for the close dot's hover growth.
  static let minSize = NSSize(
    width: 360, height: OverlayTranscriptActionsLayout.minimumExpandedHeight)
  /// First-launch content size (no persisted value yet). The width keeps the
  /// landscape proportion of the old 470 × 280 bar, but the height sits on
  /// the expanded floor: the fixed transcript action column must fit in full
  /// from the first frame, and a default below `minSize` could never be the
  /// frame the window actually takes. Resizing persists, so users who prefer
  /// a taller panel drag it once and keep it.
  static let defaultSize = NSSize(
    width: 470, height: OverlayTranscriptActionsLayout.minimumExpandedHeight)
  /// Independent transcription/agents sizes replace the shared v6 frame.
  private static let sizeDefaultsKey = "DictationOverlayPanel.contentSize.v7"

  /// Build the floating overlay panel around an injected `OverlayState`.
  /// The state's `engine`, `onClose`, and `onSendToAgent` are wired by the
  /// orchestrator before the panel is shown.
  @MainActor
  static func make(
    state: OverlayState,
    textScale: TextScaleController,
    reduceTransparencyOverride: Bool? = nil,
    contrastOverride: ColorSchemeContrast? = nil,
    sizeDefaults: UserDefaults = .standard
  ) -> NSPanel {
    // Wrap in TextScaleRoot so ⌘+/-/0 on this panel scale the overlay text
    // (transcript + status) via `\.csTextScale`, independently of the chat.
    let root = TextScaleRoot(controller: textScale) {
      DictationOverlayView(state: state)
        .environment(\.csReduceTransparencyOverride, reduceTransparencyOverride)
        .environment(\.csContrastOverride, contrastOverride)
    }
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
      contentRect: NSRect(
        origin: .zero,
        size: restoredContentSize(surface: state.windowContentSurface, defaults: sizeDefaults)),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered,
      defer: false
    )
    panel.contentSurface = state.windowContentSurface
    panel.contentSizeDefaults = sizeDefaults
    panel.delegate = panel
    state.onPresentationModeChanged = { [weak panel, weak state] mode in
      guard let panel else { return }
      if let state { panel.setContentSurface(state.windowContentSurface) }
      panel.setPresentationMode(
        mode,
        animated: panel.isVisible && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
      state?.presentationMorphDuration = panel.lastMorphDuration
    }
    panel.onResizePresentationSelected = { [weak state] mode in
      state?.setPresentationMode(mode)
    }
    panel.setMiniHugsAgentControls(state.channelAudioCaptureActive)
    // Compact's width follows the channel map (bus intercom spec 5.1, 13.1).
    let fitMidiToChannelMap = { [weak panel, weak state] in
      guard let panel, let state else { return }
      panel.setMidiNaturalWidth(
        midiNaturalWidth(
          map: state.channelMapLayoutModel, textScale: textScale.scale,
          visibleWidth: (panel.screen ?? NSScreen.main)?.visibleFrame.width))
    }
    fitMidiToChannelMap()
    state.onChannelMapLayoutChanged = fitMidiToChannelMap
    state.onRestoreMidiNaturalWidth = { [weak panel] in panel?.restoreMidiNaturalWidth() }
    state.onAgentCaptureChanged = { [weak panel] capturing in
      panel?.setMiniHugsAgentControls(capturing)
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
    surface: OverlayContentSurface = .transcription,
    defaults: UserDefaults = .standard
  ) -> NSSize {
    var key = sizeDefaultsKey + "." + surface.rawValue
    // The old shared frame could be a full-screen agent bus. Keep that useful
    // agents size; a first transcription starts compact instead of inheriting it.
    if surface == .agents, defaults.object(forKey: key + ".w") == nil {
      key = "DictationOverlayPanel.contentSize.v6"
    }
    let width = defaults.double(forKey: key + ".w")
    let height = defaults.double(forKey: key + ".h")
    let raw = (width > 0 && height > 0) ? NSSize(width: width, height: height) : defaultSize
    return clamp(raw, to: screen)
  }

  /// Persist the current content size so it survives relaunch. Called on hide().
  static func persist(
    size: NSSize, surface: OverlayContentSurface = .transcription,
    defaults: UserDefaults = .standard
  ) {
    let key = sizeDefaultsKey + "." + surface.rawValue
    defaults.set(Double(size.width), forKey: key + ".w")
    defaults.set(Double(size.height), forKey: key + ".h")
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

  var isObserving: Bool {
    localMonitor != nil || globalMonitor != nil || workspaceObserver != nil
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

  func noteScreenshotIfNeeded(_ event: NSEvent) {
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
    // A screenshot lifts the overlay only when it is already on screen. An
    // ordered-out panel stays hidden: the chord is not a reason to show it.
    if policy == .capture, panel.isVisible {
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
