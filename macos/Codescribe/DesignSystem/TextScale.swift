import SwiftUI

// Per-surface text scale. Two INDEPENDENT multipliers — one for the dictation
// overlay (read from a distance while dictating) and one for the agent chat (read
// up close) — each persisted to UserDefaults and driven by ⌘+ / ⌘- / ⌘0 on the
// focused window. The scale multiplies TEXT point sizes only; window chrome,
// layout paddings and control geometry keep their intrinsic dimensions, so a
// bigger transcript never resizes the panel or the buttons.
//
// Flow: AppDelegate installs one local key monitor that routes ⌘+/-/0 to the
// controller for the key window (overlay panel vs agent window). Each controller
// publishes `scale`; `TextScaleRoot` injects it into `\.csTextScale` at that
// window's SwiftUI root; text views read it via `.csFont` / `.csMono` (or, for
// the markdown body / composer field, directly through `@Environment`).

// MARK: - Environment

private struct CSTextScaleKey: EnvironmentKey {
    static let defaultValue: CGFloat = 1.0
}

extension EnvironmentValues {
    /// Text-size multiplier for the enclosing surface (default 1.0). Set once at a
    /// window's root by `TextScaleRoot`; read by the scaled-font helpers.
    var csTextScale: CGFloat {
        get { self[CSTextScaleKey.self] }
        set { self[CSTextScaleKey.self] = newValue }
    }
}

// MARK: - Controller

/// A single persisted text-scale value for one surface. Clamped to a readable
/// range and snapped to a fixed step so a persisted or hand-edited value can never
/// drift off-grid or out of bounds.
@MainActor
final class TextScaleController: ObservableObject {
    static let minScale: CGFloat = 0.8
    static let maxScale: CGFloat = 1.6
    static let step: CGFloat = 0.1

    @Published private(set) var scale: CGFloat
    private let defaultsKey: String

    init(key: String) {
        defaultsKey = key
        let stored = UserDefaults.standard.double(forKey: key)
        scale = Self.clamp(stored > 0 ? CGFloat(stored) : 1.0)
    }

    func increase() { apply(scale + Self.step) }
    func decrease() { apply(scale - Self.step) }
    func reset() { apply(1.0) }

    private func apply(_ value: CGFloat) {
        let clamped = Self.clamp(value)
        guard clamped != scale else { return }
        scale = clamped
        UserDefaults.standard.set(Double(clamped), forKey: defaultsKey)
    }

    /// Snap to the nearest `step` and clamp into `[minScale, maxScale]`.
    static func clamp(_ value: CGFloat) -> CGFloat {
        let snapped = (value / step).rounded() * step
        return min(max(snapped, minScale), maxScale)
    }
}

// MARK: - Root injector

/// Injects a controller's live scale into `\.csTextScale` for a window's root.
/// Observing the controller here (not deeper) keeps the re-render scoped: bumping
/// the scale re-evaluates the surface's text, not the whole app.
struct TextScaleRoot<Content: View>: View {
    @ObservedObject var controller: TextScaleController
    @ViewBuilder var content: Content
    var body: some View {
        content.environment(\.csTextScale, controller.scale)
    }
}

// MARK: - Scaled font helpers

extension View {
    /// Space Grotesk UI font at `size`, multiplied by the surrounding `\.csTextScale`.
    func csFont(_ size: CGFloat, _ weight: Font.Weight = .regular) -> some View {
        modifier(CSScaledFont(size: size, weight: weight, mono: false))
    }

    /// JetBrains Mono font at `size`, multiplied by the surrounding `\.csTextScale`.
    func csMono(_ size: CGFloat, _ weight: Font.Weight = .regular) -> some View {
        modifier(CSScaledFont(size: size, weight: weight, mono: true))
    }
}

private struct CSScaledFont: ViewModifier {
    @Environment(\.csTextScale) private var scale
    let size: CGFloat
    let weight: Font.Weight
    let mono: Bool

    func body(content: Content) -> some View {
        content.font(mono ? CSFont.mono(size * scale, weight) : CSFont.ui(size * scale, weight))
    }
}
