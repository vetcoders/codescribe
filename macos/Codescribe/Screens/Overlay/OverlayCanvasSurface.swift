import AppKit
import SwiftUI

/// Appearance-aware physical sheet. Window interaction is owned by explicit
/// inert regions in `DictationOverlayView`, not by a full-sheet AppKit layer.
struct OverlayCanvasSurface<Content: View>: View {
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.colorSchemeContrast) private var contrast
  let palette: OverlayAppearancePalette
  @ViewBuilder let content: Content

  var body: some View {
    content
      .background {
        OverlayCanvasBackdrop(
          palette: palette, reduceTransparency: reduceTransparency,
          increasedContrast: contrast == .increased)
      }
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.window, style: .continuous))
      .overlay {
        RoundedRectangle(cornerRadius: CSRadius.window, style: .continuous)
          .strokeBorder(palette.border.color, lineWidth: 1)
          .allowsHitTesting(false)
      }
      .shadow(
        color: .black.opacity(palette.shadowOpacity),
        radius: 20,
        x: 0,
        y: 9
      )
  }

}

struct OverlayCanvasBackdrop: View {
  let palette: OverlayAppearancePalette
  let reduceTransparency: Bool
  var increasedContrast = false

  var body: some View {
    ZStack {
      if reduceTransparency || increasedContrast {
        Rectangle().fill(palette.desktopBackground.color)
      } else {
        OverlayDesktopMaterial(palette: palette)
        Rectangle()
          .fill(Color(nsColor: Self.glassVeil(palette)))
          .allowsHitTesting(false)
          .accessibilityHidden(true)
      }
    }
  }

  /// The palette's own sheet colour over the shared glass: near-black on ink,
  /// paper on paper, with the desktop left as a faint blurred hint (Maciej,
  /// 2026-10-15: "bardziej czarno", "dark almost black"). The 22 % veil of
  /// 2026-10-14 still took the page's brightness; the macOS Liquid Glass slider
  /// does not darken it.
  ///
  /// The veil is a SwiftUI layer above the glass, not the glass's `tintColor`:
  /// Liquid Glass drops its tint and brightens when the window is not key, so a
  /// tinted sheet read #09 active and #50 inactive on the same wizard (Maciej's
  /// screenshots, 2026-10-15 17:36). A layer above the glass keeps 84 % of the
  /// sheet constant in both states; only the 16 % of desktop hint still follows
  /// the window.
  static func glassVeil(_ palette: OverlayAppearancePalette) -> NSColor {
    palette.desktopBackground.nsColor.withAlphaComponent(0.84)
  }
}

/// Sample the desktop behind the non-activating panel, not its empty content.
/// Keep the material active while the user types into another application.
private struct OverlayDesktopMaterial: NSViewRepresentable {
  let palette: OverlayAppearancePalette

  func makeNSView(context: Context) -> NSView {
    if #available(macOS 26, *) {
      let view = OverlayDesktopGlassView()
      view.style = .regular
      view.cornerRadius = CSRadius.window
      applyAppearance(to: view)
      return view
    }
    let view = OverlayDesktopEffectView()
    view.material = .hudWindow
    view.blendingMode = .behindWindow
    view.state = .active
    applyAppearance(to: view)
    return view
  }

  func updateNSView(_ nsView: NSView, context: Context) {
    applyAppearance(to: nsView)
  }

  private func applyAppearance(to view: NSView) {
    let name: NSAppearance.Name = palette.appearance == .dark ? .darkAqua : .aqua
    if view.appearance?.name != name {
      view.appearance = NSAppearance(named: name)
    }
    if #available(macOS 26, *), let glass = view as? OverlayDesktopGlassView,
      glass.tintColor != nil
    {
      // The sheet colour lives in `OverlayCanvasBackdrop.glassVeil` above the
      // glass; a tint here would vanish whenever the window is not key.
      glass.tintColor = nil
    }
  }
}

/// One stable background, independent of recording, editing, and toolbelt state.
@available(macOS 26, *)
final class OverlayDesktopGlassView: NSGlassEffectView {
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

final class OverlayDesktopEffectView: NSVisualEffectView {
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

extension View {
  /// Overlay lists never wear a legacy scroller. SwiftUI's `.scrollIndicators`
  /// only chooses between hidden and the style the system "Show scroll bars"
  /// setting picked, so with a mouse attached a visible indicator is the thick
  /// bar that never goes away (Founder, 2026-10-18). The composer pins the
  /// overlay style on its own `NSScrollView`
  /// (`OverlayConversationComposer.makeNSView`); this does the same to the
  /// scroll view SwiftUI builds, so the scroller is a thin band that fades in
  /// while scrolling and fades out after it.
  func overlayAutohidingScroller() -> some View {
    background(OverlayScrollerStyle())
  }
}

/// Attaches inside the document view, so `enclosingScrollView` resolves to the
/// list it was placed in rather than an outer pane — the way the chat's
/// live-scroll observer finds its own scroll view.
private struct OverlayScrollerStyle: NSViewRepresentable {
  func makeCoordinator() -> Coordinator { Coordinator() }

  func makeNSView(context: Context) -> AttachmentView {
    let view = AttachmentView()
    view.onAttach = { [weak coordinator = context.coordinator] scrollView in
      coordinator?.attach(to: scrollView)
    }
    return view
  }

  func updateNSView(_ nsView: AttachmentView, context: Context) {
    nsView.attachWhenReady()
  }

  static func dismantleNSView(_ nsView: AttachmentView, coordinator: Coordinator) {
    coordinator.detach()
  }

  @MainActor
  final class AttachmentView: NSView {
    var onAttach: ((NSScrollView) -> Void)?

    /// The view exists to configure the scroll view around it; it spans the
    /// whole document and must not take a click away from the content.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      attachWhenReady()
    }

    /// The backing scroll view exists one layout pass after the representable
    /// is installed.
    func attachWhenReady() {
      Task { @MainActor [weak self] in
        await Task.yield()
        guard let self, let scrollView = enclosingScrollView else { return }
        onAttach?(scrollView)
      }
    }
  }

  @MainActor
  final class Coordinator {
    private weak var scrollView: NSScrollView?
    private var styleObserver: NSObjectProtocol?

    func attach(to scrollView: NSScrollView) {
      self.scrollView = scrollView
      pinOverlayScrollers()
      guard styleObserver == nil else { return }
      // AppKit re-reads the system preference whenever it changes and would
      // hand the list its legacy scroller straight back.
      styleObserver = NotificationCenter.default.addObserver(
        forName: NSScroller.preferredScrollerStyleDidChangeNotification,
        object: nil,
        queue: nil
      ) { [weak self] _ in
        MainActor.assumeIsolated {
          self?.pinOverlayScrollers()
        }
      }
    }

    func detach() {
      if let styleObserver { NotificationCenter.default.removeObserver(styleObserver) }
      styleObserver = nil
      scrollView = nil
    }

    private func pinOverlayScrollers() {
      guard let scrollView else { return }
      scrollView.scrollerStyle = .overlay
      scrollView.autohidesScrollers = true
      scrollView.hasHorizontalScroller = false
    }
  }
}

/// Header and composer use the same system-owned control surface.
///
/// A surface flush with the window's own edges must carry the window's corner,
/// not the pill's: the header spans the full width at the very top, so its
/// `CSRadius.pill` circular corner sat inside the canvas's `CSRadius.window`
/// continuous clip and the two curves read as a cut-off right end (Founder,
/// 2026-10-18). The composer floats inside the sheet and keeps the pill radius;
/// the corner style is `.continuous` for both, the way every other shape on
/// this canvas is drawn.
struct OverlayControlGlass: ViewModifier {
  var cornerRadius: CGFloat = CSRadius.pill

  @ViewBuilder
  func body(content: Content) -> some View {
    let shape = RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
    if #available(macOS 26.0, *) {
      content.glassEffect(.regular.interactive(), in: shape)
    } else {
      content.background(.regularMaterial, in: shape)
    }
  }
}
