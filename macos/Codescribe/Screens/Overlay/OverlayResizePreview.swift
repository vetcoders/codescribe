import AppKit
import SwiftUI

/// Measures the live header's flexible region once, before its bitmap is frozen.
/// It paints nothing and never participates in the header's hit testing.
struct OverlayResizeAnchor: NSViewRepresentable {
  func makeNSView(context: Context) -> OverlayResizeAnchorView {
    OverlayResizeAnchorView()
  }
  func updateNSView(_ nsView: OverlayResizeAnchorView, context: Context) {}
}

final class OverlayResizeAnchorView: NSView {
  override func hitTest(_ point: NSPoint) -> NSView? { nil }

  static func find(in view: NSView) -> OverlayResizeAnchorView? {
    if let anchor = view as? OverlayResizeAnchorView { return anchor }
    for child in view.subviews {
      if let anchor = find(in: child) { return anchor }
    }
    return nil
  }
}

/// Transient native feedback for a window resize gesture. The windows being
/// resized keep their frames and hide while the gesture lasts; this panel
/// alone follows the pointer, so a window full of Markdown never reflows at
/// pointer frequency. Main-overlay gestures retain one frozen content image;
/// grouped fiszki keep their header-only feedback. The owner commits the final
/// frame on release. It has no
/// document, draft or delivery state and performs no SwiftUI layout work.
///
/// Introduced for fiszki; the main overlay uses the same preview since
/// 2026-11-04 ("ten niskobudżetowy układ w sensie przeciągania i resizingu z
/// … floating przenieść", Maciej, 2026-11-03).
@MainActor
final class OverlayResizePreview {
  private let panel: NSPanel
  private let drawing: OverlayResizePreviewView
  private let sources: [(window: NSWindow, alpha: CGFloat, ignoresMouseEvents: Bool)]
  private let freezesMouse: Bool
  private let restore: (NSWindow) -> Void
  private var finished = false
  private(set) var frame: CGRect?

  /// `freezesMouse` makes the hidden windows click-through, for owners that
  /// keep the gesture alive with their own event monitor. `restore` runs for
  /// each window before it lays out and becomes visible again.
  init(
    windows: [NSWindow], group: Bool, headerHeight: CGFloat,
    palette: OverlayAppearancePalette, cornerRadius: CGFloat = 10,
    capturesContent: Bool = false,
    freezesMouse: Bool = true, restore: @escaping (NSWindow) -> Void = { _ in }
  ) {
    sources = windows.map { ($0, $0.alphaValue, $0.ignoresMouseEvents) }
    self.freezesMouse = freezesMouse
    self.restore = restore
    let headers: [OverlayResizePreviewView.Header?] = windows.map { window in
      guard let view = window.contentView else { return nil }
      let height = min(headerHeight, view.bounds.height)
      let rect = NSRect(
        x: view.bounds.minX,
        y: view.isFlipped ? view.bounds.minY : view.bounds.maxY - height,
        width: view.bounds.width, height: height)
      guard !rect.isEmpty,
        let bitmap = view.bitmapImageRepForCachingDisplay(in: rect)
      else { return nil }
      view.cacheDisplay(in: rect, to: bitmap)
      guard let image = bitmap.cgImage else { return nil }
      let anchor = OverlayResizeAnchorView.find(in: view)
      // The one-point cut is in the existing gap after the name/spacer, so
      // only empty chrome stretches; labels and right controls stay 1:1.
      let splitX = anchor.map { $0.convert($0.bounds, to: view).maxX - rect.minX + 1 }
      var frozenContent: NSImage?
      if capturesContent {
        let body = NSRect(
          x: view.bounds.minX,
          y: view.isFlipped ? rect.maxY : view.bounds.minY,
          width: view.bounds.width, height: max(0, view.bounds.height - height))
        if !body.isEmpty, let bitmap = view.bitmapImageRepForCachingDisplay(in: body) {
          view.cacheDisplay(in: body, to: bitmap)
          if let image = bitmap.cgImage {
            frozenContent = NSImage(cgImage: image, size: body.size)
          }
        }
      }
      return OverlayResizePreviewView.Header(
        image: NSImage(cgImage: image, size: rect.size),
        splitX: min(rect.width, max(0, splitX ?? rect.width)), content: frozenContent)
    }
    drawing = OverlayResizePreviewView(
      headers: headers, group: group, headerHeight: headerHeight, palette: palette,
      cornerRadius: cornerRadius)
    panel = NSPanel(
      contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = false
    panel.ignoresMouseEvents = true
    panel.hidesOnDeactivate = false
    panel.level =
      windows.map(\.level).max(by: { $0.rawValue < $1.rawValue })
      ?? OverlayIntercomPanel.passiveLevel
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
    panel.contentView = drawing
    for source in sources {
      if freezesMouse { source.window.ignoresMouseEvents = true }
      source.window.alphaValue = 0
    }
  }

  func update(frame: CGRect) {
    guard !finished else { return }
    self.frame = frame
    panel.setFrame(frame, display: true)
    drawing.needsDisplay = true
    if !panel.isVisible { panel.orderFrontRegardless() }
  }

  func finish() {
    guard !finished else { return }
    finished = true
    for source in sources {
      restore(source.window)
      source.window.contentView?.layoutSubtreeIfNeeded()
      source.window.alphaValue = source.alpha
      if freezesMouse { source.window.ignoresMouseEvents = source.ignoresMouseEvents }
    }
    // Settlement is atomic. An old fade must never dim the next gesture or
    // leave a second header over newly interactive controls.
    panel.close()
  }
}

final class OverlayResizePreviewView: NSView {
  struct Header {
    let image: NSImage
    let splitX: CGFloat
    var content: NSImage? = nil
  }
  let headers: [Header?]
  let group: Bool
  let headerHeight: CGFloat
  let palette: OverlayAppearancePalette
  let cornerRadius: CGFloat

  init(
    headers: [Header?], group: Bool, headerHeight: CGFloat,
    palette: OverlayAppearancePalette, cornerRadius: CGFloat
  ) {
    self.headers = headers
    self.group = group
    self.headerHeight = headerHeight
    self.palette = palette
    self.cornerRadius = cornerRadius
    super.init(frame: .zero)
  }
  required init?(coder: NSCoder) { return nil }
  override var isOpaque: Bool { false }
  override func draw(_ dirtyRect: NSRect) {
    let count = max(0, headers.count - 1)
    let rowHeight =
      group && count > 0
      ? (bounds.height - headerHeight) / CGFloat(count) - OverlayIntercomWidgetLayout.gap
      : bounds.height
    var top = bounds.maxY
    for (index, header) in headers.enumerated() {
      let height = group && index == 0 ? headerHeight : rowHeight
      let rect = NSRect(x: bounds.minX, y: top - height, width: bounds.width, height: height)
      guard height > 0 else { continue }
      let path = NSBezierPath(
        roundedRect: rect.insetBy(dx: 1, dy: 1), xRadius: cornerRadius, yRadius: cornerRadius)
      palette.desktopBackground.nsColor.withAlphaComponent(0.92).setFill()
      path.fill()
      // The preview edge is chrome, not a voice state.
      palette.primaryText.nsColor.withAlphaComponent(0.45).setStroke()
      path.lineWidth = 1
      path.stroke()
      if let header {
        NSGraphicsContext.saveGraphicsState()
        path.addClip()
        if let content = header.content {
          NSGraphicsContext.saveGraphicsState()
          let body = NSRect(
            x: rect.minX, y: rect.minY, width: rect.width,
            height: max(0, rect.height - header.image.size.height))
          NSBezierPath(rect: body).addClip()
          // A gesture repaints this bitmap only. Anchor at the top-left and
          // clip at the new edges: never rescale text or relayout Markdown.
          content.draw(
            in: NSRect(
              x: body.minX, y: body.maxY - content.size.height,
              width: content.size.width, height: content.size.height),
            from: .zero, operation: .sourceOver, fraction: 1)
          NSGraphicsContext.restoreGraphicsState()
        }
        let image = header.image
        let rightWidth = image.size.width - header.splitX
        let rightX = rect.maxX - rightWidth
        let y = rect.maxY - image.size.height
        // Extend the empty gutter, not glyphs. The captured trailing fragment
        // includes the original right inset and follows the new frame exactly.
        if rightX > rect.minX + header.splitX, header.splitX >= 1 {
          image.draw(
            in: NSRect(
              x: rect.minX + header.splitX, y: y,
              width: rightX - rect.minX - header.splitX, height: image.size.height),
            from: NSRect(x: header.splitX - 1, y: 0, width: 1, height: image.size.height),
            operation: .sourceOver, fraction: 1)
        }
        let leftWidth = min(header.splitX, max(0, rightX - rect.minX))
        if leftWidth > 0 {
          image.draw(
            in: NSRect(x: rect.minX, y: y, width: leftWidth, height: image.size.height),
            from: NSRect(x: 0, y: 0, width: leftWidth, height: image.size.height),
            operation: .sourceOver, fraction: 1)
        }
        if rightWidth > 0 {
          image.draw(
            in: NSRect(x: rightX, y: y, width: rightWidth, height: image.size.height),
            from: NSRect(x: header.splitX, y: 0, width: rightWidth, height: image.size.height),
            operation: .sourceOver, fraction: 1)
        }
        NSGraphicsContext.restoreGraphicsState()
      }
      top = rect.minY - OverlayIntercomWidgetLayout.gap
    }
  }
}
