import AppKit
import SwiftUI

/// One hover hue for every header control: the listening terracotta.
/// Transcription, Agents, Pin and presentation share it. Role does not pick
/// olive, violet or a second accent. The microphone keeps its recording tint
/// and only borrows this hue for the hover ring.
enum OverlayHeaderAccent {
  case transcription
  case agents
  case state

  func color(in palette: OverlayAppearancePalette) -> Color {
    switch self {
    case .transcription, .agents, .state:
      palette.listeningStatus.color
    }
  }
}

extension EnvironmentValues {
  /// Set by `OverlayHeaderControlChrome` while the pointer is inside a control
  /// that has an accent. Nil at rest, and nil for hint-only controls.
  @Entry var overlayHeaderHoverTint: Color? = nil
}

/// Resting header marks stay on their neutral tint. A hovered accent replaces
/// that tint in the descendant, which an outer `foregroundStyle` cannot do
/// once the mark has painted its own color.
struct OverlayHeaderHoverForeground: ViewModifier {
  @Environment(\.overlayHeaderHoverTint) private var hoverTint
  let resting: Color

  func body(content: Content) -> some View {
    content.foregroundStyle(hoverTint ?? resting)
  }
}

/// Hover chrome of a header control: a hairline ring in the control's accent
/// and the action pill's hint. In mini and compact form the header is the
/// whole 46 pt window, so a hint cannot sit inside the panel the way the
/// pill's sits above the canvas; it floats just below the control in a
/// click-through child window of the overlay panel. A nil accent keeps the
/// hint without the ring (the close dot has its own hover).
struct OverlayHeaderControlChrome: ViewModifier {
  @Environment(\.displayScale) private var displayScale
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  let title: String
  let accent: OverlayHeaderAccent?
  let palette: OverlayAppearancePalette
  @State private var hovered = false
  @State private var anchor = OverlayHintAnchor()

  func body(content: Content) -> some View {
    let hoverTint: Color? = hovered ? accent?.color(in: palette) : nil
    content
      .environment(\.overlayHeaderHoverTint, hoverTint)
      .background {
        if let hoverTint {
          Circle()
            .fill(hoverTint.opacity(0.10))
            .frame(width: OverlayMicrophoneGlyph.diameter, height: OverlayMicrophoneGlyph.diameter)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
      }
      .overlay {
        if let hoverTint {
          Circle()
            .strokeBorder(
              hoverTint,
              lineWidth: 1 / max(displayScale, 1)
            )
            .frame(width: OverlayMicrophoneGlyph.diameter, height: OverlayMicrophoneGlyph.diameter)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
      }
      .background { OverlayHintAnchorView(anchor: anchor) }
      .background { OverlayMenuCursorRegion() }
      .onHover { inside in
        hovered = inside
        if inside { anchor.show(title, palette: palette) } else { anchor.hide() }
      }
      .onChange(of: title) { _, title in
        if hovered { anchor.show(title, palette: palette) }
      }
      .onDisappear { anchor.hide() }
      .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: hovered)
  }
}

/// The control's AppKit position for its floating hint.
@MainActor
final class OverlayHintAnchor {
  weak var view: NSView?

  func show(_ title: String, palette: OverlayAppearancePalette) {
    guard let view else { return }
    OverlayHintPanel.shared.show(title, palette: palette, below: view, owner: self)
  }

  func hide() {
    OverlayHintPanel.shared.hide(owner: self)
  }
}

private struct OverlayHintAnchorView: NSViewRepresentable {
  let anchor: OverlayHintAnchor

  func makeNSView(context: Context) -> OverlayHintAnchorNSView {
    let view = OverlayHintAnchorNSView()
    anchor.view = view
    return view
  }

  func updateNSView(_ nsView: OverlayHintAnchorNSView, context: Context) {
    anchor.view = nsView
  }
}

/// Geometry only: never a click, drag or cursor target.
final class OverlayHintAnchorNSView: NSView {
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
  override func isAccessibilityElement() -> Bool { false }
}

/// One click-through hint window for the header, owned by the hovered control.
/// As a child window of the overlay panel it moves with a drag and leaves when
/// the panel is ordered out, so no hint outlives its header.
@MainActor
final class OverlayHintPanel {
  static let shared = OverlayHintPanel()
  /// Distance between the control and its hint.
  static let gap: CGFloat = 6
  /// Distance kept from the screen's visible edges.
  static let screenInset: CGFloat = 4

  private var panel: NSPanel?
  private var controller: NSHostingController<OverlayTooltipChip>?
  private weak var owner: OverlayHintAnchor?

  /// The window currently presenting a hint, for tests and diagnostics.
  var presentedPanel: NSPanel? { panel?.isVisible == true ? panel : nil }

  func show(
    _ title: String, palette: OverlayAppearancePalette, below anchor: NSView,
    owner: OverlayHintAnchor
  ) {
    guard let parent = anchor.window, parent.isVisible else { return }
    let chip = OverlayTooltipChip(title: title, palette: palette, filled: false)
    let (panel, controller) = presentation()
    controller.rootView = chip
    let size = controller.sizeThatFits(
      in: NSSize(width: OverlayTooltipLayout.maxHintWidth, height: 1_000))
    let anchorRect = parent.convertToScreen(anchor.convert(anchor.bounds, to: nil))
    let visible = parent.screen?.visibleFrame ?? NSScreen.main?.visibleFrame ?? parent.frame
    panel.appearance = NSAppearance(named: palette.appearance == .dark ? .darkAqua : .aqua)
    panel.setFrame(Self.frame(size: size, below: anchorRect, within: visible), display: true)
    if panel.parent !== parent {
      panel.parent?.removeChildWindow(panel)
      parent.addChildWindow(panel, ordered: .above)
    }
    panel.orderFront(nil)
    self.owner = owner
  }

  func hide(owner: OverlayHintAnchor) {
    guard self.owner === owner else { return }
    self.owner = nil
    guard let panel else { return }
    panel.parent?.removeChildWindow(panel)
    panel.orderOut(nil)
  }

  /// Below the control and centered on it; above it when the screen ends
  /// first; always inside the screen's visible width.
  static func frame(size: CGSize, below anchor: NSRect, within visible: NSRect) -> NSRect {
    var origin = NSPoint(x: anchor.midX - size.width / 2, y: anchor.minY - gap - size.height)
    if origin.y < visible.minY + screenInset { origin.y = anchor.maxY + gap }
    origin.x = min(
      max(origin.x, visible.minX + screenInset), visible.maxX - screenInset - size.width)
    return NSRect(origin: origin, size: size)
  }

  private func presentation() -> (NSPanel, NSHostingController<OverlayTooltipChip>) {
    if let panel, let controller { return (panel, controller) }
    let panel = NSPanel(
      contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered,
      defer: true)
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = true
    panel.ignoresMouseEvents = true
    panel.isReleasedWhenClosed = false
    panel.hidesOnDeactivate = false
    panel.animationBehavior = .none
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient, .ignoresCycle]
    panel.setAccessibilityElement(false)

    // The overlay's own glass is not behind a separate window; the system
    // tooltip material blurs the desktop under the chip instead.
    let effect = NSVisualEffectView()
    effect.material = .toolTip
    effect.blendingMode = .behindWindow
    effect.state = .active
    effect.maskImage = Self.roundedMask(radius: CSRadius.chip)
    let controller = NSHostingController(
      rootView: OverlayTooltipChip(title: "", palette: .light, filled: false))
    controller.view.frame = effect.bounds
    controller.view.autoresizingMask = [.width, .height]
    effect.addSubview(controller.view)
    panel.contentView = effect
    self.panel = panel
    self.controller = controller
    return (panel, controller)
  }

  private static func roundedMask(radius: CGFloat) -> NSImage {
    let edge = radius * 2 + 1
    let image = NSImage(size: NSSize(width: edge, height: edge), flipped: false) { rect in
      NSColor.black.setFill()
      NSBezierPath(roundedRect: rect, xRadius: radius, yRadius: radius).fill()
      return true
    }
    image.capInsets = NSEdgeInsets(top: radius, left: radius, bottom: radius, right: radius)
    image.resizingMode = .stretch
    return image
  }
}
