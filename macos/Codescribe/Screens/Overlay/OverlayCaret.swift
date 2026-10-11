import SwiftUI

/// Word-reveal caret: 8×18 terracotta block, soft-pulsing on a 1s cycle.
struct BlinkingCaret: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  let animating: Bool

  var body: some View {
    Group {
      if reduceMotion || !animating {
        caret.opacity(1)
      } else {
        // Remove the stateful view to tear down repeatForever, as StatusPill
        // does. A nil animation transaction alone cannot cancel the loop.
        AnimatedOverlayCaret()
      }
    }
    .transaction { transaction in
      if reduceMotion || !animating {
        transaction.animation = nil
        transaction.disablesAnimations = true
      }
    }
  }

  private var caret: some View {
    RoundedRectangle(cornerRadius: 1, style: .continuous)
      .fill(CSColor.terracotta)
      .frame(width: 7, height: 15)
      .padding(.bottom, 3)
  }
}

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
