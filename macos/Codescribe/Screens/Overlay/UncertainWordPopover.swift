import AppKit
import SwiftUI

/// Value model for the uncertain-word popover, so the content contract
/// (reason, source, diagnostic gating, available actions) is testable without
/// presenting UI.
struct UncertainWordPopoverModel: Equatable {
  let word: String
  let reason: String
  let sourceLine: String
  /// Raw source-scale value. Present only in diagnostics (power) mode (d8).
  let diagnosticValue: String?
  let canPlay: Bool
  let canTeach: Bool

  init(
    word: OverlayUncertainWord,
    showsDiagnostics: Bool,
    canPlay: Bool,
    canTeach: Bool
  ) {
    self.word = word.word
    reason = UncertainWordCopy.reason(for: word)
    sourceLine = UncertainWordCopy.sourceLine(for: word)
    diagnosticValue = showsDiagnostics ? UncertainWordCopy.diagnosticValue(for: word) : nil
    self.canPlay = canPlay
    self.canTeach = canTeach
  }
}

/// Small popover that explains one uncertain word and offers its actions:
/// Play the word's own PCM (pinned to the occurrence) or Teach the dictionary
/// through the existing `quality_teach_span` path.
struct UncertainWordPopoverView: View {
  let model: UncertainWordPopoverModel
  let palette: OverlayAppearancePalette
  let onPlay: () -> Void
  let onTeach: (String) -> Void
  @State private var correction: String

  init(
    model: UncertainWordPopoverModel,
    palette: OverlayAppearancePalette,
    onPlay: @escaping () -> Void,
    onTeach: @escaping (String) -> Void
  ) {
    self.model = model
    self.palette = palette
    self.onPlay = onPlay
    self.onTeach = onTeach
    _correction = State(initialValue: model.word)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      Text(model.word)
        .font(.system(size: 13, weight: .semibold))
        .foregroundStyle(palette.primaryText.color)
      Text(model.reason)
        .font(.system(size: 12))
        .foregroundStyle(palette.bodyText.color)
        .fixedSize(horizontal: false, vertical: true)
      Text(model.sourceLine)
        .font(.system(size: 10))
        .foregroundStyle(palette.mutedText.color)
      if let diagnostic = model.diagnosticValue {
        Text(diagnostic)
          .font(.system(size: 10, design: .monospaced))
          .foregroundStyle(palette.mutedText.color)
          .accessibilityIdentifier("uncertain-word-diagnostic")
      }
      HStack(spacing: 8) {
        if model.canPlay {
          Button("Play", action: onPlay)
            .controlSize(.small)
            .accessibilityIdentifier("uncertain-word-play")
        }
        if model.canTeach {
          TextField("Teach", text: $correction)
            .textFieldStyle(.roundedBorder)
            .frame(minWidth: 90)
            .accessibilityIdentifier("uncertain-word-teach-field")
          Button("Teach") { onTeach(correction) }
            .controlSize(.small)
            .disabled(correction.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            .accessibilityIdentifier("uncertain-word-teach")
        }
      }
    }
    .padding(12)
    .frame(maxWidth: 260)
    .background(palette.desktopBackground.color)
  }
}

/// Presents `UncertainWordPopoverView` from the transcript canvas at the
/// clicked word's glyph rect. The popover is transient: clicking elsewhere or
/// starting a new take dismisses it.
@MainActor
final class UncertainWordPopoverPresenter {
  private var popover: NSPopover?

  func present(
    word: OverlayUncertainWord,
    in textView: NSTextView,
    palette: OverlayAppearancePalette,
    showsDiagnostics: Bool,
    onPlay: (() -> Void)?,
    onTeach: ((String) -> Void)?
  ) {
    dismiss()
    let model = UncertainWordPopoverModel(
      word: word,
      showsDiagnostics: showsDiagnostics,
      canPlay: onPlay != nil,
      canTeach: onTeach != nil
    )
    let content = UncertainWordPopoverView(
      model: model,
      palette: palette,
      onPlay: { onPlay?() },
      onTeach: { canonical in onTeach?(canonical) }
    )
    let hosting = NSHostingController(rootView: content)
    hosting.sizingOptions = .preferredContentSize
    let popover = NSPopover()
    popover.behavior = .transient
    popover.contentViewController = hosting
    let rect = textView.firstRect(forCharacterRange: word.range, actualRange: nil)
    popover.show(relativeTo: rect, of: textView, preferredEdge: .maxY)
    self.popover = popover
  }

  func dismiss() {
    popover?.performClose(nil)
    popover = nil
  }
}
