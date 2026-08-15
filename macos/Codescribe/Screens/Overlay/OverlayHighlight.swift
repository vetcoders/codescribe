import AppKit
import SwiftUI

/// One span-based overlay highlight. Sample fields carry W13-3A PCM identity;
/// char offsets are the adapter onto already-committed utterance text.
struct OverlayHighlight: Equatable, Identifiable {
  enum Kind: String, Equatable {
    case lexiconCorrected = "lexicon_corrected"
    case speechGap = "speech_gap"
  }

  var id: String {
    "\(kind.rawValue):\(utteranceId):\(charStart):\(charEnd):\(sampleStart):\(sampleEnd)"
  }

  var kind: Kind
  var utteranceId: UInt64
  var charStart: UInt64
  var charEnd: UInt64
  var session: String
  var captureEpoch: UInt64
  var sampleStart: UInt64
  var sampleEnd: UInt64
  var before: String
  var after: String
  var taught: Bool = false

  var teachKind: String { kind.rawValue }

  var teachVariant: String {
    switch kind {
    case .lexiconCorrected: return before
    case .speechGap: return after
    }
  }

  var teachCanonical: String {
    switch kind {
    case .lexiconCorrected: return after
    case .speechGap: return after
    }
  }

  var accessibilityLabel: String {
    switch kind {
    case .lexiconCorrected:
      return "Lexicon correction \(before) to \(after)"
    case .speechGap:
      return "Speech gap, no words landed"
    }
  }
}

/// Visual run on the live canvas. Highlights stay buttons so one click
/// sends the span to Teach without a new permission prompt.
enum OverlayCanvasRun: Equatable {
  case text(String)
  case highlight(OverlayHighlight)
}

enum OverlayCanvas {
  static func lexiconHighlight(
    utteranceId: UInt64,
    start: UInt64,
    replacement: String,
    before: String,
    session: String = "overlay",
    captureEpoch: UInt64 = 0,
    sampleStart: UInt64 = 0,
    sampleEnd: UInt64 = 0
  ) -> OverlayHighlight? {
    let trimmed = replacement.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return nil }
    return OverlayHighlight(
      kind: .lexiconCorrected,
      utteranceId: utteranceId,
      charStart: start,
      charEnd: start + UInt64(replacement.count),
      session: session,
      captureEpoch: captureEpoch,
      sampleStart: sampleStart,
      sampleEnd: sampleEnd,
      before: before,
      after: replacement
    )
  }

  static func speechGap(
    utteranceId: UInt64,
    session: String = "overlay",
    captureEpoch: UInt64 = 0,
    sampleStart: UInt64 = 0,
    sampleEnd: UInt64 = 0
  ) -> OverlayHighlight {
    OverlayHighlight(
      kind: .speechGap,
      utteranceId: utteranceId,
      charStart: 0,
      charEnd: 0,
      session: session,
      captureEpoch: captureEpoch,
      sampleStart: sampleStart,
      sampleEnd: sampleEnd,
      before: "",
      after: "∅"
    )
  }

  /// Split committed utterance text on highlight char ranges, then append
  /// speech-gap markers and the live preview. Pure; used by the view and tests.
  static func runs(
    segments: [(utteranceId: UInt64?, text: String)],
    highlights: [OverlayHighlight],
    preview: String
  ) -> [OverlayCanvasRun] {
    var out: [OverlayCanvasRun] = []
    for (index, segment) in segments.enumerated() {
      if index > 0 { appendText(&out, " ") }
      let owned = highlights.filter { highlight in
        guard let utteranceId = segment.utteranceId else { return false }
        return highlight.utteranceId == utteranceId
      }
      appendSegmentRuns(&out, text: segment.text, highlights: owned)
    }
    let preview = preview.trimmingCharacters(in: .whitespacesAndNewlines)
    if !preview.isEmpty {
      if !out.isEmpty { appendText(&out, " ") }
      appendText(&out, preview)
    }
    return mergeAdjacentText(out)
  }

  private static func appendSegmentRuns(
    _ out: inout [OverlayCanvasRun],
    text: String,
    highlights: [OverlayHighlight]
  ) {
    let lexicon =
      highlights
      .filter { $0.kind == .lexiconCorrected }
      .sorted { $0.charStart < $1.charStart }
    var cursor = 0
    let chars = Array(text)
    for highlight in lexicon {
      let start = min(max(Int(highlight.charStart), cursor), chars.count)
      let end = min(max(Int(highlight.charEnd), start), chars.count)
      if start > cursor {
        appendText(&out, String(chars[cursor..<start]))
      }
      if end > start {
        out.append(.highlight(highlight))
      }
      cursor = end
    }
    if cursor < chars.count {
      appendText(&out, String(chars[cursor...]))
    }
    for gap in highlights where gap.kind == .speechGap {
      if !out.isEmpty { appendText(&out, " ") }
      out.append(.highlight(gap))
    }
  }

  private static func appendText(_ out: inout [OverlayCanvasRun], _ text: String) {
    guard !text.isEmpty else { return }
    out.append(.text(text))
  }

  private static func mergeAdjacentText(_ runs: [OverlayCanvasRun]) -> [OverlayCanvasRun] {
    var merged: [OverlayCanvasRun] = []
    for run in runs {
      if case .text(let next) = run, case .text(let prev)? = merged.last {
        merged[merged.count - 1] = .text(prev + next)
      } else {
        merged.append(run)
      }
    }
    return merged
  }
}

/// Live transcript with lexicon tints and clickable pustka markers.
struct OverlayHighlightCanvas: View {
  let runs: [OverlayCanvasRun]
  let selectedId: String?
  let onSelect: (OverlayHighlight) -> Void

  var body: some View {
    // Wrapping HStack of runs: the live canvas is short enough that a
    // flow layout is unnecessary, and Text+Button keeps hit targets honest.
    WrappedRuns(runs: runs, selectedId: selectedId, onSelect: onSelect)
      .accessibilityIdentifier("overlay-highlight-canvas")
  }
}

private struct WrappedRuns: View {
  let runs: [OverlayCanvasRun]
  let selectedId: String?
  let onSelect: (OverlayHighlight) -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      FlowText(runs: runs, selectedId: selectedId, onSelect: onSelect)
    }
  }
}

/// Single wrapping text line built from mixed text + highlight buttons.
private struct FlowText: View {
  let runs: [OverlayCanvasRun]
  let selectedId: String?
  let onSelect: (OverlayHighlight) -> Void

  var body: some View {
    runs.reduce(Text("")) { acc, run in
      acc + rendered(run)
    }
    .csFont(15, .medium)
    .lineSpacing(5)
    .fixedSize(horizontal: false, vertical: true)
    .overlay(alignment: .topLeading) {
      // Invisible hit buttons stacked over highlight ranges are brittle.
      // A row of explicit chips under the line is the Teach affordance.
      EmptyView()
    }
    .accessibilityIdentifier("overlay-transcript-live")
  }

  private func rendered(_ run: OverlayCanvasRun) -> Text {
    switch run {
    case .text(let text):
      return Text(text).foregroundColor(CSColor.textBody)
    case .highlight(let highlight):
      return Text(highlight.after)
        .foregroundColor(highlightColor(highlight))
        .underline(highlight.kind == .speechGap, color: CSColor.amber)
    }
  }

  private func highlightColor(_ highlight: OverlayHighlight) -> Color {
    switch highlight.kind {
    case .lexiconCorrected:
      return highlight.taught ? CSColor.oliveLight : CSColor.terracottaLight
    case .speechGap:
      return CSColor.amber
    }
  }
}

/// Clickable Teach chips for the highlighted spans. Separate from the
/// transcript text so TextEditor / wrapping Text stay simple.
struct OverlayHighlightTeachBar: View {
  let highlights: [OverlayHighlight]
  let selectedId: String?
  let onSelect: (OverlayHighlight) -> Void
  let onTeach: (OverlayHighlight) -> Void

  var body: some View {
    if !highlights.isEmpty {
      VStack(alignment: .leading, spacing: 6) {
        Text("Highlights")
          .csMono(10, .medium)
          .foregroundStyle(CSColor.textFaint)
          .accessibilityIdentifier("overlay-highlight-eyebrow")
        ForEach(highlights) { highlight in
          HStack(spacing: 8) {
            Button {
              onSelect(highlight)
            } label: {
              Text(chipLabel(highlight))
                .csFont(12, .medium)
                .foregroundStyle(chipForeground(highlight))
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(chipBackground(highlight), in: Capsule())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier(chipIdentifier(highlight))
            .accessibilityLabel(highlight.accessibilityLabel)

            Button("Teach") {
              onTeach(highlight)
            }
            .buttonStyle(.plain)
            .csMono(11, .medium)
            .foregroundStyle(CSColor.terracottaLight)
            .accessibilityIdentifier("overlay-highlight-teach-\(highlight.id)")
            .disabled(highlight.taught)
          }
        }
      }
      .accessibilityIdentifier("overlay-highlight-teach-bar")
    }
  }

  private func chipLabel(_ highlight: OverlayHighlight) -> String {
    switch highlight.kind {
    case .lexiconCorrected:
      return "\(highlight.before) → \(highlight.after)"
    case .speechGap:
      return "pustka \(highlight.after)"
    }
  }

  private func chipIdentifier(_ highlight: OverlayHighlight) -> String {
    switch highlight.kind {
    case .lexiconCorrected: return "overlay-highlight-lexicon-\(highlight.utteranceId)"
    case .speechGap: return "overlay-highlight-gap-\(highlight.utteranceId)"
    }
  }

  private func chipForeground(_ highlight: OverlayHighlight) -> Color {
    selectedId == highlight.id ? CSColor.ink : CSColor.textBody
  }

  private func chipBackground(_ highlight: OverlayHighlight) -> Color {
    if selectedId == highlight.id { return CSColor.terracottaLight }
    switch highlight.kind {
    case .lexiconCorrected: return CSColor.terracotta.opacity(0.28)
    case .speechGap: return CSColor.amber.opacity(0.22)
    }
  }
}

/// Compact canvas used for the W13-6B screenshot verifier (no live engine).
struct OverlayHighlightScreenshot: View {
  let runs: [OverlayCanvasRun]
  let highlights: [OverlayHighlight]

  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      Text("Codescribe · live canvas")
        .csMono(10, .medium)
        .foregroundStyle(CSColor.textFaint)
      OverlayHighlightCanvas(runs: runs, selectedId: highlights.first?.id, onSelect: { _ in })
      OverlayHighlightTeachBar(
        highlights: highlights,
        selectedId: highlights.first?.id,
        onSelect: { _ in },
        onTeach: { _ in }
      )
    }
    .padding(20)
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    .background(CSColor.ink)
  }
}
