import AppKit
import ImageIO
import PaperKit
import SwiftUI

/// Nightly builds open the attachment preview as its own resizable window with
/// system Markup. Stable keeps the fixed sheet: with the gate off, every
/// presenter takes its original `.sheet` branch unchanged.
enum AttachmentPreviewPresentation {
  static func usesWindow(in bundle: Bundle = .main) -> Bool {
    BuildChannel.isNightly(in: bundle)
  }
}

/// The preview window. Non-activating like the overlay it opens from, so the
/// app the user is working in keeps its activation, but it can become key:
/// Markup text boxes need the keyboard.
@MainActor
final class AttachmentPreviewPanel: NSPanel {
  override var canBecomeKey: Bool { true }
  override var canBecomeMain: Bool { true }
}

/// Owns one preview window per attachment path. A second click on the same
/// attachment brings its window forward instead of opening another one.
@MainActor
final class AttachmentPreviewWindowController: NSObject, NSWindowDelegate {
  static let defaultContentSize = NSSize(width: 760, height: 720)
  static let minimumContentSize = NSSize(width: 520, height: 420)

  private static var openWindows: [String: AttachmentPreviewWindowController] = [:]

  /// Opens (or focuses) the preview window above `parent`.
  /// - Parameters:
  ///   - onRemove: removes the attachment the window currently shows.
  ///   - onAnnotated: repoints the attachment from the shown file to the
  ///     annotated copy. Markup is offered only when this is present, so a
  ///     sent message (nothing left to repoint) previews without it.
  static func present(
    _ attachment: MessageAttachment,
    from parent: NSWindow? = nil,
    onRemove: ((_ current: URL?) -> Void)? = nil,
    onAnnotated: ((_ current: URL, _ annotated: URL) -> Void)? = nil
  ) {
    let key = registryKey(for: attachment)
    if let existing = openWindows[key] {
      existing.panel.makeKeyAndOrderFront(nil)
      return
    }
    let controller = AttachmentPreviewWindowController(
      attachment: attachment, onRemove: onRemove, onAnnotated: onAnnotated)
    openWindows[key] = controller
    // The click that asked for the preview names the chat window it came from.
    controller.show(above: parent ?? NSApp.currentEvent?.window ?? NSApp.keyWindow)
  }

  static func registryKey(for attachment: MessageAttachment) -> String {
    attachment.url?.standardizedFileURL.path ?? "name:\(attachment.name)"
  }

  private let panel: AttachmentPreviewPanel
  private var attachment: MessageAttachment
  private let onRemove: ((URL?) -> Void)?
  private let onAnnotated: ((URL, URL) -> Void)?

  private init(
    attachment: MessageAttachment,
    onRemove: ((URL?) -> Void)?,
    onAnnotated: ((URL, URL) -> Void)?
  ) {
    self.attachment = attachment
    self.onRemove = onRemove
    self.onAnnotated = onAnnotated
    panel = AttachmentPreviewPanel(
      contentRect: NSRect(origin: .zero, size: Self.defaultContentSize),
      styleMask: [.titled, .closable, .miniaturizable, .resizable, .nonactivatingPanel],
      backing: .buffered,
      defer: false)
    super.init()
    panel.isReleasedWhenClosed = false
    panel.hidesOnDeactivate = false
    panel.isFloatingPanel = true
    panel.becomesKeyOnlyIfNeeded = false
    panel.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
    panel.contentMinSize = Self.minimumContentSize
    panel.tabbingMode = .disallowed
    panel.delegate = self
    panel.identifier = NSUserInterfaceItemIdentifier("attachment-preview-window")
    render()
  }

  private func render() {
    panel.title = attachment.name
    panel.representedURL = attachment.url
    var sheet = AttachmentPreviewSheet(attachment: attachment)
    if let onRemove {
      // The sheet closes itself through `onClose` after removing.
      sheet.onRemove = { [weak self] in onRemove(self?.attachment.url) }
    }
    sheet.onClose = { [weak self] in self?.panel.close() }
    if let onAnnotated, let current = attachment.url {
      sheet.onAnnotated = { [weak self] annotated in
        onAnnotated(current, annotated)
        self?.repoint(to: annotated)
      }
    }
    // A new identity per file: the hosted sheet reloads its image and drops
    // the finished Markup session instead of keeping the previous state.
    let root = sheet.id(Self.registryKey(for: attachment))
    if let hosting = panel.contentViewController as? NSHostingController<AnyView> {
      hosting.rootView = AnyView(root)
    } else {
      let hosting = NSHostingController(rootView: AnyView(root))
      hosting.sizingOptions = []
      panel.contentViewController = hosting
      panel.setContentSize(Self.defaultContentSize)
    }
  }

  private func repoint(to annotated: URL) {
    let oldKey = Self.registryKey(for: attachment)
    attachment = MessageAttachment(name: annotated.lastPathComponent, url: annotated)
    Self.openWindows[oldKey] = nil
    Self.openWindows[Self.registryKey(for: attachment)] = self
    render()
  }

  private func show(above parent: NSWindow?) {
    // Above the chat: never below the window it was opened from, and at least
    // floating so the overlay (itself floating) does not cover it.
    let parentLevel = parent?.level ?? .floating
    panel.level = parentLevel.rawValue > NSWindow.Level.floating.rawValue ? parentLevel : .floating
    if let parent, let screen = parent.screen ?? NSScreen.main {
      let size = panel.frame.size
      let visible = screen.visibleFrame
      var origin = NSPoint(
        x: parent.frame.midX - size.width / 2,
        y: parent.frame.midY - size.height / 2)
      origin.x = min(max(origin.x, visible.minX), visible.maxX - size.width)
      origin.y = min(max(origin.y, visible.minY), visible.maxY - size.height)
      panel.setFrameOrigin(origin)
    } else {
      panel.center()
    }
    panel.makeKeyAndOrderFront(nil)
  }

  func windowWillClose(_ notification: Notification) {
    let key = Self.registryKey(for: attachment)
    if Self.openWindows[key] === self { Self.openWindows[key] = nil }
  }
}

// MARK: - Markup

/// One Markup pass over an image: the system PaperKit editor and toolbar over
/// the untouched original, flattened to a PNG on save.
@MainActor
final class AttachmentMarkupSession {
  let image: CGImage
  /// Image pixels; the markup canvas, the editor's content view and the
  /// flattened PNG all share this coordinate space.
  let canvas: CGRect
  let paper: PaperMarkupViewController
  let toolbar: MarkupToolbarViewController
  private let initialMarkup: PaperMarkup

  init?(url: URL) {
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
      let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
      image.width > 0, image.height > 0
    else { return nil }
    let canvas = CGRect(x: 0, y: 0, width: image.width, height: image.height)
    let initialMarkup = PaperMarkup(bounds: canvas)
    let paper = PaperMarkupViewController(markup: initialMarkup, supportedFeatureSet: .latest)
    let toolbar = MarkupToolbarViewController(supportedFeatureSet: .latest)
    self.image = image
    self.canvas = canvas
    self.initialMarkup = initialMarkup
    self.paper = paper
    self.toolbar = toolbar
    toolbar.delegate = paper

    // The original sits under the markup as content, not as a markup
    // element, so no tool can move or delete the screenshot itself.
    let imageView = NSImageView(frame: canvas)
    imageView.image = NSImage(cgImage: image, size: canvas.size)
    imageView.imageScaling = .scaleAxesIndependently
    paper.contentView = imageView
  }

  var hasChanges: Bool {
    (paper.markup ?? initialMarkup) != initialMarkup
  }

  /// The original pixels with the markup drawn on top, encoded as PNG.
  func flattenedPNG() async -> Data? {
    let markup = paper.markup ?? initialMarkup
    guard let space = CGColorSpace(name: CGColorSpace.sRGB),
      let context = CGContext(
        data: nil,
        width: image.width,
        height: image.height,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: space,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
    else { return nil }
    context.draw(image, in: canvas)
    // The canvas is a screenshot, not app chrome: render light-style markup.
    await markup.draw(
      in: context, frame: canvas, options: RenderingOptions(darkUserInterfaceStyle: false))
    guard let flattened = context.makeImage() else { return nil }
    return NSBitmapImageRep(cgImage: flattened).representation(using: .png, properties: [:])
  }
}

/// Hosts the system Markup toolbar above the PaperKit editor.
struct AttachmentMarkupEditor: NSViewControllerRepresentable {
  let session: AttachmentMarkupSession

  func makeNSViewController(context: Context) -> AttachmentMarkupContainerController {
    AttachmentMarkupContainerController(session: session)
  }

  func updateNSViewController(
    _ controller: AttachmentMarkupContainerController, context: Context
  ) {}
}

@MainActor
final class AttachmentMarkupContainerController: NSViewController {
  private let session: AttachmentMarkupSession

  init(session: AttachmentMarkupSession) {
    self.session = session
    super.init(nibName: nil, bundle: nil)
  }

  @available(*, unavailable)
  required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

  override func loadView() {
    let root = NSView()
    addChild(session.toolbar)
    addChild(session.paper)
    let toolbarView = session.toolbar.view
    let paperView = session.paper.view
    toolbarView.translatesAutoresizingMaskIntoConstraints = false
    paperView.translatesAutoresizingMaskIntoConstraints = false
    root.addSubview(paperView)
    root.addSubview(toolbarView)
    NSLayoutConstraint.activate([
      toolbarView.topAnchor.constraint(equalTo: root.topAnchor),
      toolbarView.centerXAnchor.constraint(equalTo: root.centerXAnchor),
      toolbarView.leadingAnchor.constraint(greaterThanOrEqualTo: root.leadingAnchor),
      toolbarView.trailingAnchor.constraint(lessThanOrEqualTo: root.trailingAnchor),
      paperView.topAnchor.constraint(equalTo: toolbarView.bottomAnchor, constant: 6),
      paperView.leadingAnchor.constraint(equalTo: root.leadingAnchor),
      paperView.trailingAnchor.constraint(equalTo: root.trailingAnchor),
      paperView.bottomAnchor.constraint(equalTo: root.bottomAnchor),
    ])
    view = root
  }

  override func viewDidAppear() {
    super.viewDidAppear()
    // PaperKit takes keys on the controller (it overrides
    // `acceptsFirstResponder`), not on its view.
    view.window?.makeFirstResponder(session.paper)
  }
}
