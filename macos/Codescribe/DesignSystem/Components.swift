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

    deinit {
        if let mouseMonitor {
            NSEvent.removeMonitor(mouseMonitor)
        }
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

/// Dark glass container: ultraThinMaterial tinted + hairline border + deep shadow.
struct GlassPanel<Content: View>: View {
    var cornerRadius: CGFloat = CSRadius.window
    var blurTint: Double = 0.84
    @ViewBuilder var content: Content

    var body: some View {
        content
            .background(
                ZStack {
                    CSColor.glassUnder
                    Rectangle().fill(.ultraThinMaterial).environment(\.colorScheme, .dark)
                    CSColor.glassBase.opacity(blurTint - 0.6) // subtle warm tint over the material
                }
            )
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .strokeBorder(CSColor.hairline(0.09), lineWidth: 1)
            )
            .shadow(color: .black.opacity(0.6), radius: 50, x: 0, y: 40)
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
    let text: String
    var color: Color = CSColor.oliveLight
    var rippling: Bool = false

    @State private var pulse = false
    @State private var ripple = false

    var body: some View {
        HStack(spacing: 6) {
            ZStack {
                if rippling {
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
        if rippling {
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
