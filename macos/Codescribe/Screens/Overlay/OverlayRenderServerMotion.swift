import AppKit
import SwiftUI

/// Decorative loops handed to the render server: the waveform's rule (UI-88)
/// applied to every repeating motion the overlay shows. A `TimelineView` at
/// 30 Hz re-evaluated its subtree and committed the window thirty times a
/// second for as long as it stayed mounted; measured on a take (2026-11-07,
/// Instruments on the installed a391 build): 53 commits/s from the main
/// thread, WindowServer at 90–96%, the pointer lagging across the desktop.
/// Here the main thread adds one Core Animation animation when the view
/// mounts and commits nothing per frame afterwards.
///
/// Geometry follows SwiftUI's conventions so call sites read like the views
/// they replace: fractions of a circle run clockwise from 3 o'clock, and a
/// positive rotation turns clockwise. The views are flipped to make it so.
enum OverlayRenderServerMotion {
  static let spinKey = "overlay.render-server.spin"
  static let pulseKey = "overlay.render-server.pulse"

  /// One clockwise turn per `period`, starting `phase` radians in.
  static func spin(period: TimeInterval, phase: Double) -> CABasicAnimation {
    let spin = CABasicAnimation(keyPath: "transform.rotation.z")
    spin.fromValue = phase
    spin.toValue = phase + 2 * Double.pi
    spin.duration = max(period, 0.05)
    spin.repeatCount = .infinity
    spin.isRemovedOnCompletion = false
    return spin
  }

  /// Opacity breathing from 1 down to `floor` and back over `period`. An
  /// ease on each half is the render server's raised cosine.
  static func pulse(period: TimeInterval, floor: Double, phase: TimeInterval) -> CABasicAnimation {
    let pulse = CABasicAnimation(keyPath: "opacity")
    pulse.fromValue = 1
    pulse.toValue = floor
    pulse.duration = max(period, 0.1) / 2
    pulse.autoreverses = true
    pulse.repeatCount = .infinity
    pulse.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
    pulse.timeOffset = phase
    pulse.isRemovedOnCompletion = false
    return pulse
  }
}

/// A stroked arc of a circle, spun by the render server. Replaces
/// `Circle().trim(from:to:).stroke(…).rotationEffect(…)` under a timeline.
struct OverlayRenderServerArc: NSViewRepresentable {
  /// Fractions of the circle, clockwise from 3 o'clock, like `trim(from:to:)`.
  let from: CGFloat
  let to: CGFloat
  let lineWidth: CGFloat
  let color: Color
  /// Seconds per turn.
  let period: TimeInterval
  /// Clockwise radians at mount, so a remount continues the turn in progress.
  let phase: Double
  var accessibilityIdentifier: String? = nil

  func makeNSView(context: Context) -> ArcView {
    let view = ArcView()
    if let accessibilityIdentifier { view.setAccessibilityIdentifier(accessibilityIdentifier) }
    view.apply(self)
    return view
  }

  func updateNSView(_ view: ArcView, context: Context) {
    view.apply(self)
  }

  final class ArcView: NSView {
    private let arc = CAShapeLayer()
    private var drawn: (from: CGFloat, to: CGFloat, lineWidth: CGFloat, size: CGSize)?
    private var spinning: (period: TimeInterval, phase: Double)?

    override init(frame: NSRect) {
      super.init(frame: frame)
      wantsLayer = true
      arc.fillColor = nil
      arc.lineCap = .round
      arc.anchorPoint = CGPoint(x: 0.5, y: 0.5)
      layer?.addSublayer(arc)
    }

    @available(*, unavailable) required init?(coder: NSCoder) { nil }

    /// SwiftUI's y-down space: `clockwise: false` arcs and positive rotations
    /// both read clockwise on screen, as the views this replaces drew them.
    override var isFlipped: Bool { true }

    /// Decoration only; the button or row beneath keeps every click.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func apply(_ model: OverlayRenderServerArc) {
      arc.strokeColor = NSColor(model.color).cgColor
      arc.lineWidth = model.lineWidth
      rebuildPath(from: model.from, to: model.to, lineWidth: model.lineWidth)
      if spinning?.period != model.period || arc.animation(forKey: OverlayRenderServerMotion.spinKey) == nil {
        arc.add(
          OverlayRenderServerMotion.spin(period: model.period, phase: model.phase),
          forKey: OverlayRenderServerMotion.spinKey)
        spinning = (model.period, model.phase)
      }
    }

    override func layout() {
      super.layout()
      arc.bounds = CGRect(origin: .zero, size: bounds.size)
      arc.position = CGPoint(x: bounds.midX, y: bounds.midY)
      if let drawn, drawn.size != bounds.size {
        rebuildPath(from: drawn.from, to: drawn.to, lineWidth: drawn.lineWidth)
      }
    }

    override func viewDidChangeBackingProperties() {
      super.viewDidChangeBackingProperties()
      arc.contentsScale = window?.backingScaleFactor ?? 2
    }

    private func rebuildPath(from: CGFloat, to: CGFloat, lineWidth: CGFloat) {
      let size = bounds.size
      if let drawn, drawn.from == from, drawn.to == to, drawn.lineWidth == lineWidth,
        drawn.size == size
      {
        return
      }
      drawn = (from, to, lineWidth, size)
      let radius = max(0, min(size.width, size.height) / 2 - lineWidth / 2)
      let path = CGMutablePath()
      path.addArc(
        center: CGPoint(x: size.width / 2, y: size.height / 2), radius: radius,
        startAngle: from * 2 * .pi, endAngle: to * 2 * .pi, clockwise: false)
      arc.path = path
    }
  }
}

/// `content` breathing between full and `floor` opacity on the render server.
/// Replaces `.opacity(pulse(at: timeline.date))` under a timeline.
struct OverlayRenderServerPulse<Content: View>: NSViewRepresentable {
  let period: TimeInterval
  let floor: Double
  /// Seconds into the cycle at mount, so a remount does not restart the breath.
  let phase: TimeInterval
  @ViewBuilder let content: () -> Content

  func makeNSView(context: Context) -> PulseView {
    let view = PulseView(rootView: AnyView(content()))
    view.apply(period: period, floor: floor, phase: phase)
    return view
  }

  func updateNSView(_ view: PulseView, context: Context) {
    view.hosting.rootView = AnyView(content())
    view.apply(period: period, floor: floor, phase: phase)
  }

  final class PulseView: NSView {
    let hosting: NSHostingView<AnyView>
    private var breathing: TimeInterval?

    init(rootView: AnyView) {
      hosting = NSHostingView(rootView: rootView)
      super.init(frame: .zero)
      wantsLayer = true
      hosting.sizingOptions = []
      hosting.autoresizingMask = [.width, .height]
      hosting.frame = bounds
      addSubview(hosting)
    }

    @available(*, unavailable) required init?(coder: NSCoder) { nil }

    override var intrinsicContentSize: NSSize { hosting.fittingSize }

    /// Decoration only; the control beneath keeps every click.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func apply(period: TimeInterval, floor: Double, phase: TimeInterval) {
      guard let layer else { return }
      if breathing != period || layer.animation(forKey: OverlayRenderServerMotion.pulseKey) == nil {
        layer.add(
          OverlayRenderServerMotion.pulse(period: period, floor: floor, phase: phase),
          forKey: OverlayRenderServerMotion.pulseKey)
        breathing = period
      }
    }
  }
}
