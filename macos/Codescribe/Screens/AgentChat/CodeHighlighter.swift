import SwiftUI
import HighlightSwift

// Syntax highlighting for finalized agent-chat code blocks.
//
// Strategy (highlight-on-close): a block is only highlighted once its fence has
// closed. While it is the live stream tail it renders plain mono (see
// CodeBlockView.highlightable) so the streaming hot path never pays for a
// re-highlight and stays fluid. The one highlight per finalized block is async:
//   - HighlightSwift's `HLJS` is a `final actor`, so the JavaScriptCore tokenize
//     runs off the main thread. The JSContext is created lazily on the first
//     highlight call — never at app start, keeping it off the cold-start path.
//   - Only the final HTML→AttributedString materialization lands on the main
//     actor (AppKit's HTML importer requires it), and only once per closed
//     block, never per streaming delta.
//
// Theme: our own CSS maps highlight.js token classes onto the CSColor brand
// palette (no bundled foreign theme) via HighlightColors.custom. The font is NOT
// carried by the CSS — HighlightSwift strips `.font` from its output, so the
// caller applies CSFont.mono uniformly and only the token *colours* come from
// here.

enum CodeHighlighter {
    // Lazily instantiated on first access (Swift `static let`), so the engine —
    // and the JSContext it wraps — never warms during app launch.
    private static let engine = Highlight()

    /// Highlight `code` with the brand token theme, off the main render path.
    ///
    /// - Parameters:
    ///   - language: the fence info-string hint (`rust`, `ts`, …). When present
    ///     it is passed as a highlight.js language alias; an unknown alias makes
    ///     HighlightSwift fall back to plain text internally. When `nil`/empty,
    ///     highlight.js auto-detects the language.
    ///   - dark: pick the dark or light token CSS.
    /// - Returns: the highlighted `AttributedString`, or `nil` on failure so the
    ///   caller keeps its plain-mono placeholder (never a crash, never empty).
    static func attributed(_ code: String, language: String?, dark: Bool) async -> AttributedString? {
        let colors = HighlightColors.custom(css: dark ? CodeTheme.darkCSS : CodeTheme.lightCSS)
        do {
            if let rawHint = language?.trimmingCharacters(in: .whitespaces), !rawHint.isEmpty {
                let hint = rawHint.lowercased()
                return try await engine.attributedText(code, language: hint, colors: colors)
            }
            return try await engine.attributedText(code, colors: colors)
        } catch {
            return nil
        }
    }
}

/// highlight.js token-class → CSColor mapping, emitted as CSS for
/// HighlightColors.custom. Hexes are the literal CSColor token values (Tokens.swift)
/// so the code palette tracks the design system. No `background` rules — the code
/// block keeps its own `surfaceRaised` fill.
enum CodeTheme {
    // Dark surface (the agent chat is pinned to .preferredColorScheme(.dark), so
    // this is the variant in use today).
    //   base            → textBodyAlt  #DFE2DB
    //   keyword/type    → terracotta   #D97757  (the one brand accent)
    //   string/addition → oliveLight   #9DB178
    //   number/meta     → amber        #D6B24E
    //   title/function  → terracottaLight #E9B79F
    //   comment         → textFaint    #6F7268
    //   attr/operator   → textMuted    #9A9D97
    //   doctag/strong   → textHigh     #F4F2EC
    static let darkCSS = """
    .hljs{color:#DFE2DB}
    .hljs-comment,.hljs-quote{color:#6F7268}
    .hljs-keyword,.hljs-selector-tag,.hljs-built_in,.hljs-type,.hljs-tag,.hljs-name,.hljs-template-tag{color:#D97757}
    .hljs-string,.hljs-symbol,.hljs-bullet,.hljs-regexp,.hljs-char.escape_,.hljs-addition{color:#9DB178}
    .hljs-number,.hljs-literal,.hljs-meta,.hljs-meta .hljs-keyword,.hljs-meta .hljs-string,.hljs-meta-keyword{color:#D6B24E}
    .hljs-title,.hljs-title.class_,.hljs-title.function_,.hljs-section,.hljs-selector-id,.hljs-selector-class{color:#E9B79F}
    .hljs-attr,.hljs-attribute,.hljs-property,.hljs-variable,.hljs-template-variable,.hljs-params,.hljs-operator,.hljs-punctuation,.hljs-subst,.hljs-selector-attr,.hljs-selector-pseudo{color:#9A9D97}
    .hljs-doctag,.hljs-strong{color:#F4F2EC}
    .hljs-deletion,.hljs-link{color:#D97757}
    """

    // Light surface: a forward-looking variant (dormant while the chat is
    // dark-pinned). Reuses darker CSColor tokens so tokens read on a light fill —
    // olive #5F6B3E, terracottaDeep #C98A6E, eyebrowOlive #7F8C5E, textFaintAlt
    // #5D6058, textMutedAlt #82857F, ink #090A0D — rather than inventing a new
    // palette the design system does not carry.
    static let lightCSS = """
    .hljs{color:#090A0D}
    .hljs-comment,.hljs-quote{color:#82857F}
    .hljs-keyword,.hljs-selector-tag,.hljs-built_in,.hljs-type,.hljs-tag,.hljs-name,.hljs-template-tag{color:#C98A6E}
    .hljs-string,.hljs-symbol,.hljs-bullet,.hljs-regexp,.hljs-char.escape_,.hljs-addition{color:#5F6B3E}
    .hljs-number,.hljs-literal,.hljs-meta,.hljs-meta .hljs-keyword,.hljs-meta .hljs-string,.hljs-meta-keyword{color:#7F8C5E}
    .hljs-title,.hljs-title.class_,.hljs-title.function_,.hljs-section,.hljs-selector-id,.hljs-selector-class{color:#C98A6E}
    .hljs-attr,.hljs-attribute,.hljs-property,.hljs-variable,.hljs-template-variable,.hljs-params,.hljs-operator,.hljs-punctuation,.hljs-subst,.hljs-selector-attr,.hljs-selector-pseudo{color:#5D6058}
    .hljs-doctag,.hljs-strong{color:#090A0D}
    .hljs-deletion,.hljs-link{color:#C98A6E}
    """
}
