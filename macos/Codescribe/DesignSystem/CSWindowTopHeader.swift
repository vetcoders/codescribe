import AppKit
import SwiftUI

/// One real-content glass surface in the existing titlebar band. AppKit owns
/// the accessory slot and native window controls; the split owns its boundary.
struct CSWindowTopHeader<Header: View>: NSViewRepresentable {
  @ViewBuilder let header: Header

  func makeCoordinator() -> Coordinator { Coordinator(header: header) }

  func makeNSView(context: Context) -> Marker {
    let marker = Marker()
    marker.updateWindow = { [weak owner = context.coordinator] window in
      owner?.attach(to: window)
    }
    return marker
  }

  func updateNSView(_ view: Marker, context: Context) {
    context.coordinator.host.content.rootView = header
    context.coordinator.attach(to: view.window)
  }

  static func dismantleNSView(_ view: Marker, coordinator: Coordinator) {
    view.updateWindow = nil
    coordinator.detach()
  }

  final class Marker: NSView {
    var updateWindow: ((NSWindow?) -> Void)?
    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      updateWindow?(window)
    }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
  }

  /// A titlebar accessory's allocated frame excludes the native traffic-light
  /// margin. Convert the split boundary into this coordinate system instead
  /// of counting that margin twice. The detail glass reaches the window edge.
  final class HeaderHost: NSView {
    let glass = NSGlassEffectView()
    let content: NSHostingView<Header>
    var refresh: (() -> Void)?

    init(header: Header) {
      content = NSHostingView(rootView: header)
      super.init(frame: .zero)
      identifier = NSUserInterfaceItemIdentifier("codescribe-window-top-header")
      clipsToBounds = false
      autoresizingMask = [.width]
      glass.identifier = NSUserInterfaceItemIdentifier("codescribe-header-glass")
      glass.style = .regular
      glass.cornerRadius = 0
      content.sizingOptions = []
      content.safeAreaRegions = []
      content.autoresizingMask = [.width, .height]
      glass.contentView = content
      addSubview(glass)
    }

    required init?(coder: NSCoder) { nil }

    override func layout() {
      super.layout()
      refresh?()
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
      let local = convert(point, from: superview)
      guard glass.frame.contains(local) else { return nil }
      return glass.hitTest(local)
    }
  }

  @MainActor
  final class Coordinator: NSObject {
    let accessory = NSTitlebarAccessoryViewController()
    let host: HeaderHost
    private weak var window: NSWindow?
    private weak var split: NSSplitViewController?

    init(header: Header) {
      host = HeaderHost(header: header)
      super.init()
      accessory.layoutAttribute = .top
      accessory.view = host
      host.refresh = { [weak self] in self?.updateGeometry() }
    }

    func attach(to target: NSWindow?) {
      guard let target else {
        detach()
        return
      }
      if window !== target {
        detach()
        guard
          !target.titlebarAccessoryViewControllers.contains(where: { $0.layoutAttribute == .top })
        else { return }
        host.frame = NSRect(x: 0, y: 0, width: target.frame.width, height: 52)
        target.toolbar?.isVisible = true
        target.addTitlebarAccessoryViewController(accessory)
        window = target
      }
      updateGeometry()
    }

    private func updateGeometry() {
      guard let window else { return }
      if split?.view.window !== window {
        NotificationCenter.default.removeObserver(self)
        split = window.contentView.flatMap { Self.navigationSplit(in: $0) }
        if let split {
          NotificationCenter.default.addObserver(
            self, selector: #selector(splitResized),
            name: NSSplitView.didResizeSubviewsNotification,
            object: split.splitView)
        }
      }
      let leading: CGFloat
      if let sidebar = split?.splitViewItems.first, !sidebar.isCollapsed {
        let view = sidebar.viewController.view
        leading = view.convert(view.bounds, to: nil).maxX
      } else {
        let lights = [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton]
          .compactMap { window.standardWindowButton($0) }
        let controls = lights + (window.toolbar?.items.compactMap(\.view) ?? [])
        leading =
          (controls.filter { $0.window === window }
            .map { $0.convert($0.bounds, to: nil).maxX }.max() ?? 0) + CSSpace.sm
      }
      let origin = host.convert(NSPoint.zero, to: nil)
      let rect = NSRect(
        x: leading - origin.x, y: 0, width: max(0, window.frame.width - leading), height: 52)
      if host.glass.frame != rect { host.glass.frame = rect }
    }

    @objc private func splitResized(_ notification: Notification) {
      updateGeometry()
    }

    func detach() {
      NotificationCenter.default.removeObserver(self)
      split = nil
      window?.titlebarAccessoryViewControllers.removeAll { $0 === accessory }
      window = nil
    }

    private static func navigationSplit(in view: NSView) -> NSSplitViewController? {
      if let view = view as? NSSplitView, let controller = view.delegate as? NSSplitViewController,
        controller.splitViewItems.count == 2,
        controller.splitViewItems.first?.behavior == .sidebar
      {
        return controller
      }
      for child in view.subviews {
        if let found = navigationSplit(in: child) { return found }
      }
      return nil
    }
  }
}
