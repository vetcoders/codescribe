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
  var onHover: ((Bool) -> Void)? = nil

  func makeNSView(context: Context) -> OverlayMenuCursorRegionView {
    let view = OverlayMenuCursorRegionView()
    view.cursor = cursor
    view.onHover = onHover
    return view
  }

  func updateNSView(_ nsView: OverlayMenuCursorRegionView, context: Context) {
    nsView.cursor = cursor
    nsView.onHover = onHover
  }
}

final class OverlayMenuCursorRegionView: NSView {
  private weak var owner: FloatingOverlayPanel?
  var cursor: NSCursor = .arrow
  var onHover: ((Bool) -> Void)?
  private(set) var hovered = false
  private var hoverArea: NSTrackingArea?

  func updateHover(at point: NSPoint?) {
    let inside =
      point.map {
        window != nil && !isHiddenOrHasHiddenAncestor
          && bounds.intersection(visibleRect).contains(convert($0, from: nil))
      } ?? false
    setHovered(inside)
  }

  private func setHovered(_ inside: Bool) {
    guard hovered != inside else { return }
    hovered = inside
    onHover?(inside)
  }

  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    let rect = bounds.intersection(visibleRect)
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
    setHovered(false)
    owner?.menuCursorRegions.remove(self)
    owner = window as? FloatingOverlayPanel
    owner?.menuCursorRegions.add(self)
  }
}

/// Geometry and mounted identity only; this background never intercepts clicks.
/// A click inside opens the panel keyboard gate for selectable SwiftUI text.
struct OverlaySelectableTextRegion: NSViewRepresentable {
  func makeNSView(context: Context) -> OverlaySelectableTextRegionView {
    OverlaySelectableTextRegionView()
  }

  func updateNSView(_ nsView: OverlaySelectableTextRegionView, context: Context) {}
}

final class OverlaySelectableTextRegionView: NSView {
  private weak var owner: FloatingOverlayPanel?

  func contains(windowPoint: NSPoint) -> Bool {
    window != nil && !isHiddenOrHasHiddenAncestor
      && bounds.intersection(visibleRect).contains(convert(windowPoint, from: nil))
  }

  override func hitTest(_ point: NSPoint) -> NSView? { nil }

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    owner?.selectableTextRegions.remove(self)
    owner = window as? FloatingOverlayPanel
    owner?.selectableTextRegions.add(self)
  }
}
