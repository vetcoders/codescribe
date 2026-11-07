import AppKit
import SwiftUI

/// Owns the detail column's real header content through AppKit's public
/// split-item accessory API. The marker never draws or intercepts events.
struct CSColumnHeaderAccessory<Header: View>: NSViewRepresentable {
  let header: Header

  func makeCoordinator() -> Coordinator { Coordinator(header: header) }

  func makeNSView(context: Context) -> Marker {
    let marker = Marker()
    marker.onWindow = { [weak coordinator = context.coordinator] window in
      coordinator?.attach(to: window)
    }
    return marker
  }

  func updateNSView(_ view: Marker, context: Context) {
    context.coordinator.host.rootView = header
    context.coordinator.attach(to: view.window)
  }

  static func dismantleNSView(_ view: Marker, coordinator: Coordinator) {
    view.onWindow = nil
    coordinator.detach()
  }

  final class Marker: NSView {
    var onWindow: ((NSWindow?) -> Void)?

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      onWindow?(window)
    }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
  }

  @MainActor
  final class Coordinator {
    let host: NSHostingView<Header>
    let accessory = NSSplitViewItemAccessoryViewController()
    private weak var item: NSSplitViewItem?

    init(header: Header) {
      host = NSHostingView(rootView: header)
      host.identifier = NSUserInterfaceItemIdentifier("codescribe-column-header")
      accessory.view = host
    }

    func attach(to window: NSWindow?) {
      guard let window else {
        detach()
        return
      }
      guard let root = window.contentViewController,
        let split = Self.navigationSplit(in: root),
        let detail = split.splitViewItems.last
      else { return }
      guard item !== detail else { return }
      detach()
      detail.addTopAlignedAccessoryViewController(accessory)
      item = detail
    }

    func detach() {
      item?.topAlignedAccessoryViewControllers.removeAll { $0 === accessory }
      item = nil
    }

    private static func navigationSplit(in controller: NSViewController) -> NSSplitViewController? {
      if let split = controller as? NSSplitViewController,
        split.splitViewItems.count == 2,
        split.splitViewItems.first?.behavior == .sidebar
      { return split }
      for child in controller.children {
        if let split = navigationSplit(in: child) { return split }
      }
      return nil
    }
  }
}
