import SwiftUI

// Reusable glass primitives shared by every screen. Build once, consume everywhere.

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
                }
                Circle().fill(color).frame(width: 6, height: 6)
                    .opacity(pulse ? 1 : 0.7)
            }
            .frame(width: 9, height: 9)
            Text(text)
                .font(CSFont.metaMono)
                .foregroundStyle(color)
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 4)
        .background(color.opacity(0.12))
        .overlay(Capsule().strokeBorder(color.opacity(0.3), lineWidth: 1))
        .clipShape(Capsule())
        .onAppear {
            withAnimation(CSMotion.softpulse) { pulse = true }
            if rippling { withAnimation(CSMotion.ripple) { ripple = true } }
        }
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
