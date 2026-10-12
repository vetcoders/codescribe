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
  var onHover: ((Bool) -> Void)? = nil

  func makeNSView(context: Context) -> OverlayMenuCursorRegionView {
    let view = OverlayMenuCursorRegionView()
    view.onHover = onHover
    return view
  }

  func updateNSView(_ nsView: OverlayMenuCursorRegionView, context: Context) {
    nsView.onHover = onHover
  }
}

final class OverlayMenuCursorRegionView: NSView {
  private weak var owner: FloatingOverlayPanel?
  var onHover: ((Bool) -> Void)?
  private var hovered = false
  private var hoverArea: NSTrackingArea?

  func updateHover(at point: NSPoint?) {
    let inside =
      point.map {
        window != nil && !isHiddenOrHasHiddenAncestor
          && visibleRect.contains(convert($0, from: nil))
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
    if let hoverArea { removeTrackingArea(hoverArea) }
    let area = NSTrackingArea(
      rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
      owner: self, userInfo: nil)
    addTrackingArea(area)
    hoverArea = area
  }

  override func mouseEntered(with event: NSEvent) { updateHover(at: event.locationInWindow) }
  override func mouseExited(with event: NSEvent) { setHovered(false) }

  override func hitTest(_ point: NSPoint) -> NSView? { nil }

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    setHovered(false)
    owner?.menuCursorRegions.remove(self)
    owner = window as? FloatingOverlayPanel
    owner?.menuCursorRegions.add(self)
  }
}
