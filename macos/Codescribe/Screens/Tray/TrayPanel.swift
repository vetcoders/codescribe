import AppKit
import SwiftUI

/// Arrow-free menu host. AppKit owns focus and placement; SwiftUI owns its content.
@MainActor
final class TrayPanel: NSPanel, NSWindowDelegate {
  private weak var anchor: NSButton?
  private var clickMonitor: Any?
  private var outsideClickMonitor: Any?
  private var contentHeight: CGFloat = 460
  private var isDismissing = false
  /// A mouse-down on the status button reaches the global monitor and the key
  /// change before the button's action fires on mouse-up. Both yield to the
  /// action while such a click is in flight, so the second click toggles the
  /// menu off instead of closing it and reopening it a frame later.
  private var anchorClickInFlight = false
  var onDismiss: () -> Void = {}

  init() {
    super.init(
      contentRect: NSRect(x: 0, y: 0, width: 300, height: 460),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    isOpaque = false
    backgroundColor = .clear
    hasShadow = true
    isReleasedWhenClosed = false
    hidesOnDeactivate = false
    level = .popUpMenu
    collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
    delegate = self
    title = String(
      localized: "Codescribe menu",
      comment: "Accessibility title of the menu bar panel; Codescribe is the product name"
    )
    NotificationCenter.default.addObserver(
      self, selector: #selector(applicationDeactivated),
      name: NSApplication.didResignActiveNotification, object: NSApp)
  }

  override var canBecomeKey: Bool { true }
  override var canBecomeMain: Bool { false }

  func present<Content: View>(from button: NSButton, @ViewBuilder content: () -> Content) {
    guard button.window != nil else { return }
    if contentViewController != nil { dismiss() }
    anchor = button
    anchorClickInFlight = false
    contentViewController = NSHostingController(
      rootView: TrayPanelSurface(content: content()) { [weak self] height in
        guard let self, height.isFinite, height > 0 else { return }
        contentHeight = height
        reposition()
      })
    reposition()
    makeKeyAndOrderFront(nil)
    makeFirstResponder(nil)
    button.highlight(true)
    clickMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) {
      [weak self] event in
      if let self, event.window !== self, event.window !== anchor?.window,
        event.window?.parent !== self
      {
        dismiss()
      }
      return event
    }
    outsideClickMonitor = NSEvent.addGlobalMonitorForEvents(
      matching: [.leftMouseDown, .rightMouseDown]
    ) { [weak self] event in
      // The status bar is not one of this app's windows, so its clicks arrive
      // here with a nil window and a screen-space location.
      self?.mouseDownOutside(at: event.locationInWindow)
    }
  }

  /// A mouse-down outside the panel. One on the status button belongs to the
  /// button's action, which toggles the menu; any other closes it.
  func mouseDownOutside(at screenPoint: NSPoint) {
    if let anchorRect, anchorRect.contains(screenPoint) {
      anchorClickInFlight = true
      return
    }
    dismiss()
  }

  func dismiss() {
    guard !isDismissing, contentViewController != nil else { return }
    isDismissing = true
    defer { isDismissing = false }
    anchorClickInFlight = false
    if let clickMonitor { NSEvent.removeMonitor(clickMonitor) }
    if let outsideClickMonitor { NSEvent.removeMonitor(outsideClickMonitor) }
    clickMonitor = nil
    outsideClickMonitor = nil
    anchor?.highlight(false)
    anchor = nil
    orderOut(nil)
    contentViewController = nil
    onDismiss()
  }

  override func cancelOperation(_ sender: Any?) { dismiss() }
  override func close() { dismiss() }
  @objc private func applicationDeactivated() { dismiss() }

  func windowDidResignKey(_ notification: Notification) {
    // The status-button action owns its second-click toggle.
    if anchorClickInFlight { return }
    if let key = NSApp.keyWindow, key.parent === self { return }
    dismiss()
  }

  private var anchorRect: NSRect? {
    guard let anchor, let window = anchor.window else { return nil }
    return window.convertToScreen(anchor.convert(anchor.bounds, to: nil))
  }

  private func reposition() {
    guard let anchorRect, let screen = anchor?.window?.screen else { return }
    setFrame(
      Self.placement(
        anchor: anchorRect, visibleScreen: screen.visibleFrame,
        contentHeight: contentHeight), display: true)
  }

  static func placement(anchor: NSRect, visibleScreen: NSRect, contentHeight: CGFloat) -> NSRect {
    let bounds = visibleScreen.insetBy(dx: 6, dy: 6)
    let top = min(anchor.minY - 6, bounds.maxY)
    let width = min(CGFloat(300), bounds.width)
    let height = min(contentHeight.rounded(.up), max(1, top - bounds.minY))
    let x = min(max(anchor.midX - width / 2, bounds.minX), bounds.maxX - width)
    return NSRect(x: x, y: top - height, width: width, height: height)
  }
}

private struct TrayPanelSurface<Content: View>: View {
  let content: Content
  let onHeightChange: (CGFloat) -> Void

  var body: some View {
    if #available(macOS 26, *) {
      menu.glassEffect(.regular, in: .rect(cornerRadius: 22))
    } else {
      menu.background(.regularMaterial, in: .rect(cornerRadius: 22))
    }
  }

  private var menu: some View {
    ScrollView(.vertical) {
      content
        .fixedSize(horizontal: false, vertical: true)
        .onGeometryChange(for: CGFloat.self) {
          $0.size.height
        } action: {
          onHeightChange($0)
        }
    }
    .scrollBounceBehavior(.basedOnSize)
    .clipShape(.rect(cornerRadius: 22))
  }
}
