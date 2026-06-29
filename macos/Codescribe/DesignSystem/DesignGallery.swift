import SwiftUI

// Visual verification surface for the design system. Open this preview to confirm
// tokens render at the exact hex, bundled fonts load, and components look right.
struct DesignGallery: View {
    private let swatches: [(String, Color)] = [
        ("ink", CSColor.ink),
        ("terracotta", CSColor.terracotta),
        ("terracottaLight", CSColor.terracottaLight),
        ("terracottaDeep", CSColor.terracottaDeep),
        ("olive", CSColor.olive),
        ("oliveLight", CSColor.oliveLight),
        ("amber", CSColor.amber),
        ("textHigh", CSColor.textHigh),
        ("eyebrowOlive", CSColor.eyebrowOlive),
    ]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                Wordmark(size: 22)

                EyebrowLabel(text: "Design System")
                Text("Speak in. Code out.")
                    .font(CSFont.hero(40))
                    .tracking(-1.6)
                    .foregroundStyle(CSColor.textHigh)

                // Type ramp
                VStack(alignment: .leading, spacing: 6) {
                    Text("Space Grotesk — body 18").font(CSFont.bodyLg).foregroundStyle(CSColor.textBody)
                    Text("Space Grotesk — body 14").font(CSFont.body).foregroundStyle(CSColor.textBodyAlt)
                    Text("JetBrains Mono — meta 11").font(CSFont.metaMono).foregroundStyle(CSColor.textFaint)
                }

                // Swatches
                LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 3), spacing: 10) {
                    ForEach(swatches, id: \.0) { name, color in
                        VStack(spacing: 6) {
                            RoundedRectangle(cornerRadius: 8).fill(color)
                                .frame(height: 44)
                                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(CSColor.hairline(), lineWidth: 1))
                            Text(name).font(CSFont.mono(9)).foregroundStyle(CSColor.textMuted)
                        }
                    }
                }

                // Components
                HStack(spacing: 12) {
                    StatusPill(text: "recording", color: CSColor.terracotta, rippling: true)
                    StatusPill(text: "Idle", color: CSColor.oliveLight)
                    StatusPill(text: "reasoned · 2.1s", color: CSColor.amber)
                }

                GlassPanel {
                    VStack(alignment: .leading, spacing: 8) {
                        Wordmark()
                        Text("GlassPanel — dark glass, hairline, deep shadow")
                            .font(CSFont.body).foregroundStyle(CSColor.textBody)
                    }
                    .padding(20)
                }
                .frame(maxWidth: .infinity)
            }
            .padding(28)
        }
        .frame(width: 560, height: 720)
        .background(CSColor.ink)
        .onAppear { FontLoader.register() }
    }
}

#Preview("Design Gallery") {
    DesignGallery()
}
