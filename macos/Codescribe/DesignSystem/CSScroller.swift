import AppKit
import SwiftUI

/// Which scrollers a surface owns.
///
/// A vertical list has no sideways axis at all, so its horizontal scroller is
/// removed outright — the way the overlay lists have always been pinned. A
/// strip that really does scroll sideways keeps every scroller it was built
/// with and only has the style pinned, so the policy can never take a
/// surface's scrolling away from it.
enum CSScrollerAxes {
  case verticalOnly
  case includesHorizontal
}

/// The app's one scroller policy: no legacy bar anywhere, at most a thin
/// overlay band that fades in while scrolling and out after it (Founder,
/// 2026-10-18).
///
/// SwiftUI cannot deliver that on macOS. `.scrollIndicators(.hidden)` and
/// `showsIndicators: false` only choose between hidden and *the style the
/// system "Show scroll bars" preference picked*, and with a mouse attached
/// that style is the legacy one: a thick bar standing down the edge for as
/// long as the surface exists. Only `NSScrollView` knows the difference, so
/// the policy is pinned there — and pinned again whenever the preference
/// changes, because AppKit re-reads it and hands the scroll view its legacy
/// scroller straight back.
///
/// One owner for every scroll surface in the app: SwiftUI panes and lists
/// reach it through `csAutohidingScrollers()`, and the hand-built
/// `NSScrollView`s (live transcript, both composers, the oversized-text
/// bubble) hold a pin of their own.
@MainActor
final class CSScrollerPin {
  private let axes: CSScrollerAxes
  private weak var scrollView: NSScrollView?
  private var styleObserver: NSObjectProtocol?

  init(axes: CSScrollerAxes = .verticalOnly) {
    self.axes = axes
  }

  /// Pins `scrollView` now and keeps it pinned until `detach()`.
  ///
  /// SwiftUI calls this again on every update of the surface. Attaching to
  /// the scroll view already held does nothing: the observer keeps the pin,
  /// and a fresh write would re-tile the scroll view.
  func attach(to scrollView: NSScrollView) {
    guard self.scrollView !== scrollView else { return }
    self.scrollView = scrollView
    pin()
    guard styleObserver == nil else { return }
    // The preference notification carries no object, so this one is global.
    styleObserver = NotificationCenter.default.addObserver(
      forName: NSScroller.preferredScrollerStyleDidChangeNotification,
      object: nil,
      queue: nil
    ) { [weak self] _ in
      MainActor.assumeIsolated {
        self?.pin()
      }
    }
  }

  func detach() {
    if let styleObserver { NotificationCenter.default.removeObserver(styleObserver) }
    styleObserver = nil
    scrollView = nil
  }

  /// Writes only what differs. Each scroller property setter re-tiles the
  /// scroll view, and SwiftUI re-runs the attachment on every update of the
  /// surface; an unchanged pin has no reason to re-tile anything.
  private func pin() {
    guard let scrollView else { return }
    if scrollView.scrollerStyle != .overlay { scrollView.scrollerStyle = .overlay }
    if !scrollView.autohidesScrollers { scrollView.autohidesScrollers = true }
    if axes == .verticalOnly, scrollView.hasHorizontalScroller {
      scrollView.hasHorizontalScroller = false
    }
  }
}

extension View {
  /// Puts this scroll surface on the app's scroller policy.
  ///
  /// Place it on the CONTENT inside the `ScrollView`, never on the scroll
  /// view itself: the attachment finds its `enclosingScrollView`, and a
  /// background applied to the scroll view lands beside that view rather than
  /// inside it. A `List` draws its rows inside its own scroll view, so there it
  /// goes on each row's content: rows are built lazily, and whichever rows are
  /// on screen hold the pin, where a single anchor row would be recycled as
  /// soon as it scrolled away. An extra zero-height row would also take the
  /// list's minimum row height and open a gap above the first section.
  func csAutohidingScrollers(_ axes: CSScrollerAxes = .verticalOnly) -> some View {
    background(CSScrollerStyleAttachment(axes: axes))
  }
}

/// Attaches inside the document view, so `enclosingScrollView` resolves to the
/// surface it was placed in rather than an outer pane — the way the chat's
/// live-scroll observer finds its own scroll view.
private struct CSScrollerStyleAttachment: NSViewRepresentable {
  let axes: CSScrollerAxes

  func makeCoordinator() -> CSScrollerPin { CSScrollerPin(axes: axes) }

  func makeNSView(context: Context) -> AttachmentView {
    let view = AttachmentView()
    view.onAttach = { [weak pin = context.coordinator] scrollView in
      pin?.attach(to: scrollView)
    }
    return view
  }

  func updateNSView(_ nsView: AttachmentView, context: Context) {
    nsView.attachWhenReady()
  }

  static func dismantleNSView(_ nsView: AttachmentView, coordinator: CSScrollerPin) {
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
}
