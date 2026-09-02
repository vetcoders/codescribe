import AppKit
import SwiftUI

// Reusable glass primitives shared by every screen. Build once, consume everywhere.

/// App-wide focus policy: pointer interaction releases keyboard focus after the
/// clicked control handles its event, while keyboard navigation and text entry
/// keep AppKit's native focus behavior and visible accessibility affordances.
///
/// Apply `csFocusPolicy()` once at a window's content root. This deliberately
/// avoids `.focusEffectDisabled()` on ordinary buttons: hiding the effect also
/// hides the keyboard-visible focus cue that macOS users rely on.
@MainActor
enum CSFocusPolicy {
  enum InputModality {
    case keyboard
    case pointer
  }

  static func shouldReleaseFocus(
    for modality: InputModality,
    hitView: NSView?
  ) -> Bool {
    modality == .pointer && !isTextInput(hitView)
  }

  static func isTextInput(_ view: NSView?) -> Bool {
    var candidate = view
    while let current = candidate {
      if current is NSTextField || current is NSTextView {
        return true
      }
      // SwiftUI's TextEditor is hosted in an NSScrollView. Depending on
      // which internal layer receives the click, hitTest can return the
      // clip/document host instead of the NSTextView itself. Treat a
      // scroll view backed by a text view as text input too, otherwise the
      // pointer focus monitor clears first responder immediately and the
      // editor looks read-only.
      if let scrollView = current as? NSScrollView,
        scrollView.documentView is NSTextView
      {
        return true
      }
      candidate = current.superview
    }
    return false
  }
}

private struct CSFocusPolicyModifier: ViewModifier {
  func body(content: Content) -> some View {
    content.background {
      CSFocusPolicyMonitor()
        .frame(width: 0, height: 0)
        .allowsHitTesting(false)
    }
  }
}

private struct CSFocusPolicyMonitor: NSViewRepresentable {
  func makeNSView(context: Context) -> CSFocusPolicyMonitorView {
    CSFocusPolicyMonitorView()
  }

  func updateNSView(_ nsView: CSFocusPolicyMonitorView, context: Context) {}

  static func dismantleNSView(_ nsView: CSFocusPolicyMonitorView, coordinator: ()) {
    nsView.invalidate()
  }
}

@MainActor
private final class CSFocusPolicyMonitorView: NSView {
  private var mouseMonitor: Any?

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    removeMouseMonitor()
    guard let window else { return }

    mouseMonitor = NSEvent.addLocalMonitorForEvents(
      matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown]
    ) { [weak window] event in
      guard let window, event.window === window else { return event }
      let hitView = window.contentView?.hitTest(event.locationInWindow)
      guard CSFocusPolicy.shouldReleaseFocus(for: .pointer, hitView: hitView) else {
        return event
      }

      // Let SwiftUI deliver the click first, then release the responder it
      // may have assigned to the button. Text inputs are excluded above.
      DispatchQueue.main.async { [weak window] in
        window?.makeFirstResponder(nil)
      }
      return event
    }
  }

  func invalidate() {
    removeMouseMonitor()
  }

  private func removeMouseMonitor() {
    guard let mouseMonitor else { return }
    NSEvent.removeMonitor(mouseMonitor)
    self.mouseMonitor = nil
  }
}

extension View {
  /// Installs Codescribe's pointer-vs-keyboard focus policy for one window.
  func csFocusPolicy() -> some View {
    modifier(CSFocusPolicyModifier())
  }
}

/// Keyboard focus ring that follows the control's own rounded geometry.
///
/// AppKit's default ring is a squarish halo that ignores a custom chip's
/// corner radius — on the dark glass surfaces it reads as a grey box stamped
/// across the control (operator screenshots 2026-08-09, next to Claude
/// Desktop's accent ring as the bar to clear). This style draws our ring —
/// a thin accent stroke hugging the control 2pt out, rounded to
/// `cornerRadius + 2` so the inner and outer curves stay concentric; the
/// weight and offset are calibrated against Claude Desktop's ring, which the
/// operator holds up as the reference ("olbrzymie i brzydkie" was the verdict
/// on the first, thicker cut). Suppressing the system halo is the adopting
/// Button's job: use `View.csFocusRing(cornerRadius:)`, never
/// `.buttonStyle(.csFocusRing(...))` alone.
///
/// Keyboard-only by construction: `CSFocusPolicy` releases focus after
/// pointer clicks, so the ring appears exactly when a keyboard user is
/// navigating — the accessibility cue stays, only its geometry is ours.
struct CSFocusRingButtonStyle: ButtonStyle {
  var cornerRadius: CGFloat
  @Environment(\.isFocused) private var isFocused

  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .opacity(configuration.isPressed ? 0.82 : 1)
      .overlay(
        RoundedRectangle(cornerRadius: cornerRadius + 2, style: .continuous)
          .strokeBorder(
            CSColor.chromeAccent.opacity(isFocused ? 0.9 : 0),
            lineWidth: 1.5
          )
          .padding(-2)
      )
      .animation(.easeOut(duration: 0.12), value: isFocused)
  }
}

extension ButtonStyle where Self == CSFocusRingButtonStyle {
  /// Plain-look button carrying the Codescribe focus ring. Use instead of
  /// `.plain` on custom-drawn chips, cards, and segments.
  static func csFocusRing(cornerRadius: CGFloat = CSRadius.chip) -> CSFocusRingButtonStyle {
    CSFocusRingButtonStyle(cornerRadius: cornerRadius)
  }
}

extension View {
  /// The one correct way to adopt the Codescribe focus ring on a Button.
  ///
  /// `focusEffectDisabled()` is an environment write and only flows DOWN the
  /// tree — inside `makeBody` it reaches the label's descendants, never the
  /// Button that actually draws AppKit's grey halo. So the kill switch must
  /// ride on the Button itself, paired here with the style so the two can't
  /// drift apart (adopting the style alone leaves the system ring stacked
  /// on top of ours — operator screenshot 2026-08-09, the "stodoła").
  func csFocusRing(cornerRadius: CGFloat = CSRadius.chip) -> some View {
    buttonStyle(.csFocusRing(cornerRadius: cornerRadius))
      .focusEffectDisabled()
  }

  /// Settings / panel card chrome. Five panels used to re-declare the same
  /// fill + hairline at 12/14/15pt padding. One modifier, one radius, one fill.
  func csSettingsCard(padding: CGFloat = CSSpace.card) -> some View {
    self
      .padding(padding)
      .background(
        RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
          .fill(CSColor.surfaceRaised(0.025))
      )
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
          .strokeBorder(CSColor.hairline(), lineWidth: 1)
      )
  }
}

/// Dark glass container: ultraThinMaterial tinted + hairline border + deep shadow.
/// Overlay passes `sitsInForest` so the panel drinks the desktop instead of
/// painting an opaque under-layer that killed the original glass.
struct GlassPanel<Content: View>: View {
  var cornerRadius: CGFloat = CSRadius.window
  var blurTint: Double = 0.84
  var sitsInForest: Bool = false
  @ViewBuilder var content: Content

  var body: some View {
    content
      .background(
        ZStack {
          if sitsInForest {
            Rectangle().fill(.ultraThinMaterial).environment(\.colorScheme, .dark)
            CSColor.ink.opacity(0.22)
          } else {
            CSColor.glassUnder
            Rectangle().fill(.ultraThinMaterial).environment(\.colorScheme, .dark)
            CSColor.glassBase.opacity(blurTint - 0.6)
          }
        }
      )
      .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
      .overlay(
        RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
          .strokeBorder(CSColor.hairline(sitsInForest ? 0.07 : 0.09), lineWidth: 1)
      )
      .shadow(
        color: .black.opacity(sitsInForest ? 0.22 : 0.6),
        radius: sitsInForest ? 22 : 50,
        x: 0,
        y: sitsInForest ? 10 : 40
      )
  }
}

/// Small mode/brand dot.
struct ModeDot: View {
  var color: Color = CSColor.terracotta
  var size: CGFloat = 9
  var body: some View {
    Circle().fill(color).frame(width: size, height: size)
  }
}

/// Status pill with a softpulsing dot and an optional expanding ripple ring.
struct StatusPill: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  let text: String
  var color: Color = CSColor.oliveLight
  var rippling: Bool = false

  @State private var pulse = false
  @State private var ripple = false

  var body: some View {
    HStack(spacing: 6) {
      ZStack {
        if rippling && !reduceMotion {
          Circle().strokeBorder(color, lineWidth: 1)
            .frame(width: 9, height: 9)
            .scaleEffect(ripple ? 2.7 : 0.5)
            .opacity(ripple ? 0 : 0.7)
          // Animated pulse dot is rendered ONLY while rippling. Removing it
          // from the view tree in Idle physically tears down the
          // repeatForever animation — a Transaction(animation: nil) snap
          // does NOT cancel an in-flight repeatForever, which left it
          // ticking the render loop at ~30% CPU in Idle.
          Circle().fill(color).frame(width: 6, height: 6)
            .opacity(pulse ? 1 : 0.7)
        } else {
          Circle().fill(color).frame(width: 6, height: 6)
            .opacity(0.7)
        }
      }
      .frame(width: 9, height: 9)
      Text(text)
        .csMono(11, .medium)
        .foregroundStyle(color)
    }
    .padding(.horizontal, 9)
    .padding(.vertical, 4)
    .background(color.opacity(0.12))
    .overlay(Capsule().strokeBorder(color.opacity(0.3), lineWidth: 1))
    .clipShape(Capsule())
    .onAppear { syncStatusAnimations() }
    .onChange(of: rippling) { _, _ in syncStatusAnimations() }
    .onChange(of: reduceMotion) { _, _ in syncStatusAnimations() }
  }

  /// `pulse` and `ripple` drive `.repeatForever` animations. They must run ONLY
  /// while the pill represents a live/active state (`rippling`). Previously the
  /// softpulse was started unconditionally in `onAppear` and never stopped — and
  /// because this pill lives in the always-visible overlay header, that left a
  /// repeatForever ticking the SwiftUI view graph every frame in Idle (100% CPU,
  /// re-rasterizing the host panel's shadow + rounded-rect strokes each frame).
  /// Gate it on `rippling` and, when inactive, snap the state back with animation
  /// disabled so the in-flight repeatForever is torn down rather than left running.
  private func syncStatusAnimations() {
    if rippling && !reduceMotion {
      withAnimation(CSMotion.softpulse) { pulse = true }
      withAnimation(CSMotion.ripple) { ripple = true }
    } else {
      var transaction = Transaction(animation: nil)
      transaction.disablesAnimations = true
      withTransaction(transaction) {
        pulse = false
        ripple = false
      }
    }
  }
}

/// Non-animated status pill for Idle/final states. A SEPARATE view type (distinct
/// SwiftUI identity) with NO @State and NO onAppear animation — so it can never
/// keep a `.repeatForever` ticking the render loop while visible in Idle. The
/// header swaps to this type whenever the pill is not in a live/rippling state, so
/// the animated pill is removed from the tree (which actually tears the animation
/// down) instead of relying on a fragile in-place cancel.
struct StaticStatusPill: View {
  let text: String
  var color: Color = CSColor.oliveLight
  var body: some View {
    HStack(spacing: 6) {
      Circle().fill(color).frame(width: 6, height: 6).opacity(0.7)
        .frame(width: 9, height: 9)
      Text(text)
        .csMono(11, .medium)
        .foregroundStyle(color)
    }
    .padding(.horizontal, 9)
    .padding(.vertical, 4)
    .background(color.opacity(0.12))
    .overlay(Capsule().strokeBorder(color.opacity(0.3), lineWidth: 1))
    .clipShape(Capsule())
  }
}

/// Wordmark lockup: brand dot + lowercase "codescribe".
struct Wordmark: View {
  var size: CGFloat = 15
  var dotColor: Color = CSColor.terracotta
  var body: some View {
    HStack(spacing: 9) {
      ModeDot(color: dotColor, size: size * 0.6)
      Text("codescribe")
        .font(CSFont.ui(size, .bold))
        .tracking(-0.3)
        .foregroundStyle(CSColor.textHigh)
    }
  }
}
