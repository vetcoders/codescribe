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
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}
