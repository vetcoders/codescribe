import AppKit
import SwiftUI

/// Hover over an unchosen header control uses the macOS accent; a chosen
/// control wears its mode's colour (`selectionTint`).
enum OverlayHeaderAccent {
  case transcription
  case agents
  case state

  func color(in palette: OverlayAppearancePalette) -> Color {
    CSColor.chromeAccent
  }

  /// The chosen control wears its mode's colour. Founder, 2026-10-15: the
  /// choice shows as a "wyraźne (nie wyraziste)" tint of the mode's colour,
  /// Agents a light lavender from the agent recording light. Transcription
  /// takes the listening terracotta of its own recording light; Pin, which is
  /// not a mode, keeps the system accent.
  func selectionTint(in palette: OverlayAppearancePalette) -> Color {
    switch self {
    case .transcription: palette.listeningStatus.color
    case .agents: palette.agentAccent.color
    case .state: CSColor.chromeAccent
    }
  }

  /// The pastel wash under a chosen control, and the deeper one under the pointer.
  static let selectedWash = 0.3
  static let selectedHoverWash = 0.42
}

/// Hover chrome of a header control: stronger rendered ink, a hairline ring
/// in the control's accent and the action pill's hint. In mini and compact
/// form the header is the whole 46 pt window, so a hint cannot sit inside the
/// panel the way the pill's sits above the canvas; it floats just below the
/// control in a click-through child window. A nil accent keeps only the hint;
/// the close dot draws its own hover from `onHover`. A selected control
/// sits on a pastel wash of its mode's colour; the pointer deepens the wash
/// and draws the ring in that colour, so a chosen control still answers hover.
struct OverlayHeaderControlChrome: ViewModifier {
  @Environment(\.displayScale) private var displayScale
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.isEnabled) private var isEnabled
  let title: String
  let accent: OverlayHeaderAccent?
  let palette: OverlayAppearancePalette
  var selected = false
  var hintLeading = false
  var hintWithinWindow = false
  /// Header controls always hint. A reused anchor passes false while a popover
  /// is open or the control has no title, so the shared panel withdraws.
  var showsHint = true
  var hintContent: AnyView? = nil
  var hintSize: CGSize? = nil
  var hintDelay: TimeInterval = 0
  var hintInteractive = false
  /// The control's own hover look reads this one region, so a control never
  /// stacks a second pointer region over the same bounds.
  var onHover: ((Bool) -> Void)? = nil
  @State private var hovered = false
  @State private var anchor = OverlayHintAnchor()
  @State private var pendingHint: Task<Void, Never>?

  func body(content: Content) -> some View {
    let selection: Color? = selected ? accent?.selectionTint(in: palette) : nil
    let emphasizesInk = isEnabled && hovered && accent != nil
    let hoverTint: Color? =
      emphasizesInk ? (selection ?? accent?.color(in: palette).opacity(0.8)) : nil
    content
      // Glyphs own their semantic tint. Emphasize their rendered pixels so
      // even an explicit inner foregroundStyle responds, without replacing
      // terracotta, violet or status hues with a chrome colour. Nil-accent
      // tooltip anchors remain hint-only, including the disabled recorder.
      .brightness(emphasizesInk ? (palette.appearance == .dark ? 0.12 : -0.12) : 0)
      .background {
        if let selection {
          Circle()
            .fill(
              selection.opacity(
                emphasizesInk
                  ? OverlayHeaderAccent.selectedHoverWash : OverlayHeaderAccent.selectedWash)
            )
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
      .background {
        OverlayMenuCursorRegion { inside in
          hovered = inside
          onHover?(inside)
          syncHint(inside: inside)
        }
      }
      .onChange(of: title) { _, _ in syncHint(inside: hovered) }
      .onChange(of: showsHint) { _, _ in syncHint(inside: hovered) }
      .onChange(of: hintLeading) { _, _ in syncHint(inside: hovered) }
      .onChange(of: hintWithinWindow) { _, _ in syncHint(inside: hovered) }
      .onChange(of: hintSize) { _, _ in syncHint(inside: hovered) }
      .onChange(of: hintInteractive) { _, _ in syncHint(inside: hovered) }
      .onChange(of: palette) { _, _ in syncHint(inside: hovered) }
      .onDisappear {
        pendingHint?.cancel()
        anchor.hide()
      }
      .animation(reduceMotion ? nil : .easeOut(duration: CSMotion.Tempo.hover), value: hovered)
  }

  /// One owner: the shared panel. Leave, a title change, or `showsHint`
  /// becoming false withdraws only this anchor, so a neighbor that has since
  /// taken the panel stays on screen.
  private func syncHint(inside: Bool) {
    pendingHint?.cancel()
    pendingHint = nil
    guard showsHint, inside, !title.isEmpty else {
      anchor.hide(allowPointerRetention: showsHint && hintInteractive)
      return
    }
    if hintDelay > 0, !anchor.isPresented {
      pendingHint = Task { @MainActor in
        do {
          try await Task.sleep(for: .seconds(hintDelay))
        } catch {
          return
        }
        guard !Task.isCancelled else { return }
        presentHint()
      }
    } else {
      presentHint()
    }
  }

  private func presentHint() {
    anchor.show(
      title, palette: palette, leading: hintLeading, withinWindow: hintWithinWindow,
      content: hintContent, size: hintSize, interactive: hintInteractive)
  }
}

/// The control's AppKit position for its floating hint.
@MainActor
final class OverlayHintAnchor {
  weak var view: NSView?

  var isPresented: Bool { OverlayHintPanel.shared.isPresented(owner: self) }

  func show(
    _ title: String, palette: OverlayAppearancePalette, leading: Bool = false,
    withinWindow: Bool = false, content: AnyView? = nil, size: CGSize? = nil,
    interactive: Bool = false
  ) {
    guard let view else { return }
    OverlayHintPanel.shared.show(
      title, palette: palette, below: view, owner: self, leading: leading,
      withinWindow: withinWindow, content: content, size: size, interactive: interactive)
  }

  func hide(allowPointerRetention: Bool = false) {
    OverlayHintPanel.shared.hide(owner: self, allowPointerRetention: allowPointerRetention)
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

/// One non-key hint window shared by header and rail controls. Ordinary hints
/// are click-through; full-message previews can receive pointer scrolling.
/// As a child window of the overlay panel it moves with a drag and leaves when
/// the panel is ordered out, so no hint outlives its overlay.
@MainActor
final class OverlayHintPanel {
  static let shared = OverlayHintPanel()
  /// Distance between the control and its hint.
  static let gap: CGFloat = 6
  /// Distance kept from the screen's visible edges.
  static let screenInset: CGFloat = 4

  private var panel: NSPanel?
  private var controller: NSHostingController<AnyView>?
  private weak var owner: OverlayHintAnchor?
  private var title: String?
  private var interactive = false
  private var pointerInside = false
  private var pendingDismissal: Task<Void, Never>?

  /// The window currently presenting a hint, for tests and diagnostics.
  var presentedPanel: NSPanel? { panel?.isVisible == true ? panel : nil }

  /// The title the visible hint carries. The chip's hosting view hangs as a
  /// child of the effect view, so a test cannot read it off the panel; the
  /// owner says what it shows (Astra, 2026-10-22).
  var presentedTitle: String? {
    presentedPanel == nil ? nil : title
  }

  func isPresented(owner: OverlayHintAnchor) -> Bool {
    self.owner === owner && presentedPanel != nil
  }

  func show(
    _ title: String, palette: OverlayAppearancePalette, below anchor: NSView,
    owner: OverlayHintAnchor, leading: Bool = false, withinWindow: Bool = false,
    content: AnyView? = nil, size desiredSize: CGSize? = nil, interactive: Bool = false
  ) {
    guard let parent = anchor.window, parent.isVisible else { return }
    let anchorRect = parent.convertToScreen(anchor.convert(anchor.bounds, to: nil))
    let visible = parent.screen?.visibleFrame ?? NSScreen.main?.visibleFrame ?? parent.frame
    let bounds = withinWindow ? parent.frame.intersection(visible) : visible
    guard !bounds.isNull, bounds.width > 2 * Self.screenInset,
      bounds.height > 2 * Self.screenInset
    else {
      hide(owner: owner)
      return
    }
    let (panel, controller) = presentation()
    pendingDismissal?.cancel()
    pendingDismissal = nil
    if self.owner !== owner { pointerInside = false }
    self.owner = owner
    self.interactive = interactive
    let size: NSSize
    if let content, let desiredSize {
      let maximumSize = NSSize(
        width: min(desiredSize.width, bounds.width - 2 * Self.screenInset),
        height: min(desiredSize.height, bounds.height - 2 * Self.screenInset))
      controller.rootView = AnyView(
        content.frame(width: maximumSize.width, height: interactive ? nil : maximumSize.height)
          .background {
            if interactive {
              RoundedRectangle(cornerRadius: CSRadius.chip)
                .fill(
                  OverlayPopoverGlass.veil(
                    palette, opacity: OverlayPopoverGlass.previewVeilOpacity)
                )
                .allowsHitTesting(false)
                .accessibilityHidden(true)
            }
          }
          .overlay {
            if interactive {
              RoundedRectangle(cornerRadius: CSRadius.chip)
                .strokeBorder(
                  OverlayPopoverGlass.edge(palette), lineWidth: OverlayPopoverGlass.edgeWidth
                )
                .allowsHitTesting(false)
                .accessibilityHidden(true)
            }
          }
          .background {
            OverlayMenuCursorRegion { [weak self, weak owner] inside in
              guard let owner else { return }
              self?.pointerHover(inside, owner: owner)
            }
          })
      if interactive {
        let fitted = controller.sizeThatFits(in: maximumSize)
        size = NSSize(
          width: maximumSize.width, height: min(maximumSize.height, max(1, ceil(fitted.height))))
      } else {
        size = maximumSize
      }
    } else {
      controller.rootView = AnyView(
        OverlayTooltipChip(title: title, palette: palette, filled: false))
      size = controller.sizeThatFits(
        in: NSSize(
          width: min(OverlayTooltipLayout.maxHintWidth, bounds.width - 2 * Self.screenInset),
          height: bounds.height - 2 * Self.screenInset))
    }
    panel.hasShadow = content != nil
    panel.ignoresMouseEvents = !interactive
    panel.alphaValue = parent.alphaValue
    if let effect = panel.contentView as? NSVisualEffectView {
      effect.material = interactive ? OverlayPopoverGlass.material : .toolTip
      effect.maskImage = OverlayPopoverGlass.roundedMask(
        radius: content == nil || interactive ? CSRadius.chip : CSRadius.card)
    }
    panel.appearance = NSAppearance(named: palette.appearance == .dark ? .darkAqua : .aqua)
    let frame =
      leading
      ? Self.frame(size: size, leading: anchorRect, within: bounds)
      : Self.frame(size: size, below: anchorRect, within: bounds)
    panel.setFrame(frame, display: true)
    panel.invalidateShadow()
    if panel.parent !== parent {
      panel.parent?.removeChildWindow(panel)
      parent.addChildWindow(panel, ordered: .above)
    }
    self.title = title
    panel.orderFront(nil)
  }

  func hide(owner: OverlayHintAnchor, allowPointerRetention: Bool = false) {
    guard self.owner === owner else { return }
    pendingDismissal?.cancel()
    pendingDismissal = nil
    if allowPointerRetention, interactive {
      pendingDismissal = Task { @MainActor [weak self, weak owner] in
        do {
          try await Task.sleep(for: .milliseconds(220))
        } catch {
          return
        }
        guard let self, let owner, self.owner === owner else { return }
        guard !self.pointerInside else { return }
        self.hide(owner: owner)
      }
      return
    }
    self.owner = nil
    interactive = false
    pointerInside = false
    title = nil
    controller?.rootView = AnyView(EmptyView())
    guard let panel else { return }
    panel.parent?.removeChildWindow(panel)
    panel.orderOut(nil)
  }

  private func pointerHover(_ inside: Bool, owner: OverlayHintAnchor) {
    guard self.owner === owner, interactive else { return }
    pointerInside = inside
    if inside {
      pendingDismissal?.cancel()
      pendingDismissal = nil
    } else {
      hide(owner: owner, allowPointerRetention: true)
    }
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

  /// Sidebar hints sit over the document, outside the rail's scroll clip.
  static func frame(size: CGSize, leading anchor: NSRect, within visible: NSRect) -> NSRect {
    let left = anchor.minX - gap - size.width
    let right = anchor.maxX + gap
    // A column against the left screen edge previews on its right. Clamping
    // the left candidate onto the row used to cover the very control hovered.
    let x =
      left >= visible.minX + screenInset
      ? left
      : right + size.width <= visible.maxX - screenInset ? right : left
    let origin = NSPoint(
      x: min(
        max(x, visible.minX + screenInset),
        visible.maxX - screenInset - size.width),
      y: min(
        max(anchor.midY - size.height / 2, visible.minY + screenInset),
        visible.maxY - screenInset - size.height))
    return NSRect(origin: origin, size: size)
  }

  private func presentation() -> (NSPanel, NSHostingController<AnyView>) {
    if let panel, let controller { return (panel, controller) }
    let panel = NSPanel(
      contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered,
      defer: true)
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = false
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
    effect.maskImage = OverlayPopoverGlass.roundedMask(radius: CSRadius.chip)
    let controller = NSHostingController(
      rootView: AnyView(OverlayTooltipChip(title: "", palette: .light, filled: false)))
    controller.view.frame = effect.bounds
    controller.view.autoresizingMask = [.width, .height]
    effect.addSubview(controller.view)
    panel.contentView = effect
    self.panel = panel
    self.controller = controller
    return (panel, controller)
  }
}
