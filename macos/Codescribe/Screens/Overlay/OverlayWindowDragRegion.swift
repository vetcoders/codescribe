import AppKit
import SwiftUI

/// A deliberately inert piece of overlay chrome that moves its owning window.
/// Controls and the native transcript remain sibling content above this region,
/// so they keep their ordinary click, scroll, and selection behavior.
struct OverlayWindowDragRegion: View {
  let identifier: String

  init(identifier: String = "overlay-window-drag-region") {
    self.identifier = identifier
  }

  var body: some View {
    AppKitWindowDragRegion(identifier: identifier)
      .accessibilityHidden(true)
  }
}

/// A narrow AppKit hit-region bridge for the non-activating panel.
/// `WindowDragGesture` reaches the SwiftUI hit path but does not move a non-key
/// `.nonactivatingPanel` reliably. `FloatingOverlayPanel.sendEvent` recognizes
/// only these inert views and derives the origin from the delivered sequence.
private struct AppKitWindowDragRegion: NSViewRepresentable {
  let identifier: String

  func makeNSView(context: Context) -> OverlayWindowDragRegionView {
    let view = OverlayWindowDragRegionView()
    view.setAccessibilityIdentifier(identifier)
    return view
  }

  func updateNSView(_ nsView: OverlayWindowDragRegionView, context: Context) {
    nsView.setAccessibilityIdentifier(identifier)
  }
}

final class OverlayWindowDragRegionView: NSView {
  private weak var owner: FloatingOverlayPanel?

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    owner?.windowDragRegions.remove(self)
    owner = window as? FloatingOverlayPanel
    owner?.windowDragRegions.add(self)
  }

  override var isOpaque: Bool { false }
  override func resetCursorRects() {
    discardCursorRects()
    addCursorRect(bounds, cursor: .arrow)
  }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

/// Geometry and mounted identity only; this background never intercepts clicks.
/// Cursor decisions stay with the existing FloatingOverlayPanel owner.
struct OverlayMenuCursorRegion: NSViewRepresentable {
  var cursor: NSCursor = .arrow
  var coalescesHover = true
  var onHover: ((Bool) -> Void)? = nil

  func makeNSView(context: Context) -> OverlayMenuCursorRegionView {
    let view = OverlayMenuCursorRegionView()
    view.cursor = cursor
    view.coalescesHover = coalescesHover
    view.onHover = onHover
    return view
  }

  func updateNSView(_ nsView: OverlayMenuCursorRegionView, context: Context) {
    nsView.cursor = cursor
    nsView.coalescesHover = coalescesHover
    nsView.onHover = onHover
  }
}

final class OverlayMenuCursorRegionView: NSView {
  private weak var owner: FloatingOverlayPanel?
  var cursor: NSCursor = .arrow
  var coalescesHover = true
  var onHover: ((Bool) -> Void)?
  private(set) var hovered = false
  private var hoverArea: NSTrackingArea?
  private var deliveredHover = false
  private var hoverTask: Task<Void, Never>?

  func updateHover(at point: NSPoint?) {
    let inside =
      point.map {
        guard let window, !isHiddenOrHasHiddenAncestor,
          reachableRect.contains(convert($0, from: nil))
        else { return false }
        // A tracking-area entry can reach the owner directly; the open
        // interactive card over this window still owns the pointer.
        return !OverlayHintPanel.shared.coversPointer(
          at: window.convertPoint(toScreen: $0), under: window)
      } ?? false
    setHovered(inside)
  }

  /// The part of this region the pointer can reach: its visible rect cut to
  /// the frame of the enclosing scroll view. A roster scrolled without a
  /// scroll clip (`scrollClipDisabled`) keeps rows past its viewport mounted,
  /// and its clip view does not bound their `visibleRect`; such a row lying
  /// under the bar header took the pointer from ✦ and opened its own card.
  private var reachableRect: NSRect {
    let rect = bounds.intersection(visibleRect)
    guard let scroll = enclosingScrollView else { return rect }
    return rect.intersection(convert(scroll.bounds, from: scroll))
  }

  private func setHovered(_ inside: Bool) {
    guard hovered != inside else { return }
    hovered = inside
    if !coalescesHover {
      deliverHover()
      return
    }
    guard onHover != nil, hoverTask == nil else { return }
    // Hit geometry and cursor ownership stay synchronous. Only SwiftUI paint
    // and hint callbacks take the latest enter/leave within one display beat.
    let framesPerSecond = max(30, min(window?.screen?.maximumFramesPerSecond ?? 60, 120))
    let delay = Duration.nanoseconds(1_000_000_000 / framesPerSecond)
    hoverTask = Task { @MainActor [weak self] in
      do {
        try await Task.sleep(for: delay)
      } catch {
        return
      }
      guard !Task.isCancelled, let self else { return }
      self.hoverTask = nil
      guard self.window != nil else { return }
      self.deliverHover()
    }
  }

  private func deliverHover() {
    guard deliveredHover != hovered else { return }
    deliveredHover = hovered
    onHover?(hovered)
  }

  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    let rect = reachableRect
    // View updates do not imply a new pointer region. Replacing an unchanged
    // area makes AppKit queue needless exit/entry events and cursor resets.
    if let hoverArea, hoverArea.rect == rect { return }
    if let hoverArea { removeTrackingArea(hoverArea) }
    hoverArea = nil
    guard !rect.isEmpty else {
      setHovered(false)
      return
    }
    let area = NSTrackingArea(
      rect: rect, options: [.mouseEnteredAndExited, .activeAlways],
      owner: self, userInfo: nil)
    addTrackingArea(area)
    hoverArea = area
  }

  /// Both native callbacks enter here with their original source identity.
  /// A late exit from a retired area must not clear a newer hover entry.
  func handleHoverEvent(at point: NSPoint?, from area: NSTrackingArea?, in sourceWindow: NSWindow?)
  {
    if let area, area !== hoverArea { return }
    if let sourceWindow, sourceWindow !== window { return }
    updateHover(at: point)
  }

  override func mouseEntered(with event: NSEvent) {
    handleHoverEvent(at: event.locationInWindow, from: event.trackingArea, in: event.window)
  }

  override func mouseExited(with event: NSEvent) {
    handleHoverEvent(at: nil, from: event.trackingArea, in: event.window)
  }

  override func hitTest(_ point: NSPoint) -> NSView? { nil }

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    hoverTask?.cancel()
    hoverTask = nil
    hovered = false
    // Removal must withdraw an already delivered hint even when no next frame
    // will run. A pending entry is discarded rather than replayed after unmount.
    deliverHover()
    owner?.menuCursorRegions.remove(self)
    owner = window as? FloatingOverlayPanel
    owner?.menuCursorRegions.add(self)
  }
}

/// Mounted bubble bounds and Copy payload; this background never intercepts clicks.
/// A click inside opens the panel keyboard gate for selectable SwiftUI text.
struct OverlaySelectableTextRegion: NSViewRepresentable {
  var copyText = ""

  func makeNSView(context: Context) -> OverlaySelectableTextRegionView {
    let view = OverlaySelectableTextRegionView()
    view.copyText = copyText
    return view
  }

  func updateNSView(_ nsView: OverlaySelectableTextRegionView, context: Context) {
    nsView.copyText = copyText
  }
}

final class OverlaySelectableTextRegionView: NSView {
  private weak var owner: NSPanel?
  var copyText = ""

  func contains(windowPoint: NSPoint) -> Bool {
    window != nil && !isHiddenOrHasHiddenAncestor
      && bounds.intersection(visibleRect).contains(convert(windowPoint, from: nil))
  }

  override func hitTest(_ point: NSPoint) -> NSView? { nil }

  func copyContent() -> Bool {
    guard copyText.contains(where: { !$0.isWhitespace }) else { return false }
    let text = copyText
    Task { @MainActor in await chatCopy(text) }
    return true
  }

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    owner?.conversationTextRegions?.remove(self)
    owner = window as? NSPanel
    owner?.conversationTextRegions?.add(self)
  }
}
