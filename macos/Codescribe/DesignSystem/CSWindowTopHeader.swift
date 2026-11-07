import AppKit
import SwiftUI

@MainActor
final class CSWindowTopHeaderLayout: ObservableObject {
  @Published var leadingWidth: CGFloat = 0
}

/// One replacement header in the existing unified titlebar band. The native
/// toolbar retains sidebar/window controls; the detail's real content lives
/// inside one rectangular glass view, not in a second row or an empty plane.
struct CSWindowTopHeader<Header: View>: NSViewRepresentable {
  let leadingWidth: CGFloat
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
    context.coordinator.requestedLeadingWidth = leadingWidth
    context.coordinator.host.rootView = HeaderRoot(layout: context.coordinator.layout, header: header)
    context.coordinator.attach(to: view.window)
  }

  static func dismantleNSView(_ view: Marker, coordinator: Coordinator) {
    view.updateWindow = nil
    coordinator.detach()
  }

  final class Marker: NSView {
    var updateWindow: ((NSWindow?) -> Void)?
    private var scheduled = false

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      updateWindow?(window)
      scheduleUpdate()
    }

    override func layout() {
      super.layout()
      scheduleUpdate()
    }

    private func scheduleUpdate() {
      guard !scheduled else { return }
      scheduled = true
      DispatchQueue.main.async { [weak self] in
        guard let self else { return }
        scheduled = false
        updateWindow?(window)
      }
    }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
  }

  struct HeaderRoot: View {
    @ObservedObject var layout: CSWindowTopHeaderLayout
    let header: Header

    var body: some View {
      HStack(spacing: 0) {
        Color.clear.frame(width: layout.leadingWidth).allowsHitTesting(false)
        CSRectangularHeaderGlass(content: header)
          .frame(maxWidth: .infinity, maxHeight: .infinity)
      }
      .frame(height: 52)
    }
  }

  final class HeaderHost: NSHostingView<HeaderRoot> {
    var leadingWidth: CGFloat = 0

    override func hitTest(_ point: NSPoint) -> NSView? {
      guard convert(point, from: superview).x >= leadingWidth else { return nil }
      return super.hitTest(point)
    }
  }

  @MainActor
  final class Coordinator {
    let layout = CSWindowTopHeaderLayout()
    let accessory = NSTitlebarAccessoryViewController()
    let host: HeaderHost
    var requestedLeadingWidth: CGFloat = 0
    private weak var window: NSWindow?

    init(header: Header) {
      host = HeaderHost(rootView: HeaderRoot(layout: layout, header: header))
      host.sizingOptions = []
      host.autoresizingMask = [.width]
      host.identifier = NSUserInterfaceItemIdentifier("codescribe-window-top-header")
      accessory.layoutAttribute = .top
      accessory.view = host
    }

    func attach(to target: NSWindow?) {
      guard let target else {
        detach()
        return
      }
      if window !== target {
        detach()
        guard !target.titlebarAccessoryViewControllers.contains(where: { $0.layoutAttribute == .top })
        else { return }
        host.frame = NSRect(x: 0, y: 0, width: target.frame.width, height: 52)
        target.toolbar?.isVisible = true
        target.addTitlebarAccessoryViewController(accessory)
        window = target
      }
      let lights = [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton]
        .compactMap { target.standardWindowButton($0) }
      let nativeControls = lights + (target.toolbar?.items.compactMap(\.view) ?? [])
      let nativeRight = nativeControls.filter { $0.window === target }
        .map { $0.convert($0.bounds, to: nil).maxX }.max() ?? 0
      let lightsRight = lights.map { $0.convert($0.bounds, to: nil).maxX }.max() ?? 0
      let leading = requestedLeadingWidth > 0
        ? max(requestedLeadingWidth, lightsRight + CSSpace.sm)
        : nativeRight + CSSpace.sm
      host.leadingWidth = leading
      if abs(layout.leadingWidth - leading) > 0.5 { layout.leadingWidth = leading }
    }

    func detach() {
      window?.titlebarAccessoryViewControllers.removeAll { $0 === accessory }
      window = nil
    }
  }
}

/// The glass owns actual header content. No tint, covering fill or clip is
/// involved; the window supplies its outer corner clipping.
private struct CSRectangularHeaderGlass<Content: View>: NSViewRepresentable {
  let content: Content

  func makeCoordinator() -> NSHostingView<Content> { NSHostingView(rootView: content) }

  func makeNSView(context: Context) -> NSGlassEffectView {
    let glass = NSGlassEffectView()
    glass.identifier = NSUserInterfaceItemIdentifier("codescribe-header-glass")
    glass.style = .regular
    glass.cornerRadius = 0
    context.coordinator.sizingOptions = []
    context.coordinator.autoresizingMask = [.width, .height]
    glass.contentView = context.coordinator
    return glass
  }

  func updateNSView(_ view: NSGlassEffectView, context: Context) {
    context.coordinator.rootView = content
  }
}
