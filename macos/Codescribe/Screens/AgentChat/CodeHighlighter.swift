import AppKit
import HighlightSwift
import SwiftUI

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
// Theme: our own CSS maps highlight.js token classes onto macOS semantic system
// colors (no bundled foreign theme, no custom brand hexes) via HighlightColors.custom.
// The font is NOT carried by the CSS — HighlightSwift strips `.font` from its output,
// so the caller applies CSFont.mono uniformly and only the token *colours* come from
// here.

enum CodeHighlighter {
  // Lazily instantiated on first access (Swift `static let`), so the engine —
  // and the JSContext it wraps — never warms during app launch.
  private static let engine = Highlight()

  /// Highlight `code` with the system token theme, off the main render path.
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

/// highlight.js token-class → macOS system color mapping, emitted as CSS for
/// HighlightColors.custom. Every color is resolved from `NSColor` in `.darkAqua`
/// or `.aqua` so the code palette uses Apple's semantic system hues without
/// custom hex literals. No `background` rules — the code block keeps its own
/// `codeWell` fill.
enum CodeTheme {
  static let darkCSS: String = makeCSS(dark: true)
  static let lightCSS: String = makeCSS(dark: false)

  static func systemHex(_ color: NSColor, dark: Bool) -> String {
    let appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
    var resolved = color
    appearance?.performAsCurrentDrawingAppearance {
      resolved = color.usingColorSpace(.sRGB) ?? color
    }
    let srgb = resolved.usingColorSpace(.sRGB) ?? resolved
    let r = Int(round(max(0, min(1, srgb.redComponent)) * 255))
    let g = Int(round(max(0, min(1, srgb.greenComponent)) * 255))
    let b = Int(round(max(0, min(1, srgb.blueComponent)) * 255))
    return String(format: "#%02X%02X%02X", r, g, b)
  }

  private static func makeCSS(dark: Bool) -> String {
    let base = systemHex(.labelColor, dark: dark)
    let comment = systemHex(.secondaryLabelColor, dark: dark)
    let keyword = systemHex(.systemIndigo, dark: dark)
    let string = systemHex(.systemGreen, dark: dark)
    let number = systemHex(.systemOrange, dark: dark)
    let title = systemHex(.systemBlue, dark: dark)
    let attr = systemHex(.secondaryLabelColor, dark: dark)
    let deletion = systemHex(.systemRed, dark: dark)
    return """
      .hljs{color:\(base)}
      .hljs-comment,.hljs-quote{color:\(comment)}
      .hljs-keyword,.hljs-selector-tag,.hljs-built_in,.hljs-type,.hljs-tag,.hljs-name,.hljs-template-tag{color:\(keyword)}
      .hljs-string,.hljs-symbol,.hljs-bullet,.hljs-regexp,.hljs-char.escape_,.hljs-addition{color:\(string)}
      .hljs-number,.hljs-literal,.hljs-meta,.hljs-meta .hljs-keyword,.hljs-meta .hljs-string,.hljs-meta-keyword{color:\(number)}
      .hljs-title,.hljs-title.class_,.hljs-title.function_,.hljs-section,.hljs-selector-id,.hljs-selector-class{color:\(title)}
      .hljs-attr,.hljs-attribute,.hljs-property,.hljs-variable,.hljs-template-variable,.hljs-params,.hljs-operator,.hljs-punctuation,.hljs-subst,.hljs-selector-attr,.hljs-selector-pseudo{color:\(attr)}
      .hljs-doctag,.hljs-strong{color:\(base)}
      .hljs-deletion,.hljs-link{color:\(deletion)}
      """
  }
}
