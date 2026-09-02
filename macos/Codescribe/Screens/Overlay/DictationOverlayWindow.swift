import AppKit
import SwiftUI

// Borderless floating window host for the dictation overlay.
//
// This is a FACTORY ONLY. Summon/dismiss wiring (hotkey, placement, focus handoff,
// activation policy) belongs to the orchestrator in App.swift — this file just
// builds a correctly-configured panel whose content is `DictationOverlayView`,
// with a clear background so the `.ultraThinMaterial` inside `GlassPanel` blurs
// whatever is underneath.

/// Borderless, non-activating panel that can still become key so the overlay's
/// buttons (Copy / Send to Agent / Close) receive clicks without stealing app focus.
final class FloatingOverlayPanel: NSPanel, NSWindowDelegate {
  var onUserMove: (() -> Void)?
  var onUserResize: (() -> Void)?
  fileprivate var presence: OverlayPresence?

  func startPresence() {
    presence?.start()
  }

  func invalidatePresence() {
    presence?.invalidate()
  }

  override var canBecomeKey: Bool { allowsKeyForEdit }
  override var canBecomeMain: Bool { false }
  var allowsKeyForEdit = false

  func windowDidMove(_ notification: Notification) {
    onUserMove?()
  }

  func windowDidResize(_ notification: Notification) {
    onUserResize?()
  }
}

/// Content container for the overlay panel. Its sole job is to keep the SwiftUI
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
private final class OverlayContentContainer: NSView {
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
    hosting.frame = bounds
    window?.invalidateCursorRects(for: self)
  }

  override func layout() {
    super.layout()
    hosting.frame = bounds
  }

  /// AppKit's borderless resize strip is ~1–2 px. Claim the 12 pt band first
  /// so SwiftUI / movable-background do not steal the edge.
  override func hitTest(_ point: NSPoint) -> NSView? {
    if OverlayResizeHit.edge(at: point, in: bounds) != nil { return self }
    return super.hitTest(point)
  }

  override func resetCursorRects() {
    discardCursorRects()
    for (rect, cursor) in OverlayResizeHit.cursorRects(in: bounds) {
      addCursorRect(rect, cursor: cursor)
    }
  }

  override func mouseDown(with event: NSEvent) {
    let local = convert(event.locationInWindow, from: nil)
    guard let edge = OverlayResizeHit.edge(at: local, in: bounds),
      let window
    else {
      super.mouseDown(with: event)
      return
    }
    OverlayResizeHit.track(edge: edge, window: window, start: event)
  }

  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

enum DictationOverlayWindow {
  /// Hard floor for the panel's content size. Enforced for user edge-drag
  /// (`minSize`/`contentMinSize`) AND for every programmatic `setFrame` via
  /// `clamp(_:to:)` (AppKit does not apply `minSize` to programmatic frames).
  /// Slim chrome cut: modeMeta + bottom action row removed; waveform moved into
  /// the primary bar. Height 300 → 260 keeps `bodyMinHeight` (~3 transcript
  /// lines) without the old action-layer mass. Width floor (320) is unchanged.
  static let minSize = NSSize(width: 320, height: 260)
  /// First-launch content size (no persisted value yet). LANDSCAPE rectangle —
  /// operator spec: the resting state is a horizontal bar (waveform + a few
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
    panel.onUserMove = { [weak state] in
      guard !OverlayController.isApplyingFrame else { return }
      state?.userDraggedOverlay()
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
    panel.hasShadow = false  // GlassPanel paints its own deep shadow.

    // Float above normal windows, ride along every Space, never take app focus.
    // sharingType stays readable so PrintScreen can see the panel; presence
    // raises to statusBar for the capture chord and yields to system alerts.
    panel.level = OverlayPresencePolicy.rest.windowLevel
    panel.sharingType = .readOnly
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
    panel.isFloatingPanel = true
    panel.hidesOnDeactivate = false
    panel.isMovableByWindowBackground = true  // chrome drag handles + empty background

    panel.titleVisibility = .hidden
    panel.titlebarAppearsTransparent = true
    panel.standardWindowButton(.closeButton)?.isHidden = true
    panel.standardWindowButton(.miniaturizeButton)?.isHidden = true
    panel.standardWindowButton(.zoomButton)?.isHidden = true

    let presence = OverlayPresence(panel: panel)
    presence.start()
    panel.presence = presence

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
  static func restoredContentSize(for screen: NSScreen? = NSScreen.main) -> NSSize {
    let defaults = UserDefaults.standard
    let width = defaults.double(forKey: sizeDefaultsKey + ".w")
    let height = defaults.double(forKey: sizeDefaultsKey + ".h")
    let raw = (width > 0 && height > 0) ? NSSize(width: width, height: height) : defaultSize
    return clamp(raw, to: screen)
  }

  /// Persist the current content size so it survives relaunch. Called on hide().
  static func persist(size: NSSize) {
    let defaults = UserDefaults.standard
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
/// is one or two pixels; this is the operator-visible target (macOS 15+).
enum OverlayResizeHit: Sendable {
  static let band: CGFloat = 12

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

  @MainActor
  static func track(edge: Edge, window: NSWindow, start: NSEvent) {
    let startFrame = window.frame
    let startMouse = start.locationInWindow
    let startScreen = window.convertToScreen(
      NSRect(origin: startMouse, size: .zero)
    ).origin
    while let next = window.nextEvent(matching: [.leftMouseDragged, .leftMouseUp]) {
      if next.type == .leftMouseUp { break }
      let now = NSEvent.mouseLocation
      let frame = apply(
        edge: edge,
        start: startFrame,
        dx: now.x - startScreen.x,
        dy: now.y - startScreen.y,
        minSize: window.minSize
      )
      window.setFrame(frame, display: true)
    }
  }
}

/// Chrome hit target: the view itself moves the window. Interactive siblings
/// (buttons, editor) sit above it and keep their clicks.
struct OverlayDragHandle: NSViewRepresentable {
  func makeNSView(context: Context) -> OverlayDragHandleView {
    OverlayDragHandleView()
  }

  func updateNSView(_ nsView: OverlayDragHandleView, context: Context) {}
}

final class OverlayDragHandleView: NSView {
  override var mouseDownCanMoveWindow: Bool { true }
  override var isOpaque: Bool { false }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

/// The overlay becomes key only while the user is editing the transcript.
/// Any other click leaves the caret in the previous app.
struct OverlayKeyGate: NSViewRepresentable {
  var editing: Bool
  var onResign: () -> Void

  func makeNSView(context: Context) -> OverlayKeyGateView {
    let view = OverlayKeyGateView()
    view.onResign = onResign
    return view
  }

  func updateNSView(_ nsView: OverlayKeyGateView, context: Context) {
    nsView.onResign = onResign
    nsView.apply(editing: editing)
  }

  static func dismantleNSView(_ nsView: OverlayKeyGateView, coordinator: ()) {
    nsView.invalidate()
  }
}

final class OverlayKeyGateView: NSView {
  var onResign: (() -> Void)?
  private var resignTask: Task<Void, Never>?

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    resignTask?.cancel()
    resignTask = nil
    guard let window else { return }
    resignTask = Task { @MainActor [weak self, weak window] in
      guard let window else { return }
      for await _ in NotificationCenter.default.notifications(
        named: NSWindow.didResignKeyNotification,
        object: window
      ) {
        guard !Task.isCancelled else { return }
        self?.onResign?()
      }
    }
  }

  func apply(editing: Bool) {
    guard let panel = window as? FloatingOverlayPanel else { return }
    panel.allowsKeyForEdit = editing
    if editing {
      if !panel.isKeyWindow { panel.makeKey() }
    } else if panel.isKeyWindow {
      panel.makeFirstResponder(nil)
      panel.resignKey()
    }
  }

  func invalidate() {
    resignTask?.cancel()
    resignTask = nil
    onResign = nil
  }
}
