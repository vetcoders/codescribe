import SwiftUI

/// Paint-only activity marker; never inserted into the transcript or clipboard.
struct TranscriptActivityUnderline: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  var body: some View {
    TimelineView(.animation(minimumInterval: 0.6, paused: reduceMotion)) { context in
      let bright = reduceMotion || Int(context.date.timeIntervalSinceReferenceDate / 0.6) % 2 == 0
      RoundedRectangle(cornerRadius: 1)
        .fill(CSColor.terracotta)
        .frame(width: 10, height: 2)
        .shadow(color: CSColor.terracotta.opacity(bright ? 0.3 : 0), radius: 2)
        .opacity(bright ? 1 : 0.35)
    }
    .frame(width: 10, height: 4, alignment: .bottom)
    .allowsHitTesting(false)
    .accessibilityHidden(true)
  }
}
