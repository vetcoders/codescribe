import SwiftUI

// Visual verification surface for the design system. Open this preview to confirm
// tokens render at the exact hex, bundled fonts load, and components look right.
struct DesignGallery: View {
  private let swatches: [(String, Color)] = [
    ("ink", CSColor.ink),
    ("terracotta", CSColor.terracotta),
    ("terracottaLight", CSColor.terracottaLight),
    ("terracottaDeep", CSColor.terracottaDeep),
    ("assistive", CSColor.assistive),
    ("assistiveLight", CSColor.assistiveLight),
    ("olive", CSColor.olive),
    ("oliveLight", CSColor.oliveLight),
    ("amber", CSColor.amber),
    ("textHigh", CSColor.textHigh),
    ("eyebrowOlive", CSColor.eyebrowOlive),
  ]

  var body: some View {
    ScrollView {
      VStack(alignment: .leading, spacing: CSSpace.lg) {
        Wordmark(size: 22)

        EyebrowLabel(text: "Design System")
        Text("Speak in. Code out.")
          .font(CSFont.hero(40))
          .tracking(-1.6)
          .foregroundStyle(CSColor.textHigh)

        // Type ramp
        VStack(alignment: .leading, spacing: CSSpace.xs) {
          Text("Space Grotesk — body 18").font(CSFont.bodyLg).foregroundStyle(CSColor.textBody)
          Text("Space Grotesk — body 14").font(CSFont.body).foregroundStyle(CSColor.textBodyAlt)
          Text("JetBrains Mono — meta 11").font(CSFont.metaMono).foregroundStyle(CSColor.textFaint)
        }

        // Swatches
        LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 3), spacing: 10) {
          ForEach(swatches, id: \.0) { name, color in
            VStack(spacing: CSSpace.xs) {
              RoundedRectangle(cornerRadius: CSRadius.chip).fill(color)
                .frame(height: CSSpace.previewInset)
                .overlay(
                  RoundedRectangle(cornerRadius: CSRadius.chip).strokeBorder(
                    CSColor.hairline(), lineWidth: 1))
              Text(name).font(CSFont.mono(9)).foregroundStyle(CSColor.textMuted)
            }
          }
        }

        // Components
        HStack(spacing: CSSpace.md) {
          StatusPill(text: "recording", color: CSColor.terracotta, rippling: true)
          StaticStatusPill(text: "Idle", color: CSColor.oliveLight)
          StaticStatusPill(text: "reasoned · 2.1s", color: CSColor.amber)
        }

        GlassPanel {
          VStack(alignment: .leading, spacing: CSSpace.sm) {
            Wordmark()
            Text("GlassPanel — dark glass, hairline, deep shadow")
              .font(CSFont.body).foregroundStyle(CSColor.textBody)
          }
          .padding(CSSpace.lg)
        }
        .frame(maxWidth: .infinity)
      }
      .padding(CSSpace.xl)
    }
    .frame(width: 560, height: 720)
    .background(CSColor.ink)
    .onAppear { FontLoader.register() }
  }
}

#if DEBUG
  #Preview("Design Gallery") {
    DesignGallery()
  }
#endif
