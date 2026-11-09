import AppKit
import Combine
import QuartzCore
import SwiftUI

/// The two canvases share a panel and fold control, but remember their own size.
enum OverlayContentSurface: String, CaseIterable, Hashable, Sendable {
  case transcription
  case agents
}

/// Motion uses geometry first. Only a mounted menu candidate may need a
/// native editable-text hit test; ordinary interior motion belongs to AppKit.
enum OverlayPointerZone: Equatable {
  case outside, interior, menu, dragStrip
  case resize(OverlayResizeHit.Edge)

  static func resolve(
    at point: NSPoint, in bounds: NSRect, mode: OverlayPresentationMode,
    resizable: Bool, menuContains: () -> Bool, dragContains: () -> Bool
  ) -> Self {
    guard bounds.contains(point) else { return .outside }
    if resizable, let edge = OverlayResizeHit.edge(at: point, in: bounds, mode: mode) {
      return .resize(edge)
    }
    if menuContains() { return .menu }
    if dragContains() { return .dragStrip }
    return .interior
  }
}

/// Remember our own application, not NSCursor.current: overlapping AppKit
/// tracking can change that global value between otherwise identical events.
struct OverlayCursorState {
  private var zone: OverlayPointerZone?
  private var cursor: ObjectIdentifier?
  private var region: ObjectIdentifier?

  fileprivate var diagnosticKey:
    (
      zone: OverlayPointerZone?, cursor: ObjectIdentifier?, region: ObjectIdentifier?
    )
  { (zone, cursor, region) }

  mutating func update(
    zone: OverlayPointerZone, cursor: ObjectIdentifier?, region: ObjectIdentifier? = nil
  ) -> Bool {
    // Adjacent mounted controls share the panel's cursor authority. Changing
    // only the region does not change its image or require registering it again.
    let changed = self.zone != zone || self.cursor != cursor
    let releasesResize: Bool
    if case .resize = self.zone, self.cursor != nil, cursor == nil,
      zone == .interior || zone == .menu
    {
      releasesResize = true
    } else {
      releasesResize = false
    }
    self.zone = zone
    self.cursor = cursor
    self.region = region
    return (cursor != nil && changed) || releasesResize
  }
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
  /// Input arriving at this panel extends the existing presentation deadline.
  /// Paints, native meter updates and pointer motion are not user input.
  var onUserActivity: (() -> Void)?
  var onUserDragEnded: ((NSPoint) -> Void)?
  var onPresentationClick: (() -> Void)?
  var onUserResize: (() -> Void)?
  var onUserResizeEnded: (() -> Void)?
  var onResizePresentationSelected: ((OverlayPresentationMode) -> Void)?
  var onFrameTransitionCompleted: (() -> Void)?
  /// Mounted controls and the drawer share this panel's cursor authority.
  /// Weak membership prevents a removed or hidden SwiftUI candidate owning it.
  let menuCursorRegions = NSHashTable<NSView>.weakObjects()
  let windowDragRegions = NSHashTable<NSView>.weakObjects()
  private var cursorState = OverlayCursorState()
  /// A resize cursor pressed inside a mounted region stays the panel's cursor
  /// until the button is released, wherever the pointer travels. The list's
  /// divider band is 10 points wide, so a drag leaves it on its first move and
  /// the arrow came back in the middle of the gesture (Maciej, 2026-11-07).
  private var grabbedCursor: NSCursor?

  enum CursorInvalidationReason {
    case boundary, mounting, orderOut
  }

  /// Aggregate evidence only. No timer, user content or cursor decisions live here.
  private struct PointerMetrics {
    var since = ProcessInfo.processInfo.systemUptime
    var decisions = 0
    var writes = 0
    var zoneChanges = 0
    var cursorChanges = 0
    var regionChanges = 0
    var boundaryResets = 0
    var mountingResets = 0
    var orderOutResets = 0
    var decisionSeconds = 0.0
    var cursorSeconds = 0.0
    var maxCursorSeconds = 0.0
  }
  private var pointerMetrics = PointerMetrics()
  private static let pointerDiagnosticQueue = DispatchQueue(
    label: "io.vetcoders.codescribe.pointer-diagnostics", qos: .utility)
  /// Selectable SwiftUI text that opens the keyboard gate without owning clicks.
  let selectableTextRegions = NSHashTable<NSView>.weakObjects()
  /// Grow-only expanded height for a candidate width. Nil keeps the remembered
  /// or default expanded size: manual sizing, an open editor, or a dirty draft.
  /// The panel remains the only writer of the transition frame.
  var qualifiedExpandedHeight: ((CGFloat) -> CGFloat?)?
  /// Reads pane/scale policy without resizing the source during a gesture.
  /// The panel remains the only writer of the frame and minimum size.
  var expandedContentMinimumSize: ((OverlayContentSurface) -> NSSize)?
  fileprivate var presence: OverlayPresence?
  fileprivate var appWindowYield: OverlayAppWindowYield?
  private var dragStart: (mouse: NSPoint, frame: NSRect, miniFrame: NSRect?)?
  private var dragMoved = false
  private var resizeStart: (mouse: NSPoint, frame: NSRect, edge: OverlayResizeHit.Edge)?
  private var requestedResizeSize: NSSize?
  private var pendingResizePoint: NSPoint?
  private var resizePaintTask: Task<Void, Never>?
  /// Expanded content never reflows at pointer frequency. The panel keeps its
  /// frame and hides behind the fiszka's resize preview, which alone follows
  /// the pointer; release commits the frame in one layout pass ("to dokładnie
  /// zrobiliśmy we floating", Maciej, 2026-11-03). Compact and mini are light
  /// strips and keep resizing live.
  private var resizePreview: OverlayResizePreview?
  /// The hidden panel is click-through, so the rest of the gesture reaches it
  /// through one local monitor, as on fiszki.
  private var resizePreviewMonitor: Any?
  private var pendingResizePresentation: (mode: OverlayPresentationMode, animated: Bool)?
  private var expandedSizes: [OverlayContentSurface: NSSize] = [:]
  private var isSettlingAgentsSplitWidth = false
  fileprivate(set) var contentSurface: OverlayContentSurface = .transcription
  var contentSizeDefaults: UserDefaults = .standard
  private var expandedSize: NSSize? {
    get { expandedSizes[contentSurface] }
    set { expandedSizes[contentSurface] = newValue }
  }
  private var miniFrame: NSRect?
  /// With the channel map enabled mini shows the brand dot alone; without it
  /// the word as well. The horizontal mini carries no channel counter
  /// (Founder, 2026-11-04).
  private var channelMapEnabled = false
  /// The vertical layout's stacked counter in mini with its gap after the
  /// dot (`DictationOverlayWindow.miniStackedCounterWidth`); zero while mini
  /// shows no counter.
  private(set) var miniStackedCounterWidth: CGFloat = 0
  /// What a live take adds to mini (`DictationOverlayWindow.miniTakeWidth`):
  /// the waveform, the clock and the addressees; zero at rest. Nothing
  /// remembers it, so mini folds back to its resting width when the take ends.
  private(set) var miniTakeWidth: CGFloat = 0
  /// Mini is as wide and as tall as its content (bus brief 2026-11-04 §3,
  /// §6): at rest, with the vertical layout's stacked counter, or grown by a
  /// live take.
  var miniSize: NSSize {
    let rest =
      channelMapEnabled
      ? DictationOverlayWindow.miniFloorWidth + miniStackedCounterWidth
      : DictationOverlayWindow.miniBrandWidth
    return NSSize(width: rest + miniTakeWidth, height: DictationOverlayWindow.miniHeight)
  }

  /// The expanded bar's measured geometry (`DictationOverlayWindow.midiBarGeometry`):
  /// its natural width, resize limits and one- or two-row height. The
  /// controller reports it whenever the map's channels change; without the
  /// horizontal map it is the classic 410 × 46 bar.
  private(set) var midiBar = OverlayMidiBarGeometry.classic
  /// The bar's default width: its content with the preferred names.
  var midiNaturalWidth: CGFloat { midiBar.naturalWidth }
  /// The width the user gave the bar with its edge in this session, and
  /// whether that was narrower than the natural width at the time. A narrowed
  /// bar keeps the user's width: roster changes no longer resize it and the
  /// map cuts its names instead (`restoreMidiNaturalWidth` gives the natural
  /// width back). A bar the user widened grows when the map outgrows it and
  /// never shrinks below the user's width. The bar's measured limits hold
  /// either width, but only a real edge drag writes it: an agent leaving for
  /// a moment narrows the bar and gives the width back when it returns.
  private var midiUserWidth: CGFloat?
  private(set) var midiShrunkByUser = false
  /// The width the bar settles to: the user's after a manual shrink, else the
  /// natural width, raised to a width the user chose; within the measured
  /// minimum and maximum.
  var midiPresentationWidth: CGFloat {
    let width =
      if midiShrunkByUser, let midiUserWidth {
        midiUserWidth
      } else {
        max(midiNaturalWidth, midiUserWidth ?? 0)
      }
    return midiBar.clampedWidth(width)
  }
  /// The bar's frame size: its width and the height of the rows that width
  /// holds, rounded once here, by the window's owner.
  var midiSize: NSSize {
    let width = midiPresentationWidth
    return NSSize(width: width, height: midiBar.height(forWidth: width))
  }
  /// Whether the panel answers its own edge drags. The panel never carries
  /// AppKit's `.resizable` style: on a borderless panel that style keeps a
  /// native ~12 pt resize corner that swallowed the close dot's first points
  /// (Founder, build 24; `OverlayResizeHitTests` with and without the style).
  /// Every resize gesture goes through `OverlayResizeHit` instead. A panel
  /// starts expanded, which resizes; mini turns it off.
  private(set) var allowsUserResize = true
  /// Refits mini and the bar when the overlay's text scale changes (`make`).
  var textScaleSink: AnyCancellable?
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
  /// The frame a resize gesture currently shows: the preview's while expanded
  /// content waits for release, otherwise the panel's own.
  var resizeGestureFrame: NSRect { resizePreview?.frame ?? frame }
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
      // Mini is the placement strip drawn at its natural size, anchored at
      // the same upper-right corner.
      guard presentationMode == .mini else { return positionedFrame.origin }
      return NSPoint(
        x: positionedFrame.maxX - DictationOverlayWindow.miniPlacementSize.width,
        y: positionedFrame.maxY - DictationOverlayWindow.miniPlacementSize.height)
    }
    if let miniFrame {
      guard let dragStart, let parked = dragStart.miniFrame else { return miniFrame.origin }
      return NSPoint(
        x: parked.minX + positionedFrame.maxX - dragStart.frame.maxX,
        y: parked.minY + positionedFrame.maxY - dragStart.frame.maxY)
    }
    return NSPoint(
      x: positionedFrame.maxX - DictationOverlayWindow.miniPlacementSize.width,
      y: positionedFrame.maxY - DictationOverlayWindow.miniPlacementSize.height)
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
    if previous == .mini && !settleResize {
      // Persist the placement strip, not the natural capsule. Both share the
      // upper-right corner; retaining the capsule's physical left edge here
      // shifted the next restored mini by that width difference.
      let parked = frameTransitionTarget ?? frame
      miniFrame = NSRect(
        x: parked.maxX - DictationOverlayWindow.miniPlacementSize.width,
        y: parked.maxY - DictationOverlayWindow.miniPlacementSize.height,
        width: DictationOverlayWindow.miniPlacementSize.width,
        height: DictationOverlayWindow.miniPlacementSize.height)
    }
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
      size = mode == .mini ? miniSize : midiSize
    } else {
      size = expandedPresentationSize()
    }
    // Intermediate frames may be smaller than the expanded window's floor.
    minSize = miniSize
    contentMinSize = minSize
    allowsUserResize = false
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
    // Bar-to-bar morphs (mini <-> bar) contain no visible document to freeze.
    // Their canvas follows the native bounds: pinning the old bar size instead
    // clips the newly selected, leading-aligned mini row from the left.
    let barMorph = previous != .expanded && mode != .expanded
    let unrolls = animated && mode == .expanded && previous != .expanded
    let folds =
      animated && !barMorph && !unrolls && mode != .expanded
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

  /// A direct request for the agent column reserves both panes before the
  /// expansion begins. Width determines geometry, never whether the click works.
  ///
  /// The width that decides is the one the split will be drawn at: the window
  /// on screen while it shows the split, the remembered size otherwise. A
  /// landed transition forgets the remembered size, so judging the shown
  /// window by the saved one asked for the same settle after every landing:
  /// the controller calls this from its transition-completed callback, and a
  /// settle that is not animated completes inside the call (a stack overflow
  /// in OverlayContentSurfaceTests since c4fe2f236).
  func ensureAgentsSplitWidth(_ minimum: CGFloat) {
    let screenWidth = (screen ?? NSScreen.main)?.visibleFrame.width ?? minimum
    let floor = min(minimum, screenWidth)
    let saved =
      expandedSizes[.agents]
      ?? DictationOverlayWindow.restoredContentSize(surface: .agents, defaults: contentSizeDefaults)
    let showsSplit = contentSurface == .agents && presentationMode == .expanded
    let width = showsSplit ? (frameTransitionTarget ?? frame).width : saved.width
    guard width < floor - 0.5 else { return }
    expandedSizes[.agents] = NSSize(width: max(saved.width, floor), height: saved.height)
    // One settle per request: a display that cannot give the floor leaves the
    // window narrower, and the landing must not ask again from inside.
    guard showsSplit, !isUserResizing, !isSettlingAgentsSplitWidth else { return }
    isSettlingAgentsSplitWidth = true
    defer { isSettlingAgentsSplitWidth = false }
    setPresentationMode(.expanded, animated: isFrameTransitioning, settleResize: true)
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
    let floor = presentationMinimumSize
    let restingWidth = max(expanded.width, floor.width)
    let restingHeight = max(expanded.height, floor.height)
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
    minSize = presentationMinimumSize
    contentMinSize = minSize
    if presentationMode != .mini { allowsUserResize = true }
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

  /// The map's channels or names changed. The bar follows at once, with its
  /// upper-right corner, and so its fold button, where they are; display
  /// containment still applies. A morph, drag or resize in flight settles
  /// first, and its completion lands here again.
  func setMidiBarGeometry(_ geometry: OverlayMidiBarGeometry) {
    guard geometry != midiBar else { return }
    // Back from 6–9 agents to 1–5 the bar changes form: the width the user
    // gave the two-row bar is forgotten, so the single row settles at its
    // natural width instead of two rows kept by a stale width (Founder,
    // 2026-11-04, 8.52–8.53 PM screenshots).
    if !midiBar.singleRowMinimumWidth.isFinite, geometry.singleRowMinimumWidth.isFinite {
      midiUserWidth = nil
      midiShrunkByUser = false
    }
    midiBar = geometry
    if presentationMode == .midi, !isFrameTransitioning, !isUserResizing {
      minSize = presentationMinimumSize
      contentMinSize = minSize
    }
    settleMidiWidth()
  }

  /// The settled form's floor: mini's content, the bar's measured minimum
  /// with the height of the rows it holds, the expanded readability floor.
  var presentationMinimumSize: NSSize {
    switch presentationMode {
    case .mini: return miniSize
    case .midi:
      return NSSize(
        width: midiBar.minimumWidth, height: midiBar.height(forWidth: midiBar.minimumWidth))
    case .expanded:
      let base = DictationOverlayWindow.minimumSize(for: .expanded)
      let content = expandedContentMinimumSize?(contentSurface) ?? base
      return NSSize(width: max(base.width, content.width), height: max(base.height, content.height))
    }
  }

  /// The width the user gave the bar is forgotten, so it settles to its
  /// natural width at its upper-right pin. A morph, drag or resize in flight
  /// settles first, and its completion lands on the natural width.
  func restoreMidiNaturalWidth() {
    midiUserWidth = nil
    midiShrunkByUser = false
    settleMidiWidth()
  }

  /// The end of a manual resize of the bar: the user's width is kept for the
  /// session, and a width below the natural one stops the roster resizing it.
  private func noteUserMidiWidth(_ width: CGFloat) {
    midiUserWidth = width
    midiShrunkByUser = width < midiNaturalWidth - 0.5
  }

  private func settleMidiWidth() {
    guard presentationMode == .midi, !isFrameTransitioning, !isUserDragging, !isUserResizing
    else { return }
    let size = midiSize
    guard abs(frame.width - size.width) > 0.5 || abs(frame.height - size.height) > 0.5 else {
      return
    }
    let wasApplyingFrame = OverlayController.isApplyingFrame
    OverlayController.isApplyingFrame = true
    defer { OverlayController.isApplyingFrame = wasApplyingFrame }
    let visible = screen?.visibleFrame ?? NSScreen.main?.visibleFrame
    // One step, no morph, as mini's width settles: the bar's content is
    // already laid out for the new map, and its upper-right corner, with the
    // trailing controls, stays put while a second row grows downward.
    let target = DictationOverlayWindow.visibleExpansionFrame(
      NSRect(
        x: frame.maxX - size.width, y: frame.maxY - size.height, width: size.width,
        height: size.height),
      in: visible)
    setFrame(target, display: true)
  }

  /// Mini is as wide as its content (Maciej, 2026-10-19: a gap beside the
  /// brand is "simply brzydka"; bus brief 2026-11-04 §3): enabling the map
  /// drops the brand word and narrows it; the vertical layout's stacked
  /// counter widens it by `stackedCounterWidth`. The upper-right corner
  /// stays, so the controls keep their screen position. A morph in flight
  /// lands on the size it started with; its completion settles here.
  func setChannelMapPresentation(enabled: Bool, stackedCounterWidth: CGFloat = 0) {
    let counter = enabled ? stackedCounterWidth : 0
    guard enabled != channelMapEnabled || abs(counter - miniStackedCounterWidth) > 0.5 else {
      return
    }
    channelMapEnabled = enabled
    miniStackedCounterWidth = counter
    settleMiniWidth()
  }

  /// A take started or ended, or its open channels changed (Founder,
  /// 2026-11-04, bus brief §6, §11D): mini grows leftward by the waveform,
  /// the clock and the addressees, and folds back after the take. The
  /// upper-right corner stays, so the tools keep their screen position. The
  /// change glides on a visible panel unless Reduce Motion is on; a morph,
  /// drag or resize in flight settles first, and its completion lands here.
  func setMiniTake(width: CGFloat) {
    guard abs(width - miniTakeWidth) > 0.5 else { return }
    miniTakeWidth = width
    settleMiniWidth(
      animated: isVisible && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
  }

  private func settleMiniWidth(animated: Bool = false) {
    guard presentationMode == .mini, !isFrameTransitioning, !isUserDragging, !isUserResizing,
      abs(frame.width - miniSize.width) > 0.5 || abs(frame.height - miniSize.height) > 0.5
    else { return }
    let wasApplyingFrame = OverlayController.isApplyingFrame
    OverlayController.isApplyingFrame = true
    defer { OverlayController.isApplyingFrame = wasApplyingFrame }
    minSize = miniSize
    contentMinSize = minSize
    let visible = screen?.visibleFrame ?? NSScreen.main?.visibleFrame
    let target = DictationOverlayWindow.visibleExpansionFrame(
      NSRect(
        x: frame.maxX - miniSize.width, y: frame.maxY - miniSize.height, width: miniSize.width,
        height: miniSize.height),
      in: visible)
    guard animated else {
      // One step, no morph: the header swaps the brand word in the same
      // pass, so an animated width would clip the trailing controls while
      // the strip widens.
      setFrame(target, display: true)
      return
    }
    // A take's glide holds one layout at the window's upper-right corner, as
    // a morph does: growing, the take's row, which the widening window
    // reveals; folding, the departing row, which the narrowing window
    // covers. The header never lays out at an intermediate width, so the
    // tools are never clipped and never move.
    frameTransitionGeneration &+= 1
    let generation = frameTransitionGeneration
    frameTransitionTarget = target
    transitionTop = frame.maxY
    transitionRight = frame.maxX
    isFrameTransitioning = true
    let container = contentView as? OverlayContentContainer
    container?.pinnedContentSize = target.width > frame.width ? target.size : frame.size
    lastMorphDuration = DictationOverlayWindow.presentationDuration(from: frame, to: target)
    OverlayPresentationTrace.shared.record(
      .frameTransitionBegan, from: .mini, mode: .mini, animated: true, frame: frame,
      target: target, generation: generation, note: "mini_take")
    animateFrame(to: target, generation: generation)
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
    outsideInteractions.stop()
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

  /// AppKit's key calls; tests record them so a hidden panel never asks the
  /// window server for the keyboard.
  var keyCalls = OverlayKeyCalls.appKit
  /// Reports the user going on in another app while the gate is open.
  var outsideInteractions: any OverlayOutsideInteractionSource =
    OverlayGlobalOutsideInteractions()

  /// The transcript canvas or a composer needs keyboard input for selection
  /// or editing. A panel that still reports key behind a closed gate holds a
  /// stale record (`OverlayKeyboardRequest.reclaimKey`): the click asks the
  /// window server again instead of trusting it.
  func takeKeyForTranscript() {
    let needsAppActivation =
      isVisible && !ignoresMouseEvents && alphaValue > 0 && !NSApp.isActive
    let wasKey = keyCalls.isKey(self)
    let request = OverlayKeyboardRequest.opening(
      gateOpen: allowsKeyForTranscript, panelIsKey: wasKey)
    // Still closed here, so the resign-key it posts leaves the responder alone.
    if request == .reclaimKey { keyCalls.resignKey(self) }
    allowsKeyForTranscript = true
    if request != .none || needsAppActivation { keyCalls.makeKey(self) }
    if needsAppActivation {
      NSApplication.shared.activate(ignoringOtherApps: true)
      if !keyCalls.isKey(self) { keyCalls.makeKey(self) }
    }
    if !outsideInteractions.isWatching {
      outsideInteractions.start { [weak self] interaction in
        self?.noteInteractionOutside(interaction)
      }
    }
    traceKeyboard("gate.taken", "request=\(request.rawValue) wasKey=\(wasKey)")
  }

  /// The canvas resigned. Drop key status so keystrokes return to the app the
  /// user was dictating into.
  func releaseKeyAfterTranscript() {
    guard allowsKeyForTranscript, !dispatchingGateClick else { return }
    allowsKeyForTranscript = false
    outsideInteractions.stop()
    let wasKey = keyCalls.isKey(self)
    if wasKey { keyCalls.resignKey(self) }
    traceKeyboard("gate.released", "wasKey=\(wasKey)")
  }

  override func resignKey() {
    super.resignKey()
    guard allowsKeyForTranscript else { return }
    allowsKeyForTranscript = false
    outsideInteractions.stop()
    makeFirstResponder(nil)
    traceKeyboard("gate.resigned", "")
  }

  /// Key left from outside: resign the canvas. An active edit uses its
  /// existing focus-exit commit; a selection has no revision side effects.
  func windowDidResignKey(_ notification: Notification) {
    guard allowsKeyForTranscript else { return }
    allowsKeyForTranscript = false
    outsideInteractions.stop()
    makeFirstResponder(nil)
    traceKeyboard("gate.resigned", "")
  }

  /// The user went on in another app while the gate was open. The window
  /// server has already moved the keyboard there and AppKit may not have told
  /// this panel, so the gate closes as on resign-key and the record follows.
  func noteInteractionOutside(_ interaction: OverlayOutsideInteraction) {
    outsideInteractions.stop()
    guard allowsKeyForTranscript else { return }
    allowsKeyForTranscript = false
    makeFirstResponder(nil)
    let wasKey = keyCalls.isKey(self)
    if wasKey { keyCalls.resignKey(self) }
    traceKeyboard("gate.left", "reason=\(interaction.rawValue) wasKey=\(wasKey)")
  }

  /// One `overlay-keyboard` line per gate decision in the diagnostic log, so
  /// the next keyboard left in another app is attributed from evidence.
  private func traceKeyboard(_ event: String, _ detail: String) {
    guard let log = DiagnosticLog.installed else { return }
    let front = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "-"
    let parts = [event, detail, "appActive=\(NSApp.isActive)", "front=\(front)"]
    log.append(category: "overlay-keyboard", parts.filter { !$0.isEmpty }.joined(separator: " "))
  }

  override func performKeyEquivalent(with event: NSEvent) -> Bool {
    if event.type == .keyDown { onUserActivity?() }
    if performCopyChord(event) { return true }
    return super.performKeyEquivalent(with: event)
  }

  /// A non-activating panel does not turn SwiftUI background hits into window
  /// motion, so intercept only explicit AppKit drag regions.
  /// Native transcript and SwiftUI controls keep ordinary AppKit dispatch.
  /// The existing resize band is tracked here without a nested event loop.
  override func sendEvent(_ event: NSEvent) {
    switch event.type {
    case .keyDown, .flagsChanged, .leftMouseDown, .rightMouseDown, .otherMouseDown,
      .leftMouseUp, .rightMouseUp, .otherMouseUp, .leftMouseDragged, .rightMouseDragged,
      .otherMouseDragged, .scrollWheel:
      onUserActivity?()
    default:
      break
    }
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
    // Mouse-moved events reach the key window, not the window under the
    // pointer, so motion over the interactive preview card hanging as a child
    // arrives here in this window's coordinates. Without this check the row
    // or button under the card took the hover, the card jumped to it and the
    // click went to the background (Founder, 2026-11-05: the popover fled the
    // pointer, as if the click went to what lay behind it).
    let pointerOverHintCard =
      (event.type == .mouseMoved || event.type == .cursorUpdate || event.type == .mouseEntered)
      && OverlayHintPanel.shared.coversPointer(
        at: convertPoint(toScreen: event.locationInWindow), under: self)
    // A tracking area under the card would take the hint (the All row's
    // speaker tooltip replaced the agents list in the 06:13 recording).
    if event.type == .mouseEntered, pointerOverHintCard { return }
    if event.type == .mouseMoved {
      for case let region as OverlayMenuCursorRegionView in menuCursorRegions.allObjects {
        region.updateHover(at: pointerOverHintCard ? nil : event.locationInWindow)
      }
    }
    if event.type == .mouseMoved || event.type == .cursorUpdate, !pointerOverHintCard {
      // A grabbed resize cursor outranks every zone until the button is up.
      if let grabbedCursor {
        if NSCursor.current !== grabbedCursor { grabbedCursor.set() }
        return
      }
      // Resolve once. Owned zones consume motion before overlapping text
      // tracking can set an I-beam. Leaving resize restores the arrow before
      // native dispatch; ordinary interior motion never writes a cursor.
      let decision = pointerDecision(at: event.locationInWindow)
      applyCursor(decision)
      if decision.cursor != nil { return }
    }
    if event.type == .leftMouseDown {
      if isUserResizing { endUserResize() }
      dragStart = nil
      dragMoved = false
      if allowsUserResize, let contentView,
        contentView.bounds.contains(event.locationInWindow),
        let edge = OverlayResizeHit.edge(
          at: event.locationInWindow, in: contentView.bounds, mode: presentationMode)
      {
        beginUserResize(edge: edge, at: screenPoint(for: event))
        return
      }
      // Only resize affordances are grabbed: a pointing hand on a button has
      // nothing to hold through a drag, and native text keeps its I-beam.
      let decision = pointerDecision(at: event.locationInWindow)
      if case .menu = decision.zone, let cursor = decision.cursor,
        cursor === NSCursor.resizeLeftRight || cursor === NSCursor.resizeUpDown
      {
        grabbedCursor = cursor
      }
    }
    if event.type == .leftMouseDragged, let grabbedCursor, NSCursor.current !== grabbedCursor {
      grabbedCursor.set()
    }
    defer {
      if event.type == .leftMouseUp, grabbedCursor != nil {
        // SwiftUI has ended the gesture by now; resolve the cursor afresh for
        // wherever the pointer was released, whatever the drag left on screen.
        grabbedCursor = nil
        cursorState = OverlayCursorState()
        refreshCursor(at: event.locationInWindow)
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
        miniFrame = NSRect(origin: origin, size: DictationOverlayWindow.miniPlacementSize)
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
          // Follow public responder capabilities, not a private class name;
          // scroll/clip wrappers around a text view count as that text view.
          let candidate = (view as? NSScrollView)?.documentView ?? view
          if candidate.acceptsFirstResponder && candidate.responds(to: #selector(NSText.copy(_:))) {
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
    guard allowsUserResize, !isFrameTransitioning, !isUserResizing,
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
      minSize: DictationOverlayWindow.miniPlacementSize)
    // Track the fold intent independently of the painted frame. Expanded
    // content cannot fit inside a strip while the gesture is still active.
    requestedResizeSize = requested.size
    // The grabbed edge follows the pointer; its opposite edge stays fixed.
    // Upper-right anchoring belongs to presentation-mode morphs, not gestures.
    var target = OverlayResizeHit.apply(
      edge: resizeStart.edge, start: resizeStart.frame,
      dx: point.x - resizeStart.mouse.x, dy: point.y - resizeStart.mouse.y,
      minSize: presentationMinimumSize)
    if presentationMode == .midi {
      // The bar's measured limits stop the edge: narrowing ends at the
      // shortest names, widening at every name whole. The height follows the
      // rows that width holds; the top edge stays where it is.
      let width = midiBar.clampedWidth(target.width)
      let height = midiBar.height(forWidth: width)
      target = NSRect(
        x: resizeStart.edge == .left ? resizeStart.frame.maxX - width : resizeStart.frame.minX,
        y: resizeStart.frame.maxY - height, width: width, height: height)
    }
    guard target != resizeGestureFrame else { return false }
    if presentationMode == .expanded {
      showResizePreview(target)
      return true
    }
    // Do not force an intermediate paint while hosting geometry is invalidated.
    setFrame(target, display: false)
    return true
  }

  private func showResizePreview(_ target: NSRect) {
    if resizePreview == nil {
      resizePreview = OverlayResizePreview(
        windows: [self], group: false, headerHeight: OverlayTranscriptActionsLayout.headerHeight,
        palette: OverlayAppearancePalette.resolve(
          effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? ColorScheme.dark : ColorScheme.light),
        cornerRadius: CSRadius.window, capturesContent: true)
      resizePreviewMonitor = NSEvent.addLocalMonitorForEvents(
        matching: [.leftMouseDragged, .leftMouseUp]
      ) { [weak self] event in
        let consumed = MainActor.assumeIsolated { () -> Bool in
          guard let self, self.isUserResizing else { return false }
          let point = self.screenPoint(for: event)
          if event.type == .leftMouseUp {
            self.pendingResizePoint = point
            self.endUserResize()
          } else {
            self.scheduleUserResize(to: point)
          }
          return true
        }
        return consumed ? nil : event
      }
    }
    resizePreview?.update(frame: target)
  }

  /// Release lays the panel out once at the preview's last frame before it
  /// becomes visible again; a cancelled gesture keeps the original frame.
  private func settleResizePreview(commit: Bool) {
    if let resizePreviewMonitor { NSEvent.removeMonitor(resizePreviewMonitor) }
    resizePreviewMonitor = nil
    guard let preview = resizePreview else { return }
    resizePreview = nil
    if commit, let target = preview.frame, target != frame {
      setFrame(target, display: true)
    }
    preview.finish()
  }

  func endUserResize() {
    flushPendingResize()
    guard let resizeStart else { return }
    settleResizePreview(commit: true)
    let requested = requestedResizeSize ?? frame.size
    let changed = frame.size != resizeStart.frame.size || requested != resizeStart.frame.size
    if changed, pendingResizePresentation == nil, presentationMode == .expanded {
      let mode = DictationOverlayWindow.presentationMode(forResizedSize: requested)
      // OverlayState remains the presentation authority. Its ordinary
      // callback queues the selected mode until this gesture ends.
      onResizePresentationSelected?(mode)
      if pendingResizePresentation == nil { pendingResizePresentation = (mode, false) }
    }
    // Only a width the edge really moved is the user's. A press that does
    // not move the frame, a push against the bar's limit included, keeps the
    // remembered width (an agent away for a moment must not shrink it).
    if pendingResizePresentation == nil, presentationMode == .midi,
      abs(frame.width - resizeStart.frame.width) > 0.5
    {
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
    settleResizePreview(commit: false)
    resizePaintTask?.cancel()
    resizePaintTask = nil
    pendingResizePoint = nil
    resizeStart = nil
    requestedResizeSize = nil
    pendingResizePresentation = nil
  }

  private struct PointerDecision {
    let zone: OverlayPointerZone
    var cursor: NSCursor?
    var region: ObjectIdentifier?
  }

  /// Invalidated by the content boundary on exit/re-entry. A cursor installed
  /// by another window must not suppress our next region entry.
  func invalidateCursorOwnership(reason: CursorInvalidationReason = .boundary) {
    cursorState = OverlayCursorState()
    grabbedCursor = nil
    guard DiagnosticLog.installed != nil else { return }
    switch reason {
    case .boundary: pointerMetrics.boundaryResets += 1
    case .mounting: pointerMetrics.mountingResets += 1
    case .orderOut: pointerMetrics.orderOutResets += 1
    }
  }

  override func orderOut(_ sender: Any?) {
    invalidateCursorOwnership(reason: .orderOut)
    super.orderOut(sender)
  }

  private func pointerDecision(at point: NSPoint) -> PointerDecision {
    let started = DiagnosticLog.installed.map { _ in ProcessInfo.processInfo.systemUptime }
    defer {
      if let started {
        pointerMetrics.decisionSeconds += ProcessInfo.processInfo.systemUptime - started
      }
    }
    guard let contentView else { return PointerDecision(zone: .outside) }
    var menu: OverlayMenuCursorRegionView?
    let zone = OverlayPointerZone.resolve(
      at: contentView.convert(point, from: nil), in: contentView.bounds, mode: presentationMode,
      resizable: allowsUserResize,
      menuContains: {
        menu = self.menuCursorRegion(at: point)
        return menu != nil
      },
      dragContains: {
        self.windowDragRegions.allObjects.contains { region in
          // The body has a drag background UNDER selectable text. Only the
          // inert header strip can claim motion by frame without hit testing.
          (self.presentationMode == .mini
            || region.accessibilityIdentifier().hasPrefix("overlay-header"))
            && self.containsCursorPoint(point, in: region)
        }
      })
    switch zone {
    case .resize(let edge):
      return PointerDecision(zone: zone, cursor: OverlayResizeHit.cursor(for: edge))
    case .menu:
      guard let menu else { return PointerDecision(zone: .interior) }
      // Exactly one hit test, and only after mounted menu geometry matched.
      let editable = nativeEditableText(at: point, in: contentView)
      return PointerDecision(
        zone: zone, cursor: editable ? nil : menu.cursor, region: ObjectIdentifier(menu))
    case .dragStrip:
      return PointerDecision(zone: zone, cursor: .arrow)
    case .interior, .outside:
      return PointerDecision(zone: zone)
    }
  }

  @discardableResult
  private func applyCursor(_ decision: PointerDecision) -> Bool {
    let previous = cursorState.diagnosticKey
    let cursorID = decision.cursor.map(ObjectIdentifier.init)
    let shouldSet = cursorState.update(
      zone: decision.zone, cursor: cursorID, region: decision.region)
    let sink = DiagnosticLog.installed
    let started = sink.map { _ in ProcessInfo.processInfo.systemUptime }
    if shouldSet { (decision.cursor ?? .arrow).set() }
    if let sink, let started {
      let now = ProcessInfo.processInfo.systemUptime
      pointerMetrics.decisions += 1
      if previous.zone != decision.zone { pointerMetrics.zoneChanges += 1 }
      if previous.cursor != cursorID { pointerMetrics.cursorChanges += 1 }
      if previous.region != decision.region { pointerMetrics.regionChanges += 1 }
      if shouldSet {
        pointerMetrics.writes += 1
        pointerMetrics.cursorSeconds += now - started
        pointerMetrics.maxCursorSeconds = max(pointerMetrics.maxCursorSeconds, now - started)
      }
      // At most one append per two seconds of actual pointer activity. Quiet
      // windows schedule no work, and native cursor tracking remains unchanged.
      if now - pointerMetrics.since >= 2 {
        let m = pointerMetrics
        let date = Date()
        let message =
          "window=\(windowNumber) surface=\(contentSurface.rawValue) mode=\(presentationMode) "
          + "seconds=\(now - m.since) decisions=\(m.decisions) writes=\(m.writes) "
          + "zoneChanges=\(m.zoneChanges) cursorChanges=\(m.cursorChanges) regionChanges=\(m.regionChanges) "
          + "boundaryResets=\(m.boundaryResets) mountingResets=\(m.mountingResets) orderOutResets=\(m.orderOutResets) "
          + "decisionMs=\(m.decisionSeconds * 1000) cursorMs=\(m.cursorSeconds * 1000) maxCursorMs=\(m.maxCursorSeconds * 1000)"
        pointerMetrics = PointerMetrics()
        // Timestamp the observation now; formatting, the log lock and file I/O
        // must not delay the next mouse event. All panels share one FIFO queue.
        Self.pointerDiagnosticQueue.async {
          sink.append(category: "overlay-pointer", message, at: date)
        }
      }
    }
    return shouldSet
  }

  @discardableResult
  func refreshCursor(at point: NSPoint) -> Bool {
    applyCursor(pointerDecision(at: point))
  }

  /// Resolve from this panel's hit surface, never the inactive app below it.
  func cursor(at point: NSPoint) -> NSCursor {
    guard let contentView else { return .arrow }
    let decision = pointerDecision(at: point)
    if let cursor = decision.cursor { return cursor }
    // Inspection for callers asking about text, never the motion path.
    var hit = contentView.hitTest(contentView.convert(point, from: nil))
    while let view = hit {
      if view is NSTextView { return .iBeam }
      if let field = view as? NSTextField, field.isEditable { return .iBeam }
      hit = view.superview
    }
    return .arrow
  }

  private func nativeEditableText(at point: NSPoint, in contentView: NSView) -> Bool {
    // The drawer can contain native text entry (live search). Its field
    // editor keeps native tracking, unlike the transcript behind the drawer.
    var hit = contentView.hitTest(contentView.convert(point, from: nil))
    while let view = hit {
      if let text = view as? NSTextView, text.isEditable,
        !(text is LiveTranscriptNativeTextView)
      {
        return true
      }
      if let field = view as? NSTextField, field.isEditable { return true }
      hit = view.superview
    }
    return false
  }

  private func containsCursorPoint(_ point: NSPoint, in region: NSView) -> Bool {
    guard region.window === self, !region.isHiddenOrHasHiddenAncestor,
      let parent = region.superview
    else { return false }
    return parent.convert(region.frame, to: nil).contains(point)
      && region.convert(region.visibleRect, to: nil).contains(point)
  }

  private func menuCursorRegion(at point: NSPoint) -> OverlayMenuCursorRegionView? {
    // Events use window coordinates. Bound the override by the mounted
    // drawer's frame as well as its clipped visible rect, in that same space.
    // A native text hit outside this frame must never inherit menu ownership.
    // A small divider region wins over the enclosing drawer. The panel stays
    // the single cursor owner; nested SwiftUI hover handlers cannot fight it.
    return menuCursorRegions.allObjects.compactMap { $0 as? OverlayMenuCursorRegionView }.filter {
      region in
      containsCursorPoint(point, in: region)
    }.min { lhs, rhs in
      lhs.bounds.width * lhs.bounds.height < rhs.bounds.width * rhs.bounds.height
    }
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
  private var cursorBoundary: NSTrackingArea?
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
    (window as? FloatingOverlayPanel)?.invalidateCursorOwnership(reason: .mounting)
    refreshGrabPad()
  }

  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    // inVisibleRect follows native geometry without replacing its identity.
    // Recreating it during layout queues artificial exits/entries that rearm
    // the panel's cursor write even when the pointer has not left the window.
    guard cursorBoundary == nil else { return }
    let area = NSTrackingArea(
      rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
      owner: self, userInfo: nil)
    addTrackingArea(area)
    cursorBoundary = area
  }

  override func mouseEntered(with event: NSEvent) {
    invalidateCursorOwnership(from: event.trackingArea, in: event.window)
  }

  override func mouseExited(with event: NSEvent) {
    invalidateCursorOwnership(from: event.trackingArea, in: event.window)
  }

  func invalidateCursorOwnership(from area: NSTrackingArea?, in sourceWindow: NSWindow?) {
    guard let cursorBoundary, area === cursorBoundary,
      sourceWindow === window
    else { return }
    (window as? FloatingOverlayPanel)?.invalidateCursorOwnership()
  }

  /// The pad follows the frame and the mode; a mini panel has no resize zone.
  func refreshGrabPad() {
    let resizable = (window as? FloatingOverlayPanel)?.allowsUserResize == true
    let mode = (window as? FloatingOverlayPanel)?.presentationMode ?? .expanded
    grabPad.update(bounds: bounds, mode: resizable ? mode : .mini)
  }

  /// AppKit's borderless resize strip is ~1–2 px. Claim the 16 pt band first,
  /// including the bottom capsule and its margin, so SwiftUI cannot steal it.
  override func hitTest(_ point: NSPoint) -> NSView? {
    if (window as? FloatingOverlayPanel)?.allowsUserResize == true,
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
    if let hitView = hosting.hitTest(point) {
      if let scroll = (hitView as? NSScrollView) ?? hitView.enclosingScrollView {
        scroll.scrollWheel(with: event)
        return
      }
    }
    var target: NSScrollView?
    var distance = CGFloat.infinity
    func visit(_ view: NSView) {
      guard !view.isHidden, !view.visibleRect.isEmpty else { return }
      if let scroll = view as? NSScrollView {
        let rect = convert(scroll.visibleRect, from: scroll)
        let dx = max(rect.minX - point.x, 0, point.x - rect.maxX)
        let dy = max(rect.minY - point.y, 0, point.y - rect.maxY)
        let candidateDistance = dx * dx + dy * dy
        if candidateDistance < distance {
          target = scroll
          distance = candidateDistance
        }
        return
      }
      for child in view.subviews { visit(child) }
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
    guard (window as? FloatingOverlayPanel)?.allowsUserResize == true else { return }
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
  /// The resting mini strip that placement, persistence and resize gestures
  /// reason in: screen anchors and the saved origin describe a 200 × 46 strip,
  /// a resize gesture never requests less, and release-to-fold selects mini
  /// below the midpoint between it and compact's floor (305 pt). Mini itself
  /// is drawn at its natural size (`miniSize`), at this strip's upper-right
  /// corner (the shared pin), so positions saved before content-sized mini
  /// land where they did. Mini does not scale with the text size; at rest its
  /// widest form, with the brand word, is 133 pt plus the word
  /// (`miniBrandWidth`; about 65 pt by estimate, not yet measured on screen),
  /// within the strip. A live take grows mini past the strip's left edge
  /// (`miniTakeWidth`); the upper-right pin keeps the strip's right edge, so
  /// placement draws the measured width and never clips it.
  static let miniPlacementSize = NSSize(width: 200, height: collapsedHeight)
  /// Mini at rest (Founder, 2026-11-04, bus brief §3), measured at text
  /// scale 1: the 9 pt close dot, then transcription, microphone, Agents and
  /// the chevron, 22 pt each (`OverlayRecordingControls.controlDiameter`)
  /// with 4 pt between them. 7 pt separate a control from the capsule's edge
  /// on every side. The empty centre between the brand and the controls has
  /// no minimum in mini and a 2 pt row gap on either side, so it closes to
  /// the 4 pt the controls keep between them. Nothing is reserved for hidden
  /// elements at rest: no counter, waveform or timer; a live take adds them
  /// (`miniTakeWidth`).
  static let miniInset: CGFloat = 7
  static let miniControl: CGFloat = 22
  static let miniControlCount: CGFloat = 4
  static let miniControlSpacing: CGFloat = 4
  static let miniOuterPadding: CGFloat = 2 * miniInset
  static let miniBrandDot: CGFloat = 9
  static let miniRowSpacing: CGFloat = miniControlSpacing / 2
  /// The word stands 6 pt after the close dot (`Wordmark` spacing in mini).
  static let miniBrandGap: CGFloat = 6
  /// Transcription, microphone, Agents and chevron with their three gaps:
  /// 4 × 22 + 3 × 4 = 100.
  static let miniTrailingControls: CGFloat =
    miniControlCount * miniControl + (miniControlCount - 1) * miniControlSpacing
  /// Mini's own height: a control and its insets, 22 + 2 × 7 = 36. The bar
  /// measures its own (`midiBarGeometry`); mini no longer borrows its 46.
  static let miniHeight: CGFloat = miniControl + 2 * miniInset
  /// The header's vertical padding around its 26 pt row in mini, so the
  /// 22 pt controls sit `miniInset` from the capsule's edge.
  static let miniVerticalPadding: CGFloat = (miniHeight - OverlayHeaderDensity.rowHeight) / 2
  /// Mini with the channel map enabled, the close dot alone: padding, the
  /// dot, the two row gaps around the closed centre and the controls
  /// = 14 + 9 + 4 + 100 = 127. The capsule never gets narrower than its
  /// controls.
  static let miniFloorWidth =
    miniOuterPadding + miniBrandDot + 2 * miniRowSpacing + miniTrailingControls

  /// Mini while a take is live (Monika, 2026-11-04, bus brief §6, §11D):
  /// between the brand and the tools mini's own 82 pt waveform
  /// (`OverlayBarMetrics.miniWaveformWidth`), the `00:00`
  /// clock and, for an agent take, one circled digit per open channel
  /// (`OverlayChannelPill` in its counter form, 19 pt) or, for channel 0,
  /// the 22 pt megaphone; a dictation alone has no addressees. The row's gaps
  /// open from 2 pt to the 4 pt the controls keep, so the take adds the
  /// waveform, the clock, the addressees and one 4 pt gap per new item; zero
  /// at rest. Like the rest of mini it does not scale with the text size.
  @MainActor
  static func miniTakeWidth(live: Bool, openDigits: [String]) -> CGFloat {
    guard live else { return 0 }
    var width = OverlayBarMetrics.miniWaveformWidth + 2 * miniControlSpacing + midiTimerWidth
    let addressees = miniAddresseesWidth(openDigits: openDigits)
    if addressees > 0 { width += miniControlSpacing + addressees }
    return ceil(width)
  }

  /// The vertical layout's stacked counter in mini (Maciej, 2026-10-23: the
  /// way to the Intercom group stays in every form): the counter as it is
  /// drawn, the stacked cards or the open digits, after the word's 6 pt gap
  /// beside the dot. Zero in the horizontal layout and while the map holds
  /// no channel, where mini shows no counter.
  @MainActor
  static func miniStackedCounterWidth(
    map: OverlayChannelMapModel, stacked: Bool, textScale: CGFloat
  ) -> CGFloat {
    guard stacked, !map.chips.isEmpty else { return 0 }
    let counter = OverlayChannelPill.counterWidth(
      figure: String(map.count), openDigitCount: map.openDigits.count,
      scale: OverlayChannelPill.headerScale(textScale), stacked: true)
    return ceil(miniBrandGap + counter)
  }

  /// The take's addressees in mini: the open channels' circled digits 4 pt
  /// apart; zero without an open channel. A broadcast on channel 0 adds
  /// nothing: the Agents button already wears the megaphone for it, and a
  /// second one beside the clock repeated the sign (render review, 2026-11-04).
  @MainActor
  static func miniAddresseesWidth(openDigits: [String]) -> CGFloat {
    if openDigits.contains("0") { return 0 }
    guard !openDigits.isEmpty else { return 0 }
    let count = CGFloat(openDigits.count)
    let circle = OverlayChannelPill.counterWidth(figure: "", openDigitCount: 1, scale: 1)
    return count * circle + (count - 1) * miniControlSpacing
  }

  /// Mini with the brand word, while the channel map is disabled: the floor
  /// plus the word after its gap. Measured the way the floor is, so the
  /// header's clip never cuts the chevron at the capsule's edge (Maciej,
  /// 2026-10-30 14:24: "w pigułce mini jest bug rozmieszczenia").
  static let miniWordSize: CGFloat = 13
  static let miniWordTracking: CGFloat = -0.3
  @MainActor
  static let miniBrandWidth: CGFloat = ceil(miniFloorWidth + miniBrandGap + brandWordWidth)

  /// The `codescribe` word at the 13 pt brand size of mini and the bar, with
  /// its −0.3 tracking (`Wordmark`), rounded up with a point of slack.
  @MainActor
  static let brandWordWidth: CGFloat = {
    let font = NSFont.systemFont(ofSize: miniWordSize, weight: .bold)
    return ceil(
      ("codescribe" as NSString).size(withAttributes: [.font: font, .kern: miniWordTracking])
        .width) + 1
  }()

  /// The classic bar, while the horizontal channel map is off or empty: its
  /// floor and its resting size, with a flexible waveform.
  static let midiSize = NSSize(width: 410, height: collapsedHeight)

  /// The bar's measured parts at text scale 1, the way mini's are. The header
  /// pads 16 pt per side and 10 pt above and below (`16 - clipInset` +
  /// `clipInset`, `10 - clipInset` + `clipInset`). The brand is the 9 pt
  /// close dot, 5 pt and the word. The clock is `00:00` in the 11 pt mono
  /// face. The trailing tools are 141 pt: transcription, Agents and Pin
  /// (22 pt each), the 51 pt recording pair (22, 7, 22) and three 8 pt gaps.
  static let midiOuterPadding: CGFloat = 32
  static let midiVerticalPadding: CGFloat = 20
  static let midiBrandGap: CGFloat = 5
  static let midiTrailingControls: CGFloat = 141
  @MainActor
  static var midiBrandWidth: CGFloat { miniBrandDot + midiBrandGap + brandWordWidth }
  /// Measured on each call: the mono face registers when the overlay first
  /// appears, and an hour's `h:mm:ss` takes its extra digits from the names.
  @MainActor
  static var midiTimerWidth: CGFloat {
    ceil(("00:00" as NSString).size(withAttributes: [.font: CSFont.nsMono(11)]).width) + 1
  }

  /// The bar's geometry with its channel map (bus brief 2026-11-04 §4, §5,
  /// §11C). The width is the content, nothing stretched: padding, brand,
  /// waveform, clock, the map and the tools. With 1–5 agents the map is one
  /// row of stacked pills after the clock: the natural width gives the names
  /// their preferred widths, the maximum every name whole, and a width below
  /// the single row's shortest names takes two rows, so the minimum is the
  /// narrower of the two. With 6–9 agents the bar always has two rows: the
  /// first row's items over the chips, as wide as the wider of them. Channel
  /// 0's circle leads either row and is measured like any pill with a
  /// nameless caption; it is not an agent, so it never moves the 1–5 / 6–9
  /// split. No agent and no broadcast is the classic bar. Every width is
  /// bounded by the screen.
  @MainActor
  static func midiBarGeometry(
    map: OverlayChannelMapModel, textScale: CGFloat, visibleWidth: CGFloat? = nil
  ) -> OverlayMidiBarGeometry {
    guard !map.chips.isEmpty else { return .classic }
    let scale = OverlayChannelPill.headerScale(textScale)
    let names = OverlayChannelMapNameWidths(chips: map.chips, scale: scale)
    let spacing = OverlayBarMetrics.itemSpacing
    let leading =
      midiOuterPadding + midiBrandWidth + spacing + OverlayBarMetrics.waveformWidth + spacing
      + midiTimerWidth
    let trailing = spacing + midiTrailingControls
    func row(_ widths: [CGFloat], _ form: OverlayChannelPill.Form) -> CGFloat {
      OverlayChannelMapModel.rowWidth(
        nameWidths: widths, form: form, scale: scale, spacing: OverlayChannelMapView.chipSpacing)
    }
    func singleRow(_ widths: [CGFloat]) -> CGFloat {
      ceil(leading + OverlayChannelMapView.leadingGap + row(widths, .stacked) + trailing)
    }
    func twoRows(_ widths: [CGFloat]) -> CGFloat {
      ceil(max(leading + trailing, midiOuterPadding + row(widths, .chip)))
    }
    let singleRowHeight = ceil(
      midiVerticalPadding + OverlayBarMetrics.singleRowHeight(textScale: textScale))
    let twoRowHeight = ceil(midiVerticalPadding + OverlayBarMetrics.twoRowHeight)
    var geometry: OverlayMidiBarGeometry
    if map.count <= OverlayChannelMapModel.singleRowAgentLimit {
      let singleRowMinimum = singleRow(names.shortest)
      geometry = OverlayMidiBarGeometry(
        naturalWidth: singleRow(names.preferred),
        minimumWidth: min(singleRowMinimum, twoRows(names.shortest)),
        maximumWidth: singleRow(names.full), singleRowMinimumWidth: singleRowMinimum,
        singleRowHeight: singleRowHeight, twoRowHeight: twoRowHeight,
        twoRowMaximumWidth: twoRows(names.full))
    } else {
      geometry = OverlayMidiBarGeometry(
        naturalWidth: twoRows(names.preferred), minimumWidth: twoRows(names.shortest),
        maximumWidth: twoRows(names.full), singleRowMinimumWidth: .infinity,
        singleRowHeight: singleRowHeight, twoRowHeight: twoRowHeight,
        twoRowMaximumWidth: twoRows(names.full))
    }
    if let visibleWidth, visibleWidth.isFinite {
      geometry.naturalWidth = min(geometry.naturalWidth, visibleWidth)
      geometry.minimumWidth = min(geometry.minimumWidth, visibleWidth)
      geometry.maximumWidth = max(
        geometry.minimumWidth, min(geometry.maximumWidth, visibleWidth))
    }
    return geometry
  }

  static func minimumSize(for mode: OverlayPresentationMode) -> NSSize {
    switch mode {
    case .mini: NSSize(width: miniFloorWidth, height: miniHeight)
    case .midi: midiSize
    case .expanded: minSize
    }
  }

  static func presentationMode(forResizedSize size: NSSize) -> OverlayPresentationMode {
    if size.height >= (collapsedHeight + minSize.height) / 2 { return .expanded }
    return size.width < (miniPlacementSize.width + midiSize.width) / 2 ? .mini : .midi
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
  /// guess: the expanded form shows seven action slots, with the rest in the
  /// column's scroll view. Its minimum fits the header, that viewport and its
  /// padding (`OverlayTranscriptActionsLayout.minimumExpandedHeight`). Mini and
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
  /// the expanded floor: seven transcript action slots fit from the first
  /// frame, and a default below `minSize` could never be the
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
      styleMask: [.borderless, .nonactivatingPanel],
      backing: .buffered,
      defer: false
    )
    panel.contentSurface = state.windowContentSurface
    panel.contentSizeDefaults = sizeDefaults
    panel.expandedContentMinimumSize = { [weak state] surface in
      let base = DictationOverlayWindow.minSize
      guard surface == .agents, let state else { return base }
      let width = OverlayAgentDrawerLayout.minimumWindowWidth(
        layout: state.agentsLayout, textScale: textScale.scale)
      return NSSize(width: max(base.width, width), height: base.height)
    }
    panel.delegate = panel
    state.onPresentationModeChanged = { [weak panel, weak state] mode in
      guard let panel else { return }
      if let state {
        if state.windowContentSurface == .agents, state.agentsLayout == .split {
          panel.ensureAgentsSplitWidth(
            OverlayAgentDrawerLayout.minimumSplitWindowWidth(textScale: textScale.scale))
        }
        panel.setContentSurface(state.windowContentSurface)
      }
      panel.setPresentationMode(
        mode,
        animated: panel.isVisible && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
      state?.presentationMorphDuration = panel.lastMorphDuration
    }
    panel.onResizePresentationSelected = { [weak state] mode in
      state?.setPresentationMode(mode)
    }
    // Mini's brand word, mini's take and the bar's geometry follow the
    // channel map (bus intercom spec 5.1; bus brief 2026-11-04 §3, §4, §6):
    // one projection, one hook, both forms.
    let fitToChannelMap = { [weak panel, weak state] (scale: CGFloat) in
      guard let panel, let state else { return }
      let map = state.channelMapLayoutModel
      panel.setChannelMapPresentation(
        enabled: state.channelMapInMini,
        stackedCounterWidth: miniStackedCounterWidth(
          map: map, stacked: state.channelMapCounterStacked, textScale: scale))
      // The vertical layout's stacked counter already circles the open digits
      // in mini, so its take adds no addressee row (integrator, 2026-11-04).
      panel.setMiniTake(
        width: miniTakeWidth(
          live: state.miniTakeLive,
          openDigits: state.channelMapCounterStacked ? [] : map.openDigits))
      panel.setMidiBarGeometry(
        state.channelMapInHeader
          ? midiBarGeometry(
            map: map, textScale: scale,
            visibleWidth: (panel.screen ?? NSScreen.main)?.visibleFrame.width) : .classic)
    }
    fitToChannelMap(textScale.scale)
    state.onChannelMapLayoutChanged = { fitToChannelMap(textScale.scale) }
    // The bar's rows and names grow with the text scale (⌘+/−), which no
    // roster event reports. `@Published` delivers the new value before the
    // property holds it, so the fit takes the value it is handed.
    panel.textScaleSink = textScale.$scale.removeDuplicates().dropFirst()
      .sink { fitToChannelMap($0) }
    state.onRestoreMidiNaturalWidth = { [weak panel] in panel?.restoreMidiNaturalWidth() }
    state.onAgentSidebarPresented = { [weak panel, weak state] in
      guard let panel else { return }
      if state?.agentsLayout == .split {
        panel.ensureAgentsSplitWidth(
          OverlayAgentDrawerLayout.minimumSplitWindowWidth(textScale: textScale.scale))
      }
      panel.makeFirstResponder(nil)
      panel.releaseKeyAfterTranscript()
    }
    panel.onUserMove = { [weak state] in
      guard !OverlayController.isApplyingFrame else { return }
      state?.userDraggedOverlay()
    }
    panel.onUserActivity = { [weak state] in state?.noteOverlayInputActivity() }
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

    // User-resizable through the panel's own edge bands (`allowsUserResize`),
    // never AppKit's `.resizable` style. Floor keeps the glass chrome + action
    // row readable.
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
    // Pinned transcription stays above the Intercom's floating cards, without
    // repeatedly ordering either surface to the front on content updates.
    case .rest: NSWindow.Level(rawValue: NSWindow.Level.floating.rawValue + 3)
    case .unpinned, .yield: .normal
    case .capture: .statusBar
    }
  }

  func shouldOrderFront(after previousPolicy: OverlayPresencePolicy?, isVisible: Bool) -> Bool {
    isVisible && (self == .capture || (previousPolicy == .rest && self == .unpinned))
  }

  static let yieldBundleIds: Set<String> = [
    "com.apple.SecurityAgent",
    "com.apple.UserNotificationCenter",
    "com.apple.CoreServicesUIAgent",
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
  private var appliedPolicy: OverlayPresencePolicy?

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
    let previousPolicy = appliedPolicy
    appliedPolicy = policy
    if panel.level != policy.windowLevel {
      panel.level = policy.windowLevel
    }
    OverlayHintPanel.shared.synchronizeLevel(with: panel)
    // Unpinning keeps a visible overlay at the front of the normal band once.
    // Later app activations leave its order alone; screenshot lifting is unchanged.
    // Neither transition brings an ordered-out panel back on screen.
    if policy.shouldOrderFront(after: previousPolicy, isVisible: panel.isVisible) {
      panel.orderFrontRegardless()
    }
  }
}

/// The expanded bar's measured geometry, owned by the window
/// (`DictationOverlayWindow.midiBarGeometry`, bus brief 2026-11-04 §11C). The
/// frame's width stays between the minimum and the maximum, and its height is
/// the rows that width holds, rounded once here.
struct OverlayMidiBarGeometry: Equatable, Sendable {
  /// The default width: the content with the preferred names.
  var naturalWidth: CGFloat
  /// The narrowest readable bar: the shortest names, in two rows where one
  /// row cannot hold them. Narrowing stops here.
  var minimumWidth: CGFloat
  /// Every name whole: widening stops here, so a wider bar never opens a hole.
  var maximumWidth: CGFloat
  /// Below this width the single row takes two rows; infinite for 6–9 agents,
  /// zero for the classic bar.
  var singleRowMinimumWidth: CGFloat
  var singleRowHeight: CGFloat
  var twoRowHeight: CGFloat
  /// Every name whole in two rows. With 1–5 agents the widths between this
  /// and `singleRowMinimumWidth` are a dead band: two rows with a hole after
  /// the chips. `clampedWidth` snaps across it. Zero, or any width below the
  /// minimum, is no two-row form to snap to: no band (the classic bar).
  var twoRowMaximumWidth: CGFloat = 0

  /// No horizontal map: the 410 × 46 floor, widened freely.
  static let classic = OverlayMidiBarGeometry(
    naturalWidth: DictationOverlayWindow.midiSize.width,
    minimumWidth: DictationOverlayWindow.midiSize.width, maximumWidth: .greatestFiniteMagnitude,
    singleRowMinimumWidth: 0, singleRowHeight: DictationOverlayWindow.collapsedHeight,
    twoRowHeight: DictationOverlayWindow.collapsedHeight)

  func clampedWidth(_ width: CGFloat) -> CGFloat {
    let width = min(max(width, minimumWidth), maximumWidth)
    // The dead band: wider than every name in two rows, narrower than one
    // row of the shortest names. The bar is either form, never the hole
    // between them (Founder, 2026-11-04): the nearer edge wins. Both edges
    // are widths the bar can take, so the two-row edge is never below the
    // minimum; an unset two-row width (zero) opens no band.
    guard twoRowMaximumWidth >= minimumWidth, twoRowMaximumWidth < singleRowMinimumWidth,
      width > twoRowMaximumWidth, width < singleRowMinimumWidth
    else { return width }
    return width - twoRowMaximumWidth < singleRowMinimumWidth - width
      ? twoRowMaximumWidth : singleRowMinimumWidth
  }

  func height(forWidth width: CGFloat) -> CGFloat {
    width + 0.5 >= singleRowMinimumWidth ? singleRowHeight : twoRowHeight
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
  /// The bar's side bands. The close dot's hit circle reaches 7.5 pt past
  /// its 9 pt dot, which sits 16 pt in, so it starts 8.5 pt from the left
  /// edge; an 8 pt band leaves every point of it to the dot.
  static let midiBand: CGFloat = 8

  static func band(for mode: OverlayPresentationMode) -> CGFloat {
    mode == .midi ? midiBand : band
  }

  enum Edge: Sendable, Hashable {
    case left, right, top, bottom
    case topLeft, topRight, bottomLeft, bottomRight
  }

  static func edge(
    at point: NSPoint, in bounds: NSRect, mode: OverlayPresentationMode = .expanded
  ) -> Edge? {
    guard mode != .mini else { return nil }
    let side = band(for: mode)
    guard bounds.width > side * 2, bounds.height > side * 2 else { return nil }
    let left = point.x <= bounds.minX + side
    let right = point.x >= bounds.maxX - side
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
          NSRect(x: bounds.minX, y: bounds.minY, width: midiBand, height: bounds.height),
          cursor(for: .left)
        ),
        (
          NSRect(x: bounds.maxX - midiBand, y: bounds.minY, width: midiBand, height: bounds.height),
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
    let side = band(for: mode)
    guard mode != .mini, bounds.width > side * 2, bounds.height > side * 2 else { return [] }
    let b = band
    let w = bounds.width
    let h = bounds.height
    if mode == .midi {
      return [
        NSRect(x: 0, y: 0, width: midiBand, height: h),
        NSRect(x: w - midiBand, y: 0, width: midiBand, height: h),
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

extension NSPanel {
  /// Mounted bubble geometry belongs to its actual presentation window.
  var conversationTextRegions: NSHashTable<NSView>? {
    if let overlay = self as? FloatingOverlayPanel { return overlay.selectableTextRegions }
    return (self as? OverlayIntercomPanel)?.selectableTextRegions
  }

  /// ⌘C for whatever holds the focus in this panel. Shared by every
  /// non-activating Codescribe panel: the overlay and the Intercom cards.
  func performCopyChord(_ event: NSEvent) -> Bool {
    guard isKeyWindow, event.type == .keyDown,
      event.modifierFlags.intersection([.command, .control, .option, .shift]) == .command,
      event.charactersIgnoringModifiers?.lowercased() == "c"
    else { return false }
    if let textView = firstResponder as? NSTextView {
      // The focused responder owns its copy: the transcript canvas routes
      // through the tagged clipboard bridge in its own `copy(_:)`, and any
      // other text view keeps the behaviour it overrides. An empty composer
      // selection must not swallow the bubble's copy chord.
      if textView.isSelectable, textView.selectedRange().length > 0 {
        textView.copy(nil)
        return true
      }
      let before = NSPasteboard.general.changeCount
      _ = textView.nextResponder?.tryToPerform(#selector(NSText.copy(_:)), with: nil)
      if NSPasteboard.general.changeCount == before {
        _ = NSApp.sendAction(#selector(NSText.copy(_:)), to: nil, from: self)
      }
      if NSPasteboard.general.changeCount != before {
        return true
      }
    } else if let responder = firstResponder,
      responder.responds(to: #selector(NSText.copy(_:))),
      responder.tryToPerform(#selector(NSText.copy(_:)), with: nil)
    {
      return true
    }
    if let region = conversationTextRegions?.allObjects
      .compactMap({ $0 as? OverlaySelectableTextRegionView })
      .first(where: { $0.contains(windowPoint: mouseLocationOutsideOfEventStream) }),
      region.copyContent()
    {
      return true
    }
    // Sending Copy back through the app would rediscover the empty editor and
    // report success without copying anything.
    if firstResponder is NSTextView { return false }
    // This panel is non-activating, so the app's Edit menu does not receive
    // key equivalents while another app is active; the action has to be sent
    // explicitly down the responder chain.
    return NSApp.sendAction(#selector(NSText.copy(_:)), to: nil, from: self)
  }
}
