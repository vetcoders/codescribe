import AppKit
import OSLog
import SwiftUI

private let windowHeaderLog = Logger(
  subsystem: "com.vetcoders.codescribe", category: "window-header")

/// One passive glass surface and one foreground accessory in the same band.
/// Material coverage never defines the native controls' pointer exclusion.
struct CSWindowTopHeader<Header: View>: NSViewRepresentable {
  @ViewBuilder let header: Header

  func makeCoordinator() -> Coordinator { Coordinator(header: header) }

  func makeNSView(context: Context) -> Marker {
    let marker = Marker(frame: .zero)
    context.coordinator.marker = marker
    marker.refresh = { [weak owner = context.coordinator] in owner?.updateGeometry() }
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
    view.refresh = nil
    coordinator.detach()
  }

  /// App-owned background under the native titlebar. It never receives input,
  /// even where its material extends behind the window buttons and sidebar item.
  final class Marker: NSView {
    let viewport = NSView()
    let glass = NSGlassEffectView()
    var updateWindow: ((NSWindow?) -> Void)?
    var refresh: (() -> Void)?

    override init(frame: NSRect) {
      super.init(frame: frame)
      identifier = NSUserInterfaceItemIdentifier("codescribe-header-material-host")
      clipsToBounds = false
      viewport.identifier = NSUserInterfaceItemIdentifier("codescribe-header-viewport")
      viewport.clipsToBounds = true
      glass.identifier = NSUserInterfaceItemIdentifier("codescribe-header-glass")
      glass.style = .regular
      glass.cornerRadius = 0
      glass.contentView = NSView()
      viewport.addSubview(glass)
      addSubview(viewport)
    }

    required init?(coder: NSCoder) { nil }

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      updateWindow?(window)
      DispatchQueue.main.async { [weak self] in
        guard let self else { return }
        updateWindow?(window)
      }
    }

    override func layout() {
      super.layout()
      refresh?()
    }

    func setVisibleFrame(_ rect: NSRect) {
      if viewport.frame != rect { viewport.frame = rect }
      let bleed = CSSpace.sm
      let material = NSRect(
        x: -bleed, y: -bleed, width: rect.width + bleed, height: rect.height + bleed)
      if glass.frame != material { glass.frame = material }
      if glass.contentView?.frame != glass.bounds { glass.contentView?.frame = glass.bounds }
    }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
  }

  /// Only real foreground content occupies the trailing native accessory.
  /// Its public container stays outside AppKit's leading control region.
  final class HeaderHost: NSView {
    let content: NSHostingView<Header>
    var refresh: (() -> Void)?

    init(header: Header) {
      content = NSHostingView(rootView: header)
      super.init(frame: .zero)
      identifier = NSUserInterfaceItemIdentifier("codescribe-window-top-header")
      content.sizingOptions = []
      content.identifier = NSUserInterfaceItemIdentifier("codescribe-header-content")
      content.safeAreaRegions = []
      content.autoresizingMask = [.width, .height]
      addSubview(content)
    }

    required init?(coder: NSCoder) { nil }

    override func layout() {
      super.layout()
      refresh?()
    }

    func setVisibleFrame(_ rect: NSRect) {
      if content.frame != rect { content.frame = rect }
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
      let local = convert(point, from: superview)
      guard content.frame.contains(local) else { return nil }
      return content.hitTest(local)
    }
  }

  @MainActor
  final class Coordinator: NSObject {
    let accessory = NSTitlebarAccessoryViewController()
    let host: HeaderHost
    weak var marker: Marker?
    private weak var window: NSWindow?
    private weak var split: NSSplitViewController?
    private var attachmentIssue: String?

    init(header: Header) {
      host = HeaderHost(header: header)
      super.init()
      // A top accessory container spans the native sidebar toggle even when
      // HeaderHost returns nil there. A trailing slot gives that region back
      // to AppKit instead of forwarding clicks or changing private views.
      accessory.layoutAttribute = .right
      // Glass sampling and scroll-edge legibility are separate system effects.
      // Keep scrolled document content on the native soft edge, without adding
      // a custom opacity strip or changing the desktop-sampling material.
      if #available(macOS 26.1, *) {
        accessory.preferredScrollEdgeEffectStyle = .soft
      }
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
          !target.titlebarAccessoryViewControllers.contains(where: {
            $0.layoutAttribute == .top || $0.layoutAttribute == .right
          })
        else {
          reportAttachmentIssue("Another titlebar accessory owns the header region", error: true)
          return
        }
        host.frame = NSRect(
          x: 0, y: 0, width: max(0, target.frame.width - CSSidebarWidth.navigation.ideal),
          height: 52)
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

    func updateGeometry() {
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
      // Fullscreen reveal can place native toolbar views in a separate
      // window. Compare screen-converted frames, never unrelated local spaces.
      func frameInWindow(_ view: NSView) -> NSRect? {
        guard let source = view.window else { return nil }
        return window.convertFromScreen(source.convertToScreen(view.convert(view.bounds, to: nil)))
      }
      let leading: CGFloat
      let materialLeading: CGFloat
      if let sidebar = split?.splitViewItems.first, !sidebar.isCollapsed,
        let frame = frameInWindow(sidebar.viewController.view)
      {
        leading = frame.maxX
        materialLeading = frame.maxX
      } else {
        let lights = [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton]
          .compactMap { window.standardWindowButton($0) }
        let controls = lights + (window.toolbar?.items.compactMap(\.view) ?? [])
        leading = (controls.compactMap(frameInWindow).map(\.maxX).max() ?? 0) + CSSpace.sm
        materialLeading = 0
      }
      let width = max(0, window.frame.width - leading)
      if abs(host.frame.width - width) > 0.5 {
        host.setFrameSize(NSSize(width: width, height: host.frame.height))
      }
      guard let source = host.window else { return }
      let origin = window.convertPoint(
        fromScreen: source.convertPoint(toScreen: host.convert(NSPoint.zero, to: nil)))
      let rect = NSRect(
        x: leading - origin.x, y: 0, width: width, height: host.bounds.height)
      host.setVisibleFrame(rect)
      if let marker, let materialWindow = marker.window {
        let materialOrigin = window.convertPoint(
          fromScreen: materialWindow.convertPoint(toScreen: marker.convert(.zero, to: nil)))
        marker.setVisibleFrame(
          NSRect(
            x: materialLeading - materialOrigin.x,
            y: window.frame.height - host.bounds.height - materialOrigin.y,
            width: max(0, window.frame.width - materialLeading), height: host.bounds.height))
      }
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
      marker?.setVisibleFrame(.zero)
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
