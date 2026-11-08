import AppKit
import OSLog
import SwiftUI

private let windowHeaderLog = Logger(
  subsystem: "com.vetcoders.codescribe", category: "window-header")

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
      // Settings adopts its scene window through HostingWindowReader on the
      // same turn. Retry after that owner has established full-size content.
      DispatchQueue.main.async { [weak self] in
        guard let self else { return }
        updateWindow?(window)
      }
    }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
  }

  /// A titlebar accessory's allocated frame excludes the native traffic-light
  /// margin. Convert the split boundary into this coordinate system instead
  /// of counting that margin twice. The detail glass reaches the window edge.
  final class HeaderHost: NSView {
    /// Native glass rims belong outside the joined sidebar and content seams.
    /// This is material bleed, not a painted line or an opaque cover.
    static var joinedEdgeBleed: CGFloat { CSSpace.sm }
    let viewport = NSView()
    let glass = NSGlassEffectView()
    let content: NSHostingView<Header>
    private let glassContent = NSView()
    var refresh: (() -> Void)?

    init(header: Header) {
      content = NSHostingView(rootView: header)
      super.init(frame: .zero)
      identifier = NSUserInterfaceItemIdentifier("codescribe-window-top-header")
      clipsToBounds = false
      autoresizingMask = [.width]
      viewport.identifier = NSUserInterfaceItemIdentifier("codescribe-header-viewport")
      viewport.clipsToBounds = true
      glass.identifier = NSUserInterfaceItemIdentifier("codescribe-header-glass")
      glass.style = .regular
      glass.cornerRadius = 0
      content.sizingOptions = []
      content.identifier = NSUserInterfaceItemIdentifier("codescribe-header-content")
      content.safeAreaRegions = []
      content.autoresizingMask = [.width, .minYMargin]
      glassContent.addSubview(content)
      glass.contentView = glassContent
      viewport.addSubview(glass)
      addSubview(viewport)
    }

    required init?(coder: NSCoder) { nil }

    override func layout() {
      super.layout()
      refresh?()
    }

    func setVisibleFrame(_ rect: NSRect) {
      if viewport.frame != rect { viewport.frame = rect }
      let bleed = Self.joinedEdgeBleed
      let material = NSRect(
        x: -bleed, y: -bleed, width: rect.width + bleed, height: rect.height + bleed)
      if glass.frame != material { glass.frame = material }
      if glassContent.frame != glass.bounds { glassContent.frame = glass.bounds }
      let controls = NSRect(x: bleed, y: bleed, width: rect.width, height: rect.height)
      if content.frame != controls { content.frame = controls }
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
      let local = convert(point, from: superview)
      guard viewport.frame.contains(local) else { return nil }
      return viewport.hitTest(local)
    }
  }

  @MainActor
  final class Coordinator: NSObject {
    let accessory = NSTitlebarAccessoryViewController()
    let host: HeaderHost
    private weak var window: NSWindow?
    private weak var split: NSSplitViewController?
    private var attachmentIssue: String?

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
      guard target.styleMask.contains(.fullSizeContentView) else {
        reportAttachmentIssue(
          "Waiting for the window owner to enable full-size content", error: false)
        return
      }
      if window !== target {
        detach()
        guard
          !target.titlebarAccessoryViewControllers.contains(where: { $0.layoutAttribute == .top })
        else {
          reportAttachmentIssue("Another top titlebar accessory owns the header slot", error: true)
          return
        }
        host.frame = NSRect(x: 0, y: 0, width: target.frame.width, height: 52)
        target.toolbar?.isVisible = true
        target.addTitlebarAccessoryViewController(accessory)
        window = target
        NotificationCenter.default.addObserver(
          self, selector: #selector(windowUpdated), name: NSWindow.didUpdateNotification,
          object: target)
      }
      attachmentIssue = nil
      updateGeometry()
    }

    private func reportAttachmentIssue(_ message: String, error: Bool) {
      guard attachmentIssue != message else { return }
      attachmentIssue = message
      if error {
        windowHeaderLog.error("Header not attached: \(message, privacy: .public)")
      } else {
        windowHeaderLog.debug("Header deferred: \(message, privacy: .public)")
      }
    }

    private func stopObservingSplit() {
      if let split {
        NotificationCenter.default.removeObserver(
          self, name: NSSplitView.didResizeSubviewsNotification, object: split.splitView)
      }
      split = nil
    }

    private func updateGeometry() {
      guard let window else { return }
      // SwiftUI can restore automatic separators while adopting its toolbar.
      // The accessory owns this band, including the absence of a second edge.
      if window.titlebarSeparatorStyle != .none { window.titlebarSeparatorStyle = .none }
      if split?.view.window !== window {
        stopObservingSplit()
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
      host.setVisibleFrame(rect)
    }

    @objc private func splitResized(_ notification: Notification) {
      updateGeometry()
    }

    @objc private func windowUpdated(_ notification: Notification) {
      updateGeometry()
    }

    func detach() {
      stopObservingSplit()
      if let window {
        NotificationCenter.default.removeObserver(
          self, name: NSWindow.didUpdateNotification, object: window)
        // AppKit exposes indexed removal. Mutating the returned array tries
        // an unsupported setTitlebarAccessoryViewControllers: selector.
        if let index = window.titlebarAccessoryViewControllers.firstIndex(
          where: { $0 === accessory })
        {
          window.removeTitlebarAccessoryViewController(at: index)
        }
      }
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
