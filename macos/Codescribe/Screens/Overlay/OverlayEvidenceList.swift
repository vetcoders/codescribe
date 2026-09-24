import SwiftUI

/// Read-only words beside the live canvas.
///
/// Rust keeps these visible without mutation authority — typically a Whisper
/// alternative the ledger refused as a whole-span replacement inside an Apple
/// occurrence. They are never canvas, copy, Bus, or delivery bytes, so this
/// list paints them as secondary, visibly non-committed text in PCM order and
/// offers no action. Live dictation cares about the words just heard, so only
/// the latest rows show and older ones fold into a count.
///
/// It reads `liveEvidence` in its own body: every compact paint replaces that
/// projection, and only this list — not the transcript canvas beside it — has
/// to re-evaluate.
struct OverlayEvidenceList: View {
  let state: OverlayState
  let palette: OverlayAppearancePalette

  private static let visibleRows = 3
  /// The overlay tools handle floats over the bottom of the body: an 8 pt
  /// inset plus a 20 pt tall hit area, 18 pt of it inside the body's own
  /// 10 pt bottom padding. Rows end above it instead of running under it.
  private static let toolsHandleClearance: CGFloat = 18

  var body: some View {
    let evidence = state.liveEvidence
    if !evidence.isEmpty {
      let shown = evidence.suffix(Self.visibleRows)
      VStack(alignment: .leading, spacing: CSSpace.xxs) {
        Label("Also heard · not committed", systemImage: "waveform")
          .csMono(10, .semibold)
          .foregroundStyle(palette.mutedText.color)
        if evidence.count > shown.count {
          Text("+\(evidence.count - shown.count) earlier")
            .csMono(10, .medium)
            .foregroundStyle(palette.mutedText.color)
        }
        ForEach(shown, id: \.rangeID) { item in
          Text(item.text)
            .csFont(13, .regular)
            .foregroundStyle(palette.mutedText.color)
            .lineLimit(2)
            .padding(.leading, CSSpace.sm)
            .overlay(alignment: .leading) {
              Capsule()
                .fill(palette.border.color)
                .frame(width: 2)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Not committed: \(item.text)")
            .accessibilityValue("Samples \(item.sampleStart) to \(item.sampleEnd)")
            .accessibilityIdentifier("overlay-unanchored-evidence-row")
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(.bottom, Self.toolsHandleClearance)
      // Paint only: the window drag region behind the body keeps every click.
      .allowsHitTesting(false)
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier("overlay-unanchored-evidence")
    }
  }
}

extension CsUnanchoredEvidence {
  /// Rust keys evidence by its PCM range: unique within one capture, and
  /// stable while a later observation relabels the same range.
  var rangeID: String { "\(sampleStart)..<\(sampleEnd)" }
}
